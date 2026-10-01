//! Read-only UDIF (.dmg) container support.
//!
//! Exposes the logical disk stored in a DMG as a `BlockSource`. Supported
//! chunk types: zero-fill, ignore, raw, zlib, bzip2, ADC, LZFSE and XZ-framed
//! LZMA. Any other type is reported as unsupported when a read touches it.

use crate::device::BlockSource;
use crate::util::{slice, u32be, u64be};
use crate::{Error, Result};
use base64::Engine;
use flate2::read::ZlibDecoder;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Mutex;

const SECTOR: u64 = 512;
const KOLY_SIZE: u64 = 512;
const MISH_HEADER: usize = 204;
const CHUNK_SIZE: usize = 40;
/// Upper bound on one decoded chunk, to bound memory on hostile input.
const MAX_CHUNK_BYTES: u64 = 64 << 20;
const MAX_CHUNKS: usize = 4_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChunkKind {
    Zero,
    Raw,
    Zlib,
    Adc,
    Bzip2,
    Lzfse,
    Lzma,
    Unsupported(u32),
}

#[derive(Debug, Clone, Copy)]
struct Chunk {
    start: u64, // logical byte offset
    len: u64,   // logical byte length
    kind: ChunkKind,
    comp_off: u64, // absolute file offset
    comp_len: u64,
}

pub struct UdifSource {
    file: Mutex<File>,
    chunks: Vec<Chunk>,
    len: u64,
    cache: Mutex<Option<(usize, Vec<u8>)>>,
}

/// True if the file ends with a UDIF "koly" trailer.
pub fn is_udif(path: impl AsRef<Path>) -> bool {
    let Ok(mut f) = File::open(path) else { return false };
    let Ok(size) = f.metadata().map(|m| m.len()) else { return false };
    if size < KOLY_SIZE {
        return false;
    }
    let mut sig = [0u8; 4];
    f.seek(SeekFrom::Start(size - KOLY_SIZE)).is_ok()
        && f.read_exact(&mut sig).is_ok()
        && &sig == b"koly"
}

impl UdifSource {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let mut file = File::open(path)?;
        let size = file.metadata()?.len();
        if size < KOLY_SIZE {
            return Err(Error::Format("file too small for UDIF"));
        }
        let mut koly = [0u8; KOLY_SIZE as usize];
        file.seek(SeekFrom::Start(size - KOLY_SIZE))?;
        file.read_exact(&mut koly)?;
        if &koly[0..4] != b"koly" {
            return Err(Error::Format("no UDIF trailer"));
        }
        let data_fork_off = u64be(&koly, 24)?;
        let xml_off = u64be(&koly, 216)?;
        let xml_len = u64be(&koly, 224)?;
        let sector_count = u64be(&koly, 492)?;
        if xml_len == 0 || xml_len > (64 << 20) || xml_off.checked_add(xml_len).map_or(true, |e| e > size) {
            return Err(Error::Format("bad UDIF property list range"));
        }
        let mut xml = vec![0u8; xml_len as usize];
        file.seek(SeekFrom::Start(xml_off))?;
        file.read_exact(&mut xml)?;
        let xml = String::from_utf8_lossy(&xml);

        let mut chunks = Vec::new();
        for mish in extract_blkx(&xml)? {
            parse_mish(&mish, data_fork_off, size, &mut chunks)?;
        }
        chunks.sort_by_key(|c| c.start);
        let len = chunks
            .iter()
            .map(|c| c.start + c.len)
            .max()
            .unwrap_or(0)
            .max(sector_count.saturating_mul(SECTOR));
        Ok(Self { file: Mutex::new(file), chunks, len, cache: Mutex::new(None) })
    }

    fn decode(&self, idx: usize) -> Result<Vec<u8>> {
        let c = &self.chunks[idx];
        let mut comp = vec![0u8; c.comp_len as usize];
        {
            let mut f = self.file.lock().unwrap();
            f.seek(SeekFrom::Start(c.comp_off))?;
            f.read_exact(&mut comp)?;
        }
        let want = c.len as usize;
        let mut out = Vec::with_capacity(want);
        match c.kind {
            ChunkKind::Zlib => {
                ZlibDecoder::new(&comp[..])
                    .take(c.len + 1)
                    .read_to_end(&mut out)
                    .map_err(|_| Error::Format("corrupt zlib chunk in DMG"))?;
            }
            ChunkKind::Bzip2 => {
                bzip2::read::BzDecoder::new(&comp[..])
                    .take(c.len + 1)
                    .read_to_end(&mut out)
                    .map_err(|_| Error::Format("corrupt bzip2 chunk in DMG"))?;
            }
            ChunkKind::Lzfse => {
                lzfse_rust::decode_bytes(&comp, &mut out)
                    .map_err(|_| Error::Format("corrupt LZFSE chunk in DMG"))?;
            }
            ChunkKind::Lzma => {
                if !comp.starts_with(&XZ_MAGIC) {
                    return Err(Error::Unsupported("raw LZMA1 chunk in DMG (only XZ-framed is supported)".into()));
                }
                lzma_rs::xz_decompress(&mut &comp[..], &mut out)
                    .map_err(|_| Error::Format("corrupt XZ chunk in DMG"))?;
            }
            ChunkKind::Adc => out = adc_decode(&comp, want)?,
            ChunkKind::Zero | ChunkKind::Raw | ChunkKind::Unsupported(_) => {
                unreachable!("only compressed chunks are decoded")
            }
        }
        if out.len() != want {
            return Err(Error::Format("DMG chunk decoded to an unexpected size"));
        }
        Ok(out)
    }
}

const XZ_MAGIC: [u8; 6] = [0xFD, b'7', b'z', b'X', b'Z', 0x00];

/// Apple Data Compression: literal runs and back-references (2- or 3-byte).
fn adc_decode(src: &[u8], want: usize) -> Result<Vec<u8>> {
    const BAD: Error = Error::Format("corrupt ADC stream in DMG");
    let mut dst = Vec::with_capacity(want);
    let mut i = 0;
    while i < src.len() && dst.len() < want {
        let ctl = src[i] as usize;
        i += 1;
        let (len, dist) = if ctl & 0x80 != 0 {
            let n = (ctl & 0x7F) + 1;
            if i + n > src.len() || dst.len() + n > want {
                return Err(BAD);
            }
            dst.extend_from_slice(&src[i..i + n]);
            i += n;
            continue;
        } else if ctl & 0x40 != 0 {
            if i + 2 > src.len() {
                return Err(BAD);
            }
            let d = ((src[i] as usize) << 8 | src[i + 1] as usize) + 1;
            i += 2;
            (ctl - 0x3C, d)
        } else {
            if i >= src.len() {
                return Err(BAD);
            }
            let d = (((ctl & 3) << 8) | src[i] as usize) + 1;
            i += 1;
            (((ctl >> 2) & 0x0F) + 3, d)
        };
        if dist > dst.len() || dst.len() + len > want {
            return Err(BAD);
        }
        for _ in 0..len {
            dst.push(dst[dst.len() - dist]);
        }
    }
    if dst.len() != want {
        return Err(BAD);
    }
    Ok(dst)
}

fn extract_blkx(xml: &str) -> Result<Vec<Vec<u8>>> {
    let k = xml.find("<key>blkx</key>").ok_or(Error::Format("DMG has no blkx table"))?;
    let rest = &xml[k..];
    let a = rest.find("<array>").ok_or(Error::Format("malformed blkx"))?;
    let b = rest[a..].find("</array>").ok_or(Error::Format("malformed blkx"))? + a;
    let mut out = Vec::new();
    let mut s = &rest[a..b];
    while let Some(i) = s.find("<data>") {
        let after = &s[i + 6..];
        let j = after.find("</data>").ok_or(Error::Format("malformed data element"))?;
        let b64: String = after[..j].chars().filter(|c| !c.is_whitespace()).collect();
        out.push(
            base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|_| Error::Format("bad base64 in blkx"))?,
        );
        s = &after[j + 7..];
    }
    Ok(out)
}

fn parse_mish(m: &[u8], data_fork_off: u64, file_size: u64, out: &mut Vec<Chunk>) -> Result<()> {
    if m.len() < MISH_HEADER || &m[0..4] != b"mish" {
        return Err(Error::Format("bad mish block"));
    }
    let first_sector = u64be(m, 8)?;
    let data_offset = u64be(m, 24)?;
    let count = u32be(m, 200)? as usize;
    if count > MAX_CHUNKS || out.len() + count > MAX_CHUNKS {
        return Err(Error::Format("too many DMG chunks"));
    }
    if m.len() < MISH_HEADER + count * CHUNK_SIZE {
        return Err(Error::Format("truncated mish chunk table"));
    }
    for i in 0..count {
        let c = slice(m, MISH_HEADER + i * CHUNK_SIZE, CHUNK_SIZE)?;
        let ty = u32be(c, 0)?;
        if ty == 0xFFFF_FFFF {
            break; // terminator
        }
        if ty == 0x7FFF_FFFE {
            continue; // comment
        }
        let sectors = u64be(c, 16)?;
        let len = sectors.checked_mul(SECTOR).filter(|&l| l <= MAX_CHUNK_BYTES);
        let len = len.ok_or(Error::Format("DMG chunk too large"))?;
        let start = first_sector
            .checked_add(u64be(c, 8)?)
            .and_then(|s| s.checked_mul(SECTOR))
            .ok_or(Error::Format("DMG chunk offset overflow"))?;
        let comp_off = data_fork_off
            .checked_add(data_offset)
            .and_then(|o| o.checked_add(u64be(c, 24).ok()?))
            .ok_or(Error::Format("DMG chunk data offset overflow"))?;
        let comp_len = u64be(c, 32)?;
        let kind = match ty {
            0 | 2 => ChunkKind::Zero,
            1 => ChunkKind::Raw,
            0x8000_0004 => ChunkKind::Adc,
            0x8000_0005 => ChunkKind::Zlib,
            0x8000_0006 => ChunkKind::Bzip2,
            0x8000_0007 => ChunkKind::Lzfse,
            0x8000_0008 => ChunkKind::Lzma,
            other => ChunkKind::Unsupported(other),
        };
        if !matches!(kind, ChunkKind::Zero | ChunkKind::Unsupported(_))
            && comp_off.checked_add(comp_len).map_or(true, |e| e > file_size)
        {
            return Err(Error::Format("DMG chunk data beyond end of file"));
        }
        if kind == ChunkKind::Raw && comp_len < len {
            return Err(Error::Format("raw DMG chunk shorter than its sectors"));
        }
        if len > 0 {
            out.push(Chunk { start, len, kind, comp_off, comp_len });
        }
    }
    Ok(())
}

impl BlockSource for UdifSource {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let end = offset.checked_add(buf.len() as u64);
        if end.map_or(true, |e| e > self.len) {
            return Err(Error::OutOfRange { offset, len: buf.len() });
        }
        buf.fill(0); // gaps between chunks read as zeros
        let end = end.unwrap();
        // First chunk whose end is past `offset`.
        let mut i = self.chunks.partition_point(|c| c.start + c.len <= offset);
        while i < self.chunks.len() && self.chunks[i].start < end {
            let c = self.chunks[i];
            let lo = offset.max(c.start);
            let hi = end.min(c.start + c.len);
            let dst = &mut buf[(lo - offset) as usize..(hi - offset) as usize];
            let within = lo - c.start;
            match c.kind {
                ChunkKind::Zero => {}
                ChunkKind::Raw => {
                    let mut f = self.file.lock().unwrap();
                    f.seek(SeekFrom::Start(c.comp_off + within))?;
                    f.read_exact(dst)?;
                }
                ChunkKind::Zlib | ChunkKind::Adc | ChunkKind::Bzip2 | ChunkKind::Lzfse | ChunkKind::Lzma => {
                    let mut cache = self.cache.lock().unwrap();
                    if cache.as_ref().map_or(true, |(ci, _)| *ci != i) {
                        *cache = Some((i, self.decode(i)?));
                    }
                    let data = &cache.as_ref().unwrap().1;
                    dst.copy_from_slice(&data[within as usize..within as usize + dst.len()]);
                }
                ChunkKind::Unsupported(t) => {
                    return Err(Error::Unsupported(format!(
                        "DMG chunk compression type 0x{t:08X} (supported: raw, zlib, bzip2, LZFSE, LZMA/XZ, ADC)"
                    )));
                }
            }
            i += 1;
        }
        Ok(())
    }
}

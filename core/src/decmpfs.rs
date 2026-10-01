//! Transparent file compression (decmpfs), shared by HFS+ and APFS.
//!
//! A compressed file has a `com.apple.decmpfs` attribute holding a 16-byte
//! header (type and decoded size) and, for the odd-numbered types, the payload
//! itself. The even-numbered types keep a chunked payload (64 KiB per chunk) in
//! the `com.apple.ResourceFork` attribute. The header is little-endian on both
//! file systems, even on HFS+ where everything else is big-endian.
//!
//! Supported: 1 (raw inline), 3/4 (zlib), 7/8 (LZVN), 11/12 (LZFSE).
//! Refused: 5 (generation-store dedup: no content here), 9/10 and 13/14.

use crate::util::{slice, u32le, u64le};
use crate::{lzvn, Error, Result};
use flate2::read::ZlibDecoder;
use std::io::Read;

pub const ATTR_NAME: &str = "com.apple.decmpfs";
pub const RESOURCE_FORK_NAME: &str = "com.apple.ResourceFork";

const HEADER_SIZE: usize = 16;
const BLOCK: u64 = 65536;
/// Largest decoded size accepted for inline (attribute-resident) data.
const MAX_INLINE_DECODED: u64 = 1 << 20;
/// Largest chunk table we will read, to bound memory on hostile input.
const MAX_BLOCKS: u64 = 1 << 24;

/// The resource fork (or other byte range) a chunked payload is read from.
pub trait Fork {
    fn len(&self) -> u64;
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize>;
}

#[derive(Debug, Clone, Copy)]
pub struct Header {
    pub method: u32,
    /// The size the file presents once decoded.
    pub size: u64,
}

/// Parse a decmpfs attribute value. `None` if it does not start with the magic.
pub fn parse_header(attr: &[u8]) -> Result<Option<Header>> {
    if attr.len() < HEADER_SIZE {
        return Err(Error::Format("decmpfs attribute shorter than its header"));
    }
    if &attr[0..4] != b"fpmc" {
        return Ok(None);
    }
    Ok(Some(Header { method: u32le(attr, 4)?, size: u64le(attr, 8)? }))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Codec {
    Raw,
    Zlib,
    Lzvn,
    Lzfse,
}

/// (codec, payload is in the resource fork)
fn classify(method: u32) -> Result<(Codec, bool)> {
    Ok(match method {
        1 => (Codec::Raw, false),
        3 => (Codec::Zlib, false),
        4 => (Codec::Zlib, true),
        7 => (Codec::Lzvn, false),
        8 => (Codec::Lzvn, true),
        11 => (Codec::Lzfse, false),
        12 => (Codec::Lzfse, true),
        5 => {
            return Err(Error::Unsupported(
                "decmpfs type 5 (generation-store dedup) does not describe file content".into(),
            ))
        }
        other => return Err(Error::Unsupported(format!("decmpfs compression type {other}"))),
    })
}

/// Whether a decmpfs type keeps its payload in the resource fork.
pub fn needs_resource_fork(method: u32) -> bool {
    classify(method).map_or(false, |(_, rsrc)| rsrc)
}

/// Decode one chunk (or one whole inline payload) to exactly `expected` bytes.
fn decode_chunk(codec: Codec, comp: &[u8], expected: usize) -> Result<Vec<u8>> {
    let raw = |body: &[u8]| -> Result<Vec<u8>> {
        if body.len() != expected {
            return Err(Error::Format("stored decmpfs chunk has the wrong length"));
        }
        Ok(body.to_vec())
    };
    if comp.is_empty() {
        return Err(Error::Format("empty decmpfs chunk"));
    }
    match codec {
        Codec::Raw => raw(comp),
        Codec::Zlib => {
            // Chunks that did not compress are stored behind a marker whose low
            // nibble is 0xF; no zlib header can have that (it is always 8).
            if comp[0] & 0x0F == 0x0F {
                return raw(&comp[1..]);
            }
            let mut out = Vec::with_capacity(expected);
            ZlibDecoder::new(comp)
                .take(expected as u64 + 1)
                .read_to_end(&mut out)
                .map_err(|_| Error::Format("corrupt zlib data in decmpfs chunk"))?;
            if out.len() != expected {
                return Err(Error::Format("decmpfs chunk decoded to the wrong length"));
            }
            Ok(out)
        }
        Codec::Lzvn => {
            if comp[0] == 0x06 {
                return raw(&comp[1..]);
            }
            lzvn::decode(comp, expected)
        }
        Codec::Lzfse => {
            // Every LZFSE block magic begins with 'b'; a chunk without it is raw.
            if comp[0] != b'b' {
                return raw(&comp[1..]);
            }
            let mut out = Vec::with_capacity(expected);
            lzfse_rust::decode_bytes(comp, &mut out)
                .map_err(|_| Error::Format("corrupt LZFSE data in decmpfs chunk"))?;
            if out.len() != expected {
                return Err(Error::Format("decmpfs chunk decoded to the wrong length"));
            }
            Ok(out)
        }
    }
}

/// Byte ranges (offset, length) of every chunk in a resource-fork payload.
fn chunk_table(codec: Codec, rsrc: &dyn Fork, size: u64) -> Result<Vec<(u64, u64)>> {
    let nblocks = size.div_ceil(BLOCK);
    if nblocks > MAX_BLOCKS {
        return Err(Error::Format("decmpfs file has too many chunks"));
    }
    let fork_len = rsrc.len();
    let read = |off: u64, len: usize| -> Result<Vec<u8>> {
        let mut b = vec![0u8; len];
        if rsrc.read_at(off, &mut b)? != len {
            return Err(Error::Format("decmpfs chunk table runs past the resource fork"));
        }
        Ok(b)
    };

    let mut out = Vec::with_capacity(nblocks as usize);
    if codec == Codec::Zlib {
        // HFS resource-fork header (big-endian data offset, 0x100), then at
        // 0x104 a little-endian block count and (offset, size) pairs whose
        // offsets are relative to 0x104.
        let head = read(0, 0x108)?;
        if u32::from_be_bytes(head[0..4].try_into().unwrap()) != 0x100 {
            return Err(Error::Format("unexpected resource fork header in zlib decmpfs file"));
        }
        let count = u32le(&head, 0x104)? as u64;
        if count != nblocks {
            return Err(Error::Format("zlib decmpfs block count does not match file size"));
        }
        let table = read(0x108, (count as usize) * 8)?;
        for i in 0..count as usize {
            let off = u32le(&table, i * 8)? as u64 + 0x104;
            let len = u32le(&table, i * 8 + 4)? as u64;
            out.push((off, len));
        }
    } else {
        // Flat little-endian offsets from the start of the fork; the first
        // entry is the table's own size and the last marks the end.
        let first = u32le(&read(0, 4)?, 0)? as u64;
        if first % 4 != 0 || first < 8 || first / 4 - 1 != nblocks {
            return Err(Error::Format("decmpfs block table does not match file size"));
        }
        let table = read(0, first as usize)?;
        for i in 0..nblocks as usize {
            let a = u32le(&table, i * 4)? as u64;
            let b = u32le(&table, i * 4 + 4)? as u64;
            if b < a {
                return Err(Error::Format("decmpfs block offsets are not increasing"));
            }
            out.push((a, b - a));
        }
    }
    for &(off, len) in &out {
        if off.checked_add(len).map_or(true, |e| e > fork_len) || len > BLOCK + 1024 {
            return Err(Error::Format("decmpfs chunk lies outside the resource fork"));
        }
    }
    Ok(out)
}

/// Read decoded bytes `[offset, offset + buf.len())` of a compressed file.
/// Returns the number read (short only at the end of the file).
pub fn read(
    attr: &[u8],
    rsrc: Option<&dyn Fork>,
    offset: u64,
    buf: &mut [u8],
) -> Result<usize> {
    let header = parse_header(attr)?.ok_or(Error::Format("not a decmpfs attribute"))?;
    let (codec, in_rsrc) = classify(header.method)?;
    if offset >= header.size {
        return Ok(0);
    }
    let want = (buf.len() as u64).min(header.size - offset) as usize;

    if !in_rsrc {
        if header.size > MAX_INLINE_DECODED {
            return Err(Error::Format("inline decmpfs data claims an implausible size"));
        }
        let data = decode_chunk(codec, slice(attr, HEADER_SIZE, attr.len() - HEADER_SIZE)?, header.size as usize)?;
        buf[..want].copy_from_slice(&data[offset as usize..offset as usize + want]);
        return Ok(want);
    }

    let rsrc = rsrc.ok_or(Error::Format("compressed file has no resource fork"))?;
    let table = chunk_table(codec, rsrc, header.size)?;
    let mut done = 0usize;
    while done < want {
        let pos = offset + done as u64;
        let idx = (pos / BLOCK) as usize;
        let (off, len) = *table.get(idx).ok_or(Error::Format("decmpfs chunk index out of range"))?;
        let mut comp = vec![0u8; len as usize];
        if rsrc.read_at(off, &mut comp)? != comp.len() {
            return Err(Error::Format("short read of a decmpfs chunk"));
        }
        let chunk_start = idx as u64 * BLOCK;
        let expected = (header.size - chunk_start).min(BLOCK) as usize;
        let data = decode_chunk(codec, &comp, expected)?;
        let within = (pos - chunk_start) as usize;
        let n = (data.len() - within).min(want - done);
        buf[done..done + n].copy_from_slice(&data[within..within + n]);
        done += n;
    }
    Ok(want)
}

#[cfg(test)]
mod tests {
    //! Synthetic vectors. Real macOS output covers only LZVN-in-resource-fork
    //! (see tests/macos_fixtures_*.rs); these cover the other types using the
    //! layouts documented in go-apfs-v2's internal/decmpfs, so they check this
    //! reader against that description, not against Apple's own writer.

    use super::*;
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use std::io::Write;

    struct Mem(Vec<u8>);
    impl Fork for Mem {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read_at(&self, off: u64, buf: &mut [u8]) -> Result<usize> {
            let off = off as usize;
            if off >= self.0.len() {
                return Ok(0);
            }
            let n = buf.len().min(self.0.len() - off);
            buf[..n].copy_from_slice(&self.0[off..off + n]);
            Ok(n)
        }
    }

    fn header(method: u32, size: u64) -> Vec<u8> {
        let mut h = b"fpmc".to_vec();
        h.extend_from_slice(&method.to_le_bytes());
        h.extend_from_slice(&size.to_le_bytes());
        h
    }

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut e = ZlibEncoder::new(Vec::new(), Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn lzfse(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        lzfse_rust::LzfseEncoder::default().encode_bytes(data, &mut out).unwrap();
        out
    }

    fn sample(n: usize) -> Vec<u8> {
        // Compressible but not trivially so.
        (0..n).map(|i| ((i * 7 + i / 251) % 61) as u8 + b' ').collect()
    }

    fn read_all(attr: &[u8], fork: Option<&dyn Fork>, size: usize) -> Vec<u8> {
        let mut out = vec![0u8; size];
        // Odd-sized reads that straddle chunk boundaries.
        let mut pos = 0;
        while pos < size {
            let want = 40_000.min(size - pos);
            let n = read(attr, fork, pos as u64, &mut out[pos..pos + want]).unwrap();
            assert!(n > 0);
            pos += n;
        }
        out
    }

    /// zlib resource fork: HFS-style header, count at 0x104, (offset, size)
    /// pairs relative to 0x104.
    fn zlib_fork(chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut fork = vec![0u8; 0x108];
        fork[0..4].copy_from_slice(&0x100u32.to_be_bytes());
        fork[0x104..0x108].copy_from_slice(&(chunks.len() as u32).to_le_bytes());
        let mut off = 4 + 8 * chunks.len() as u32;
        let mut table = Vec::new();
        for c in chunks {
            table.extend_from_slice(&off.to_le_bytes());
            table.extend_from_slice(&(c.len() as u32).to_le_bytes());
            off += c.len() as u32;
        }
        fork.extend_from_slice(&table);
        for c in chunks {
            fork.extend_from_slice(c);
        }
        fork
    }

    /// LZVN/LZFSE resource fork: flat LE offsets from the start of the fork,
    /// with a trailing end marker.
    fn flat_fork(chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut off = 4 * (chunks.len() as u32 + 1);
        let mut fork = Vec::new();
        for c in chunks {
            fork.extend_from_slice(&off.to_le_bytes());
            off += c.len() as u32;
        }
        fork.extend_from_slice(&off.to_le_bytes());
        for c in chunks {
            fork.extend_from_slice(c);
        }
        fork
    }

    #[test]
    fn inline_zlib() {
        let data = sample(3000);
        let mut attr = header(3, data.len() as u64);
        attr.extend_from_slice(&zlib(&data));
        assert_eq!(read_all(&attr, None, data.len()), data);
    }

    #[test]
    fn inline_lzfse_and_raw() {
        let data = sample(2000);
        let mut attr = header(11, data.len() as u64);
        attr.extend_from_slice(&lzfse(&data));
        assert_eq!(read_all(&attr, None, data.len()), data);

        let mut raw = header(11, data.len() as u64);
        raw.push(0xFF);
        raw.extend_from_slice(&data);
        assert_eq!(read_all(&raw, None, data.len()), data);

        let mut plain = header(1, data.len() as u64);
        plain.extend_from_slice(&data);
        assert_eq!(read_all(&plain, None, data.len()), data);
    }

    #[test]
    fn chunked_zlib_with_a_stored_chunk() {
        let data = sample(65536 * 2 + 1234);
        let mut chunks: Vec<Vec<u8>> = data.chunks(65536).map(zlib).collect();
        // Store the middle chunk raw behind a 0xFF marker, as macOS does when
        // compression does not help.
        let mut stored = vec![0xFF];
        stored.extend_from_slice(&data[65536..131072]);
        chunks[1] = stored;
        let fork = Mem(zlib_fork(&chunks));
        let attr = header(4, data.len() as u64);
        assert_eq!(read_all(&attr, Some(&fork), data.len()), data);
    }

    #[test]
    fn chunked_lzfse() {
        let data = sample(65536 + 777);
        let chunks: Vec<Vec<u8>> = data.chunks(65536).map(lzfse).collect();
        let fork = Mem(flat_fork(&chunks));
        let attr = header(12, data.len() as u64);
        assert_eq!(read_all(&attr, Some(&fork), data.len()), data);
    }

    #[test]
    fn chunked_lzvn_stored_marker() {
        // 0x06-prefixed chunks are stored raw; that path needs no encoder.
        let data = sample(65536 + 10);
        let chunks: Vec<Vec<u8>> = data
            .chunks(65536)
            .map(|c| {
                let mut v = vec![0x06];
                v.extend_from_slice(c);
                v
            })
            .collect();
        let fork = Mem(flat_fork(&chunks));
        let attr = header(8, data.len() as u64);
        assert_eq!(read_all(&attr, Some(&fork), data.len()), data);
    }

    #[test]
    fn refuses_what_it_cannot_decode() {
        let mut buf = [0u8; 4];
        for method in [5u32, 9, 10, 13, 14, 99] {
            let attr = header(method, 10);
            assert!(read(&attr, None, 0, &mut buf).is_err(), "type {method}");
        }
        // Missing resource fork for a resource-fork type.
        assert!(read(&header(4, 10), None, 0, &mut buf).is_err());
        // Table that disagrees with the declared size.
        let fork = Mem(flat_fork(&[vec![0x06, 1, 2, 3]]));
        assert!(read(&header(8, 3 * 65536), Some(&fork), 0, &mut buf).is_err());
        // Reads at or past the end return nothing.
        let mut attr = header(1, 3);
        attr.extend_from_slice(b"abc");
        assert_eq!(read(&attr, None, 3, &mut buf).unwrap(), 0);
    }
}

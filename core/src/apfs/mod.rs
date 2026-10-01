//! Read-only APFS driver (unencrypted, non-Fusion containers, current state
//! only: snapshots are not exposed).
//!
//! Every object read is verified against its Fletcher-64 checksum. Volumes
//! without the UNENCRYPTED flag are refused rather than returned as garbage.

mod btree;

use crate::decmpfs;
use crate::device::BlockSource;
use crate::util::{slice, u16le, u32le, u64le};
use crate::{Error, Result};
pub use crate::vfs::EntryKind;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use unicode_normalization::UnicodeNormalization;

const NX_MAGIC: u32 = 0x4253_584E; // 'NXSB'
const APFS_MAGIC: u32 = 0x4253_5041; // 'APSB'

const OBJ_TYPE_MASK: u32 = 0xFFFF;
const OBJ_TYPE_SUPERBLOCK: u32 = 1;

const NX_INCOMPAT_FUSION: u64 = 0x100;

const APFS_FS_UNENCRYPTED: u64 = 0x1;
const APFS_INCOMPAT_CASE_INSENSITIVE: u64 = 0x1;
const APFS_INCOMPAT_NORMALIZATION_INSENSITIVE: u64 = 0x8;

const OMAP_VAL_DELETED: u32 = 0x1;
const OMAP_VAL_ENCRYPTED: u32 = 0x4;

const J_TYPE_INODE: u8 = 3;
const J_TYPE_XATTR: u8 = 4;
const J_TYPE_FILE_EXTENT: u8 = 8;
const J_TYPE_DIR_REC: u8 = 9;

const ROOT_DIR_INO: u64 = 2;
const INO_EXT_TYPE_DSTREAM: u8 = 8;
const UF_COMPRESSED: u32 = 0x20;
const INODE_HAS_UNCOMPRESSED_SIZE: u64 = 0x0004_0000;
const XATTR_DATA_STREAM: u16 = 0x1;
const XATTR_DATA_EMBEDDED: u16 = 0x2;
const SYMLINK_XATTR: &str = "com.apple.fs.symlink";
const MAX_XATTR_READ: u64 = 256 << 20;

const CACHE_LIMIT: usize = 4096;

/// Fletcher-64 over everything after the 8-byte checksum field.
fn checksum_ok(block: &[u8]) -> bool {
    if block.len() < 16 || block.len() % 4 != 0 {
        return false;
    }
    const M: u64 = 0xFFFF_FFFF;
    let (mut s1, mut s2) = (0u64, 0u64);
    for w in block[8..].chunks_exact(4) {
        s1 += u32::from_le_bytes([w[0], w[1], w[2], w[3]]) as u64;
        s2 += s1;
    }
    let (s1, s2) = (s1 % M, s2 % M);
    let c1 = M - (s1 + s2) % M;
    let c2 = M - (s1 + c1) % M;
    let stored = u64::from_le_bytes(block[0..8].try_into().unwrap());
    stored == (c2 << 32) | c1
}

pub struct Container<'a> {
    src: &'a dyn BlockSource,
    pub block_size: u32,
    pub block_count: u64,
    /// Transaction id of the checkpoint superblock in use.
    pub xid: u64,
    pub uuid: [u8; 16],
    omap_paddr: u64,
    fs_oids: Vec<u64>,
    cache: Mutex<HashMap<u64, Arc<Vec<u8>>>>,
}

impl<'a> Container<'a> {
    pub fn open(src: &'a dyn BlockSource) -> Result<Self> {
        let mut head = vec![0u8; 4096];
        src.read_at(0, &mut head)?;
        if u32le(&head, 32)? != NX_MAGIC {
            return Err(Error::Format("not an APFS container"));
        }
        let block_size = u32le(&head, 36)?;
        if !block_size.is_power_of_two() || !(4096..=65536).contains(&block_size) {
            return Err(Error::Format("bad APFS block size"));
        }
        let bs = block_size as u64;
        let read_block = |n: u64| -> Result<Vec<u8>> {
            let mut b = vec![0u8; block_size as usize];
            let off = n.checked_mul(bs).ok_or(Error::Format("block number overflow"))?;
            src.read_at(off, &mut b)?;
            Ok(b)
        };

        // Candidate superblocks: block 0 and every superblock in the checkpoint
        // descriptor area. The valid one with the highest xid is current.
        let mut best: Option<Vec<u8>> = None;
        let consider = |b: Vec<u8>, best: &mut Option<Vec<u8>>| {
            let ok = checksum_ok(&b)
                && u32le(&b, 24).map_or(false, |t| t & OBJ_TYPE_MASK == OBJ_TYPE_SUPERBLOCK)
                && u32le(&b, 32).map_or(false, |m| m == NX_MAGIC);
            if ok {
                let xid = u64le(&b, 16).unwrap_or(0);
                if best.as_ref().map_or(true, |p| xid > u64le(p, 16).unwrap_or(0)) {
                    *best = Some(b);
                }
            }
        };
        let block0 = read_block(0)?;
        let desc_blocks = u32le(&block0, 104)?;
        let desc_base = u64le(&block0, 112)?;
        consider(block0, &mut best);
        if desc_blocks & 0x8000_0000 == 0 && desc_blocks <= 65536 {
            for i in 0..desc_blocks as u64 {
                if let Ok(b) = read_block(desc_base.wrapping_add(i)) {
                    consider(b, &mut best);
                }
            }
        }
        let sb = best.ok_or(Error::Format("no valid APFS container superblock found"))?;

        let incompat = u64le(&sb, 64)?;
        if incompat & NX_INCOMPAT_FUSION != 0 {
            return Err(Error::Unsupported("Fusion Drive containers".into()));
        }
        let max_fs = (u32le(&sb, 180)? as usize).min(100);
        let mut fs_oids = Vec::new();
        for i in 0..max_fs {
            let oid = u64le(&sb, 184 + 8 * i)?;
            if oid != 0 {
                fs_oids.push(oid);
            }
        }
        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(slice(&sb, 72, 16)?);
        Ok(Self {
            src,
            block_size,
            block_count: u64le(&sb, 40)?,
            xid: u64le(&sb, 16)?,
            uuid,
            omap_paddr: u64le(&sb, 160)?,
            fs_oids,
            cache: Mutex::new(HashMap::new()),
        })
    }

    /// Read one checksum-verified object block.
    fn read_obj(&self, paddr: u64) -> Result<Arc<Vec<u8>>> {
        if let Some(b) = self.cache.lock().unwrap().get(&paddr) {
            return Ok(b.clone());
        }
        if paddr >= self.block_count {
            return Err(Error::Format("object address beyond container"));
        }
        let mut b = vec![0u8; self.block_size as usize];
        self.src.read_at(paddr * self.block_size as u64, &mut b)?;
        if !checksum_ok(&b) {
            return Err(Error::Format("APFS object checksum mismatch"));
        }
        let b = Arc::new(b);
        let mut cache = self.cache.lock().unwrap();
        if cache.len() >= CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(paddr, b.clone());
        Ok(b)
    }

    /// Resolve a virtual object id to a physical address through an object map.
    fn omap_lookup(&self, omap_paddr: u64, oid: u64, xid: u64) -> Result<u64> {
        let omap = self.read_obj(omap_paddr)?;
        let tree_paddr = u64le(&omap, 48)?;
        let root = self.read_obj(tree_paddr)?;
        let cmp = |k: &[u8]| {
            let (o, x) = (u64le(k, 0).unwrap_or(0), u64le(k, 8).unwrap_or(0));
            match o.cmp(&oid) {
                Ordering::Equal if x > xid => Ordering::Greater,
                Ordering::Equal => Ordering::Equal,
                other => other,
            }
        };
        let mut found: Option<(u32, u64)> = None;
        // Keys are ordered by xid within an oid, so the last match is the newest.
        btree::walk(
            root,
            &|child| self.read_obj(child),
            &cmp,
            &mut |_, v| {
                found = Some((u32le(v, 0)?, u64le(v, 8)?));
                Ok(true)
            },
        )?;
        match found {
            Some((flags, _)) if flags & OMAP_VAL_DELETED != 0 => {
                Err(Error::NotFound(format!("object {oid} is deleted")))
            }
            Some((flags, _)) if flags & OMAP_VAL_ENCRYPTED != 0 => {
                Err(Error::Unsupported("encrypted object".into()))
            }
            Some((_, paddr)) => Ok(paddr),
            None => Err(Error::NotFound(format!("object {oid} (xid <= {xid})"))),
        }
    }

    pub fn volume_count(&self) -> usize {
        self.fs_oids.len()
    }

    pub fn volume(&self, index: usize) -> Result<Volume<'_, 'a>> {
        let oid = *self.fs_oids.get(index).ok_or_else(|| Error::NotFound(format!("volume {index}")))?;
        let paddr = self.omap_lookup(self.omap_paddr, oid, self.xid)?;
        let sb = self.read_obj(paddr)?;
        Volume::parse(self, sb)
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub oid: u64,
    pub parent: u64,
    pub kind: EntryKind,
    pub mode: u16,
    /// Data length for files; child count for directories.
    pub size: u64,
    /// Seconds since the Unix epoch.
    pub create_time: i64,
    pub modify_time: i64,
    pub uid: u32,
    pub gid: u32,
    /// BSD UF_COMPRESSED: contents live in a decmpfs attribute (not yet readable).
    pub compressed: bool,
    private_id: u64,
}

pub struct Volume<'c, 'a> {
    c: &'c Container<'a>,
    pub name: String,
    pub role: u16,
    pub incompat_features: u64,
    pub encrypted: bool,
    pub num_files: u64,
    pub num_directories: u64,
    omap_paddr: u64,
    root_tree_oid: u64,
    resolved: Mutex<HashMap<u64, u64>>,
}

impl<'c, 'a> Volume<'c, 'a> {
    fn parse(c: &'c Container<'a>, sb: Arc<Vec<u8>>) -> Result<Self> {
        if u32le(&sb, 32)? != APFS_MAGIC {
            return Err(Error::Format("bad APFS volume superblock magic"));
        }
        let raw_name = slice(&sb, 704, 256)?;
        let end = raw_name.iter().position(|&b| b == 0).unwrap_or(raw_name.len());
        Ok(Self {
            c,
            name: String::from_utf8_lossy(&raw_name[..end]).into_owned(),
            role: u16le(&sb, 964)?,
            incompat_features: u64le(&sb, 56)?,
            encrypted: u64le(&sb, 264)? & APFS_FS_UNENCRYPTED == 0,
            num_files: u64le(&sb, 184)?,
            num_directories: u64le(&sb, 192)?,
            omap_paddr: u64le(&sb, 128)?,
            root_tree_oid: u64le(&sb, 136)?,
            resolved: Mutex::new(HashMap::new()),
        })
    }

    pub fn is_case_insensitive(&self) -> bool {
        self.incompat_features & APFS_INCOMPAT_CASE_INSENSITIVE != 0
    }

    fn check_readable(&self) -> Result<()> {
        if self.encrypted {
            return Err(Error::Unsupported("encrypted APFS volumes".into()));
        }
        Ok(())
    }

    fn resolve(&self, oid: u64) -> Result<u64> {
        if let Some(&p) = self.resolved.lock().unwrap().get(&oid) {
            return Ok(p);
        }
        let p = self.c.omap_lookup(self.omap_paddr, oid, self.c.xid)?;
        self.resolved.lock().unwrap().insert(oid, p);
        Ok(p)
    }

    fn read_virtual(&self, oid: u64) -> Result<Arc<Vec<u8>>> {
        self.c.read_obj(self.resolve(oid)?)
    }

    /// Visit file-system-tree records whose key falls in (object id, type).
    fn scan(
        &self,
        obj_id: u64,
        ty: u8,
        visit: &mut dyn FnMut(&[u8], &[u8]) -> Result<bool>,
    ) -> Result<()> {
        self.check_readable()?;
        let cmp = |k: &[u8]| {
            let h = u64le(k, 0).unwrap_or(0);
            let (o, t) = (h & 0x0FFF_FFFF_FFFF_FFFF, (h >> 60) as u8);
            o.cmp(&obj_id).then(t.cmp(&ty))
        };
        let root = self.read_virtual(self.root_tree_oid)?;
        btree::walk(root, &|child| self.read_virtual(child), &cmp, visit)
    }

    fn inode(&self, oid: u64) -> Result<Option<Inode>> {
        let mut out = None;
        self.scan(oid, J_TYPE_INODE, &mut |_, v| {
            out = Some(Inode::parse(v)?);
            Ok(false)
        })?;
        Ok(out)
    }

    fn entry_from(&self, name: String, oid: u64, ino: &Inode) -> Entry {
        let kind = match ino.mode & 0o170000 {
            0o040000 => EntryKind::Dir,
            0o120000 => EntryKind::Symlink,
            _ => EntryKind::File,
        };
        let compressed = kind == EntryKind::File && ino.bsd_flags & UF_COMPRESSED != 0;
        let mut size = if kind == EntryKind::Dir { ino.nchildren.max(0) as u64 } else { ino.size };
        if compressed {
            // A compressed file's data stream is empty; its real size is in the
            // inode when flagged, otherwise in the decmpfs header.
            if let Some(s) = ino.uncompressed_size {
                size = s;
            } else if let Ok(Some(attr)) = self.xattr(oid, decmpfs::ATTR_NAME) {
                if let Ok(Some(h)) = decmpfs::parse_header(&attr) {
                    size = h.size;
                }
            }
        }
        Entry {
            name,
            oid,
            parent: ino.parent_id,
            kind,
            mode: ino.mode,
            size,
            create_time: ino.create_ns / 1_000_000_000,
            modify_time: ino.modify_ns / 1_000_000_000,
            uid: ino.uid,
            gid: ino.gid,
            compressed,
            private_id: ino.private_id,
        }
    }

    /// Value of an extended attribute, or `None` if the inode has no such one.
    /// Stream-stored values (large ones, and resource forks) are read in full.
    pub fn xattr(&self, oid: u64, name: &str) -> Result<Option<Vec<u8>>> {
        match self.xattr_location(oid, name)? {
            None => Ok(None),
            Some(XattrData::Embedded(v)) => Ok(Some(v)),
            Some(XattrData::Stream { oid, size }) => {
                if size > MAX_XATTR_READ {
                    return Err(Error::Unsupported("extended attribute larger than 256 MiB".into()));
                }
                let mut buf = vec![0u8; size as usize];
                let n = self.read_stream(oid, size, 0, &mut buf)?;
                buf.truncate(n);
                Ok(Some(buf))
            }
        }
    }

    fn xattr_location(&self, oid: u64, name: &str) -> Result<Option<XattrData>> {
        let mut found = None;
        self.scan(oid, J_TYPE_XATTR, &mut |k, v| {
            if xattr_key_name(k)? != name {
                return Ok(true);
            }
            let flags = u16le(v, 0)?;
            let xlen = u16le(v, 2)? as usize;
            let data = slice(v, 4, xlen)?;
            found = Some(if flags & XATTR_DATA_EMBEDDED != 0 {
                XattrData::Embedded(data.to_vec())
            } else if flags & XATTR_DATA_STREAM != 0 {
                // xattr_obj_id (u64) then a j_dstream_t whose first field is the size.
                XattrData::Stream { oid: u64le(data, 0)?, size: u64le(data, 8)? }
            } else {
                return Err(Error::Format("extended attribute with unknown storage flags"));
            });
            Ok(false)
        })?;
        Ok(found)
    }

    /// Target of a symbolic link.
    pub fn read_link(&self, e: &Entry) -> Result<String> {
        if e.kind != EntryKind::Symlink {
            return Err(Error::Format("not a symbolic link"));
        }
        let v = self
            .xattr(e.oid, SYMLINK_XATTR)?
            .ok_or(Error::Format("symbolic link has no target attribute"))?;
        let end = v.iter().position(|&b| b == 0).unwrap_or(v.len());
        Ok(String::from_utf8_lossy(&v[..end]).into_owned())
    }

    pub fn root(&self) -> Result<Entry> {
        let ino = self.inode(ROOT_DIR_INO)?.ok_or(Error::Format("volume has no root directory inode"))?;
        Ok(self.entry_from(String::new(), ROOT_DIR_INO, &ino))
    }

    pub fn read_dir(&self, dir_oid: u64) -> Result<Vec<Entry>> {
        let mut recs: Vec<(String, u64)> = Vec::new();
        self.scan(dir_oid, J_TYPE_DIR_REC, &mut |k, v| {
            recs.push((drec_name(k)?, u64le(v, 0)?));
            Ok(true)
        })?;
        let mut out = Vec::with_capacity(recs.len());
        for (name, oid) in recs {
            let ino = self
                .inode(oid)?
                .ok_or(Error::Format("directory entry points at a missing inode"))?;
            out.push(self.entry_from(name, oid, &ino));
        }
        Ok(out)
    }

    fn names_equal(&self, a: &str, b: &str) -> bool {
        let ci = self.is_case_insensitive();
        let ni = self.incompat_features & APFS_INCOMPAT_NORMALIZATION_INSENSITIVE != 0;
        let fold = |s: &str| -> String {
            let s: String = if ci || ni { s.nfd().collect() } else { s.to_string() };
            if ci { s.to_lowercase() } else { s }
        };
        fold(a) == fold(b)
    }

    /// Resolve a '/'-separated path from the volume root.
    pub fn lookup(&self, path: &str) -> Result<Entry> {
        let mut cur = self.root()?;
        for comp in path.split('/').filter(|c| !c.is_empty()) {
            if cur.kind != EntryKind::Dir {
                return Err(Error::NotFound(path.to_string()));
            }
            cur = self
                .read_dir(cur.oid)?
                .into_iter()
                .find(|e| self.names_equal(&e.name, comp))
                .ok_or_else(|| Error::NotFound(path.to_string()))?;
        }
        Ok(cur)
    }

    pub fn read_data(&self, e: &Entry, offset: u64, buf: &mut [u8]) -> Result<usize> {
        if e.kind == EntryKind::Dir {
            return Err(Error::Format("is a directory"));
        }
        if e.compressed {
            let attr = self
                .xattr(e.oid, decmpfs::ATTR_NAME)?
                .ok_or(Error::Format("compressed file has no decmpfs attribute"))?;
            let header = decmpfs::parse_header(&attr)?.ok_or(Error::Format("bad decmpfs attribute"))?;
            let fork = if decmpfs::needs_resource_fork(header.method) {
                match self.xattr_location(e.oid, decmpfs::RESOURCE_FORK_NAME)? {
                    Some(XattrData::Stream { oid, size }) => Some(StreamFork { vol: self, oid, size }),
                    Some(XattrData::Embedded(_)) => {
                        return Err(Error::Unsupported("resource fork stored inline in its attribute".into()))
                    }
                    None => None,
                }
            } else {
                None
            };
            return decmpfs::read(&attr, fork.as_ref().map(|f| f as &dyn decmpfs::Fork), offset, buf);
        }
        self.read_stream(e.private_id, e.size, offset, buf)
    }

    /// Read `size` bytes of data stored as file extents under object id `oid`.
    fn read_stream(&self, oid: u64, size: u64, offset: u64, buf: &mut [u8]) -> Result<usize> {
        if offset >= size {
            return Ok(0);
        }
        let want = (buf.len() as u64).min(size - offset) as usize;
        let buf = &mut buf[..want];
        buf.fill(0); // holes and unwritten tails read as zeros
        let bs = self.c.block_size as u64;
        let end = offset + want as u64;

        let mut err = None;
        self.scan(oid, J_TYPE_FILE_EXTENT, &mut |k, v| {
            let laddr = u64le(k, 8)?;
            let len_flags = u64le(v, 0)?;
            let len = len_flags & 0x00FF_FFFF_FFFF_FFFF;
            let phys = u64le(v, 8)?;
            if laddr >= end {
                return Ok(false);
            }
            let ext_end = laddr.saturating_add(len);
            if ext_end <= offset || phys == 0 {
                return Ok(true);
            }
            let (lo, hi) = (offset.max(laddr), end.min(ext_end));
            let disk = match phys.checked_mul(bs).and_then(|p| p.checked_add(lo - laddr)) {
                Some(d) => d,
                None => {
                    err = Some(Error::Format("extent address overflow"));
                    return Ok(false);
                }
            };
            let dst = &mut buf[(lo - offset) as usize..(hi - offset) as usize];
            if let Err(e) = self.c.src.read_at(disk, dst) {
                err = Some(e);
                return Ok(false);
            }
            Ok(true)
        })?;
        match err {
            Some(e) => Err(e),
            None => Ok(want),
        }
    }
}

enum XattrData {
    Embedded(Vec<u8>),
    Stream { oid: u64, size: u64 },
}

/// A stream-stored attribute (the resource fork) presented to decmpfs.
struct StreamFork<'v, 'c, 'a> {
    vol: &'v Volume<'c, 'a>,
    oid: u64,
    size: u64,
}

impl decmpfs::Fork for StreamFork<'_, '_, '_> {
    fn len(&self) -> u64 {
        self.size
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        self.vol.read_stream(self.oid, self.size, offset, buf)
    }
}

/// Extended-attribute key: header, then u16 name length and NUL-terminated UTF-8.
fn xattr_key_name(k: &[u8]) -> Result<String> {
    let len = u16le(k, 8)? as usize;
    let raw = slice(k, 10, len)?;
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    Ok(String::from_utf8_lossy(&raw[..end]).into_owned())
}

struct Inode {
    parent_id: u64,
    private_id: u64,
    create_ns: i64,
    modify_ns: i64,
    nchildren: i32,
    bsd_flags: u32,
    uid: u32,
    gid: u32,
    mode: u16,
    size: u64,
    /// Decoded size of a compressed file, when the inode records it.
    uncompressed_size: Option<u64>,
}

impl Inode {
    fn parse(v: &[u8]) -> Result<Self> {
        let internal_flags = u64le(v, 48)?;
        let mut size = 0;
        // Optional extended fields: count, used bytes, then (type, flags, size)
        // descriptors followed by 8-byte-aligned data.
        if v.len() >= 96 {
            let n = u16le(v, 92)? as usize;
            let mut data = 96 + 4 * n;
            for i in 0..n {
                let d = slice(v, 96 + 4 * i, 4)?;
                let (ty, xsize) = (d[0], u16le(d, 2)? as usize);
                if ty == INO_EXT_TYPE_DSTREAM && xsize >= 8 {
                    size = u64le(v, data)?;
                }
                data += (xsize + 7) & !7;
            }
        }
        Ok(Self {
            parent_id: u64le(v, 0)?,
            private_id: u64le(v, 8)?,
            create_ns: u64le(v, 16)? as i64,
            modify_ns: u64le(v, 24)? as i64,
            nchildren: u32le(v, 56)? as i32,
            bsd_flags: u32le(v, 68)?,
            uid: u32le(v, 72)?,
            gid: u32le(v, 76)?,
            mode: u16le(v, 80)?,
            size,
            uncompressed_size: if internal_flags & INODE_HAS_UNCOMPRESSED_SIZE != 0 {
                Some(u64le(v, 84)?)
            } else {
                None
            },
        })
    }
}

/// Directory record key: header, then a hashed (u32 length+hash) or plain
/// (u16 length) name, NUL-terminated UTF-8.
fn drec_name(k: &[u8]) -> Result<String> {
    let hashed_len = (u32le(k, 8)? & 0x3FF) as usize;
    let (start, len) = if 12 + hashed_len == k.len() {
        (12, hashed_len)
    } else {
        (10, u16le(k, 8)? as usize)
    };
    let raw = slice(k, start, len)?;
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    Ok(String::from_utf8_lossy(&raw[..end]).into_owned())
}

impl crate::vfs::FileSystem for Volume<'_, '_> {
    type Entry = Entry;

    fn label(&self) -> String {
        self.name.clone()
    }
    fn stats(&self) -> crate::vfs::VolumeStats {
        crate::vfs::VolumeStats {
            total_bytes: self.c.block_count * self.c.block_size as u64,
            free_bytes: 0,
        }
    }
    fn root(&self) -> Result<Entry> {
        Volume::root(self)
    }
    fn info(&self, e: &Entry) -> crate::vfs::Info {
        crate::vfs::Info {
            id: e.oid,
            name: e.name.clone(),
            kind: e.kind,
            size: e.size,
            mode: e.mode,
            create_time: e.create_time,
            modify_time: e.modify_time,
            compressed: e.compressed,
        }
    }
    fn read_dir(&self, dir: &Entry) -> Result<Vec<Entry>> {
        Volume::read_dir(self, dir.oid)
    }
    fn lookup(&self, path: &str) -> Result<Entry> {
        Volume::lookup(self, path)
    }
    fn read(&self, e: &Entry, offset: u64, buf: &mut [u8]) -> Result<usize> {
        self.read_data(e, offset, buf)
    }
    fn read_link(&self, e: &Entry) -> Result<String> {
        Volume::read_link(self, e)
    }
}

/// Human-readable name of an APFS volume role.
pub fn role_name(role: u16) -> &'static str {
    match role {
        0x0000 => "none",
        0x0001 => "system",
        0x0002 => "user",
        0x0004 => "recovery",
        0x0008 => "vm",
        0x0010 => "preboot",
        0x0020 => "installer",
        0x0040 => "data",
        0x0080 => "baseband",
        0x00C0 => "update",
        0x0100 => "xart",
        0x0140 => "hardware",
        0x0180 => "backup",
        0x0240 => "enterprise",
        0x02C0 => "prelogin",
        _ => "other",
    }
}

impl Container<'_> {
    /// The volume a user most likely wants. On a Mac's startup disk the
    /// system volume holds the operating system and the Data volume holds the
    /// user's files, so Data wins, then a plain user volume, then an
    /// unclassified one, then the first volume.
    pub fn default_volume(&self) -> Result<usize> {
        let mut best: Option<(u8, usize)> = None;
        for i in 0..self.volume_count() {
            let rank = match self.volume(i).map(|v| v.role) {
                Ok(0x0040) => 4,
                Ok(0x0002) => 3,
                Ok(0x0000) => 2,
                Ok(_) => 1,
                Err(_) => 0,
            };
            if best.map_or(true, |(r, _)| rank > r) {
                best = Some((rank, i));
            }
        }
        best.map(|(_, i)| i).ok_or_else(|| Error::NotFound("any APFS volume".into()))
    }
}

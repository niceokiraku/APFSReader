//! Read-only HFS+ / HFSX driver.
//!
//! Limitations (tracked for later): the journal is not replayed, hard links
//! and decmpfs-compressed files are reported but not resolved, and name
//! comparison is a simple case-insensitive match rather than Apple's full
//! Unicode folding table.

mod btree;
mod fork;
pub mod journal;

use crate::decmpfs;
use crate::device::BlockSource;
use crate::util::{slice, u16be, u32be, u8at};
use crate::{Error, Result};
use btree::{key_span, BTree};
use std::sync::Mutex;
use unicode_normalization::UnicodeNormalization;
pub use fork::{Extent, ForkData};

const SIG_HFS_PLUS: u16 = 0x482B; // 'H+'
const SIG_HFSX: u16 = 0x4858; // 'HX'
const SIG_HFS_WRAPPER: u16 = 0x4244; // 'BD'

const ATTR_UNMOUNTED: u32 = 1 << 8;
const ATTR_INCONSISTENT: u32 = 1 << 11;
const ATTR_JOURNALED: u32 = 1 << 13;

const CNID_ROOT_FOLDER: u32 = 2;
const CNID_EXTENTS_FILE: u32 = 3;
const CNID_CATALOG_FILE: u32 = 4;

const FORK_DATA: u8 = 0x00;
const FORK_RESOURCE: u8 = 0xFF;

const HFS_TO_UNIX_EPOCH: i64 = 2_082_844_800;

fn hfs_time(t: u32) -> i64 {
    if t == 0 {
        0
    } else {
        t as i64 - HFS_TO_UNIX_EPOCH
    }
}

#[derive(Debug, Clone)]
pub struct VolumeHeader {
    pub signature: u16,
    pub version: u16,
    pub attributes: u32,
    pub last_mounted_version: u32,
    pub journal_info_block: u32,
    pub create_time: i64,
    pub modify_time: i64,
    pub file_count: u32,
    pub folder_count: u32,
    pub block_size: u32,
    pub total_blocks: u32,
    pub free_blocks: u32,
    pub next_catalog_id: u32,
    pub allocation_file: ForkData,
    pub extents_file: ForkData,
    pub catalog_file: ForkData,
    pub attributes_file: ForkData,
}

impl VolumeHeader {
    fn parse(b: &[u8]) -> Result<Self> {
        let signature = u16be(b, 0)?;
        match signature {
            SIG_HFS_PLUS | SIG_HFSX => {}
            SIG_HFS_WRAPPER => {
                return Err(Error::Format("HFS wrapper volume (embedded HFS+) is not supported yet"))
            }
            _ => return Err(Error::Format("not an HFS+ volume")),
        }
        let block_size = u32be(b, 40)?;
        if !block_size.is_power_of_two() || block_size < 512 {
            return Err(Error::Format("bad HFS+ allocation block size"));
        }
        Ok(Self {
            signature,
            version: u16be(b, 2)?,
            attributes: u32be(b, 4)?,
            last_mounted_version: u32be(b, 8)?,
            journal_info_block: u32be(b, 12)?,
            create_time: hfs_time(u32be(b, 16)?),
            modify_time: hfs_time(u32be(b, 20)?),
            file_count: u32be(b, 32)?,
            folder_count: u32be(b, 36)?,
            block_size,
            total_blocks: u32be(b, 44)?,
            free_blocks: u32be(b, 48)?,
            next_catalog_id: u32be(b, 64)?,
            allocation_file: ForkData::parse(slice(b, 112, ForkData::SIZE)?)?,
            extents_file: ForkData::parse(slice(b, 192, ForkData::SIZE)?)?,
            catalog_file: ForkData::parse(slice(b, 272, ForkData::SIZE)?)?,
            attributes_file: ForkData::parse(slice(b, 352, ForkData::SIZE)?)?,
        })
    }

    pub fn is_hfsx(&self) -> bool {
        self.signature == SIG_HFSX
    }
    pub fn is_journaled(&self) -> bool {
        self.attributes & ATTR_JOURNALED != 0
    }
    /// False if the volume was not cleanly unmounted. On a journaled volume the
    /// on-disk metadata may then be stale until the journal is replayed.
    pub fn is_clean(&self) -> bool {
        self.attributes & ATTR_UNMOUNTED != 0 && self.attributes & ATTR_INCONSISTENT == 0
    }
}

pub use crate::vfs::EntryKind;

const CNID_ATTRIBUTES_FILE: u32 = 8;
const FILETYPE_HLNK: u32 = 0x686C_6E6B; // 'hlnk'
const CREATOR_HFS_PLUS: u32 = 0x6866_732B; // 'hfs+'
const ATTR_RECORD_INLINE: u32 = 0x10;
const ATTR_RECORD_FORK: u32 = 0x20;
const MAX_XATTR_READ: u64 = 256 << 20;
const UF_COMPRESSED: u8 = 0x20;
const PRIVATE_DATA_PREFIX: &str = "\0\0\0\0HFS+ Private Data";
const PRIVATE_DIR_PREFIX: &str = ".HFS+ Private Directory Data";

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub cnid: u32,
    /// The CNID that owns this entry's forks and attributes: its own, or the
    /// shared inode for a hard link.
    pub file_id: u32,
    pub parent: u32,
    pub kind: EntryKind,
    pub mode: u16,
    /// Data length for files (decoded length when compressed), valence for directories.
    pub size: u64,
    pub create_time: i64,
    pub modify_time: i64,
    /// BSD UF_COMPRESSED: contents are stored by decmpfs.
    pub compressed: bool,
    pub data_fork: ForkData,
    pub resource_fork: ForkData,
    /// Inode number of a hard link target, until resolved.
    link_inode: Option<u32>,
}

impl Entry {
    fn root() -> Self {
        Self {
            name: String::new(),
            cnid: CNID_ROOT_FOLDER,
            file_id: CNID_ROOT_FOLDER,
            parent: 1,
            kind: EntryKind::Dir,
            mode: 0,
            size: 0,
            create_time: 0,
            modify_time: 0,
            compressed: false,
            data_fork: ForkData::default(),
            resource_fork: ForkData::default(),
            link_inode: None,
        }
    }
}

pub struct HfsPlus<'a> {
    src: &'a dyn BlockSource,
    pub header: VolumeHeader,
    overflow: Option<BTree<'a>>,
    catalog: BTree<'a>,
    attributes: Option<BTree<'a>>,
    /// Children of the hard-link private directory, loaded on first use.
    private_inodes: Mutex<Option<Vec<Entry>>>,
}

/// Extents of a fork: the eight in its record, plus any in the overflow tree.
fn resolve_extents(
    overflow: Option<&BTree<'_>>,
    fork: &ForkData,
    file_id: u32,
    fork_type: u8,
) -> Result<Vec<Extent>> {
    let mut out: Vec<Extent> = fork.extents.iter().copied().filter(|e| e.block_count > 0).collect();
    let mut covered: u64 = out.iter().map(|e| e.block_count as u64).sum();
    while covered < fork.total_blocks as u64 {
        let tree = overflow.ok_or(Error::Format("fork needs overflow extents but none exist"))?;
        let start = covered as u32;
        let search = (file_id, fork_type, start);
        let leaf = tree.descend(|rec| Ok(overflow_key(rec)? <= search))?;
        let mut found = None;
        for i in 0..leaf.num_records() {
            let rec = leaf.record(i);
            if overflow_key(rec)? == search {
                found = Some(rec);
                break;
            }
        }
        let rec = found.ok_or(Error::Format("missing overflow extent record"))?;
        let mut progressed = false;
        for i in 0..8 {
            let e = Extent::parse(slice(rec, 12 + i * 8, 8)?)?;
            if e.block_count == 0 {
                break;
            }
            covered += e.block_count as u64;
            out.push(e);
            progressed = true;
        }
        if !progressed {
            return Err(Error::Format("empty overflow extent record"));
        }
    }
    Ok(out)
}

/// Overflow key ordered as (fileID, forkType, startBlock).
fn overflow_key(rec: &[u8]) -> Result<(u32, u8, u32)> {
    Ok((u32be(rec, 4)?, u8at(rec, 2)?, u32be(rec, 8)?))
}

fn utf16_be(raw: &[u8]) -> Vec<u16> {
    raw.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect()
}

fn decode_name(rec: &[u8]) -> Result<String> {
    let klen = u16be(rec, 0)? as usize;
    let nlen = u16be(rec, 6)? as usize;
    if 6 + 2 * nlen > klen {
        return Err(Error::Format("catalog name longer than key"));
    }
    let units = utf16_be(slice(rec, 8, 2 * nlen)?);
    // On-disk names are NFD; present them as NFC (what Windows users expect).
    Ok(String::from_utf16_lossy(&units).nfc().collect())
}

/// Attribute key: keyLength, pad, fileID, startBlock, then a UTF-16 name.
fn attr_key_name(rec: &[u8]) -> Result<String> {
    let klen = u16be(rec, 0)? as usize;
    let nlen = u16be(rec, 12)? as usize;
    if 12 + 2 * nlen > klen {
        return Err(Error::Format("attribute name longer than key"));
    }
    Ok(String::from_utf16_lossy(&utf16_be(slice(rec, 14, 2 * nlen)?)))
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('\0') || name.starts_with(PRIVATE_DIR_PREFIX)
}

fn parse_entry(parent: u32, name: String, d: &[u8]) -> Result<Option<Entry>> {
    let rtype = u16be(d, 0)? as i16;
    let is_dir = match rtype {
        1 => true,
        2 => false,
        _ => return Ok(None),
    };
    let cnid = u32be(d, 8)?;
    let mode = u16be(d, 42)?;
    let mut e = Entry {
        name,
        cnid,
        file_id: cnid,
        parent,
        kind: EntryKind::Dir,
        mode,
        size: 0,
        create_time: hfs_time(u32be(d, 12)?),
        modify_time: hfs_time(u32be(d, 16)?),
        compressed: false,
        data_fork: ForkData::default(),
        resource_fork: ForkData::default(),
        link_inode: None,
    };
    if is_dir {
        e.size = u32be(d, 4)? as u64;
    } else {
        e.data_fork = ForkData::parse(slice(d, 88, ForkData::SIZE)?)?;
        e.resource_fork = ForkData::parse(slice(d, 168, ForkData::SIZE)?)?;
        e.size = e.data_fork.logical_size;
        e.compressed = u8at(d, 41)? & UF_COMPRESSED != 0;
        e.kind = if mode & 0o170000 == 0o120000 { EntryKind::Symlink } else { EntryKind::File };
        // Hard links are stand-in files; the data lives in iNode<n> in the
        // private directory, n being the BSD "special" field.
        if u32be(d, 48)? == FILETYPE_HLNK && u32be(d, 52)? == CREATOR_HFS_PLUS {
            e.link_inode = Some(u32be(d, 44)?);
        }
    }
    Ok(Some(e))
}

/// A resource fork presented to decmpfs.
struct ForkReader<'s> {
    src: &'s dyn BlockSource,
    block_size: u64,
    extents: Vec<Extent>,
    size: u64,
}

impl decmpfs::Fork for ForkReader<'_> {
    fn len(&self) -> u64 {
        self.size
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<usize> {
        fork::read_extents(self.src, self.block_size, &self.extents, self.size, offset, buf)
    }
}

impl<'a> HfsPlus<'a> {
    pub fn open(src: &'a dyn BlockSource) -> Result<Self> {
        let mut b = [0u8; 512];
        src.read_at(1024, &mut b)?;
        let header = VolumeHeader::parse(&b)?;
        let bs = header.block_size as u64;

        let overflow = if header.extents_file.logical_size > 0 {
            let ex: Vec<Extent> =
                header.extents_file.extents.iter().copied().filter(|e| e.block_count > 0).collect();
            Some(BTree::open(src, bs, ex, header.extents_file.logical_size)?)
        } else {
            None
        };
        let cat_extents =
            resolve_extents(overflow.as_ref(), &header.catalog_file, CNID_CATALOG_FILE, FORK_DATA)?;
        let catalog = BTree::open(src, bs, cat_extents, header.catalog_file.logical_size)?;
        let attributes = if header.attributes_file.logical_size > 0 {
            let ex = resolve_extents(
                overflow.as_ref(),
                &header.attributes_file,
                CNID_ATTRIBUTES_FILE,
                FORK_DATA,
            )?;
            Some(BTree::open(src, bs, ex, header.attributes_file.logical_size)?)
        } else {
            None
        };
        let _ = CNID_EXTENTS_FILE;
        Ok(Self { src, header, overflow, catalog, attributes, private_inodes: Mutex::new(None) })
    }

    /// Every catalog record whose parent is `parent`, as stored: nothing is
    /// hidden and hard links are not resolved.
    fn catalog_children(&self, parent: u32) -> Result<Vec<Entry>> {
        let leaf = self.catalog.descend(|rec| {
            let p = u32be(rec, 2)?;
            let nlen = u16be(rec, 6)?;
            Ok(p < parent || (p == parent && nlen == 0))
        })?;
        let mut out = Vec::new();
        let mut node = leaf;
        let mut hops = 0u32;
        loop {
            for i in 0..node.num_records() {
                let rec = node.record(i);
                let p = u32be(rec, 2)?;
                if p < parent {
                    continue;
                }
                if p > parent {
                    return Ok(out);
                }
                let span = key_span(rec)?;
                let name = decode_name(rec)?;
                if let Some(e) = parse_entry(parent, name, &rec[span..])? {
                    out.push(e);
                }
            }
            let next = node.flink();
            if next == 0 {
                break;
            }
            hops += 1;
            if hops > self.catalog.total_nodes {
                return Err(Error::Format("B-tree leaf chain loops"));
            }
            node = self.catalog.node(next)?;
        }
        Ok(out)
    }

    /// Children of a directory, in catalog order. The hard-link private
    /// directories are hidden; hard links and compressed sizes are resolved.
    pub fn read_dir(&self, parent: u32) -> Result<Vec<Entry>> {
        let mut out = Vec::new();
        for mut e in self.catalog_children(parent)? {
            if is_hidden(&e.name) {
                continue;
            }
            self.finish_entry(&mut e)?;
            out.push(e);
        }
        Ok(out)
    }

    fn finish_entry(&self, e: &mut Entry) -> Result<()> {
        if let Some(n) = e.link_inode {
            self.resolve_link(e, n)?;
        }
        if e.compressed && e.kind == EntryKind::File {
            if let Ok(Some(attr)) = self.attribute(e.file_id, decmpfs::ATTR_NAME) {
                if let Ok(Some(h)) = decmpfs::parse_header(&attr) {
                    e.size = h.size;
                }
            }
        }
        Ok(())
    }

    /// Point a hard link at the shared inode that holds the data.
    fn resolve_link(&self, e: &mut Entry, inode_number: u32) -> Result<()> {
        let mut cache = self.private_inodes.lock().unwrap();
        if cache.is_none() {
            let private = self
                .catalog_children(CNID_ROOT_FOLDER)?
                .into_iter()
                .find(|c| c.kind == EntryKind::Dir && c.name.starts_with(PRIVATE_DATA_PREFIX));
            *cache = Some(match private {
                Some(dir) => self.catalog_children(dir.cnid)?,
                None => Vec::new(),
            });
        }
        let want = format!("iNode{inode_number}");
        let target = cache
            .as_ref()
            .unwrap()
            .iter()
            .find(|i| i.name == want)
            .ok_or(Error::Format("hard link target is missing from the private directory"))?;
        e.data_fork = target.data_fork.clone();
        e.resource_fork = target.resource_fork.clone();
        e.size = target.size;
        e.compressed = target.compressed;
        e.mode = target.mode;
        e.kind = target.kind;
        e.file_id = target.cnid;
        e.link_inode = None;
        Ok(())
    }

    fn names_equal(&self, a: &str, b: &str) -> bool {
        let fold = |s: &str| -> String {
            let s: String = s.nfd().collect();
            if self.header.is_hfsx() { s } else { s.to_lowercase() }
        };
        fold(a) == fold(b)
    }

    /// Resolve a '/'-separated path from the volume root.
    pub fn lookup(&self, path: &str) -> Result<Entry> {
        let mut cur = Entry::root();
        for comp in path.split('/').filter(|c| !c.is_empty()) {
            if cur.kind != EntryKind::Dir {
                return Err(Error::NotFound(path.to_string()));
            }
            cur = self
                .read_dir(cur.cnid)?
                .into_iter()
                .find(|e| self.names_equal(&e.name, comp))
                .ok_or_else(|| Error::NotFound(path.to_string()))?;
        }
        Ok(cur)
    }

    /// Visit the attribute records of `file_id` as (name, record payload).
    fn scan_attributes(
        &self,
        file_id: u32,
        visit: &mut dyn FnMut(&str, &[u8]) -> Result<bool>,
    ) -> Result<()> {
        let Some(tree) = self.attributes.as_ref() else { return Ok(()) };
        if tree.root == 0 {
            return Ok(());
        }
        let mut node = tree.descend(|rec| {
            let p = u32be(rec, 4)?;
            let nlen = u16be(rec, 12)?;
            Ok(p < file_id || (p == file_id && nlen == 0))
        })?;
        let mut hops = 0u32;
        loop {
            for i in 0..node.num_records() {
                let rec = node.record(i);
                let p = u32be(rec, 4)?;
                if p < file_id {
                    continue;
                }
                if p > file_id {
                    return Ok(());
                }
                let span = key_span(rec)?;
                if !visit(&attr_key_name(rec)?, &rec[span..])? {
                    return Ok(());
                }
            }
            let next = node.flink();
            if next == 0 {
                return Ok(());
            }
            hops += 1;
            if hops > tree.total_nodes {
                return Err(Error::Format("B-tree leaf chain loops"));
            }
            node = tree.node(next)?;
        }
    }

    /// Value held by an attribute record: inline, or in its own fork.
    fn attr_value(&self, rec: &[u8]) -> Result<Option<Vec<u8>>> {
        match u32be(rec, 0)? {
            ATTR_RECORD_INLINE => {
                let size = u32be(rec, 12)? as usize;
                Ok(Some(slice(rec, 16, size)?.to_vec()))
            }
            ATTR_RECORD_FORK => {
                let fork = ForkData::parse(slice(rec, 8, ForkData::SIZE)?)?;
                if fork.logical_size > MAX_XATTR_READ {
                    return Err(Error::Unsupported("extended attribute larger than 256 MiB".into()));
                }
                let extents: Vec<Extent> =
                    fork.extents.iter().copied().filter(|e| e.block_count > 0).collect();
                let covered: u64 = extents.iter().map(|e| e.block_count as u64).sum();
                if covered < fork.total_blocks as u64 {
                    return Err(Error::Unsupported("attribute fork with overflow extents".into()));
                }
                let mut buf = vec![0u8; fork.logical_size as usize];
                let n = fork::read_extents(
                    self.src,
                    self.header.block_size as u64,
                    &extents,
                    fork.logical_size,
                    0,
                    &mut buf,
                )?;
                buf.truncate(n);
                Ok(Some(buf))
            }
            _ => Ok(None), // extent records and anything newer
        }
    }

    fn attribute(&self, file_id: u32, name: &str) -> Result<Option<Vec<u8>>> {
        let mut found = None;
        let mut failure = None;
        self.scan_attributes(file_id, &mut |n, rec| {
            if n != name {
                return Ok(true);
            }
            match self.attr_value(rec) {
                Ok(Some(v)) => found = Some(v),
                Ok(None) => return Ok(true),
                Err(e) => failure = Some(e),
            }
            Ok(false)
        })?;
        match failure {
            Some(e) => Err(e),
            None => Ok(found),
        }
    }

    /// Extended attribute value. `com.apple.ResourceFork` maps to the resource
    /// fork, as on macOS.
    pub fn xattr(&self, e: &Entry, name: &str) -> Result<Option<Vec<u8>>> {
        if name == decmpfs::RESOURCE_FORK_NAME {
            let size = e.resource_fork.logical_size;
            if size == 0 {
                return Ok(None);
            }
            if size > MAX_XATTR_READ {
                return Err(Error::Unsupported("resource fork larger than 256 MiB".into()));
            }
            let mut buf = vec![0u8; size as usize];
            let n = self.read_resource(e, 0, &mut buf)?;
            buf.truncate(n);
            return Ok(Some(buf));
        }
        self.attribute(e.file_id, name)
    }

    fn read_fork(&self, e: &Entry, fork_type: u8, offset: u64, buf: &mut [u8]) -> Result<usize> {
        if e.kind == EntryKind::Dir {
            return Err(Error::Format("is a directory"));
        }
        let fork = if fork_type == FORK_RESOURCE { &e.resource_fork } else { &e.data_fork };
        let extents = resolve_extents(self.overflow.as_ref(), fork, e.file_id, fork_type)?;
        fork::read_extents(
            self.src,
            self.header.block_size as u64,
            &extents,
            fork.logical_size,
            offset,
            buf,
        )
    }

    pub fn read_data(&self, e: &Entry, offset: u64, buf: &mut [u8]) -> Result<usize> {
        if e.compressed && e.kind == EntryKind::File {
            let attr = self
                .attribute(e.file_id, decmpfs::ATTR_NAME)?
                .ok_or(Error::Format("compressed file has no decmpfs attribute"))?;
            let header = decmpfs::parse_header(&attr)?.ok_or(Error::Format("bad decmpfs attribute"))?;
            let fork = if decmpfs::needs_resource_fork(header.method) {
                let extents = resolve_extents(
                    self.overflow.as_ref(),
                    &e.resource_fork,
                    e.file_id,
                    FORK_RESOURCE,
                )?;
                Some(ForkReader {
                    src: self.src,
                    block_size: self.header.block_size as u64,
                    extents,
                    size: e.resource_fork.logical_size,
                })
            } else {
                None
            };
            return decmpfs::read(&attr, fork.as_ref().map(|f| f as &dyn decmpfs::Fork), offset, buf);
        }
        self.read_fork(e, FORK_DATA, offset, buf)
    }

    pub fn read_resource(&self, e: &Entry, offset: u64, buf: &mut [u8]) -> Result<usize> {
        self.read_fork(e, FORK_RESOURCE, offset, buf)
    }

    /// Target of a symbolic link (stored as the file's data).
    pub fn read_link(&self, e: &Entry) -> Result<String> {
        if e.kind != EntryKind::Symlink {
            return Err(Error::Format("not a symbolic link"));
        }
        let mut buf = vec![0u8; e.size.min(65536) as usize];
        let n = self.read_data(e, 0, &mut buf)?;
        Ok(String::from_utf8_lossy(&buf[..n]).into_owned())
    }
}

impl crate::vfs::FileSystem for HfsPlus<'_> {
    type Entry = Entry;

    fn label(&self) -> String {
        // The root folder's record sits under parent id 1, named after the volume.
        self.catalog_children(1)
            .ok()
            .and_then(|v| v.into_iter().next())
            .map(|e| e.name)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "HFS+".to_string())
    }
    fn stats(&self) -> crate::vfs::VolumeStats {
        let bs = self.header.block_size as u64;
        crate::vfs::VolumeStats {
            total_bytes: self.header.total_blocks as u64 * bs,
            free_bytes: self.header.free_blocks as u64 * bs,
        }
    }
    fn root(&self) -> Result<Entry> {
        Ok(Entry::root())
    }
    fn info(&self, e: &Entry) -> crate::vfs::Info {
        crate::vfs::Info {
            id: e.cnid as u64,
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
        HfsPlus::read_dir(self, dir.cnid)
    }
    fn lookup(&self, path: &str) -> Result<Entry> {
        HfsPlus::lookup(self, path)
    }
    fn read(&self, e: &Entry, offset: u64, buf: &mut [u8]) -> Result<usize> {
        self.read_data(e, offset, buf)
    }
    fn read_link(&self, e: &Entry) -> Result<String> {
        HfsPlus::read_link(self, e)
    }
}

//! A file-system-neutral, read-only view, so front ends (the CLI, a WinFsp
//! mount) can serve HFS+ and APFS volumes through one code path.

use crate::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Dir,
    File,
    Symlink,
}

/// What a front end needs to know about an entry.
#[derive(Debug, Clone)]
pub struct Info {
    /// Stable identifier within the volume (catalog node id / inode number).
    pub id: u64,
    pub name: String,
    pub kind: EntryKind,
    /// Byte length for files and links; child count for directories.
    pub size: u64,
    pub mode: u16,
    /// Seconds since the Unix epoch (0 if unknown).
    pub create_time: i64,
    pub modify_time: i64,
    /// True if the contents are stored with transparent compression.
    pub compressed: bool,
}

/// Capacity figures for a volume, in bytes.
#[derive(Debug, Clone, Copy, Default)]
pub struct VolumeStats {
    pub total_bytes: u64,
    pub free_bytes: u64,
}

/// A mounted, read-only volume. Entries are cheap handles produced by the
/// volume itself; they are only meaningful to the volume that made them.
pub trait FileSystem: Send + Sync {
    type Entry: Clone + Send + Sync;

    fn label(&self) -> String;
    fn stats(&self) -> VolumeStats {
        VolumeStats::default()
    }
    fn root(&self) -> Result<Self::Entry>;
    fn info(&self, e: &Self::Entry) -> Info;
    fn read_dir(&self, dir: &Self::Entry) -> Result<Vec<Self::Entry>>;
    /// Resolve a '/'-separated path from the root.
    fn lookup(&self, path: &str) -> Result<Self::Entry>;
    /// Read file contents; returns the bytes read (short only at end of file).
    fn read(&self, e: &Self::Entry, offset: u64, buf: &mut [u8]) -> Result<usize>;
    fn read_link(&self, e: &Self::Entry) -> Result<String>;
}

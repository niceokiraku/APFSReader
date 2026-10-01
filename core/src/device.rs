use crate::{Error, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Mutex;

/// Read-only random-access block source (disk image or raw device).
pub trait BlockSource: Send + Sync {
    fn len(&self) -> u64;
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()>;

    /// Logical sector size of the underlying device. Images are taken to use
    /// 512-byte sectors; a 4Kn disk reports 4096. Partition tables and the HFS+
    /// journal count in these units.
    fn sector_size(&self) -> u64 {
        512
    }
}

/// A file-backed source (disk images; raw devices later).
pub struct FileSource {
    file: Mutex<File>,
    len: u64,
}

impl FileSource {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Ok(Self { file: Mutex::new(file), len })
    }
}

impl BlockSource for FileSource {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let end = offset.checked_add(buf.len() as u64);
        if end.map_or(true, |e| e > self.len) {
            return Err(Error::OutOfRange { offset, len: buf.len() });
        }
        let mut f = self.file.lock().unwrap();
        f.seek(SeekFrom::Start(offset))?;
        f.read_exact(buf)?;
        Ok(())
    }
}

/// A window onto a source, used for a partition.
pub struct Slice<'a> {
    inner: &'a dyn BlockSource,
    start: u64,
    len: u64,
}

impl<'a> Slice<'a> {
    pub fn new(inner: &'a dyn BlockSource, start: u64, len: u64) -> Self {
        Self { inner, start, len }
    }
}

impl BlockSource for Slice<'_> {
    fn len(&self) -> u64 {
        self.len
    }

    fn sector_size(&self) -> u64 {
        self.inner.sector_size()
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let end = offset.checked_add(buf.len() as u64);
        if end.map_or(true, |e| e > self.len) {
            return Err(Error::OutOfRange { offset, len: buf.len() });
        }
        self.inner.read_at(self.start + offset, buf)
    }
}

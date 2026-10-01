//! Sector-aligned reading for raw devices.
//!
//! A raw disk only accepts reads whose offset and length are whole multiples
//! of its sector size, while the file system code asks for arbitrary byte
//! ranges. `AlignedSource` bridges the two: every request to the device is
//! aligned, and recently read chunks are cached so the many small reads of a
//! B-tree walk do not each become a device request.
//!
//! The device side is the `RawRead` trait, so the alignment logic is testable
//! with a fake device that refuses misaligned requests.

use crate::device::BlockSource;
use crate::{Error, Result};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Unit of caching and read-ahead.
const CHUNK: u64 = 256 * 1024;
const CACHE_CHUNKS: usize = 64;

/// A device that only serves sector-aligned reads.
pub trait RawRead: Send + Sync {
    /// Fill `buf` from `offset`. Both are multiples of the sector size and the
    /// range lies inside the device.
    fn read_aligned(&self, offset: u64, buf: &mut [u8]) -> Result<()>;
}

pub struct AlignedSource<R: RawRead> {
    raw: R,
    len: u64,
    sector: u64,
    cache: Mutex<HashMap<u64, Arc<Vec<u8>>>>,
}

impl<R: RawRead> AlignedSource<R> {
    /// `len` and `sector` describe the device; `len` is rounded down to a
    /// whole number of sectors.
    pub fn new(raw: R, len: u64, sector: u64) -> Result<Self> {
        if !sector.is_power_of_two() || !(512..=65536).contains(&sector) {
            return Err(Error::Format("implausible device sector size"));
        }
        Ok(Self { raw, len: len / sector * sector, sector, cache: Mutex::new(HashMap::new()) })
    }

    fn chunk(&self, index: u64) -> Result<Arc<Vec<u8>>> {
        if let Some(c) = self.cache.lock().unwrap().get(&index) {
            return Ok(c.clone());
        }
        let start = index * CHUNK;
        let size = CHUNK.min(self.len - start) as usize;
        let mut data = vec![0u8; size];
        self.raw.read_aligned(start, &mut data)?;
        let data = Arc::new(data);
        let mut cache = self.cache.lock().unwrap();
        if cache.len() >= CACHE_CHUNKS {
            cache.clear();
        }
        cache.insert(index, data.clone());
        Ok(data)
    }
}

impl<R: RawRead> BlockSource for AlignedSource<R> {
    fn len(&self) -> u64 {
        self.len
    }

    fn sector_size(&self) -> u64 {
        self.sector
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let end = offset.checked_add(buf.len() as u64).filter(|&e| e <= self.len);
        let Some(end) = end else {
            return Err(Error::OutOfRange { offset, len: buf.len() });
        };
        if buf.is_empty() {
            return Ok(());
        }
        // Large reads go straight to the device, widened to whole sectors.
        if buf.len() as u64 >= CHUNK {
            let start = offset / self.sector * self.sector;
            let stop = end.div_ceil(self.sector) * self.sector;
            let mut tmp = vec![0u8; (stop - start) as usize];
            self.raw.read_aligned(start, &mut tmp)?;
            let skip = (offset - start) as usize;
            buf.copy_from_slice(&tmp[skip..skip + buf.len()]);
            return Ok(());
        }
        let mut done = 0usize;
        while done < buf.len() {
            let pos = offset + done as u64;
            let index = pos / CHUNK;
            let chunk = self.chunk(index)?;
            let within = (pos - index * CHUNK) as usize;
            let n = (chunk.len() - within).min(buf.len() - done);
            buf[done..done + n].copy_from_slice(&chunk[within..within + n]);
            done += n;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Refuses any request that is not whole sectors inside the device, like a
    /// real disk handle.
    struct Strict {
        data: Vec<u8>,
        sector: u64,
        requests: AtomicUsize,
    }
    impl RawRead for Strict {
        fn read_aligned(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
            self.requests.fetch_add(1, Ordering::Relaxed);
            assert_eq!(offset % self.sector, 0, "unaligned offset {offset}");
            assert_eq!(buf.len() as u64 % self.sector, 0, "unaligned length {}", buf.len());
            let end = offset as usize + buf.len();
            assert!(end <= self.data.len(), "read past the device");
            buf.copy_from_slice(&self.data[offset as usize..end]);
            Ok(())
        }
    }

    fn device(sector: u64, sectors: u64) -> (AlignedSource<Strict>, Vec<u8>) {
        let data: Vec<u8> = (0..sector * sectors).map(|i| (i * 31 % 251) as u8).collect();
        let src = AlignedSource::new(
            Strict { data: data.clone(), sector, requests: AtomicUsize::new(0) },
            data.len() as u64,
            sector,
        )
        .unwrap();
        (src, data)
    }

    #[test]
    fn arbitrary_ranges_read_correctly_through_a_strict_device() {
        for sector in [512u64, 4096] {
            let (src, data) = device(sector, 3000);
            for (off, len) in [(0, 1), (1, 1), (511, 2), (4095, 3), (5, 700_000), (262_143, 2), (1_000_001, 300_000), (data.len() as u64 - 7, 7)] {
                let mut buf = vec![0u8; len];
                src.read_at(off, &mut buf).unwrap();
                assert_eq!(&buf[..], &data[off as usize..off as usize + len], "sector {sector} off {off} len {len}");
            }
        }
    }

    #[test]
    fn reads_outside_the_device_are_rejected() {
        let (src, data) = device(512, 1000);
        let mut buf = [0u8; 8];
        assert!(src.read_at(data.len() as u64 - 7, &mut buf).is_err());
        assert!(src.read_at(u64::MAX - 3, &mut buf).is_err());
        assert!(src.read_at(0, &mut []).is_ok());
    }

    #[test]
    fn small_reads_in_one_chunk_hit_the_device_once() {
        let (src, _) = device(512, 3000);
        let mut b = [0u8; 16];
        for off in [0u64, 100, 20_000, 65_000, 200_000] {
            src.read_at(off, &mut b).unwrap();
        }
        assert_eq!(src.raw.requests.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn a_length_that_is_not_a_whole_number_of_sectors_is_trimmed() {
        let data = vec![7u8; 5000];
        let src = AlignedSource::new(
            Strict { data: data.clone(), sector: 512, requests: AtomicUsize::new(0) },
            5000,
            512,
        )
        .unwrap();
        assert_eq!(src.len(), 4608);
        let mut b = [0u8; 4];
        assert!(src.read_at(4606, &mut b).is_err());
        assert!(src.read_at(4604, &mut b).is_ok());
    }

    #[test]
    fn nonsense_sector_sizes_are_refused() {
        let raw = || Strict { data: vec![0; 1024], sector: 512, requests: AtomicUsize::new(0) };
        assert!(AlignedSource::new(raw(), 1024, 100).is_err());
        assert!(AlignedSource::new(raw(), 1024, 0).is_err());
    }
}

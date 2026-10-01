//! HFS+ journal replay, done in memory.
//!
//! A volume that was not cleanly unmounted can hold committed metadata changes
//! only in its journal. `JournaledSource` wraps a volume and serves blocks as
//! they would be after a replay, without writing anything to the device.
//!
//! Format (Apple's vfs_journal.c, TN1150): the JournalInfoBlock names the
//! journal's offset and size; the journal starts with a header (in the byte
//! order of the machine that wrote it) and is a circular log of transactions,
//! each a block-list header followed by the new contents of the listed blocks.

use crate::device::BlockSource;
use crate::util::{slice, u32be, u64be};
use crate::{Error, Result};
use std::collections::HashMap;

const JOURNAL_MAGIC: u32 = 0x4a4e_4c78; // 'JNLx'
const ENDIAN_MAGIC: u32 = 0x1234_5678;
const HEADER_CKSUM_LEN: usize = 44;
const BLHDR_CKSUM_LEN: usize = 32;
const BLHDR_SIZE_MIN: usize = 32;
const SECTOR: u64 = 512;

const JI_IN_FS: u32 = 0x1;
const JI_NEED_INIT: u32 = 0x4;
const ATTR_JOURNALED: u32 = 1 << 13;

/// Upper bounds that keep a hostile journal from exhausting memory.
const MAX_TRANSACTIONS: usize = 1 << 20;
const MAX_REPLAY_BYTES: u64 = 512 << 20;

/// Apple's journal checksum.
fn checksum(data: &[u8]) -> u32 {
    let mut sum: u32 = 0;
    for &b in data {
        sum = (sum << 8) ^ sum.wrapping_add(b as u32);
    }
    !sum
}

#[derive(Clone, Copy)]
struct Order(bool); // true = journal fields are little-endian

impl Order {
    fn u32(self, b: &[u8], o: usize) -> Result<u32> {
        let s = slice(b, o, 4)?;
        Ok(if self.0 {
            u32::from_le_bytes(s.try_into().unwrap())
        } else {
            u32::from_be_bytes(s.try_into().unwrap())
        })
    }
    fn u64(self, b: &[u8], o: usize) -> Result<u64> {
        let s = slice(b, o, 8)?;
        Ok(if self.0 {
            u64::from_le_bytes(s.try_into().unwrap())
        } else {
            u64::from_be_bytes(s.try_into().unwrap())
        })
    }
}

/// What a replay found.
#[derive(Debug, Clone, Default)]
pub struct JournalReport {
    /// The volume header says it has a journal.
    pub journaled: bool,
    /// Committed transactions that were applied in memory.
    pub transactions: usize,
    /// 512-byte sectors those transactions rewrite.
    pub sectors: usize,
    /// Set if the log ended at a transaction that failed validation; the
    /// transactions before it were still applied.
    pub stopped_early: bool,
}

/// A volume as it would look after journal replay.
pub struct JournaledSource<'a> {
    inner: &'a dyn BlockSource,
    overlay: HashMap<u64, Box<[u8; SECTOR as usize]>>,
    /// Bytes per journal block number: the device's logical sector size.
    unit: u64,
    pub report: JournalReport,
}

impl<'a> JournaledSource<'a> {
    /// Wrap `inner`, which must start at the HFS+ volume. A volume that is not
    /// HFS+, not journaled or has an empty log is passed through unchanged.
    pub fn new(inner: &'a dyn BlockSource) -> Result<Self> {
        let unit = inner.sector_size().max(SECTOR);
        let mut out = Self { inner, overlay: HashMap::new(), unit, report: JournalReport::default() };
        let mut vh = [0u8; 512];
        inner.read_at(1024, &mut vh)?;
        if &vh[0..2] != b"H+" && &vh[0..2] != b"HX" {
            return Ok(out);
        }
        let attributes = u32be(&vh, 4)?;
        if attributes & ATTR_JOURNALED == 0 {
            return Ok(out);
        }
        out.report.journaled = true;
        let block_size = u32be(&vh, 40)? as u64;
        let jib_block = u32be(&vh, 12)? as u64;
        if !block_size.is_power_of_two() || block_size < 512 || jib_block == 0 {
            return Ok(out);
        }
        out.replay(block_size, jib_block)?;
        Ok(out)
    }

    fn replay(&mut self, block_size: u64, jib_block: u64) -> Result<()> {
        let mut jib = [0u8; 52];
        self.inner.read_at(jib_block * block_size, &mut jib)?;
        let flags = u32be(&jib, 0)?;
        if flags & JI_NEED_INIT != 0 {
            return Ok(()); // journal never initialised: nothing in it
        }
        if flags & JI_IN_FS == 0 {
            return Err(Error::Unsupported("HFS+ journal on another device".into()));
        }
        let (jnl_off, jnl_size) = (u64be(&jib, 36)?, u64be(&jib, 44)?);
        if jnl_size < 4096 || jnl_off.checked_add(jnl_size).map_or(true, |e| e > self.inner.len()) {
            return Err(Error::Format("HFS+ journal lies outside the volume"));
        }

        let mut hdr = [0u8; 52];
        self.inner.read_at(jnl_off, &mut hdr)?;
        let order = match (u32::from_le_bytes(hdr[4..8].try_into().unwrap()), u32::from_be_bytes(hdr[4..8].try_into().unwrap())) {
            (ENDIAN_MAGIC, _) => Order(true),
            (_, ENDIAN_MAGIC) => Order(false),
            _ => return Err(Error::Format("HFS+ journal header has no valid byte-order marker")),
        };
        if order.u32(&hdr, 0)? != JOURNAL_MAGIC {
            return Err(Error::Format("bad HFS+ journal magic"));
        }
        let mut check = hdr;
        check[36..40].fill(0);
        if checksum(&check[..HEADER_CKSUM_LEN]) != order.u32(&hdr, 36)? {
            return Err(Error::Format("HFS+ journal header checksum mismatch"));
        }
        let (start, end, size) = (order.u64(&hdr, 8)?, order.u64(&hdr, 16)?, order.u64(&hdr, 24)?);
        let blhdr_size = order.u32(&hdr, 32)? as usize;
        let jhdr_size = order.u32(&hdr, 40)? as u64;
        if size != jnl_size
            || jhdr_size < 512
            || jhdr_size >= size
            || blhdr_size < BLHDR_SIZE_MIN
            || blhdr_size as u64 > size / 2
            || start < jhdr_size
            || start >= size
            || end < jhdr_size
            || end >= size
        {
            return Err(Error::Format("implausible HFS+ journal header"));
        }
        if start == end {
            return Ok(()); // empty log: everything is already on disk
        }

        let log = Log { src: self.inner, base: jnl_off, size, jhdr_size };
        let mut pos = start;
        let mut replayed_bytes = 0u64;
        while pos != end {
            if self.report.transactions >= MAX_TRANSACTIONS {
                self.report.stopped_early = true;
                break;
            }
            match self.read_transaction(&log, order, blhdr_size, pos, &mut replayed_bytes) {
                Ok(next) => {
                    self.report.transactions += 1;
                    pos = next;
                }
                Err(_) => {
                    // Apple's replay also stops at the first bad transaction and
                    // treats everything before it as committed.
                    self.report.stopped_early = true;
                    break;
                }
            }
        }
        self.report.sectors = self.overlay.len();
        Ok(())
    }

    /// Apply one transaction to the overlay; returns the next transaction's position.
    fn read_transaction(
        &mut self,
        log: &Log,
        order: Order,
        blhdr_size: usize,
        pos: u64,
        replayed: &mut u64,
    ) -> Result<u64> {
        let blhdr = log.read(pos, blhdr_size)?;
        let max_blocks = u16_of(order, &blhdr, 0)? as usize;
        let num_blocks = u16_of(order, &blhdr, 2)? as usize;
        let bytes_used = order.u32(&blhdr, 4)? as u64;
        let stored = order.u32(&blhdr, 8)?;
        let mut check = blhdr[..BLHDR_CKSUM_LEN].to_vec();
        check[8..12].fill(0);
        if checksum(&check) != stored {
            return Err(Error::Format("journal transaction checksum mismatch"));
        }
        if num_blocks == 0
            || num_blocks > max_blocks.max(1)
            || 16 + 16 * num_blocks > blhdr_size
            || bytes_used < blhdr_size as u64
            || bytes_used > log.size - log.jhdr_size
        {
            return Err(Error::Format("implausible journal transaction"));
        }

        // Entry 0 describes the header itself; data for entries 1.. follows it.
        let mut cursor = log.advance(pos, blhdr_size as u64);
        let mut staged: Vec<(u64, Vec<u8>)> = Vec::new();
        for i in 1..num_blocks {
            let e = 16 * (i + 1);
            let bnum = order.u64(&blhdr, e)?;
            let bsize = order.u32(&blhdr, e + 8)? as u64;
            if bnum == u64::MAX {
                continue; // block killed within the transaction: no data, not written
            }
            if bsize == 0 || bsize % SECTOR != 0 || bsize > (16 << 20) {
                return Err(Error::Format("bad block size in journal transaction"));
            }
            *replayed += bsize;
            if *replayed > MAX_REPLAY_BYTES {
                return Err(Error::Format("journal replay larger than the safety limit"));
            }
            let data = log.read(cursor, bsize as usize)?;
            cursor = log.advance(cursor, bsize);
            let byte_off = bnum
                .checked_mul(self.unit)
                .filter(|o| o.checked_add(bsize).map_or(false, |e| e <= self.inner.len()))
                .ok_or(Error::Format("journal block lies outside the volume"))?;
            staged.push((byte_off, data));
        }
        // Only a fully validated transaction is applied.
        for (off, data) in staged {
            for (i, sector) in data.chunks_exact(SECTOR as usize).enumerate() {
                let mut s = Box::new([0u8; SECTOR as usize]);
                s.copy_from_slice(sector);
                self.overlay.insert(off / SECTOR + i as u64, s);
            }
        }
        Ok(log.advance(pos, bytes_used))
    }
}

fn u16_of(order: Order, b: &[u8], o: usize) -> Result<u16> {
    let s = slice(b, o, 2)?;
    Ok(if order.0 { u16::from_le_bytes([s[0], s[1]]) } else { u16::from_be_bytes([s[0], s[1]]) })
}

/// The circular part of the journal, addressed by journal-relative offsets.
struct Log<'a> {
    src: &'a dyn BlockSource,
    base: u64,
    size: u64,
    jhdr_size: u64,
}

impl Log<'_> {
    /// `pos + len`, wrapping from the end of the journal back past its header.
    fn advance(&self, pos: u64, len: u64) -> u64 {
        let next = pos + len;
        if next >= self.size {
            next - self.size + self.jhdr_size
        } else {
            next
        }
    }

    fn read(&self, pos: u64, len: usize) -> Result<Vec<u8>> {
        let mut out = vec![0u8; len];
        let first = ((self.size - pos) as usize).min(len);
        self.src.read_at(self.base + pos, &mut out[..first])?;
        if first < len {
            self.src.read_at(self.base + self.jhdr_size, &mut out[first..])?;
        }
        Ok(out)
    }
}

impl BlockSource for JournaledSource<'_> {
    fn len(&self) -> u64 {
        self.inner.len()
    }

    fn sector_size(&self) -> u64 {
        self.inner.sector_size()
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        self.inner.read_at(offset, buf)?;
        if self.overlay.is_empty() || buf.is_empty() {
            return Ok(());
        }
        let first = offset / SECTOR;
        let last = (offset + buf.len() as u64 - 1) / SECTOR;
        for sector in first..=last {
            let Some(data) = self.overlay.get(&sector) else { continue };
            let s_start = sector * SECTOR;
            let lo = offset.max(s_start);
            let hi = (offset + buf.len() as u64).min(s_start + SECTOR);
            buf[(lo - offset) as usize..(hi - offset) as usize]
                .copy_from_slice(&data[(lo - s_start) as usize..(hi - s_start) as usize]);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_is_the_complement_of_a_rolling_sum() {
        assert_eq!(checksum(&[]), !0);
        // sum: 0 -> (0<<8)^(0+1) = 1 -> (1<<8)^(1+2) = 0x103
        assert_eq!(checksum(&[1, 2]), !0x103);
    }

    struct Mem(Vec<u8>);
    impl BlockSource for Mem {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
            let end = offset as usize + buf.len();
            if end > self.0.len() {
                return Err(Error::OutOfRange { offset, len: buf.len() });
            }
            buf.copy_from_slice(&self.0[offset as usize..end]);
            Ok(())
        }
    }

    /// A 64 KiB volume with an 8 KiB journal at 8192 holding one transaction
    /// that straddles the end of the log: its header is at 7168 and its second
    /// block wraps round to just after the journal header.
    fn wrapped_journal(le: bool) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let u32b = |v: u32| if le { v.to_le_bytes() } else { v.to_be_bytes() };
        let u64b = |v: u64| if le { v.to_le_bytes() } else { v.to_be_bytes() };
        let mut img = vec![0u8; 65536];
        img[1024..1026].copy_from_slice(b"H+");
        img[1028..1032].copy_from_slice(&0x2000u32.to_be_bytes()); // journaled
        img[1036..1040].copy_from_slice(&1u32.to_be_bytes()); // journalInfoBlock
        img[1064..1068].copy_from_slice(&4096u32.to_be_bytes()); // block size
        img[4096..4100].copy_from_slice(&1u32.to_be_bytes()); // journal in the file system
        img[4096 + 36..4096 + 44].copy_from_slice(&8192u64.to_be_bytes());
        img[4096 + 44..4096 + 52].copy_from_slice(&8192u64.to_be_bytes());

        let j = 8192usize;
        img[j..j + 4].copy_from_slice(&u32b(JOURNAL_MAGIC));
        img[j + 4..j + 8].copy_from_slice(&u32b(ENDIAN_MAGIC));
        img[j + 8..j + 16].copy_from_slice(&u64b(7168)); // start
        img[j + 16..j + 24].copy_from_slice(&u64b(1024)); // end, after wrapping
        img[j + 24..j + 32].copy_from_slice(&u64b(8192)); // size
        img[j + 32..j + 36].copy_from_slice(&u32b(512)); // blhdr_size
        img[j + 40..j + 44].copy_from_slice(&u32b(512)); // jhdr_size
        let sum = checksum(&img[j..j + 44]);
        img[j + 36..j + 40].copy_from_slice(&u32b(sum));

        // Transaction header at journal offset 7168: header entry + 2 blocks.
        let h = j + 7168;
        let put16 = |v: u16| if le { v.to_le_bytes() } else { v.to_be_bytes() };
        img[h..h + 2].copy_from_slice(&put16(8)); // max_blocks
        img[h + 2..h + 4].copy_from_slice(&put16(3)); // num_blocks
        img[h + 4..h + 8].copy_from_slice(&u32b(512 + 1024)); // bytes_used
        img[h + 32..h + 40].copy_from_slice(&u64b(100)); // block 1 -> sector 100
        img[h + 40..h + 44].copy_from_slice(&u32b(512));
        img[h + 48..h + 56].copy_from_slice(&u64b(101)); // block 2 -> sector 101
        img[h + 56..h + 60].copy_from_slice(&u32b(512));
        let mut ck = img[h..h + 32].to_vec();
        ck[8..12].fill(0);
        let sum = checksum(&ck);
        img[h + 8..h + 12].copy_from_slice(&u32b(sum));

        let (a, b) = (vec![0xA1u8; 512], vec![0xB2u8; 512]);
        img[j + 7680..j + 8192].copy_from_slice(&a); // fills the log up to its end
        img[j + 512..j + 1024].copy_from_slice(&b); // wrapped to after the header
        (img, a, b)
    }

    #[test]
    fn a_transaction_wrapping_the_end_of_the_log_is_replayed() {
        for le in [true, false] {
            let (img, a, b) = wrapped_journal(le);
            let src = Mem(img);
            let j = JournaledSource::new(&src).unwrap();
            assert_eq!(j.report.transactions, 1, "little-endian: {le}");
            assert!(!j.report.stopped_early);
            let mut got = [0u8; 1024];
            j.read_at(100 * 512, &mut got).unwrap();
            assert_eq!(&got[..512], &a[..], "little-endian: {le}");
            assert_eq!(&got[512..], &b[..], "little-endian: {le}");
            // The device itself is never modified.
            let mut raw = [0u8; 512];
            src.read_at(100 * 512, &mut raw).unwrap();
            assert_eq!(raw, [0u8; 512]);
        }
    }

    #[test]
    fn a_block_outside_the_volume_rejects_the_transaction() {
        let (mut img, ..) = wrapped_journal(true);
        let h = 8192 + 7168;
        img[h + 32..h + 40].copy_from_slice(&(1_000_000u64).to_le_bytes());
        let mut ck = img[h..h + 32].to_vec();
        ck[8..12].fill(0);
        let sum = checksum(&ck);
        img[h + 8..h + 12].copy_from_slice(&sum.to_le_bytes());
        let src = Mem(img);
        let j = JournaledSource::new(&src).unwrap();
        assert!(j.report.stopped_early);
        assert_eq!(j.report.transactions, 0);
        assert_eq!(j.report.sectors, 0);
    }
}

/// One transaction as found in the log, for inspection and tests.
#[doc(hidden)]
#[derive(Debug)]
pub struct RawTransaction {
    pub position: u64,
    pub bytes_used: u64,
    /// (sector number, size in bytes, first four bytes of the new contents)
    pub blocks: Vec<(u64, u64, [u8; 4])>,
}

/// Walk the log from just after its header, ignoring `start` and `end`, and
/// return every transaction that parses. After a replay the stale transactions
/// stay behind in the log, so on a quiet volume this is the way to read them.
#[doc(hidden)]
pub fn scan_stale_transactions(inner: &dyn BlockSource) -> Result<Vec<RawTransaction>> {
    let mut vh = [0u8; 512];
    inner.read_at(1024, &mut vh)?;
    let block_size = u32be(&vh, 40)? as u64;
    let mut jib = [0u8; 52];
    inner.read_at(u32be(&vh, 12)? as u64 * block_size, &mut jib)?;
    let (jnl_off, jnl_size) = (u64be(&jib, 36)?, u64be(&jib, 44)?);
    let mut hdr = [0u8; 52];
    inner.read_at(jnl_off, &mut hdr)?;
    let order = Order(u32::from_le_bytes(hdr[4..8].try_into().unwrap()) == ENDIAN_MAGIC);
    let blhdr_size = order.u32(&hdr, 32)? as usize;
    let jhdr_size = order.u32(&hdr, 40)? as u64;
    let log = Log { src: inner, base: jnl_off, size: jnl_size, jhdr_size };

    let mut out = Vec::new();
    let mut pos = jhdr_size;
    while pos + blhdr_size as u64 <= jnl_size {
        let Ok(blhdr) = log.read(pos, blhdr_size) else { break };
        let (Ok(num), Ok(used), Ok(stored)) =
            (u16_of(order, &blhdr, 2), order.u32(&blhdr, 4), order.u32(&blhdr, 8))
        else {
            break;
        };
        let mut check = blhdr[..BLHDR_CKSUM_LEN].to_vec();
        check[8..12].fill(0);
        if checksum(&check) != stored || num == 0 || 16 + 16 * num as usize > blhdr_size {
            break;
        }
        let mut cursor = log.advance(pos, blhdr_size as u64);
        let mut blocks = Vec::new();
        for i in 1..num as usize {
            let e = 16 * (i + 1);
            let (Ok(bnum), Ok(bsize)) = (order.u64(&blhdr, e), order.u32(&blhdr, e + 8)) else { break };
            if bnum == u64::MAX {
                continue;
            }
            let mut first = [0u8; 4];
            if let Ok(d) = log.read(cursor, 4) {
                first.copy_from_slice(&d);
            }
            blocks.push((bnum, bsize as u64, first));
            cursor = log.advance(cursor, bsize as u64);
        }
        out.push(RawTransaction { position: pos, bytes_used: used as u64, blocks });
        pos = log.advance(pos, used as u64);
        if used == 0 || pos < jhdr_size {
            break;
        }
    }
    Ok(out)
}

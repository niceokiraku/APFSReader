//! Journal replay against the NPS nps-2009-hfsjtest1 images: macOS-written
//! journaled HFS+ volumes. Skipped when the images have not been downloaded.
//!
//! Both images were cleanly unmounted, so their live log is empty and the
//! earlier transactions are stale. To exercise replay, the tests copy an image
//! into memory, rewind the journal's `start` to its first transaction (as an
//! unclean shutdown would leave it) and damage the on-disk blocks those
//! transactions rewrite; replay must restore them.

use apfsreader_core::device::{BlockSource, FileSource};
use apfsreader_core::hfsplus::journal::{scan_stale_transactions, JournaledSource};
use apfsreader_core::hfsplus::HfsPlus;
use apfsreader_core::vfs::FileSystem;
use apfsreader_core::{Error, Result};
use std::path::PathBuf;

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

fn load(name: &str) -> Option<Vec<u8>> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../testdata/nps-hfsjtest1").join(name);
    p.exists().then(|| std::fs::read(p).unwrap())
}

/// Apple's journal checksum (see journal.rs); the real header's checksum was
/// verified with this algorithm by the parser, so it is safe to reuse here.
fn checksum(data: &[u8]) -> u32 {
    let mut sum: u32 = 0;
    for &b in data {
        sum = (sum << 8) ^ sum.wrapping_add(b as u32);
    }
    !sum
}

fn journal_offset(img: &[u8]) -> usize {
    let block_size = u32::from_be_bytes(img[1024 + 40..1024 + 44].try_into().unwrap()) as usize;
    let jib = u32::from_be_bytes(img[1024 + 12..1024 + 16].try_into().unwrap()) as usize;
    u64::from_be_bytes(img[jib * block_size + 36..jib * block_size + 44].try_into().unwrap()) as usize
}

/// Point the journal's start at `start` and fix the header checksum.
fn set_journal_start(img: &mut [u8], start: u64) {
    let j = journal_offset(img);
    img[j + 8..j + 16].copy_from_slice(&start.to_le_bytes());
    img[j + 36..j + 40].fill(0);
    let sum = checksum(&img[j..j + 44]);
    img[j + 36..j + 40].copy_from_slice(&sum.to_le_bytes());
}

#[test]
fn stale_transactions_parse_with_valid_checksums_and_512_byte_units() {
    let Some(img) = load("image.gen1.dmg") else { return };
    let txs = scan_stale_transactions(&Mem(img)).unwrap();
    assert_eq!(txs.len(), 6);
    // Each transaction starts where the previous one's bytes_used says it ends.
    for w in txs.windows(2) {
        assert_eq!(w[0].position + w[0].bytes_used, w[1].position);
    }
    // Sector 2 is the volume header ('H+', version 4); the alternate header
    // sits two sectors before the end of a 10 MiB volume. That both land on
    // real headers fixes the block-number unit at 512 bytes.
    let vh = |sector: u64| {
        txs.iter().flat_map(|t| &t.blocks).find(|b| b.0 == sector).map(|b| b.2)
    };
    assert_eq!(vh(2), Some([0x48, 0x2b, 0x00, 0x04]));
    assert_eq!(vh(10 * 1024 * 1024 / 512 - 2), Some([0x48, 0x2b, 0x00, 0x04]));
}

#[test]
fn an_empty_log_changes_nothing() {
    let Some(img) = load("image.gen1.dmg") else { return };
    let src = Mem(img.clone());
    let j = JournaledSource::new(&src).unwrap();
    assert!(j.report.journaled);
    assert_eq!(j.report.transactions, 0);
    let mut a = vec![0u8; 8192];
    let mut b = vec![0u8; 8192];
    src.read_at(0, &mut a).unwrap();
    j.read_at(0, &mut b).unwrap();
    assert_eq!(a, b);
}

#[test]
fn replay_restores_blocks_damaged_on_disk() {
    let Some(original) = load("image.gen1.dmg") else { return };
    let mut dirty = original.clone();
    set_journal_start(&mut dirty, 512); // replay every transaction in the log
    // Damage the volume header and the allocation bitmap's first block, both
    // rewritten by journaled transactions.
    dirty[1024 + 20] ^= 0xFF;
    dirty[1024 + 21] ^= 0xFF;
    dirty[4096..4104].fill(0xAA);

    let src = Mem(dirty);
    let j = JournaledSource::new(&src).unwrap();
    assert_eq!(j.report.transactions, 6);
    assert!(!j.report.stopped_early);

    let mut vh = [0u8; 512];
    j.read_at(1024, &mut vh).unwrap();
    assert_eq!(&vh[..], &original[1024..1536], "volume header restored from the journal");
    let mut bm = [0u8; 8];
    j.read_at(4096, &mut bm).unwrap();
    assert_eq!(&bm[..], &original[4096..4104], "allocation bitmap restored from the journal");

    // Unjournaled bytes are untouched, and a read straddling a replayed sector
    // splices correctly.
    let mut span = vec![0u8; 3000];
    j.read_at(500, &mut span).unwrap();
    assert_eq!(&span[..524], &original[500..1024]);
    assert_eq!(&span[524..], &original[1024..1024 + 2476]);

    // The replayed volume is a working file system.
    let fs = HfsPlus::open(&j).unwrap();
    let names: Vec<String> = fs
        .read_dir(2) // the root folder's CNID
        .unwrap()
        .iter()
        .map(|e| fs.info(e).name)
        .collect();
    assert!(names.contains(&"file1.txt".to_string()), "{names:?}");
}

#[test]
fn a_damaged_transaction_stops_replay_without_applying_it() {
    let Some(original) = load("image.gen1.dmg") else { return };
    let txs = scan_stale_transactions(&Mem(original.clone())).unwrap();
    let mut dirty = original.clone();
    set_journal_start(&mut dirty, 512);
    // Corrupt the third transaction's block-list header checksum area.
    let j = journal_offset(&dirty);
    dirty[j + txs[2].position as usize + 2] ^= 0x01; // num_blocks
    let src = Mem(dirty);
    let r = JournaledSource::new(&src).unwrap().report;
    assert!(r.stopped_early);
    assert_eq!(r.transactions, 2, "transactions before the bad one are kept");
}

#[test]
fn a_nonsense_journal_header_is_reported_not_trusted() {
    let Some(mut img) = load("image.gen1.dmg") else { return };
    let j = journal_offset(&img);
    img[j + 4] ^= 0xFF; // destroy the byte-order marker
    assert!(JournaledSource::new(&Mem(img)).is_err());
}

#[test]
fn file_backed_image_is_also_accepted() {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../testdata/nps-hfsjtest1/image.gen0.dmg");
    if !p.exists() {
        return;
    }
    let src = FileSource::open(p).unwrap();
    let j = JournaledSource::new(&src).unwrap();
    assert!(j.report.journaled);
}

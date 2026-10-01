//! Tests against images produced by scripts/gen-testdata.ps1 (go-apfs-v2 output).
//! Skipped when the images have not been generated.
//!
//! These images come from a third-party writer, so they cross-check parsing but
//! are not an authority on Apple's behaviour; see testdata/nps-hfsjtest1 for
//! images produced by macOS itself.

use apfsreader_core::device::BlockSource;
use apfsreader_core::hfsplus::{EntryKind, HfsPlus};
use apfsreader_core::udif::UdifSource;
use std::path::PathBuf;

fn dmg(name: &str) -> Option<UdifSource> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../testdata/gen").join(name);
    p.exists().then(|| UdifSource::open(p).unwrap())
}

fn read_all(fs: &HfsPlus, path: &str) -> Vec<u8> {
    let e = fs.lookup(path).unwrap();
    let mut buf = vec![0u8; e.size as usize];
    let n = fs.read_data(&e, 0, &mut buf).unwrap();
    buf.truncate(n);
    buf
}

#[test]
fn udif_exposes_logical_disk() {
    let Some(src) = dmg("hfsplus-none.dmg") else { return };
    assert_eq!(src.len() % 512, 0);
    let mut sig = [0u8; 2];
    src.read_at(1024, &mut sig).unwrap();
    assert!(&sig == b"HX" || &sig == b"H+");
}

#[test]
fn many_files_span_multiple_btree_nodes() {
    let Some(src) = dmg("hfsplus-none.dmg") else { return };
    let fs = HfsPlus::open(&src).unwrap();
    let many = fs.lookup("/many").unwrap();
    assert_eq!(many.kind, EntryKind::Dir);
    let kids = fs.read_dir(many.cnid).unwrap();
    assert_eq!(kids.len(), 300);
    assert_eq!(read_all(&fs, "/many/file_300.txt"), b"content 300\n");
}

#[test]
fn nfd_names_resolve_from_nfc_paths() {
    let Some(src) = dmg("hfsplus-none.dmg") else { return };
    let fs = HfsPlus::open(&src).unwrap();
    assert_eq!(
        read_all(&fs, "/日本語フォルダ/テスト.txt"),
        "日本語の内容です\n".as_bytes()
    );
    assert_eq!(read_all(&fs, "/docs/deep/er/est/leaf.txt"), b"deep leaf\n");
}

#[test]
fn large_file_reads_across_blocks_and_offsets() {
    let Some(src) = dmg("hfsplus-none.dmg") else { return };
    let fs = HfsPlus::open(&src).unwrap();
    let e = fs.lookup("/random5m.bin").unwrap();
    assert_eq!(e.size, 5 * 1024 * 1024);
    let mut whole = vec![0u8; e.size as usize];
    assert_eq!(fs.read_data(&e, 0, &mut whole).unwrap(), whole.len());
    // An unaligned window must match the same bytes read in one go.
    let mut win = vec![0u8; 10_000];
    assert_eq!(fs.read_data(&e, 4_090, &mut win).unwrap(), win.len());
    assert_eq!(&win[..], &whole[4_090..14_090]);
    // Reading past EOF returns what remains, then nothing.
    let mut tail = vec![0u8; 100];
    assert_eq!(fs.read_data(&e, e.size - 40, &mut tail).unwrap(), 40);
    assert_eq!(fs.read_data(&e, e.size, &mut tail).unwrap(), 0);
}

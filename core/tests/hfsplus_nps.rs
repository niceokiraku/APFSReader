//! Integration tests against the NPS nps-2009-hfsjtest1 images (HFS+, journaled).
//! Skipped when the image has not been downloaded into testdata/.

use apfsreader_core::device::FileSource;
use apfsreader_core::hfsplus::{EntryKind, HfsPlus};
use std::path::PathBuf;

fn image(name: &str) -> Option<FileSource> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../testdata/nps-hfsjtest1")
        .join(name);
    p.exists().then(|| FileSource::open(p).unwrap())
}

fn read_all(fs: &HfsPlus, path: &str) -> Vec<u8> {
    let e = fs.lookup(path).unwrap();
    let mut buf = vec![0u8; e.size as usize];
    let n = fs.read_data(&e, 0, &mut buf).unwrap();
    buf.truncate(n);
    buf
}

#[test]
fn header_and_listing() {
    let Some(src) = image("image.gen1.dmg") else { return };
    let fs = HfsPlus::open(&src).unwrap();
    assert!(!fs.header.is_hfsx());
    assert!(fs.header.is_journaled());
    assert_eq!(fs.header.block_size, 4096);

    let names: Vec<String> = fs.read_dir(2).unwrap().into_iter().map(|e| e.name).collect();
    assert!(names.contains(&"file1.txt".to_string()));
    assert!(names.contains(&"file2.txt".to_string()));
    assert!(!names.iter().any(|n| n.starts_with(".HFS+ Private")));
}

#[test]
fn file_contents_and_case_insensitive_lookup() {
    let Some(src) = image("image.gen1.dmg") else { return };
    let fs = HfsPlus::open(&src).unwrap();
    assert_eq!(read_all(&fs, "/file1.txt"), b"New file 1 contents - snarf\n");
    assert_eq!(read_all(&fs, "/FILE2.TXT"), b"This is file 2 - snarf\n");
}

#[test]
fn nested_directory_and_missing_path() {
    let Some(src) = image("image.gen1.dmg") else { return };
    let fs = HfsPlus::open(&src).unwrap();
    let d = fs.lookup("/.fseventsd").unwrap();
    assert_eq!(d.kind, EntryKind::Dir);
    assert!(fs.lookup("/.fseventsd/fseventsd-uuid").is_ok());
    assert!(fs.lookup("/nope").is_err());
}

#[test]
fn rejects_non_hfs_input() {
    let Some(src) = image("image.gen1.dmg") else { return };
    // Offset the volume by one block so the signature is not where it should be.
    let shifted = apfsreader_core::device::Slice::new(&src, 4096, 4096 * 100);
    assert!(HfsPlus::open(&shifted).is_err());
}

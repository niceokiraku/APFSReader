//! Tests against a disk image written by macOS Disk Utility (APFS, 2017,
//! from https://thinkdfir.com/2017/09/27/playing-with-apfs/). Unlike the
//! generated images, this is Apple's own writer, so it is the ground truth.
//! Skipped when testdata/UNENCRYPTED.dmg is absent.
//!
//! The file is a raw disk image (GPT, one APFS partition) despite its name.

use apfsreader_core::apfs::{Container, EntryKind, Volume};
use apfsreader_core::device::{FileSource, Slice};
use apfsreader_core::gpt;
use std::path::PathBuf;

fn image() -> Option<FileSource> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../testdata/UNENCRYPTED.dmg");
    p.exists().then(|| FileSource::open(p).unwrap())
}

fn with_volume(f: impl FnOnce(&Volume)) {
    let Some(img) = image() else { return };
    let parts = gpt::read_partitions(&img).unwrap();
    let p = parts.iter().find(|p| p.type_guid == gpt::GUID_APFS).expect("APFS partition");
    let part = Slice::new(&img, p.offset(), p.len());
    let c = Container::open(&part).unwrap();
    assert_eq!(c.volume_count(), 1);
    f(&c.volume(0).unwrap());
}

fn read_all(v: &Volume, path: &str) -> Vec<u8> {
    let e = v.lookup(path).unwrap();
    let mut buf = vec![0u8; e.size as usize];
    let n = v.read_data(&e, 0, &mut buf).unwrap();
    buf.truncate(n);
    buf
}

#[test]
fn partition_and_container() {
    let Some(img) = image() else { return };
    let parts = gpt::read_partitions(&img).unwrap();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].type_guid, gpt::GUID_APFS);
    let part = Slice::new(&img, parts[0].offset(), parts[0].len());
    let c = Container::open(&part).unwrap();
    assert_eq!(c.block_size, 4096);
    assert!(c.xid > 1, "a used volume has advanced past the first checkpoint");
}

#[test]
fn volume_metadata() {
    with_volume(|v| {
        assert_eq!(v.name, "UNENCRYPTED");
        assert!(!v.encrypted);
        assert!(v.is_case_insensitive());
    });
}

#[test]
fn root_listing_matches_what_was_written() {
    with_volume(|v| {
        let root = v.root().unwrap();
        let kids = v.read_dir(root.oid).unwrap();
        let name_of = |n: &str| kids.iter().find(|e| e.name == n);
        // The blog post: five files created, one permanently deleted, one
        // trashed, leaving three visible at the root.
        for n in ["file1.txt", "file2.txt", "file3.txt"] {
            assert_eq!(name_of(n).unwrap_or_else(|| panic!("missing {n}")).kind, EntryKind::File);
        }
        for n in [".Trashes", ".fseventsd", ".TemporaryItems"] {
            assert_eq!(name_of(n).unwrap_or_else(|| panic!("missing {n}")).kind, EntryKind::Dir);
        }
        assert!(name_of(".DS_Store").is_some());
    });
}

#[test]
fn file_contents() {
    with_volume(|v| {
        assert_eq!(read_all(v, "/file1.txt"), b"file1");
        assert_eq!(read_all(v, "/file2.txt"), b"file2");
        assert_eq!(read_all(v, "/file3.txt"), b"file3");
        // Case-insensitive volume.
        assert_eq!(read_all(v, "/FILE1.TXT"), b"file1");
    });
}

#[test]
fn ds_store_has_its_magic() {
    with_volume(|v| {
        let data = read_all(v, "/.DS_Store");
        assert_eq!(data.len(), 6148);
        assert_eq!(&data[4..8], b"Bud1");
    });
}

#[test]
fn nested_directories() {
    with_volume(|v| {
        let trashes = v.lookup("/.Trashes").unwrap();
        let kids = v.read_dir(trashes.oid).unwrap();
        assert_eq!(kids.len(), 1);
        assert_eq!(kids[0].name, "501");
        assert!(v.lookup("/.fseventsd/fseventsd-uuid").is_ok());
    });
}

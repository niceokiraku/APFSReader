//! Finding the mountable volumes in the macOS-written fixtures.

use apfsreader_core::image::{self, discover, preferred, Kind};
use std::path::PathBuf;

fn fixture(name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../testdata/macos-fixtures").join(name);
    p.exists().then_some(p)
}

fn found(name: &str) -> Option<Vec<image::Found>> {
    let src = image::open_source(fixture(name)?).unwrap();
    Some(discover(&*src).unwrap())
}

#[test]
fn an_apfs_disk_lists_its_volume() {
    let Some(f) = found("basic.img") else { return };
    assert_eq!(f.len(), 1);
    assert_eq!((f[0].kind, f[0].name.as_str(), f[0].encrypted), (Kind::Apfs, "ACCEPT", false));
    assert_eq!((f[0].partition, f[0].volume), (Some(0), Some(0)));
    assert_eq!(preferred(&f), Some(0));
}

#[test]
fn an_hfs_plus_volume_is_named_after_its_label() {
    let Some(f) = found("hfs-basic.dmg") else { return };
    assert_eq!(f.len(), 1);
    assert_eq!((f[0].kind, f[0].name.as_str()), (Kind::HfsPlus, "HFSTEST"));
    assert_eq!(f[0].volume, None);
}

#[test]
fn an_apple_partition_map_disk_reports_the_right_partition_index() {
    let Some(f) = found("hfs-apm.img") else { return };
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].kind, Kind::HfsPlus);
    // Partition 0 is the map itself; the HFS+ volume is partition 1.
    assert_eq!(f[0].partition, Some(1));
}

#[test]
fn an_encrypted_volume_is_found_but_not_preferred() {
    let Some(f) = found("encrypted.img") else { return };
    assert_eq!(f.len(), 1);
    assert!(f[0].encrypted);
    assert_eq!(preferred(&f), None);
}

#[test]
fn a_bare_volume_has_no_partition_and_unrecognised_data_has_nothing() {
    let Some(p) = fixture("hfs-basic.dmg") else { return };
    let src = image::open_source(p).unwrap();
    // The partition inside the GPT, read as if it were a whole image.
    let loc = image::locate_volume(&*src, None).unwrap();
    let inner = apfsreader_core::device::Slice::new(&*src, loc.offset, loc.len);
    let f = discover(&inner).unwrap();
    assert_eq!((f.len(), f[0].partition), (1, None));

    struct Zeros;
    impl apfsreader_core::device::BlockSource for Zeros {
        fn len(&self) -> u64 {
            1 << 20
        }
        fn read_at(&self, _o: u64, buf: &mut [u8]) -> apfsreader_core::Result<()> {
            buf.fill(0);
            Ok(())
        }
    }
    assert!(discover(&Zeros).unwrap().is_empty());
}

#[test]
fn preferred_choice_favours_data_over_system() {
    use image::Found;
    let mk = |name: &str, role: &'static str, enc: bool| Found {
        partition: Some(0),
        volume: Some(0),
        kind: Kind::Apfs,
        name: name.into(),
        role: Some(role),
        encrypted: enc,
        size: 0,
        problem: None,
    };
    let list = vec![mk("Macintosh HD", "system", false), mk("Preboot", "preboot", false), mk("Macintosh HD - Data", "data", false)];
    assert_eq!(preferred(&list), Some(2));
    let encrypted_data = vec![mk("Sys", "system", false), mk("Data", "data", true)];
    assert_eq!(preferred(&encrypted_data), Some(0), "an encrypted Data volume is skipped");
}

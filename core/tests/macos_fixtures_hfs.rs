//! Checks every entry of the macOS-written HFS+ fixture against the manifest
//! recorded when it was made (size, SHA-256, mode, symlink target, extended
//! attributes). Same provenance as the APFS fixture; see
//! macos_fixtures_apfs.rs. Skipped when the fixtures are absent.

use apfsreader_core::device::{BlockSource, Slice};
use apfsreader_core::gpt;
use apfsreader_core::hfsplus::{Entry, EntryKind, HfsPlus};
use apfsreader_core::udif::UdifSource;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

fn fixture(name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../testdata/macos-fixtures").join(name);
    p.exists().then_some(p)
}

fn sha(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn read_all(fs: &HfsPlus, e: &Entry) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = vec![0u8; 100_000];
    loop {
        let n = fs.read_data(e, out.len() as u64, &mut buf).unwrap();
        if n == 0 {
            return out;
        }
        out.extend_from_slice(&buf[..n]);
    }
}

/// The HFS+ partition of a GPT disk image.
fn partition(src: &dyn BlockSource) -> (u64, u64) {
    let parts = gpt::read_partitions(src).unwrap();
    let p = parts.iter().find(|p| p.type_guid == gpt::GUID_HFS_PLUS).expect("HFS+ partition");
    (p.offset(), p.len())
}

fn check(src: &dyn BlockSource, m: &Value) {
    let (off, len) = partition(src);
    let part = Slice::new(src, off, len);
    let fs = HfsPlus::open(&part).unwrap();
    let mut checked = 0;
    for (path, info) in m["files"].as_object().unwrap() {
        let e = fs.lookup(path).unwrap_or_else(|err| panic!("{path}: {err}"));
        match info["type"].as_str().unwrap() {
            "file" => {
                assert_eq!(e.kind, EntryKind::File, "{path}");
                let data = read_all(&fs, &e);
                assert_eq!(data.len() as u64, info["size"].as_u64().unwrap(), "{path}: size");
                assert_eq!(e.size, data.len() as u64, "{path}: entry size");
                assert_eq!(sha(&data), info["sha256"].as_str().unwrap(), "{path}: content");
                assert_eq!(e.compressed, info["compressed"].as_bool().unwrap_or(false), "{path}: compressed flag");
            }
            "symlink" => {
                assert_eq!(e.kind, EntryKind::Symlink, "{path}");
                assert_eq!(fs.read_link(&e).unwrap(), info["target"].as_str().unwrap(), "{path}: target");
            }
            other => panic!("unknown manifest type {other}"),
        }
        if let Some(mode) = info["mode"].as_str() {
            let want = u16::from_str_radix(mode.trim_start_matches("0o"), 8).unwrap();
            assert_eq!(e.mode & 0o7777, want, "{path}: mode");
        }
        if let Some(xa) = info["xattrs"].as_object() {
            for (name, x) in xa {
                let val = fs.xattr(&e, name).unwrap().unwrap_or_else(|| panic!("{path}: missing xattr {name}"));
                assert_eq!(val.len() as u64, x["size"].as_u64().unwrap(), "{path}: xattr {name} size");
                assert_eq!(sha(&val), x["sha256"].as_str().unwrap(), "{path}: xattr {name}");
            }
        }
        checked += 1;
    }
    assert!(checked >= 10, "manifest unexpectedly small");
}

#[test]
fn zlib_dmg_matches_manifest() {
    let (Some(mp), Some(p)) = (fixture("hfs-manifest.json"), fixture("hfs-basic.dmg")) else { return };
    let m: Value = serde_json::from_slice(&std::fs::read(mp).unwrap()).unwrap();
    let src = UdifSource::open(p).unwrap();
    check(&src, &m);
}

#[test]
fn hard_links_share_content_and_do_not_show_the_private_directory() {
    let Some(p) = fixture("hfs-basic.dmg") else { return };
    let src = UdifSource::open(p).unwrap();
    let (off, len) = partition(&src);
    let part = Slice::new(&src, off, len);
    let fs = HfsPlus::open(&part).unwrap();
    let a = fs.lookup("/hello.txt").unwrap();
    let b = fs.lookup("/hardlink-to-hello").unwrap();
    assert_eq!(read_all(&fs, &a), read_all(&fs, &b));
    assert!(!read_all(&fs, &a).is_empty());
    let names: Vec<String> = fs.read_dir(2).unwrap().into_iter().map(|e| e.name).collect();
    assert!(!names.iter().any(|n| n.contains("Private")), "{names:?}");
}

#[test]
fn compressed_fixture_codec() {
    let Some(p) = fixture("hfs-basic.dmg") else { return };
    let src = UdifSource::open(p).unwrap();
    let (off, len) = partition(&src);
    let part = Slice::new(&src, off, len);
    let fs = HfsPlus::open(&part).unwrap();
    let e = fs.lookup("/compressed.txt").unwrap();
    let attr = fs.xattr(&e, apfsreader_core::decmpfs::ATTR_NAME).unwrap().unwrap();
    let h = apfsreader_core::decmpfs::parse_header(&attr).unwrap().unwrap();
    // 8 = LZVN with the payload in the resource fork: the only decmpfs type the
    // macOS-written fixtures contain, so it is the one verified against real output.
    assert_eq!(h.method, 8);
    assert_eq!(h.size, 23893);
    assert_eq!(attr.len(), 16, "no inline payload for a resource-fork type");
}

// The same HFS+ volume, re-wrapped by hdiutil with other DMG chunk codecs.

fn dmg_matches_manifest(name: &str) {
    let (Some(mp), Some(p)) = (fixture("hfs-manifest.json"), fixture(name)) else { return };
    let m: Value = serde_json::from_slice(&std::fs::read(mp).unwrap()).unwrap();
    check(&UdifSource::open(p).unwrap(), &m);
}

#[test]
fn adc_dmg_matches_manifest() {
    dmg_matches_manifest("hfs-basic-adc.dmg");
}

#[test]
fn lzma_dmg_matches_manifest() {
    dmg_matches_manifest("hfs-basic-lzma.dmg");
}

#[test]
fn apple_partition_map_disk() {
    use apfsreader_core::device::FileSource;
    use apfsreader_core::partition::{self, PartKind};
    let Some(p) = fixture("hfs-apm.img") else { return };
    let src = FileSource::open(p).unwrap();
    let parts = partition::scan(&src).unwrap();
    assert!(parts.iter().all(|p| p.scheme == "APM"));
    let hfs = parts.iter().find(|p| p.kind == PartKind::HfsPlus).expect("an Apple_HFS partition");
    assert!(hfs.offset > 0 && hfs.len > 0);
    let part = Slice::new(&src, hfs.offset, hfs.len);
    let fs = HfsPlus::open(&part).unwrap();
    let f = fs.lookup("/f.txt").unwrap();
    assert_eq!(f.size, 12);
    assert_eq!(read_all(&fs, &f).len(), 12);
}

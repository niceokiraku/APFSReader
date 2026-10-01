//! Checks every entry of the macOS-written APFS fixture against the manifest
//! recorded when the image was made (testdata/macos-fixtures/manifest.json:
//! size, SHA-256, mode, symlink target and extended attributes). The images
//! come from go-apfs-v2's repository but were produced by macOS tooling
//! (hdiutil, ditto, xattr); see scripts/gen-fixtures.sh there.
//!
//! Skipped when the fixtures are absent.

use apfsreader_core::apfs::{Container, EntryKind, Volume};
use apfsreader_core::device::{BlockSource, FileSource, Slice};
use apfsreader_core::gpt;
use apfsreader_core::udif::UdifSource;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

fn fixture(name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../testdata/macos-fixtures").join(name);
    p.exists().then_some(p)
}

fn manifest() -> Option<Value> {
    let p = fixture("manifest.json")?;
    Some(serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap())
}

fn sha(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn read_all(v: &Volume, e: &apfsreader_core::apfs::Entry) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = vec![0u8; 100_000]; // deliberately not a divisor of any block size
    loop {
        let n = v.read_data(e, out.len() as u64, &mut buf).unwrap();
        if n == 0 {
            return out;
        }
        out.extend_from_slice(&buf[..n]);
    }
}

fn check_volume(src: &dyn BlockSource, m: &Value) {
    let parts = gpt::read_partitions(src).unwrap();
    let p = parts.iter().find(|p| p.type_guid == gpt::GUID_APFS).expect("APFS partition");
    let part = Slice::new(src, p.offset(), p.len());
    let c = Container::open(&part).unwrap();
    let v = c.volume(0).unwrap();
    assert_eq!(v.name, m["volumeName"].as_str().unwrap());

    let mut checked = 0;
    for (path, info) in m["files"].as_object().unwrap() {
        let e = v.lookup(path).unwrap_or_else(|err| panic!("{path}: {err}"));
        match info["type"].as_str().unwrap() {
            "file" => {
                assert_eq!(e.kind, EntryKind::File, "{path}");
                let data = read_all(&v, &e);
                assert_eq!(data.len() as u64, info["size"].as_u64().unwrap(), "{path}: size");
                assert_eq!(e.size, data.len() as u64, "{path}: entry size");
                assert_eq!(sha(&data), info["sha256"].as_str().unwrap(), "{path}: content");
                assert_eq!(e.compressed, info["compressed"].as_bool().unwrap_or(false), "{path}: compressed flag");
            }
            "symlink" => {
                assert_eq!(e.kind, EntryKind::Symlink, "{path}");
                assert_eq!(v.read_link(&e).unwrap(), info["target"].as_str().unwrap(), "{path}: target");
            }
            other => panic!("unknown manifest type {other}"),
        }
        if let Some(mode) = info["mode"].as_str() {
            let want = u16::from_str_radix(mode.trim_start_matches("0o"), 8).unwrap();
            assert_eq!(e.mode & 0o7777, want, "{path}: mode");
        }
        if let Some(xa) = info["xattrs"].as_object() {
            for (name, x) in xa {
                let val = v.xattr(e.oid, name).unwrap().unwrap_or_else(|| panic!("{path}: missing xattr {name}"));
                assert_eq!(val.len() as u64, x["size"].as_u64().unwrap(), "{path}: xattr {name} size");
                assert_eq!(sha(&val), x["sha256"].as_str().unwrap(), "{path}: xattr {name}");
            }
        }
        checked += 1;
    }
    assert!(checked >= 10, "manifest unexpectedly small");
}

#[test]
fn raw_gpt_image_matches_manifest() {
    let (Some(m), Some(p)) = (manifest(), fixture("basic.img")) else { return };
    check_volume(&FileSource::open(p).unwrap(), &m);
}

#[test]
fn zlib_dmg_matches_manifest() {
    let (Some(m), Some(p)) = (manifest(), fixture("basic.dmg")) else { return };
    check_volume(&UdifSource::open(p).unwrap(), &m);
}

#[test]
fn encrypted_volume_is_refused_not_misread() {
    let Some(p) = fixture("encrypted.img") else { return };
    // A bare container with no partition table.
    let src = FileSource::open(p).unwrap();
    let c = Container::open(&src).unwrap();
    let v = c.volume(0).unwrap();
    assert!(v.encrypted);
    assert!(v.root().is_err());
}

// The same APFS volume, re-wrapped by hdiutil with other DMG chunk codecs.

#[test]
fn bzip2_dmg_matches_manifest() {
    let (Some(m), Some(p)) = (manifest(), fixture("basic-bz2.dmg")) else { return };
    check_volume(&UdifSource::open(p).unwrap(), &m);
}

#[test]
fn lzfse_dmg_matches_manifest() {
    let (Some(m), Some(p)) = (manifest(), fixture("basic-lzfse.dmg")) else { return };
    check_volume(&UdifSource::open(p).unwrap(), &m);
}

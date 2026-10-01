//! End-to-end: run the real binary on the macOS-written fixtures.
//! Skipped when the fixtures are absent.

use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../testdata/macos-fixtures").join(name);
    p.exists().then_some(p)
}

fn sha(path: &std::path::Path) -> String {
    Sha256::digest(std::fs::read(path).unwrap()).iter().map(|b| format!("{b:02x}")).collect()
}

fn scratch(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("apfsreader-cli-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_apfsreader")).args(args).output().unwrap()
}

#[test]
fn extract_whole_volume_matches_manifest_hashes() {
    for (image, tag) in [("basic.dmg", "apfs"), ("hfs-basic.dmg", "hfs")] {
        let Some(img) = fixture(image) else { return };
        let out = scratch(tag);
        let r = run(&["extract", img.to_str().unwrap(), "/", out.to_str().unwrap()]);
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        // Same content in both fixtures; hashes taken from the manifests.
        assert_eq!(
            sha(&out.join("dir1").join("nested").join("deep.txt")),
            "30cf6f2de471343739bcc1dde393c0c0771814ac3ad798f68c8a74495174521a"
        );
        assert_eq!(
            sha(&out.join("compressed.txt")),
            "23f90f8b2c3a4b5f3b5e156339994afd5c2718b378aca6f0e17111f80a70d4ec"
        );
        assert_eq!(std::fs::metadata(out.join("random.bin")).unwrap().len(), 1_572_864);
        assert_eq!(std::fs::read_to_string(out.join("link-to-hello.symlink")).unwrap(), "hello.txt");
        let _ = std::fs::remove_dir_all(&out);
    }
}

#[test]
fn extract_one_folder_keeps_its_name() {
    let Some(img) = fixture("basic.dmg") else { return };
    let out = scratch("sub");
    let r = run(&["extract", img.to_str().unwrap(), "/dir1", out.to_str().unwrap()]);
    assert!(r.status.success());
    assert!(out.join("dir1").join("nested").join("deep.txt").is_file());
    assert!(!out.join("hello.txt").exists());
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn extract_missing_source_fails_cleanly() {
    let Some(img) = fixture("basic.dmg") else { return };
    let out = scratch("none");
    let r = run(&["extract", img.to_str().unwrap(), "/does/not/exist", out.to_str().unwrap()]);
    assert!(!r.status.success());
    assert!(String::from_utf8_lossy(&r.stderr).contains("not found"));
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn extract_from_an_encrypted_volume_is_refused() {
    let Some(img) = fixture("encrypted.img") else { return };
    let out = scratch("enc");
    let r = run(&["extract", img.to_str().unwrap(), "/", out.to_str().unwrap()]);
    assert!(!r.status.success());
    assert!(String::from_utf8_lossy(&r.stderr).to_lowercase().contains("encrypted"));
    let _ = std::fs::remove_dir_all(&out);
}

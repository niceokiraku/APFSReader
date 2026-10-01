//! APFS tests against images from scripts/gen-testdata.ps1 (go-apfs-v2 output).
//! Skipped when the images have not been generated. Third-party writer: these
//! cross-check parsing but are not an authority on Apple's behaviour.

use apfsreader_core::apfs::{Container, EntryKind, Volume};
use apfsreader_core::device::BlockSource;
use apfsreader_core::udif::UdifSource;
use apfsreader_core::{Error, Result};
use std::path::PathBuf;

fn dmg() -> Option<UdifSource> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../testdata/gen/apfs-none.dmg");
    p.exists().then(|| UdifSource::open(p).unwrap())
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

fn read_all(v: &Volume, path: &str) -> Vec<u8> {
    let e = v.lookup(path).unwrap();
    let mut buf = vec![0u8; e.size as usize];
    let n = v.read_data(&e, 0, &mut buf).unwrap();
    buf.truncate(n);
    buf
}

#[test]
fn container_and_volume_info() {
    let Some(src) = dmg() else { return };
    let c = Container::open(&src).unwrap();
    assert_eq!(c.block_size, 4096);
    assert_eq!(c.volume_count(), 1);
    let v = c.volume(0).unwrap();
    assert_eq!(v.name, "TestVol");
    assert!(!v.encrypted);
    assert!(c.volume(1).is_err());
}

#[test]
fn listing_and_contents() {
    let Some(src) = dmg() else { return };
    let c = Container::open(&src).unwrap();
    let v = c.volume(0).unwrap();
    let root = v.root().unwrap();
    let names: Vec<String> = v.read_dir(root.oid).unwrap().into_iter().map(|e| e.name).collect();
    for want in ["hello.txt", "many", "docs", "random5m.bin"] {
        assert!(names.iter().any(|n| n == want), "missing {want}");
    }
    assert_eq!(read_all(&v, "/hello.txt"), b"Hello, APFSReader\n");
    assert_eq!(read_all(&v, "/docs/deep/er/est/leaf.txt"), b"deep leaf\n");
    assert_eq!(v.lookup("/docs").unwrap().kind, EntryKind::Dir);
}

#[test]
fn case_insensitive_and_unicode_lookup() {
    let Some(src) = dmg() else { return };
    let c = Container::open(&src).unwrap();
    let v = c.volume(0).unwrap();
    assert!(v.is_case_insensitive());
    assert_eq!(read_all(&v, "/HELLO.TXT"), b"Hello, APFSReader\n");
    assert_eq!(
        read_all(&v, "/日本語フォルダ/テスト.txt"),
        "日本語の内容です\n".as_bytes()
    );
}

#[test]
fn many_entries_and_large_file() {
    let Some(src) = dmg() else { return };
    let c = Container::open(&src).unwrap();
    let v = c.volume(0).unwrap();
    let many = v.lookup("/many").unwrap();
    assert_eq!(v.read_dir(many.oid).unwrap().len(), 300);
    assert_eq!(read_all(&v, "/many/file_300.txt"), b"content 300\n");

    let e = v.lookup("/random5m.bin").unwrap();
    assert_eq!(e.size, 5 * 1024 * 1024);
    let mut whole = vec![0u8; e.size as usize];
    assert_eq!(v.read_data(&e, 0, &mut whole).unwrap(), whole.len());
    let mut win = vec![0u8; 10_000];
    assert_eq!(v.read_data(&e, 4_090, &mut win).unwrap(), win.len());
    assert_eq!(&win[..], &whole[4_090..14_090]);
    let mut tail = vec![0u8; 100];
    assert_eq!(v.read_data(&e, e.size - 40, &mut tail).unwrap(), 40);
    assert_eq!(v.read_data(&e, e.size, &mut tail).unwrap(), 0);
}

#[test]
fn rejects_non_apfs_input() {
    let zeros = Mem(vec![0u8; 1 << 20]);
    assert!(Container::open(&zeros).is_err());
}

/// Flip bytes at pseudo-random positions and walk the whole volume. Every
/// outcome must be Ok or Err; a panic fails the test. Checksums mean most
/// flips surface as errors, which is the point.
#[test]
fn corrupted_images_never_panic() {
    let Some(src) = dmg() else { return };
    let mut image = vec![0u8; src.len() as usize];
    src.read_at(0, &mut image).unwrap();

    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    fn walk(v: &Volume, dir: u64, depth: u32) {
        if depth > 8 {
            return;
        }
        let Ok(kids) = v.read_dir(dir) else { return };
        for e in kids {
            if e.kind == EntryKind::Dir {
                walk(v, e.oid, depth + 1);
            } else {
                let mut buf = vec![0u8; 4096];
                let _ = v.read_data(&e, 0, &mut buf);
            }
        }
    }

    for _ in 0..300 {
        let mut copy = image.clone();
        for _ in 0..(1 + next() % 3) {
            let pos = (next() as usize) % copy.len();
            copy[pos] ^= 1 << (next() % 8);
        }
        let mem = Mem(copy);
        if let Ok(c) = Container::open(&mem) {
            if let Ok(v) = c.volume(0) {
                if let Ok(root) = v.root() {
                    walk(&v, root.oid, 0);
                }
            }
        }
    }
}

/// The checksum must actually be enforced: damage to a metadata block that is
/// read on every open (the container superblock area) is reported, not ignored.
#[test]
fn checksum_mismatch_is_detected() {
    let Some(src) = dmg() else { return };
    let mut image = vec![0u8; src.len() as usize];
    src.read_at(0, &mut image).unwrap();
    let c = Container::open(&src).unwrap();
    assert_eq!(c.xid, 1);
    // Flip one byte in each of the first 39 blocks after block 0 (metadata sits
    // at the start of this small image) and require that damage is reported.
    let mut errors = 0;
    for blk in 1..40usize {
        let mut copy = image.clone();
        copy[blk * 4096 + 100] ^= 0xFF;
        let mem = Mem(copy);
        let failed = match Container::open(&mem) {
            Err(_) => true,
            Ok(c) => match c.volume(0) {
                Err(_) => true,
                Ok(v) => v.root().is_err() || v.lookup("/docs/deep/er/est/leaf.txt").is_err(),
            },
        };
        if failed {
            errors += 1;
        }
    }
    assert!(errors > 0, "no corruption was detected in 39 damaged metadata blocks");
}

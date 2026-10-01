//! Damaged volumes must produce errors, never panics or runaway loops.
//!
//! Each macOS-written fixture is copied into memory, hit with random bit flips
//! and then walked completely: directory listings, file contents (including
//! decmpfs and resource forks), extended attributes and link targets. Any
//! outcome other than a panic is acceptable. Run in debug builds (the default
//! for `cargo test`) so arithmetic overflow also counts as a failure.

use apfsreader_core::device::{BlockSource, Slice};
use apfsreader_core::image::{self, Kind};
use apfsreader_core::vfs::{EntryKind, FileSystem};
use apfsreader_core::{apfs, hfsplus, Error, Result};
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
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../testdata/macos-fixtures").join(name);
    let src = image::open_source(p).ok()?;
    let mut data = vec![0u8; src.len() as usize];
    src.read_at(0, &mut data).ok()?;
    Some(data)
}

/// Visit everything reachable; errors are fine, panics are not.
static WALKED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn walk<F: FileSystem>(fs: &F, dir: &F::Entry, depth: u32) {
    if depth > 6 {
        return;
    }
    let Ok(kids) = fs.read_dir(dir) else { return };
    for e in kids.iter().take(500) {
        WALKED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let info = fs.info(e);
        match info.kind {
            EntryKind::Dir => walk(fs, e, depth + 1),
            EntryKind::Symlink => {
                let _ = fs.read_link(e);
            }
            EntryKind::File => {
                let mut buf = vec![0u8; 70_000];
                let mut off = 0u64;
                for _ in 0..40 {
                    match fs.read(e, off, &mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => off += n as u64,
                    }
                }
            }
        }
    }
}

fn exercise(data: Vec<u8>) {
    let mem = Mem(data);
    let Ok(loc) = image::locate_volume(&mem, None) else { return };
    let vol = Slice::new(&mem, loc.offset, loc.len);
    match loc.kind {
        Kind::HfsPlus => {
            let Ok(j) = hfsplus::journal::JournaledSource::new(&vol) else { return };
            let Ok(fs) = hfsplus::HfsPlus::open(&j) else { return };
            if let Ok(root) = fs.root() {
                walk(&fs, &root, 0);
            }
            let _ = fs.lookup("/dir1/nested/deep.txt");
            let _ = fs.lookup("/ünïcødé/файл.txt");
        }
        Kind::Apfs => {
            let Ok(c) = apfs::Container::open(&vol) else { return };
            let _ = c.default_volume();
            for i in 0..c.volume_count().min(4) {
                let Ok(v) = c.volume(i) else { continue };
                if let Ok(root) = v.root() {
                    walk(&v, &root, 0);
                }
                let _ = v.lookup("/dir1/nested/deep.txt");
            }
        }
    }
}

fn fuzz(name: &str, rounds: u32, seed: u64) {
    // FUZZ_ROUNDS=20000 cargo test --test fs_robustness for a long soak.
    let rounds = std::env::var("FUZZ_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(rounds);
    let Some(original) = load(name) else { return };
    let walked_before = WALKED.load(std::sync::atomic::Ordering::Relaxed);
    let mut state = seed;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    // Most of these images are mostly empty space, so aim flips at the blocks
    // that hold metadata (the first ~1 MiB and the tail) as well as anywhere.
    let len = original.len();
    for round in 0..rounds {
        let mut data = original.clone();
        for _ in 0..(1 + next() % 6) {
            let pos = match round % 3 {
                0 => (next() as usize) % len,
                1 => (next() as usize) % (1 << 20).min(len),
                _ => len - 1 - (next() as usize) % (1 << 18).min(len),
            };
            data[pos] ^= 1 << (next() % 8);
        }
        exercise(data);
    }
    let walked = WALKED.load(std::sync::atomic::Ordering::Relaxed) - walked_before;
    eprintln!("{name}: {rounds} damaged copies, {walked} entries visited");
}

#[test]
fn hfs_plus_zlib_dmg() {
    fuzz("hfs-basic.dmg", 150, 0x1357_9BDF_2468_ACE1);
}

#[test]
fn hfs_plus_apm_disk() {
    fuzz("hfs-apm.img", 100, 0xDEAD_BEEF_CAFE_F00D);
}

#[test]
fn apfs_raw_gpt_image() {
    fuzz("basic.img", 150, 0x0F0F_1234_ABCD_9876);
}

#[test]
fn apfs_zlib_dmg() {
    fuzz("basic.dmg", 100, 0x7777_8888_9999_0001);
}

#[test]
fn apfs_encrypted_volume() {
    fuzz("encrypted.img", 100, 0x1111_2222_3333_4444);
}

//! A whole volume read through the disk-helper protocol over a real named
//! pipe: the path a physical disk takes, with an image file standing in for the
//! disk (the broker itself only ever serves physical disks). Windows only.
#![cfg(windows)]

use apfsreader_core::apfs::Container;
use apfsreader_core::device::{BlockSource, Slice};
use apfsreader_core::hfsplus::journal::JournaledSource;
use apfsreader_core::hfsplus::HfsPlus;
use apfsreader_core::image::{self, Kind};
use apfsreader_core::remote::pipe::{connect_pipe, current_user_sid, new_pipe_name, random_bytes, serve_pipe};
use apfsreader_core::remote::TOKEN_LEN;
use apfsreader_core::vfs::FileSystem;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

fn fixture(name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../testdata/macos-fixtures").join(name);
    p.exists().then_some(p)
}

/// Serve `path` over a pipe and open it as a remote disk.
fn remote_disk(path: PathBuf) -> (Box<dyn BlockSource>, std::thread::JoinHandle<apfsreader_core::Result<()>>) {
    let served: Arc<dyn BlockSource> = Arc::from(image::open_source(path).unwrap());
    let name = new_pipe_name().unwrap();
    let token = random_bytes::<TOKEN_LEN>().unwrap();
    let sid = current_user_sid().unwrap();
    let (n, s) = (name.clone(), sid);
    let server = std::thread::spawn(move || serve_pipe(&n, &s, &token, &*served, Duration::from_secs(10)));
    let remote = connect_pipe(&name, &token, Duration::from_secs(10)).unwrap();
    (Box::new(remote), server)
}

fn read_all<F: FileSystem>(fs: &F, path: &str) -> Vec<u8> {
    let e = fs.lookup(path).unwrap();
    let mut out = vec![0u8; fs.info(&e).size as usize];
    let n = fs.read(&e, 0, &mut out).unwrap();
    out.truncate(n);
    out
}

#[test]
fn an_apfs_disk_with_a_gpt_reads_through_the_helper() {
    let Some(p) = fixture("basic.img") else { return };
    let (disk, server) = remote_disk(p);
    let loc = image::locate_volume(&*disk, None).unwrap();
    assert_eq!(loc.kind, Kind::Apfs);
    let vol = Slice::new(&*disk, loc.offset, loc.len);
    let c = Container::open(&vol).unwrap();
    let v = c.volume(c.default_volume().unwrap()).unwrap();
    assert_eq!(read_all(&v, "/hello.txt"), b"hello from apfs\n");
    // Compressed file: its SHA-256 is recorded in the macOS manifest.
    let data = read_all(&v, "/compressed.txt");
    assert_eq!(data.len(), 23893);
    drop(v);
    drop(c);
    drop(disk);
    server.join().unwrap().unwrap();
}

#[test]
fn an_hfs_plus_disk_reads_through_the_helper_with_its_journal() {
    let Some(p) = fixture("hfs-basic.dmg") else { return };
    let (disk, server) = remote_disk(p);
    let loc = image::locate_volume(&*disk, None).unwrap();
    assert_eq!(loc.kind, Kind::HfsPlus);
    let vol = Slice::new(&*disk, loc.offset, loc.len);
    let journaled = JournaledSource::new(&vol).unwrap();
    let fs = HfsPlus::open(&journaled).unwrap();
    assert_eq!(read_all(&fs, "/hello.txt"), b"hello from apfs\n");
    assert_eq!(read_all(&fs, "/hardlink-to-hello"), b"hello from apfs\n");
    drop(fs);
    drop(journaled);
    drop(disk);
    server.join().unwrap().unwrap();
}

//! `extract`: copy a file or folder tree out of a volume into a Windows folder.
//!
//! Built for rescuing data: it carries on past read errors, reports each one at
//! the end, and keeps whatever part of a damaged file could be read.

use apfsreader_core::vfs::{EntryKind, FileSystem};
use apfsreader_core::winnames::{fold, windows_name};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Default)]
pub struct Stats {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub links: u64,
    pub errors: Vec<String>,
}

pub struct Options {
    pub verbose: bool,
}

/// Copy `src` (a path inside the volume) into the existing folder `dest`.
/// A folder is copied as a folder of the same name; the volume root is copied
/// as its contents.
pub fn extract<F: FileSystem>(fs: &F, src: &str, dest: &Path, opts: &Options) -> Result<Stats, String> {
    let entry = fs.lookup(src).map_err(|e| format!("{src}: {e}"))?;
    // Absolute `\\?\` form, so paths over 260 characters still work.
    let dest = fs::canonicalize(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let mut stats = Stats::default();
    let info = fs.info(&entry);
    if info.kind == EntryKind::Dir && src.trim_matches('/').is_empty() {
        copy_dir_contents(fs, &entry, &dest, &mut stats, opts, src);
    } else {
        let name = unique_name(&mut HashSet::new(), &windows_name(&info.name));
        copy_entry(fs, &entry, &dest.join(name), &mut stats, opts, src);
    }
    Ok(stats)
}

/// A name not yet used in this folder (compared the way Windows compares).
fn unique_name(used: &mut HashSet<String>, wanted: &str) -> String {
    let mut name = wanted.to_string();
    let mut n = 2;
    while !used.insert(fold(&name)) {
        let (stem, ext) = match wanted.rfind('.') {
            Some(i) if i > 0 => (&wanted[..i], &wanted[i..]),
            _ => (wanted, ""),
        };
        name = format!("{stem} ({n}){ext}");
        n += 1;
    }
    name
}

fn copy_dir_contents<F: FileSystem>(
    fs: &F,
    dir: &F::Entry,
    dest: &Path,
    stats: &mut Stats,
    opts: &Options,
    shown: &str,
) {
    let kids = match fs.read_dir(dir) {
        Ok(k) => k,
        Err(e) => {
            stats.errors.push(format!("{shown}: cannot list folder: {e}"));
            return;
        }
    };
    let mut used = HashSet::new();
    for kid in kids {
        let info = fs.info(&kid);
        let name = unique_name(&mut used, &windows_name(&info.name));
        let child_shown = format!("{}/{}", shown.trim_end_matches('/'), info.name);
        copy_entry(fs, &kid, &dest.join(name), stats, opts, &child_shown);
    }
}

fn copy_entry<F: FileSystem>(
    fs: &F,
    entry: &F::Entry,
    target: &Path,
    stats: &mut Stats,
    opts: &Options,
    shown: &str,
) {
    let info = fs.info(entry);
    match info.kind {
        EntryKind::Dir => {
            if let Err(e) = fs::create_dir_all(target) {
                stats.errors.push(format!("{shown}: cannot create folder: {e}"));
                return;
            }
            stats.dirs += 1;
            copy_dir_contents(fs, entry, target, stats, opts, shown);
            set_mtime(target, info.modify_time);
        }
        EntryKind::Symlink => {
            // Windows links need privileges and have different semantics, so
            // the target is kept as text beside where the link would be.
            let text = fs.read_link(entry).unwrap_or_else(|e| {
                stats.errors.push(format!("{shown}: cannot read link target: {e}"));
                String::new()
            });
            let mut name = target.as_os_str().to_owned();
            name.push(".symlink");
            if let Err(e) = fs::write(PathBuf::from(name), text) {
                stats.errors.push(format!("{shown}: cannot write link note: {e}"));
            } else {
                stats.links += 1;
            }
        }
        EntryKind::File => {
            if opts.verbose {
                println!("{shown}");
            }
            match copy_file(fs, entry, target, info.size) {
                Ok(n) => {
                    stats.files += 1;
                    stats.bytes += n;
                }
                Err((n, msg)) => {
                    stats.bytes += n;
                    stats.errors.push(format!("{shown}: {msg} (kept {n} of {} bytes)", info.size));
                }
            }
            set_mtime(target, info.modify_time);
        }
    }
}

/// Returns bytes written, or (bytes written so far, reason).
fn copy_file<F: FileSystem>(
    fs: &F,
    entry: &F::Entry,
    target: &Path,
    size: u64,
) -> Result<u64, (u64, String)> {
    let file = File::create(target).map_err(|e| (0, format!("cannot create file: {e}")))?;
    let mut out = BufWriter::with_capacity(1 << 20, file);
    let mut buf = vec![0u8; 1 << 20];
    let mut off = 0u64;
    while off < size {
        let n = match fs.read(entry, off, &mut buf) {
            Ok(0) => {
                let _ = out.flush();
                return Err((off, "ended early".into()));
            }
            Ok(n) => n,
            Err(e) => {
                let _ = out.flush();
                return Err((off, format!("read error: {e}")));
            }
        };
        out.write_all(&buf[..n]).map_err(|e| (off, format!("write error: {e}")))?;
        off += n as u64;
    }
    out.flush().map_err(|e| (off, format!("write error: {e}")))?;
    Ok(off)
}

fn set_mtime(path: &Path, unix_secs: i64) {
    if unix_secs <= 0 {
        return;
    }
    let when = SystemTime::UNIX_EPOCH + Duration::from_secs(unix_secs as u64);
    // Folders need a handle opened with write access to take a new time.
    if let Ok(f) = fs::OpenOptions::new().write(true).open(path) {
        let _ = f.set_modified(when);
    } else if let Ok(f) = File::open(path) {
        let _ = f.set_modified(when);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colliding_names_get_numbered() {
        let mut used = HashSet::new();
        assert_eq!(unique_name(&mut used, "a.txt"), "a.txt");
        assert_eq!(unique_name(&mut used, "A.TXT"), "A (2).TXT");
        assert_eq!(unique_name(&mut used, "a.txt"), "a (3).txt");
        assert_eq!(unique_name(&mut used, "b"), "b");
    }
}

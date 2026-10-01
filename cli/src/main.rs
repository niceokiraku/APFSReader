use apfsreader_core::apfs::{role_name, Container};
use apfsreader_core::device::Slice;
use apfsreader_core::image::{self, Kind};
use apfsreader_core::partition::{self, PartKind};
use apfsreader_core::hfsplus::journal::JournaledSource;
use apfsreader_core::hfsplus::{self, HfsPlus};
use apfsreader_core::vfs::{EntryKind, FileSystem, Info};
mod extract;
use std::io::Write;
use std::process::exit;

const USAGE: &str = "usage:
  apfsreader parts <image>
  apfsreader info  <image> [--part N] [--vol N]
  apfsreader ls    <image> [path] [--part N] [--vol N]
  apfsreader cat   <image> <path> [--part N] [--vol N]
  apfsreader disks                      list physical disks (no administrator rights needed)

<image> can also be a physical disk such as \\\\.\\PhysicalDrive2 (administrator rights needed).
  apfsreader extract <image> <path> <dest folder> [-v] [--part N] [--vol N]

<image> may be a raw image, a UDIF .dmg, or a disk image with a GPT or Apple Partition Map.
--part selects a partition; --vol selects an APFS volume (default: the Data volume if there is one, else volume 0).";

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("error: {msg}");
    exit(1);
}

fn take_opt(args: &mut Vec<String>, name: &str) -> Option<usize> {
    let i = args.iter().position(|a| a == name)?;
    if i + 1 >= args.len() {
        die(format!("{name} needs a number"));
    }
    let v = args[i + 1].parse::<usize>().unwrap_or_else(|_| die(format!("bad {name} value")));
    args.drain(i..i + 2);
    Some(v)
}

fn pump(
    mut read: impl FnMut(u64, &mut [u8]) -> Result<usize, String>,
    sink: &mut dyn Write,
) -> Result<(), String> {
    let mut buf = vec![0u8; 1 << 20];
    let mut off = 0u64;
    loop {
        let n = read(off, &mut buf)?;
        if n == 0 {
            return Ok(());
        }
        sink.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        off += n as u64;
    }
}

fn print_listing(items: Vec<Info>) {
    for it in items {
        let tag = match it.kind {
            EntryKind::Dir => 'd',
            EntryKind::Symlink => 'l',
            EntryKind::File => '-',
        };
        println!("{tag} {:>12} {}{}", it.size, it.name, if it.compressed { "  [compressed]" } else { "" });
    }
}

fn run<F: FileSystem>(fs: &F, cmd: &str, args: &[String]) {
    match cmd {
        "ls" => {
            let path = args.get(2).map(String::as_str).unwrap_or("/");
            let dir = fs.lookup(path).unwrap_or_else(|e| die(e));
            let items = if fs.info(&dir).kind == EntryKind::Dir {
                fs.read_dir(&dir).unwrap_or_else(|e| die(e)).iter().map(|e| fs.info(e)).collect()
            } else {
                vec![fs.info(&dir)]
            };
            print_listing(items);
        }
        "cat" => {
            let Some(path) = args.get(2) else { die("cat needs a path") };
            let e = fs.lookup(path).unwrap_or_else(|e| die(e));
            pump(|off, b| fs.read(&e, off, b).map_err(|e| e.to_string()), &mut std::io::stdout().lock())
                .unwrap_or_else(|e| die(e));
        }
        "extract" => {
            let (Some(src), Some(dest)) = (args.get(2), args.get(3)) else {
                die("extract needs a source path inside the volume and a destination folder")
            };
            std::fs::create_dir_all(dest).unwrap_or_else(|e| die(format!("{dest}: {e}")));
            let opts = extract::Options { verbose: args.iter().any(|a| a == "-v") };
            let stats = extract::extract(fs, src, std::path::Path::new(dest), &opts).unwrap_or_else(|e| die(e));
            println!(
                "extracted {} file(s), {} folder(s), {} link note(s), {} bytes",
                stats.files, stats.dirs, stats.links, stats.bytes
            );
            if !stats.errors.is_empty() {
                eprintln!("{} problem(s):", stats.errors.len());
                for e in &stats.errors {
                    eprintln!("  {e}");
                }
                exit(3);
            }
        }
        _ => {
            eprintln!("{USAGE}");
            exit(2);
        }
    }
}

#[cfg(windows)]
fn list_disks() {
    use apfsreader_core::physical;
    let elevated = physical::is_elevated();
    println!("{}", if elevated { "running as administrator" } else { "not running as administrator: disks can be listed but not read" });
    for d in physical::list_disks() {
        println!(
            "{:<26} {:>10.1} GB  {:<10} {}{}{}",
            d.path(),
            d.size as f64 / 1e9,
            d.bus,
            d.display_name(),
            if d.removable { "  [removable]" } else { "" },
            if d.is_system { "  [Windows system disk]" } else { "" }
        );
    }
}

#[cfg(not(windows))]
fn list_disks() {
    die("listing physical disks is only supported on Windows");
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let part = take_opt(&mut args, "--part");
    let vol_index = take_opt(&mut args, "--vol");
    if args.first().map(String::as_str) == Some("disks") {
        list_disks();
        return;
    }
    let (Some(cmd), Some(image)) = (args.first().cloned(), args.get(1).cloned()) else {
        eprintln!("{USAGE}");
        exit(2);
    };
    let src = image::open_source(&image).unwrap_or_else(|e| die(e));
    let src = &*src;

    if cmd == "parts" {
        println!("size: {} bytes", src.len());
        match partition::scan(src) {
            Ok(parts) => {
                for (i, p) in parts.iter().enumerate() {
                    let kind = match p.kind {
                        PartKind::Apfs => "APFS",
                        PartKind::HfsPlus => "HFS+",
                        PartKind::Other => "other",
                    };
                    println!(
                        "#{i} {kind:<5} {} offset={} len={} name={:?}",
                        p.scheme, p.offset, p.len, p.name
                    );
                }
            }
            Err(e) => match image::sniff(src, 0).unwrap_or_else(|e| die(e)) {
                Some(Kind::HfsPlus) => println!("bare HFS+ volume (no partition table)"),
                Some(Kind::Apfs) => println!("bare APFS container (no partition table)"),
                None => die(format!("unrecognised image: {e}")),
            },
        }
        return;
    }

    let loc = image::locate_volume(src, part).unwrap_or_else(|e| die(e));
    let vol_src = Slice::new(src, loc.offset, loc.len);

    match loc.kind {
        Kind::HfsPlus => {
            // Serve the volume as it would be after a journal replay; nothing is written.
            let journaled = JournaledSource::new(&vol_src).unwrap_or_else(|e| die(e));
            let fs = HfsPlus::open(&journaled).unwrap_or_else(|e| die(e));
            if cmd == "info" {
                let h: &hfsplus::VolumeHeader = &fs.header;
                println!("type:        {}", if h.is_hfsx() { "HFSX (case-sensitive)" } else { "HFS+" });
                println!("block size:  {}", h.block_size);
                println!("blocks:      {} total, {} free", h.total_blocks, h.free_blocks);
                println!("files/dirs:  {} / {}", h.file_count, h.folder_count);
                println!("journaled:   {}", h.is_journaled());
                let r = &journaled.report;
                if r.transactions > 0 {
                    println!(
                        "journal:     {} transaction(s) replayed in memory ({} sectors); the device was not modified",
                        r.transactions, r.sectors
                    );
                }
                if r.stopped_early {
                    println!("warning: the journal ends at a damaged transaction; later changes were ignored");
                }
                println!("clean:       {}", h.is_clean());
                if !h.is_clean() && !r.journaled {
                    println!("warning: volume was not cleanly unmounted and has no journal; contents may be stale");
                }
            } else {
                run(&fs, &cmd, &args);
            }
        }
        Kind::Apfs => {
            let c = Container::open(&vol_src).unwrap_or_else(|e| die(e));
            if cmd == "info" {
                println!("type:        APFS container");
                println!("block size:  {}", c.block_size);
                println!("size:        {} blocks", c.block_count);
                println!("checkpoint:  xid {}", c.xid);
                println!("volumes:     {}", c.volume_count());
                let default = c.default_volume().ok();
                for i in 0..c.volume_count() {
                    match c.volume(i) {
                        Ok(v) => println!(
                            "  #{i} {:?}  role={} files={} dirs={} case-insensitive={} encrypted={}{}",
                            v.name,
                            role_name(v.role),
                            v.num_files,
                            v.num_directories,
                            v.is_case_insensitive(),
                            v.encrypted,
                            if default == Some(i) { "  (default)" } else { "" }
                        ),
                        Err(e) => println!("  #{i} unreadable: {e}"),
                    }
                }
            } else {
                let index = vol_index.unwrap_or_else(|| c.default_volume().unwrap_or_else(|e| die(e)));
                let v = c.volume(index).unwrap_or_else(|e| die(e));
                run(&v, &cmd, &args);
            }
        }
    }
}

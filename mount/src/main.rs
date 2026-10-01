//! Mount an HFS+ or APFS disk image (read-only) as a Windows drive via WinFsp.

use apfsreader_mount::{free_drive_letter, mount, MountRequest, Source};
use std::path::PathBuf;
use std::process::exit;

const USAGE: &str = "usage: apfsreader-mount <image> <mount point> [--part N] [--vol N]

  <image>        raw image, UDIF .dmg, or a disk image with a GPT / Apple Partition Map
  <mount point>  a drive letter such as X:, an empty folder, or * for the next free letter
  --part N       partition to use (default: the first HFS+ or APFS one)
  --vol N        APFS volume within the container (default: the Data volume if any, else 0)

The volume is mounted read-only. Press Enter or Ctrl+C to unmount.
Set APFSREADER_TRACE=1 to log every request.";

fn die(msg: impl std::fmt::Display) -> ! {
    eprintln!("error: {msg}");
    exit(1);
}

fn take_opt(args: &mut Vec<String>, name: &str) -> Option<usize> {
    let i = args.iter().position(|a| a == name)?;
    let v = args.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or_else(|| die(format!("{name} needs a number")));
    args.drain(i..i + 2);
    Some(v)
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let partition = take_opt(&mut args, "--part");
    let volume = take_opt(&mut args, "--vol");
    let (Some(path), Some(mut mount_point)) = (args.first().cloned(), args.get(1).cloned()) else {
        eprintln!("{USAGE}");
        exit(2);
    };
    if std::env::var_os("APFSREADER_TRACE").is_some() {
        apfsreader_mount::set_trace(true);
    }
    if mount_point == "*" {
        mount_point = free_drive_letter().unwrap_or_else(|| die("no free drive letter"));
    }

    let mounted = mount(MountRequest { source: Source::Path(PathBuf::from(path)), mount_point, partition, volume })
        .unwrap_or_else(|e| die(e));
    for note in &mounted.info.notes {
        eprintln!("note: {note}");
    }
    println!(
        "Mounted \"{}\" at {} (read-only). Press Enter or Ctrl+C to unmount.",
        mounted.info.label, mounted.info.mount_point
    );
    wait_for_unmount_request();
    mounted.unmount();
}

/// Block until the user presses Enter or Ctrl+C (or closes the console), so
/// the caller can unmount cleanly instead of being killed mid-request.
fn wait_for_unmount_request() {
    use std::sync::mpsc::{channel, Sender};
    use std::sync::OnceLock;
    use windows::core::BOOL;
    use windows::Win32::System::Console::SetConsoleCtrlHandler;

    static STOP: OnceLock<Sender<()>> = OnceLock::new();

    unsafe extern "system" fn on_ctrl(_event: u32) -> BOOL {
        if let Some(tx) = STOP.get() {
            let _ = tx.send(());
        }
        BOOL(1) // handled: do not terminate the process before we unmount
    }

    let (tx, rx) = channel::<()>();
    let _ = STOP.set(tx.clone());
    unsafe {
        let _ = SetConsoleCtrlHandler(Some(on_ctrl), true);
    }
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        let _ = tx.send(());
    });
    let _ = rx.recv();
}

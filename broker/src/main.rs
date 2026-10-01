//! `apfsreader-broker`: the one piece of APFSReader that runs with administrator
//! rights. It opens a single physical disk read-only and serves raw reads of it
//! to the launching user's app over a private named pipe, then exits when the
//! app disconnects.
//!
//! It deliberately serves *only* `\\.\PhysicalDriveN`, never a file path: a
//! helper that read arbitrary files with administrator rights on request would
//! hand every program running as the user a way to read protected files.
//!
//! No window or console: a failure is reported through the exit code.
//!   0 finished normally      2 bad arguments        3 the disk could not be opened
//!   4 the pipe failed        5 no client connected in time
#![windows_subsystem = "windows"]

use apfsreader_core::device::BlockSource;
use apfsreader_core::physical;
use apfsreader_core::remote::pipe::{from_hex, serve_pipe};
use apfsreader_core::remote::TOKEN_LEN;
use std::process::exit;
use std::time::Duration;

const EXIT_ARGS: i32 = 2;
const EXIT_DISK: i32 = 3;
const EXIT_PIPE: i32 = 4;
const EXIT_TIMEOUT: i32 = 5;

struct Args {
    disk: u32,
    pipe: String,
    token: [u8; TOKEN_LEN],
    sid: String,
    timeout: Duration,
}

fn parse(args: &[String]) -> Option<Args> {
    let value = |name: &str| {
        let i = args.iter().position(|a| a == name)?;
        args.get(i + 1).cloned()
    };
    let pipe = value("--pipe")?;
    // The name ends up inside a path, so it must be a plain token.
    if pipe.is_empty() || !pipe.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    let sid = value("--sid")?;
    if !sid.starts_with("S-1-") || !sid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    Some(Args {
        disk: value("--disk")?.parse().ok().filter(|n| *n < 64)?,
        pipe,
        token: from_hex(&value("--token")?)?,
        sid,
        timeout: Duration::from_secs(value("--timeout").and_then(|s| s.parse().ok()).unwrap_or(60).min(600)),
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(a) = parse(&args) else { exit(EXIT_ARGS) };

    // Open before creating the pipe so the app learns about a refusal at once.
    let disk = match physical::open_disk(a.disk) {
        Ok(d) => d,
        Err(_) => exit(EXIT_DISK),
    };
    let _: &dyn BlockSource = &disk;

    match serve_pipe(&a.pipe, &a.sid, &a.token, &disk, a.timeout) {
        Ok(()) => exit(0),
        Err(apfsreader_core::Error::Io(e)) if e.kind() == std::io::ErrorKind::TimedOut => exit(EXIT_TIMEOUT),
        Err(_) => exit(EXIT_PIPE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    const GOOD: &str = "--disk 3 --pipe apfsreader-0a1b --token 00112233445566778899aabbccddeeff --sid S-1-5-21-1-2-3-1001";

    #[test]
    fn well_formed_arguments_parse() {
        let a = parse(&args(GOOD)).unwrap();
        assert_eq!((a.disk, a.pipe.as_str()), (3, "apfsreader-0a1b"));
        assert_eq!(a.timeout, Duration::from_secs(60));
    }

    #[test]
    fn hostile_or_malformed_arguments_are_rejected() {
        for bad in [
            GOOD.replace("--disk 3", "--disk 99"),
            GOOD.replace("--disk 3", "--disk -1"),
            GOOD.replace("apfsreader-0a1b", "..\\evil"),
            GOOD.replace("apfsreader-0a1b", "a\\b"),
            GOOD.replace("0011", "zz11"),
            GOOD.replace("S-1-5-21-1-2-3-1001", "O:SYD:(A;;GA;;;WD)"),
            GOOD.replace("--sid S-1-5-21-1-2-3-1001", ""),
            GOOD.replace("--token 00112233445566778899aabbccddeeff", "--token 0011"),
        ] {
            assert!(parse(&args(&bad)).is_none(), "accepted: {bad}");
        }
    }

    #[test]
    fn the_timeout_is_capped() {
        let a = parse(&args(&format!("{GOOD} --timeout 100000"))).unwrap();
        assert_eq!(a.timeout, Duration::from_secs(600));
    }
}

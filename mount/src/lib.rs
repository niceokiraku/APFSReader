//! Mount HFS+ / APFS volumes read-only as Windows drives through WinFsp.
//!
//! Licensed GPL-3.0 because it links winfsp-rs; the `apfsreader-core` library
//! it builds on is MIT.

mod session;
mod winfs;

pub use session::{
    ensure_winfsp, free_drive_letter, mount, MountError, MountInfo, MountRequest, Mounted, Source,
};

/// Log every WinFsp request to stderr (diagnostics).
pub fn set_trace(on: bool) {
    winfs::TRACE.store(on, std::sync::atomic::Ordering::Relaxed);
}

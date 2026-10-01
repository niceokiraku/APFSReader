//! Mounting a volume as a Windows drive, and cleanly releasing it again.
//!
//! The file system code borrows its block source (`HfsPlus<'a>` borrows a
//! journal wrapper, which borrows a partition slice, which borrows the image),
//! but a running WinFsp host needs everything to live until it stops. `Stack`
//! owns the whole chain on the heap and the borrows are extended to `'static`
//! internally. That is sound because `Mounted` stops and drops the WinFsp host
//! *first* and only then drops the stack, in a fixed order, so nothing the
//! host can reach is freed while it can still run. Unlike leaking the chain,
//! this releases the image file or physical disk handle on unmount.

use crate::winfs::Context;
use apfsreader_core::apfs::{self, Container, Volume};
use apfsreader_core::device::{BlockSource, Slice};
use apfsreader_core::hfsplus::journal::JournaledSource;
use apfsreader_core::hfsplus::HfsPlus;
use apfsreader_core::image::{self, Kind};
use apfsreader_core::vfs::FileSystem;
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use winfsp::host::{FileSystemHost, VolumeParams};

#[derive(Debug)]
pub enum MountError {
    /// WinFsp is not installed (its DLL could not be loaded).
    WinFspMissing,
    /// The image or disk could not be read, or holds nothing supported.
    Volume(apfsreader_core::Error),
    /// The drive letter or folder cannot be used.
    MountPoint(String),
    /// WinFsp refused to create or start the file system.
    Host(String),
}

impl fmt::Display for MountError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MountError::WinFspMissing => write!(f, "WinFsp is not installed (winfsp-x64.dll could not be loaded)"),
            MountError::Volume(e) => write!(f, "{e}"),
            MountError::MountPoint(m) => write!(f, "{m}"),
            MountError::Host(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for MountError {}

impl From<apfsreader_core::Error> for MountError {
    fn from(e: apfsreader_core::Error) -> Self {
        MountError::Volume(e)
    }
}

/// Where the data comes from.
pub enum Source {
    /// An image file (raw or `.dmg`) or a `\\.\PhysicalDriveN` path.
    Path(PathBuf),
    /// A source opened elsewhere, for example served by the disk helper.
    Opened(Box<dyn BlockSource>),
}

pub struct MountRequest {
    pub source: Source,
    /// `X:` or the path of an empty folder.
    pub mount_point: String,
    /// Partition to use; `None` picks the first HFS+ or APFS one.
    pub partition: Option<usize>,
    /// APFS volume to use; `None` prefers the Data volume.
    pub volume: Option<usize>,
}

/// What was mounted, for display.
#[derive(Debug, Clone)]
pub struct MountInfo {
    pub label: String,
    pub kind: Kind,
    pub mount_point: String,
    /// Things worth telling the user, such as a journal having been replayed.
    pub notes: Vec<String>,
}

/// Everything the volume borrows from, in drop order (first field drops first).
#[allow(dead_code)] // the fields are held only so they drop in order
struct Stack {
    fs: Held,
    journaled: Option<Box<JournaledSource<'static>>>,
    slice: Box<Slice<'static>>,
    source: Box<dyn BlockSource>,
}

#[allow(dead_code)] // held only so the volume drops in the right order
enum Held {
    Hfs(Box<HfsPlus<'static>>),
    Apfs { volume: Box<Volume<'static, 'static>>, container: Box<Container<'static>> },
}

/// Type-erased handle to a running WinFsp host.
trait Host: Send {
    fn shutdown(&mut self);
}

struct Running<F: FileSystem + 'static>(FileSystemHost<Context<F>>);

impl<F: FileSystem + 'static> Host for Running<F> {
    fn shutdown(&mut self) {
        self.0.stop();
        self.0.unmount();
    }
}

/// A mounted volume. Dropping it unmounts and releases the image or disk.
pub struct Mounted {
    // Field order is the drop order: the host must go before the data it serves.
    host: Option<Box<dyn Host>>,
    _stack: Stack,
    pub info: MountInfo,
}

impl Mounted {
    /// Unmount now (also what dropping does).
    pub fn unmount(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        if let Some(mut h) = self.host.take() {
            h.shutdown();
            drop(h);
        }
    }
}

impl Drop for Mounted {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Load WinFsp from its install directory by absolute path, then initialise
/// it. The winfsp crate alone only looks next to the executable.
///
/// Only success is remembered: if WinFsp is missing, the next call looks again,
/// so installing it while the app is running is noticed.
pub fn ensure_winfsp() -> Result<(), MountError> {
    static READY: AtomicBool = AtomicBool::new(false);
    if READY.load(Ordering::Relaxed) {
        return Ok(());
    }
    preload_winfsp();
    if winfsp::winfsp_init().is_ok() {
        READY.store(true, Ordering::Relaxed);
        Ok(())
    } else {
        Err(MountError::WinFspMissing)
    }
}

fn preload_winfsp() {
    use windows::core::HSTRING;
    use windows::Win32::System::LibraryLoader::LoadLibraryW;

    let dll = if cfg!(target_arch = "aarch64") { "winfsp-a64.dll" } else { "winfsp-x64.dll" };
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(d) = std::env::var_os("WINFSP_DIR") {
        dirs.push(d.into());
    }
    for var in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(pf) = std::env::var_os(var) {
            dirs.push(std::path::Path::new(&pf).join("WinFsp").join("bin"));
        }
    }
    for dir in dirs {
        let path = dir.join(dll);
        if path.is_file() && unsafe { LoadLibraryW(&HSTRING::from(path.as_os_str())) }.is_ok() {
            return;
        }
    }
}

/// The highest unused drive letter, e.g. `"Z:"`.
pub fn free_drive_letter() -> Option<String> {
    ('D'..='Z').rev().map(|c| format!("{c}:")).find(|d| !std::path::Path::new(&format!("{d}\\")).exists())
}

fn check_mount_point(mp: &str) -> Result<(), MountError> {
    let is_letter = mp.len() == 2 && mp.as_bytes()[1] == b':' && mp.as_bytes()[0].is_ascii_alphabetic();
    if is_letter {
        if std::path::Path::new(&format!("{mp}\\")).exists() {
            return Err(MountError::MountPoint(format!("drive {mp} is already in use")));
        }
        return Ok(());
    }
    let p = std::path::Path::new(mp);
    match std::fs::read_dir(p) {
        Ok(mut entries) => {
            if entries.next().is_none() {
                Ok(())
            } else {
                Err(MountError::MountPoint(format!("{mp} is not empty")))
            }
        }
        Err(e) => Err(MountError::MountPoint(format!("{mp}: {e}"))),
    }
}

/// Extend a borrow to `'static`.
///
/// # Safety
/// The referent must outlive every use of the result. `Stack` guarantees that
/// by owning the referent in a `Box` (stable address) and being dropped last.
unsafe fn extend<'a, T: ?Sized>(r: &'a T) -> &'static T {
    &*(r as *const T)
}

fn start<F: FileSystem + 'static>(fs: &'static F, mount_point: &str) -> Result<Box<dyn Host>, MountError> {
    let mut params = VolumeParams::new();
    params
        .filesystem_name("APFSReader")
        .sector_size(512)
        .sectors_per_allocation_unit(8)
        .max_component_length(255)
        .case_sensitive_search(false)
        .case_preserved_names(true)
        .unicode_on_disk(true)
        .read_only_volume(true)
        .file_info_timeout(1000);
    let mut host: FileSystemHost<Context<F>> = FileSystemHost::new(params, Context::new(fs))
        .map_err(|e| MountError::Host(format!("could not create the WinFsp file system: {e:?}")))?;
    host.mount(mount_point)
        .map_err(|e| MountError::Host(format!("could not mount at {mount_point}: {e:?}")))?;
    host.start().map_err(|e| MountError::Host(format!("could not start the file system: {e:?}")))?;
    Ok(Box::new(Running(host)))
}

/// Open the source, find the volume and mount it read-only.
pub fn mount(req: MountRequest) -> Result<Mounted, MountError> {
    ensure_winfsp()?;
    check_mount_point(&req.mount_point)?;

    let source: Box<dyn BlockSource> = match req.source {
        Source::Path(p) => image::open_source(&p)?,
        Source::Opened(s) => s,
    };
    // SAFETY (all `extend` calls below): see `Stack`; each referent is boxed and owned by the stack.
    let src_ref: &'static dyn BlockSource = unsafe { extend(&*source) };
    let loc = image::locate_volume(src_ref, req.partition)?;
    let slice = Box::new(Slice::new(src_ref, loc.offset, loc.len));
    let slice_ref: &'static Slice<'static> = unsafe { extend(&*slice) };

    let mut notes = Vec::new();
    match loc.kind {
        Kind::HfsPlus => {
            let journaled = Box::new(JournaledSource::new(slice_ref)?);
            let r = journaled.report.clone();
            if r.transactions > 0 {
                notes.push(format!(
                    "{} journal transaction(s) were replayed in memory; the disk was not modified",
                    r.transactions
                ));
            }
            if r.stopped_early {
                notes.push("the journal ends at a damaged transaction; later changes were ignored".into());
            }
            let j_ref: &'static JournaledSource<'static> = unsafe { extend(&*journaled) };
            let fs = Box::new(HfsPlus::open(j_ref)?);
            if !fs.header.is_clean() && !r.journaled {
                notes.push("the volume was not cleanly unmounted and has no journal; contents may be stale".into());
            }
            let fs_ref: &'static HfsPlus<'static> = unsafe { extend(&*fs) };
            let label = FileSystem::label(fs_ref);
            let host = start(fs_ref, &req.mount_point)?;
            Ok(Mounted {
                host: Some(host),
                _stack: Stack { fs: Held::Hfs(fs), journaled: Some(journaled), slice, source },
                info: MountInfo { label, kind: Kind::HfsPlus, mount_point: req.mount_point, notes },
            })
        }
        Kind::Apfs => {
            let container = Box::new(Container::open(slice_ref)?);
            let c_ref: &'static Container<'static> = unsafe { extend(&*container) };
            let index = match req.volume {
                Some(i) => i,
                None => c_ref.default_volume()?,
            };
            let volume = Box::new(c_ref.volume(index)?);
            if volume.encrypted {
                return Err(MountError::Volume(apfsreader_core::Error::Unsupported(
                    "this APFS volume is encrypted".into(),
                )));
            }
            if c_ref.volume_count() > 1 {
                notes.push(format!(
                    "the container has {} volumes; mounted \"{}\" ({})",
                    c_ref.volume_count(),
                    volume.name,
                    apfs::role_name(volume.role)
                ));
            }
            let v_ref: &'static Volume<'static, 'static> = unsafe { extend(&*volume) };
            let label = FileSystem::label(v_ref);
            let host = start(v_ref, &req.mount_point)?;
            Ok(Mounted {
                host: Some(host),
                _stack: Stack { fs: Held::Apfs { volume, container }, journaled: None, slice, source },
                info: MountInfo { label, kind: Kind::Apfs, mount_point: req.mount_point, notes },
            })
        }
    }
}

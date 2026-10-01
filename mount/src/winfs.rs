//! A read-only WinFsp file system over any `apfsreader_core::vfs::FileSystem`.

use apfsreader_core::winnames::{filetime, fold, same_form_as, windows_name};
use apfsreader_core::vfs::{EntryKind, FileSystem, Info};
use apfsreader_core::Error as CoreError;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use winfsp::filesystem::{
    DirBuffer, DirInfo, DirMarker, FileInfo, FileSecurity, FileSystemContext, OpenFileInfo,
    VolumeInfo, WideNameInfo,
};
use winfsp::{FspError, Result, U16CStr};
use winfsp_sys::FILE_ACCESS_RIGHTS;

// NTSTATUS values used here.
const STATUS_END_OF_FILE: i32 = 0xC000_0011u32 as i32;
const STATUS_NOT_SUPPORTED: i32 = 0xC000_00BBu32 as i32;
const STATUS_OBJECT_NAME_NOT_FOUND: i32 = 0xC000_0034u32 as i32;
const STATUS_OBJECT_PATH_NOT_FOUND: i32 = 0xC000_003Au32 as i32;
const STATUS_NOT_A_DIRECTORY: i32 = 0xC000_0103u32 as i32;
const STATUS_FILE_IS_A_DIRECTORY: i32 = 0xC000_00BAu32 as i32;
const STATUS_DISK_CORRUPT_ERROR: i32 = 0xC000_0032u32 as i32;
const STATUS_INTERNAL_ERROR: i32 = 0xC000_00E5u32 as i32;
const STATUS_IO_DEVICE_ERROR: i32 = 0xC000_0185u32 as i32;

const FILE_DIRECTORY_FILE: u32 = 0x0000_0001;
const FILE_NON_DIRECTORY_FILE: u32 = 0x0000_0040;
const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

const ALLOCATION_UNIT: u64 = 4096;
/// Directory listings kept in memory; cleared wholesale when exceeded.
const LISTING_CACHE_LIMIT: usize = 128;

/// Set by `APFSREADER_TRACE=1`: log each request to stderr (diagnostics only).
pub static TRACE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

macro_rules! trace {
    ($($arg:tt)*) => {
        if TRACE.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!($($arg)*);
        }
    };
}

fn nt(status: i32) -> FspError {
    FspError::NTSTATUS(status)
}

/// Run a request handler, turning a panic (a bug triggered by a malformed
/// volume, say) into an error for that one request. A panic must not unwind
/// through WinFsp's C frames, which would take the whole process down and
/// leave the drive hanging.
fn guarded<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .unwrap_or_else(|_| Err(nt(STATUS_INTERNAL_ERROR)))
}

/// What a failed read of the volume looks like to Windows.
fn map_error(e: &CoreError) -> FspError {
    nt(match e {
        CoreError::Io(_) | CoreError::OutOfRange { .. } => STATUS_IO_DEVICE_ERROR,
        CoreError::Format(_) => STATUS_DISK_CORRUPT_ERROR,
        CoreError::Unsupported(_) => STATUS_NOT_SUPPORTED,
        CoreError::NotFound(_) => STATUS_OBJECT_NAME_NOT_FOUND,
    })
}

struct Item<E> {
    /// The name as Windows will see it.
    display: String,
    info: Info,
    entry: E,
}

type Listing<E> = Arc<Vec<Item<E>>>;

/// An open file or directory.
pub struct Handle<E> {
    entry: E,
    info: Info,
    is_dir: bool,
    is_root: bool,
    /// Link target text, served as the contents of a symbolic link.
    link: Option<Vec<u8>>,
    dir: DirBuffer,
}

pub struct Context<F: FileSystem + 'static> {
    fs: &'static F,
    listings: Mutex<HashMap<u64, Listing<F::Entry>>>,
}

impl<F: FileSystem + 'static> Context<F> {
    pub fn new(fs: &'static F) -> Self {
        Self { fs, listings: Mutex::new(HashMap::new()) }
    }

    fn listing(&self, dir: &F::Entry, id: u64) -> Result<Listing<F::Entry>> {
        if let Some(l) = self.listings.lock().unwrap().get(&id) {
            return Ok(l.clone());
        }
        let kids = self.fs.read_dir(dir).map_err(|e| map_error(&e))?;
        let items: Vec<Item<F::Entry>> = kids
            .into_iter()
            .map(|entry| {
                let info = self.fs.info(&entry);
                Item { display: windows_name(&info.name), info, entry }
            })
            .collect();
        let l = Arc::new(items);
        let mut cache = self.listings.lock().unwrap();
        if cache.len() >= LISTING_CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(id, l.clone());
        Ok(l)
    }

    /// Resolve a Windows path (`\dir\file`). Returns the entry, its info and
    /// the normalised path (names in the case the volume stores them).
    fn resolve(&self, path: &str) -> Result<(F::Entry, Info, String)> {
        let root = self.fs.root().map_err(|e| map_error(&e))?;
        let mut info = self.fs.info(&root);
        let mut cur = root;
        let mut normalized = String::new();

        let comps: Vec<&str> = path.split('\\').filter(|c| !c.is_empty()).collect();
        for (i, comp) in comps.iter().enumerate() {
            if info.kind != EntryKind::Dir {
                return Err(nt(STATUS_OBJECT_PATH_NOT_FOUND));
            }
            let listing = self.listing(&cur, info.id)?;
            let want = fold(comp);
            // An exact match wins, so volumes with names differing only by
            // case still resolve each to itself.
            let hit = listing
                .iter()
                .find(|it| it.display == *comp)
                .or_else(|| listing.iter().find(|it| fold(&it.display) == want));
            match hit {
                Some(it) => {
                    normalized.push('\\');
                    normalized.push_str(&same_form_as(comp, &it.display));
                    cur = it.entry.clone();
                    info = it.info.clone();
                }
                None if i + 1 == comps.len() => return Err(nt(STATUS_OBJECT_NAME_NOT_FOUND)),
                None => return Err(nt(STATUS_OBJECT_PATH_NOT_FOUND)),
            }
        }
        if normalized.is_empty() {
            normalized.push('\\');
        }
        Ok((cur, info, normalized))
    }
}

fn file_size(info: &Info, link: Option<&Vec<u8>>) -> u64 {
    match (info.kind, link) {
        (EntryKind::Dir, _) => 0,
        (_, Some(l)) => l.len() as u64,
        _ => info.size,
    }
}

fn fill(info: &Info, link: Option<&Vec<u8>>, out: &mut FileInfo) {
    let size = file_size(info, link);
    out.file_attributes =
        if info.kind == EntryKind::Dir { FILE_ATTRIBUTE_DIRECTORY } else { FILE_ATTRIBUTE_READONLY };
    out.reparse_tag = 0;
    out.file_size = size;
    out.allocation_size = size.div_ceil(ALLOCATION_UNIT) * ALLOCATION_UNIT;
    out.creation_time = filetime(info.create_time);
    out.last_write_time = filetime(info.modify_time);
    out.change_time = out.last_write_time;
    out.last_access_time = out.last_write_time;
    out.index_number = info.id;
    out.hard_links = 0;
    out.ea_size = 0;
}

impl<F: FileSystem + 'static> FileSystemContext for Context<F> {
    type FileContext = Handle<F::Entry>;

    fn get_security_by_name(
        &self,
        file_name: &U16CStr,
        _security_descriptor: Option<&mut [c_void]>,
        _reparse_point_resolver: impl FnOnce(&U16CStr) -> Option<FileSecurity>,
    ) -> Result<FileSecurity> {
        guarded(|| {
        let (_, info, _) = self.resolve(&file_name.to_string_lossy())?;
        let attributes =
            if info.kind == EntryKind::Dir { FILE_ATTRIBUTE_DIRECTORY } else { FILE_ATTRIBUTE_READONLY };
        // No security descriptors are stored on these volumes.
        Ok(FileSecurity { reparse: false, sz_security_descriptor: 0, attributes })
        })
    }

    fn open(
        &self,
        file_name: &U16CStr,
        create_options: u32,
        _granted_access: FILE_ACCESS_RIGHTS,
        file_info: &mut OpenFileInfo,
    ) -> Result<Self::FileContext> {
        guarded(|| {
        trace!("open {}", file_name.to_string_lossy());
        let (entry, info, normalized) = self.resolve(&file_name.to_string_lossy())?;
        let is_dir = info.kind == EntryKind::Dir;
        if create_options & FILE_DIRECTORY_FILE != 0 && !is_dir {
            return Err(nt(STATUS_NOT_A_DIRECTORY));
        }
        if create_options & FILE_NON_DIRECTORY_FILE != 0 && is_dir {
            return Err(nt(STATUS_FILE_IS_A_DIRECTORY));
        }
        let link = if info.kind == EntryKind::Symlink {
            Some(self.fs.read_link(&entry).map_err(|e| map_error(&e))?.into_bytes())
        } else {
            None
        };
        fill(&info, link.as_ref(), file_info.as_mut());
        let wide: Vec<u16> = normalized.encode_utf16().collect();
        if wide.len() * 2 < 60_000 {
            file_info.set_normalized_name(&wide, None);
        }
        let is_root = normalized == "\\";
        Ok(Handle { entry, info, is_dir, is_root, link, dir: DirBuffer::new() })
        })
    }

    fn close(&self, _context: Self::FileContext) {}

    fn get_file_info(&self, context: &Self::FileContext, file_info: &mut FileInfo) -> Result<()> {
        fill(&context.info, context.link.as_ref(), file_info);
        Ok(())
    }

    fn get_security(
        &self,
        _context: &Self::FileContext,
        _security_descriptor: Option<&mut [c_void]>,
    ) -> Result<u64> {
        Ok(0)
    }

    fn read(&self, context: &Self::FileContext, buffer: &mut [u8], offset: u64) -> Result<u32> {
        guarded(|| {
        if context.is_dir {
            return Err(nt(STATUS_FILE_IS_A_DIRECTORY));
        }
        let size = file_size(&context.info, context.link.as_ref());
        if offset >= size {
            return Err(nt(STATUS_END_OF_FILE));
        }
        if let Some(link) = &context.link {
            let start = offset as usize;
            let n = buffer.len().min(link.len() - start);
            buffer[..n].copy_from_slice(&link[start..start + n]);
            return Ok(n as u32);
        }
        let n = self.fs.read(&context.entry, offset, buffer).map_err(|e| map_error(&e))?;
        Ok(n as u32)
        })
    }

    fn read_directory(
        &self,
        context: &Self::FileContext,
        _pattern: Option<&U16CStr>,
        marker: DirMarker,
        buffer: &mut [u8],
    ) -> Result<u32> {
        guarded(|| {
        trace!("read_directory id={} marker_none={}", context.info.id, marker.is_none());
        if !context.is_dir {
            return Err(nt(STATUS_NOT_A_DIRECTORY));
        }
        if let Ok(lock) = context.dir.acquire(marker.is_none(), None) {
            let listing = self.listing(&context.entry, context.info.id)?;
            let mut di = DirInfo::<255>::new();
            if !context.is_root {
                for dot in [".", ".."] {
                    di.reset();
                    di.set_name_raw(dot.encode_utf16().collect::<Vec<u16>>().as_slice())?;
                    fill(&context.info, None, di.file_info_mut());
                    lock.write(&mut di)?;
                }
            }
            for it in listing.iter() {
                di.reset();
                // Names Windows cannot represent (over 255 UTF-16 units) are skipped.
                let units: Vec<u16> = it.display.encode_utf16().collect();
                if di.set_name_raw(units.as_slice()).is_err() {
                    continue;
                }
                fill(&it.info, None, di.file_info_mut());
                lock.write(&mut di)?;
            }
        }
        Ok(context.dir.read(marker, buffer))
        })
    }

    fn get_volume_info(&self, out: &mut VolumeInfo) -> Result<()> {
        let s = self.fs.stats();
        out.total_size = s.total_bytes;
        out.free_size = s.free_bytes;
        out.set_volume_label(self.fs.label());
        Ok(())
    }
}


#[cfg(test)]
mod bench {
    use super::*;
    use apfsreader_core::apfs::Container;
    use apfsreader_core::device::Slice;
    use apfsreader_core::image;
    use std::time::Instant;

    #[test]
    fn resolve_cost_in_a_big_folder() {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../testdata/gen/big-apfs.dmg");
        if !p.exists() {
            return;
        }
        let src: &'static dyn apfsreader_core::device::BlockSource =
            Box::leak(image::open_source(p).unwrap());
        let loc = image::locate_volume(src, None).unwrap();
        let vol: &'static Slice<'static> = Box::leak(Box::new(Slice::new(src, loc.offset, loc.len)));
        let c: &'static Container<'static> = Box::leak(Box::new(Container::open(vol).unwrap()));
        let v = Box::leak(Box::new(c.volume(0).unwrap()));
        let ctx = Context::new(&*v);

        let t = Instant::now();
        let (_, info, _) = ctx.resolve(r"\flat").unwrap();
        eprintln!("resolve flat: {:?} (id {})", t.elapsed(), info.id);
        let t = Instant::now();
        let l = ctx.listing(&ctx.resolve(r"\flat").unwrap().0, info.id).unwrap();
        eprintln!("first listing of 8000: {:?} ({} items)", t.elapsed(), l.len());
        let t = Instant::now();
        let l2 = ctx.listing(&ctx.resolve(r"\flat").unwrap().0, info.id).unwrap();
        eprintln!("cached listing: {:?} ({} items)", t.elapsed(), l2.len());
        let t = Instant::now();
        for i in (1..8000).step_by(40) {
            ctx.resolve(&format!(r"\flat\f{i}.txt")).unwrap();
        }
        eprintln!("200 resolves of files in flat: {:?}", t.elapsed());
    }
}

//! Physical disks on Windows: listing them and opening one for reading.
//!
//! Everything here is read-only. Disks are opened with `GENERIC_READ` only and
//! the `BlockSource` interface has no way to write. Listing works without
//! administrator rights (a handle with no access rights can still be queried);
//! reading the contents needs them.

use crate::aligned::{AlignedSource, RawRead};
use crate::{Error, Result};
use std::fs::{File, OpenOptions};
use std::os::windows::fs::{FileExt, OpenOptionsExt};
use std::os::windows::io::AsRawHandle;
use std::sync::Mutex;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows::Win32::System::Ioctl::{
    DISK_GEOMETRY_EX, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, IOCTL_STORAGE_QUERY_PROPERTY,
    VOLUME_DISK_EXTENTS,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::System::IO::DeviceIoControl;

const FILE_SHARE_READ_WRITE: u32 = 0x1 | 0x2;
/// CTL_CODE(IOCTL_VOLUME_BASE = 'V', 0, METHOD_BUFFERED, FILE_ANY_ACCESS)
const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 = 0x0056_0000;
/// Highest disk number probed. Windows numbers disks from 0 and gaps are possible.
const MAX_DISKS: u32 = 64;

#[derive(Debug, Clone)]
pub struct DiskInfo {
    pub number: u32,
    pub vendor: String,
    pub model: String,
    pub bus: &'static str,
    pub size: u64,
    pub sector_size: u32,
    pub removable: bool,
    /// The disk Windows itself is installed on.
    pub is_system: bool,
}

impl DiskInfo {
    pub fn path(&self) -> String {
        format!(r"\\.\PhysicalDrive{}", self.number)
    }

    pub fn display_name(&self) -> String {
        let name = format!("{} {}", self.vendor, self.model).trim().to_string();
        if name.is_empty() { format!("Disk {}", self.number) } else { name }
    }
}

/// True if this process runs with administrator rights.
pub fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
        .is_ok();
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

fn open_for_query(path: &str) -> std::io::Result<File> {
    // No access rights requested: enough for IOCTLs that need none.
    OpenOptions::new().access_mode(0).share_mode(FILE_SHARE_READ_WRITE).open(path)
}

fn ioctl<T>(file: &File, code: u32, input: Option<&[u8]>, out: &mut [u8]) -> std::io::Result<usize> {
    let _ = std::marker::PhantomData::<T>;
    let mut returned = 0u32;
    let handle = HANDLE(file.as_raw_handle());
    unsafe {
        DeviceIoControl(
            handle,
            code,
            input.map(|i| i.as_ptr().cast()),
            input.map_or(0, |i| i.len() as u32),
            Some(out.as_mut_ptr().cast()),
            out.len() as u32,
            Some(&mut returned),
            None,
        )
        .map_err(|e| std::io::Error::from_raw_os_error((e.code().0 & 0xFFFF) as i32))?;
    }
    Ok(returned as usize)
}

/// (size in bytes, bytes per sector)
fn geometry(file: &File) -> std::io::Result<(u64, u32)> {
    let mut buf = vec![0u8; std::mem::size_of::<DISK_GEOMETRY_EX>() + 256];
    ioctl::<()>(file, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, None, &mut buf)?;
    // SAFETY: the buffer is at least as large as the structure and was just filled by the driver.
    let g = unsafe { std::ptr::read_unaligned(buf.as_ptr().cast::<DISK_GEOMETRY_EX>()) };
    Ok((g.DiskSize.max(0) as u64, g.Geometry.BytesPerSector))
}

fn cstr_at(buf: &[u8], offset: u32) -> String {
    let o = offset as usize;
    if o == 0 || o >= buf.len() {
        return String::new();
    }
    let end = buf[o..].iter().position(|&b| b == 0).map_or(buf.len(), |p| o + p);
    String::from_utf8_lossy(&buf[o..end]).trim().to_string()
}

fn bus_name(bus: u32) -> &'static str {
    match bus {
        1 => "SCSI",
        2 => "ATAPI",
        3 => "ATA",
        4 => "IEEE 1394",
        7 => "USB",
        8 => "RAID",
        9 => "iSCSI",
        10 => "SAS",
        11 => "SATA",
        12 => "SD",
        13 => "MMC",
        14 => "Virtual",
        15 => "File-backed virtual",
        16 => "Storage Spaces",
        17 => "NVMe",
        _ => "Unknown bus",
    }
}

/// (vendor, product, bus type, removable) from IOCTL_STORAGE_QUERY_PROPERTY.
fn descriptor(file: &File) -> std::io::Result<(String, String, u32, bool)> {
    // STORAGE_PROPERTY_QUERY { PropertyId: StorageDeviceProperty = 0, QueryType: PropertyStandardQuery = 0, AdditionalParameters[1] }
    let query = [0u8; 12];
    let mut buf = vec![0u8; 1024];
    let n = ioctl::<()>(file, IOCTL_STORAGE_QUERY_PROPERTY, Some(&query), &mut buf)?;
    buf.truncate(n);
    if buf.len() < 36 {
        return Err(std::io::Error::other("short storage descriptor"));
    }
    let u32_at = |o: usize| u32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
    Ok((cstr_at(&buf, u32_at(12)), cstr_at(&buf, u32_at(16)), u32_at(28), buf[10] != 0))
}

/// The number of the disk holding the Windows system drive, if it can be found.
fn system_disk_number() -> Option<u32> {
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let file = open_for_query(&format!(r"\\.\{}", drive.trim_end_matches('\\'))).ok()?;
    let mut buf = vec![0u8; std::mem::size_of::<VOLUME_DISK_EXTENTS>() + 256];
    ioctl::<()>(&file, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, None, &mut buf).ok()?;
    // VOLUME_DISK_EXTENTS { NumberOfDiskExtents: u32, (padding), Extents[]: DISK_EXTENT { DiskNumber: u32, .. } }
    let count = u32::from_le_bytes(buf[0..4].try_into().unwrap());
    (count > 0).then(|| u32::from_le_bytes(buf[8..12].try_into().unwrap()))
}

/// Describe every physical disk Windows exposes. Needs no special rights.
pub fn list_disks() -> Vec<DiskInfo> {
    let system = system_disk_number();
    let mut out = Vec::new();
    for number in 0..MAX_DISKS {
        let path = format!(r"\\.\PhysicalDrive{number}");
        let Ok(file) = open_for_query(&path) else { continue };
        let Ok((size, sector)) = geometry(&file) else { continue };
        let (vendor, model, bus, removable) = descriptor(&file).unwrap_or_default();
        out.push(DiskInfo {
            number,
            vendor,
            model,
            bus: bus_name(bus),
            size,
            sector_size: sector,
            removable,
            is_system: system == Some(number),
        });
    }
    out
}

pub struct WinDisk {
    file: Mutex<File>,
}

impl RawRead for WinDisk {
    fn read_aligned(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        // One request at a time: positional reads on one handle are not
        // guaranteed safe to interleave, and a disk gains nothing from it.
        let file = self.file.lock().unwrap();
        let mut done = 0usize;
        while done < buf.len() {
            let n = file.seek_read(&mut buf[done..], offset + done as u64)?;
            if n == 0 {
                return Err(Error::Io(std::io::ErrorKind::UnexpectedEof.into()));
            }
            done += n;
        }
        Ok(())
    }
}

/// A physical disk opened read-only.
pub type DiskSource = AlignedSource<WinDisk>;

/// Open `\\.\PhysicalDriveN` for reading. Fails with a permission error unless
/// the process is elevated.
pub fn open_disk(number: u32) -> Result<DiskSource> {
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ_WRITE)
        .open(format!(r"\\.\PhysicalDrive{number}"))?;
    let (size, sector) = geometry(&file)?;
    AlignedSource::new(WinDisk { file: Mutex::new(file) }, size, sector as u64)
}

/// True if `e` means "run as administrator".
pub fn is_access_denied(e: &Error) -> bool {
    matches!(e, Error::Io(io) if io.kind() == std::io::ErrorKind::PermissionDenied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_does_not_need_administrator_rights_and_finds_the_system_disk() {
        let disks = list_disks();
        // Any Windows machine has at least one disk, and exactly one carries the system drive.
        assert!(!disks.is_empty());
        assert!(disks.iter().filter(|d| d.is_system).count() <= 1);
        for d in &disks {
            assert!(d.size > 0 && d.sector_size >= 512, "{d:?}");
        }
    }

    #[test]
    fn opening_a_disk_without_rights_is_a_permission_error_not_a_panic() {
        if is_elevated() {
            return; // an elevated run could read it; the test is about the refusal
        }
        let disks = list_disks();
        let Some(d) = disks.first() else { return };
        let err = open_disk(d.number).err().expect("must not open without rights");
        assert!(is_access_denied(&err), "{err:?}");
    }
}

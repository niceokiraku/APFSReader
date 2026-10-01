//! Opening a disk image and finding the volume inside it; shared by the
//! command line tool and the mount front end.

use crate::device::{BlockSource, FileSource};
use crate::partition::{self, PartKind, Partition};
use crate::udif::{self, UdifSource};
use crate::{Error, Result};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    HfsPlus,
    Apfs,
}

/// Where a volume lives within the image.
#[derive(Debug, Clone, Copy)]
pub struct VolumeLocation {
    pub offset: u64,
    pub len: u64,
    pub kind: Kind,
}

/// Open a raw image or a UDIF .dmg as a block source.
pub fn open_source(path: impl AsRef<Path>) -> Result<Box<dyn BlockSource>> {
    let path = path.as_ref();
    if let Some(n) = physical_disk_number(path) {
        #[cfg(windows)]
        return Ok(Box::new(crate::physical::open_disk(n)?));
        #[cfg(not(windows))]
        {
            let _ = n;
            return Err(Error::Unsupported("physical disks are only supported on Windows".into()));
        }
    }
    Ok(if udif::is_udif(path) {
        Box::new(UdifSource::open(path)?)
    } else {
        Box::new(FileSource::open(path)?)
    })
}

/// `N` for a path of the form `\\.\PhysicalDriveN` (any case), else `None`.
pub fn physical_disk_number(path: &Path) -> Option<u32> {
    let s = path.to_str()?;
    let rest = s.strip_prefix(r"\\.\")?;
    let (head, digits) = rest.split_at(rest.len().min(13));
    (head.eq_ignore_ascii_case("PhysicalDrive") && !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
        .then(|| digits.parse().ok())
        .flatten()
}

/// Recognise a bare HFS+ or APFS volume starting at `off`. Reads that run past
/// the end mean "not a volume"; any other read failure (an unsupported DMG
/// codec, say) is a real error and is returned.
pub fn sniff(src: &dyn BlockSource, off: u64) -> Result<Option<Kind>> {
    let mut sig = [0u8; 2];
    match src.read_at(off + 1024, &mut sig) {
        Ok(()) if matches!(&sig, b"H+" | b"HX") => return Ok(Some(Kind::HfsPlus)),
        Ok(()) | Err(Error::OutOfRange { .. }) => {}
        Err(e) => return Err(e),
    }
    let mut magic = [0u8; 4];
    match src.read_at(off + 32, &mut magic) {
        Ok(()) if &magic == b"NXSB" => Ok(Some(Kind::Apfs)),
        Ok(()) | Err(Error::OutOfRange { .. }) => Ok(None),
        Err(e) => Err(e),
    }
}

fn partition_kind(src: &dyn BlockSource, p: &Partition) -> Result<Option<Kind>> {
    Ok(match p.kind {
        PartKind::Apfs => Some(Kind::Apfs),
        // An APM Apple_HFS partition may be classic HFS; the header decides.
        PartKind::HfsPlus => sniff(src, p.offset)?.filter(|k| *k == Kind::HfsPlus),
        PartKind::Other => sniff(src, p.offset)?,
    })
}

/// The whole image if it is a bare volume, otherwise the chosen partition (or
/// the first HFS+/APFS one).
pub fn locate_volume(src: &dyn BlockSource, part: Option<usize>) -> Result<VolumeLocation> {
    if part.is_none() {
        if let Some(kind) = sniff(src, 0)? {
            return Ok(VolumeLocation { offset: 0, len: src.len(), kind });
        }
    }
    let parts = partition::scan(src)
        .map_err(|_| Error::Format("unrecognised image: no HFS+/APFS volume or partition table"))?;
    let chosen = match part {
        Some(i) => parts.get(i).ok_or_else(|| Error::NotFound(format!("partition {i}")))?,
        None => {
            let mut found = None;
            for p in &parts {
                if partition_kind(src, p)?.is_some() {
                    found = Some(p);
                    break;
                }
            }
            found.ok_or(Error::Format("no HFS+ or APFS partition found"))?
        }
    };
    let kind = partition_kind(src, chosen)?.ok_or(Error::Format("partition is neither HFS+ nor APFS"))?;
    Ok(VolumeLocation { offset: chosen.offset, len: chosen.len, kind })
}

/// One mountable volume found in an image or disk.
#[derive(Debug, Clone)]
pub struct Found {
    /// Partition index to pass as `--part`; `None` for a bare volume.
    pub partition: Option<usize>,
    /// APFS volume index within the container; `None` for HFS+.
    pub volume: Option<usize>,
    pub kind: Kind,
    pub name: String,
    /// APFS role ("data", "system", ...) when there is one.
    pub role: Option<&'static str>,
    pub encrypted: bool,
    /// Size of the partition or image holding it.
    pub size: u64,
    /// Set if the volume was found but could not be read.
    pub problem: Option<String>,
}

fn describe(
    src: &dyn BlockSource,
    offset: u64,
    len: u64,
    kind: Kind,
    partition: Option<usize>,
    out: &mut Vec<Found>,
) {
    use crate::apfs::{role_name, Container};
    use crate::device::Slice;
    use crate::hfsplus::journal::JournaledSource;
    use crate::hfsplus::HfsPlus;
    use crate::vfs::FileSystem;

    let slice = Slice::new(src, offset, len);
    let base = Found {
        partition,
        volume: None,
        kind,
        name: String::new(),
        role: None,
        encrypted: false,
        size: len,
        problem: None,
    };
    match kind {
        Kind::HfsPlus => {
            let result = JournaledSource::new(&slice).and_then(|j| HfsPlus::open(&j).map(|fs| fs.label()));
            match result {
                Ok(name) => out.push(Found { name, ..base }),
                Err(e) => out.push(Found { name: "HFS+".into(), problem: Some(e.to_string()), ..base }),
            }
        }
        Kind::Apfs => match Container::open(&slice) {
            Err(e) => out.push(Found { name: "APFS".into(), problem: Some(e.to_string()), ..base }),
            Ok(c) => {
                for i in 0..c.volume_count() {
                    match c.volume(i) {
                        Ok(v) => out.push(Found {
                            volume: Some(i),
                            name: v.name.clone(),
                            role: Some(role_name(v.role)),
                            encrypted: v.encrypted,
                            ..base.clone()
                        }),
                        Err(e) => out.push(Found {
                            volume: Some(i),
                            name: format!("Volume {i}"),
                            problem: Some(e.to_string()),
                            ..base.clone()
                        }),
                    }
                }
            }
        },
    }
}

/// Every HFS+ or APFS volume in `src`, whether it is a bare volume or lies in
/// partitions. Unrecognised content yields an empty list; a volume that is
/// there but unreadable is reported with `problem` set.
pub fn discover(src: &dyn BlockSource) -> Result<Vec<Found>> {
    let mut out = Vec::new();
    if let Some(kind) = sniff(src, 0)? {
        describe(src, 0, src.len(), kind, None, &mut out);
        return Ok(out);
    }
    let Ok(parts) = partition::scan(src) else { return Ok(out) };
    for (i, p) in parts.iter().enumerate() {
        if let Some(kind) = partition_kind(src, p)? {
            describe(src, p.offset, p.len, kind, Some(i), &mut out);
        }
    }
    Ok(out)
}

/// The volume most likely wanted: an APFS Data volume, then any readable
/// unencrypted one.
pub fn preferred(found: &[Found]) -> Option<usize> {
    let usable = |f: &Found| f.problem.is_none() && !f.encrypted;
    found
        .iter()
        .position(|f| usable(f) && f.role == Some("data"))
        .or_else(|| found.iter().position(|f| usable(f) && f.role != Some("system")))
        .or_else(|| found.iter().position(usable))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_disk_paths_are_recognised() {
        let n = |s: &str| physical_disk_number(Path::new(s));
        assert_eq!(n(r"\\.\PhysicalDrive3"), Some(3));
        assert_eq!(n(r"\\.\physicaldrive12"), Some(12));
        assert_eq!(n(r"\\.\PhysicalDrive"), None);
        assert_eq!(n(r"\\.\PhysicalDrive1x"), None);
        assert_eq!(n(r"\.\PhysicalDrive3"), None); // missing a leading backslash
        assert_eq!(n(r"C:\images\disk.dmg"), None);
        assert_eq!(n(r"\\.\C:"), None);
    }
}

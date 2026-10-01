//! Partition tables: GPT and Apple Partition Map, behind one type.

use crate::device::BlockSource;
use crate::util::{slice, u16be, u32be};
use crate::{gpt, Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartKind {
    Apfs,
    /// HFS family. In an APM this is `Apple_HFS`, which may be classic HFS or
    /// HFS+; the volume header tells them apart.
    HfsPlus,
    Other,
}

#[derive(Debug, Clone)]
pub struct Partition {
    pub kind: PartKind,
    /// Byte offset and length within the disk.
    pub offset: u64,
    pub len: u64,
    pub name: String,
    /// "GPT" or "APM".
    pub scheme: &'static str,
}

/// Parse whichever partition table the disk carries.
pub fn scan(src: &dyn BlockSource) -> Result<Vec<Partition>> {
    match gpt::read_partitions(src) {
        Ok(parts) => Ok(parts
            .into_iter()
            .map(|p| Partition {
                kind: match p.type_guid.as_str() {
                    gpt::GUID_APFS => PartKind::Apfs,
                    gpt::GUID_HFS_PLUS => PartKind::HfsPlus,
                    _ => PartKind::Other,
                },
                offset: p.offset(),
                len: p.len(),
                name: p.name,
                scheme: "GPT",
            })
            .collect()),
        Err(_) => read_apm(src),
    }
}

const DDM_SIG: u16 = 0x4552; // 'ER'
const PM_SIG: u16 = 0x504D; // 'PM'
const MAX_ENTRIES: u32 = 256;

fn read_apm(src: &dyn BlockSource) -> Result<Vec<Partition>> {
    let mut ddm = [0u8; 512];
    src.read_at(0, &mut ddm)?;
    if u16be(&ddm, 0)? != DDM_SIG {
        return Err(Error::Format("no GPT or Apple Partition Map found"));
    }
    // The map is addressed in blocks of the size the driver descriptor names.
    let block = u16be(&ddm, 2)? as u64;
    if !block.is_power_of_two() || !(512..=4096).contains(&block) {
        return Err(Error::Format("implausible APM block size"));
    }

    let entry = |index: u64| -> Result<[u8; 512]> {
        let mut b = [0u8; 512];
        src.read_at((1 + index) * block, &mut b)?;
        Ok(b)
    };
    let first = entry(0)?;
    if u16be(&first, 0)? != PM_SIG {
        return Err(Error::Format("APM has no partition map entry"));
    }
    let count = u32be(&first, 4)?;
    if count == 0 || count > MAX_ENTRIES {
        return Err(Error::Format("implausible APM entry count"));
    }

    let mut out = Vec::new();
    for i in 0..count as u64 {
        let e = if i == 0 { first } else { entry(i)? };
        if u16be(&e, 0)? != PM_SIG {
            return Err(Error::Format("bad APM entry signature"));
        }
        let (start, blocks) = (u32be(&e, 8)? as u64, u32be(&e, 12)? as u64);
        let cstr = |o: usize| {
            let raw = slice(&e, o, 32).unwrap_or(&[]);
            let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
            String::from_utf8_lossy(&raw[..end]).into_owned()
        };
        let ty = cstr(48);
        let kind = match ty.as_str() {
            "Apple_APFS" => PartKind::Apfs,
            "Apple_HFS" | "Apple_HFSX" => PartKind::HfsPlus,
            _ => PartKind::Other,
        };
        if blocks == 0 {
            continue;
        }
        out.push(Partition {
            kind,
            offset: start * block,
            len: blocks * block,
            name: format!("{} ({ty})", cstr(16)),
            scheme: "APM",
        });
    }
    Ok(out)
}

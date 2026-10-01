//! GUID Partition Table parser (read-only).

use crate::device::BlockSource;
use crate::{Error, Result};

pub const GUID_APFS: &str = "7C3457EF-0000-11AA-AA11-00306543ECAC";
pub const GUID_HFS_PLUS: &str = "48465300-0000-11AA-AA11-00306543ECAC";

#[derive(Debug, Clone)]
pub struct Partition {
    /// Sector size the table is addressed in (512, or 4096 on a 4Kn disk).
    pub sector: u64,
    pub type_guid: String,
    pub first_lba: u64,
    pub last_lba: u64,
    pub name: String,
}

impl Partition {
    pub fn offset(&self) -> u64 {
        self.first_lba * self.sector
    }
    pub fn len(&self) -> u64 {
        (self.last_lba - self.first_lba + 1) * self.sector
    }
}

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64le(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// Mixed-endian on-disk GUID -> canonical uppercase string.
fn guid_string(b: &[u8]) -> String {
    format!(
        "{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        u32le(b, 0),
        u16::from_le_bytes([b[4], b[5]]),
        u16::from_le_bytes([b[6], b[7]]),
        b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    )
}

pub fn read_partitions(src: &dyn BlockSource) -> Result<Vec<Partition>> {
    // The header is the second sector, so its position reveals the sector size.
    let mut candidates = vec![src.sector_size(), 512, 4096];
    candidates.dedup();
    let mut found = None;
    for sector in candidates {
        let mut hdr = [0u8; 512];
        if src.read_at(sector, &mut hdr).is_ok() && &hdr[0..8] == b"EFI PART" {
            found = Some((sector, hdr));
            break;
        }
    }
    let Some((sector, hdr)) = found else {
        return Err(Error::Format("no GPT header"));
    };
    let entries_lba = u64le(&hdr, 72);
    let count = u32le(&hdr, 80) as usize;
    let entry_size = u32le(&hdr, 84) as usize;
    if entry_size < 128 || entry_size > 4096 || count > 4096 {
        return Err(Error::Format("implausible GPT geometry"));
    }

    let mut table = vec![0u8; count * entry_size];
    src.read_at(entries_lba * sector, &mut table)?;

    let mut out = Vec::new();
    for e in table.chunks_exact(entry_size) {
        if e[0..16].iter().all(|&x| x == 0) {
            continue;
        }
        let first = u64le(e, 32);
        let last = u64le(e, 40);
        if last < first {
            continue;
        }
        let name_units: Vec<u16> = e[56..128]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&c| c != 0)
            .collect();
        out.push(Partition {
            sector,
            type_guid: guid_string(&e[0..16]),
            first_lba: first,
            last_lba: last,
            name: String::from_utf16_lossy(&name_units),
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Disk {
        data: Vec<u8>,
        sector: u64,
    }
    impl BlockSource for Disk {
        fn len(&self) -> u64 {
            self.data.len() as u64
        }
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
            let end = offset as usize + buf.len();
            if end > self.data.len() {
                return Err(Error::OutOfRange { offset, len: buf.len() });
            }
            buf.copy_from_slice(&self.data[offset as usize..end]);
            Ok(())
        }
        fn sector_size(&self) -> u64 {
            self.sector
        }
    }

    /// A GPT with one APFS partition covering sectors 6..=99, written for `sector`-byte sectors.
    fn disk(sector: u64, reported: u64) -> Disk {
        let s = sector as usize;
        let mut data = vec![0u8; s * 128];
        data[s..s + 8].copy_from_slice(b"EFI PART");
        data[s + 72..s + 80].copy_from_slice(&2u64.to_le_bytes()); // entries at LBA 2
        data[s + 80..s + 84].copy_from_slice(&1u32.to_le_bytes()); // one entry
        data[s + 84..s + 88].copy_from_slice(&128u32.to_le_bytes());
        let e = 2 * s;
        // 7C3457EF-0000-11AA-AA11-00306543ECAC in mixed-endian on-disk form.
        data[e..e + 16].copy_from_slice(&[
            0xEF, 0x57, 0x34, 0x7C, 0x00, 0x00, 0xAA, 0x11, 0xAA, 0x11, 0x00, 0x30, 0x65, 0x43, 0xEC, 0xAC,
        ]);
        data[e + 32..e + 40].copy_from_slice(&6u64.to_le_bytes());
        data[e + 40..e + 48].copy_from_slice(&99u64.to_le_bytes());
        Disk { data, sector: reported }
    }

    #[test]
    fn gpt_with_512_byte_sectors() {
        let p = &read_partitions(&disk(512, 512)).unwrap()[0];
        assert_eq!(p.type_guid, GUID_APFS);
        assert_eq!((p.sector, p.offset(), p.len()), (512, 6 * 512, 94 * 512));
    }

    #[test]
    fn gpt_on_a_4kn_disk_uses_4096_byte_sectors() {
        let p = &read_partitions(&disk(4096, 4096)).unwrap()[0];
        assert_eq!((p.sector, p.offset(), p.len()), (4096, 6 * 4096, 94 * 4096));
    }

    #[test]
    fn a_4kn_gpt_is_found_even_if_the_source_reports_512() {
        // A raw dump of a 4Kn disk read from a file says nothing about its sector size.
        let p = &read_partitions(&disk(4096, 512)).unwrap()[0];
        assert_eq!((p.sector, p.offset()), (4096, 6 * 4096));
    }

    #[test]
    fn no_header_is_an_error() {
        let d = Disk { data: vec![0u8; 65536], sector: 512 };
        assert!(read_partitions(&d).is_err());
    }
}

use crate::device::BlockSource;
use crate::util::{slice, u32be, u64be};
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, Default)]
pub struct Extent {
    pub start_block: u32,
    pub block_count: u32,
}

impl Extent {
    pub fn parse(b: &[u8]) -> Result<Self> {
        Ok(Self { start_block: u32be(b, 0)?, block_count: u32be(b, 4)? })
    }
}

#[derive(Debug, Clone, Default)]
pub struct ForkData {
    pub logical_size: u64,
    pub clump_size: u32,
    pub total_blocks: u32,
    pub extents: [Extent; 8],
}

impl ForkData {
    pub const SIZE: usize = 80;

    pub fn parse(b: &[u8]) -> Result<Self> {
        let mut extents = [Extent::default(); 8];
        for (i, e) in extents.iter_mut().enumerate() {
            *e = Extent::parse(slice(b, 16 + i * 8, 8)?)?;
        }
        Ok(Self {
            logical_size: u64be(b, 0)?,
            clump_size: u32be(b, 8)?,
            total_blocks: u32be(b, 12)?,
            extents,
        })
    }
}

/// Read from a fork through its resolved extent list. Returns the number of
/// bytes read (short only at the end of the fork).
pub fn read_extents(
    src: &dyn BlockSource,
    block_size: u64,
    extents: &[Extent],
    logical_size: u64,
    offset: u64,
    buf: &mut [u8],
) -> Result<usize> {
    if offset >= logical_size {
        return Ok(0);
    }
    let want = (buf.len() as u64).min(logical_size - offset) as usize;
    let mut done = 0usize;
    while done < want {
        let pos = offset + done as u64;
        let mut blk = pos / block_size;
        let within = pos % block_size;
        let mut found = None;
        for e in extents {
            if blk < e.block_count as u64 {
                found = Some(e);
                break;
            }
            blk -= e.block_count as u64;
        }
        let e = found.ok_or(Error::Format("fork extent map too short"))?;
        let run = (e.block_count as u64 - blk) * block_size - within;
        let n = run.min((want - done) as u64) as usize;
        let disk = (e.start_block as u64 + blk) * block_size + within;
        src.read_at(disk, &mut buf[done..done + n])?;
        done += n;
    }
    Ok(done)
}

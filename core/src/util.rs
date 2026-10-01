//! Bounds-checked big-endian readers for untrusted on-disk data.

use crate::{Error, Result};

fn take(b: &[u8], o: usize, n: usize) -> Result<&[u8]> {
    o.checked_add(n)
        .and_then(|end| b.get(o..end))
        .ok_or(Error::Format("truncated structure"))
}

pub fn u8at(b: &[u8], o: usize) -> Result<u8> {
    Ok(take(b, o, 1)?[0])
}
pub fn u16be(b: &[u8], o: usize) -> Result<u16> {
    Ok(u16::from_be_bytes(take(b, o, 2)?.try_into().unwrap()))
}
pub fn u32be(b: &[u8], o: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(take(b, o, 4)?.try_into().unwrap()))
}
pub fn u64be(b: &[u8], o: usize) -> Result<u64> {
    Ok(u64::from_be_bytes(take(b, o, 8)?.try_into().unwrap()))
}
pub fn slice(b: &[u8], o: usize, n: usize) -> Result<&[u8]> {
    take(b, o, n)
}

pub fn u16le(b: &[u8], o: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(take(b, o, 2)?.try_into().unwrap()))
}
pub fn u32le(b: &[u8], o: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(take(b, o, 4)?.try_into().unwrap()))
}
pub fn u64le(b: &[u8], o: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(take(b, o, 8)?.try_into().unwrap()))
}

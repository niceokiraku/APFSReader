//! APFS B-tree nodes and ordered range walks.
//!
//! APFS nodes have no sibling links, so range queries recurse through the
//! index nodes. A query is described by a comparator that classifies each key
//! as before / inside / after the wanted range.

use crate::util::{slice, u16le, u32le, u64le};
use crate::{Error, Result};
use std::cmp::Ordering;
use std::sync::Arc;

const OBJ_HEADER: usize = 32;
const NODE_HEADER: usize = 56;
const INFO_SIZE: usize = 40;

const FLAG_ROOT: u16 = 0x1;
const FLAG_LEAF: u16 = 0x2;
const FLAG_FIXED_KV: u16 = 0x4;

const TYPE_BTREE: u32 = 2;
const TYPE_BTREE_NODE: u32 = 3;

const MAX_DEPTH: usize = 32;

pub struct Node {
    data: Arc<Vec<u8>>,
    flags: u16,
    nkeys: usize,
    key_area: usize,
    val_end: usize,
    toc: usize,
    key_size: usize,
    val_size: usize,
}

impl Node {
    fn parse(data: Arc<Vec<u8>>, key_size: usize, val_size: usize) -> Result<Self> {
        let ty = u32le(&data, 24)? & 0xFFFF;
        if ty != TYPE_BTREE && ty != TYPE_BTREE_NODE {
            return Err(Error::Format("object is not a B-tree node"));
        }
        let flags = u16le(&data, OBJ_HEADER)?;
        let nkeys = u32le(&data, OBJ_HEADER + 4)? as usize;
        let toc_off = u16le(&data, OBJ_HEADER + 8)? as usize;
        let toc_len = u16le(&data, OBJ_HEADER + 10)? as usize;
        let size = data.len();
        let toc = NODE_HEADER + toc_off;
        let key_area = toc + toc_len;
        let val_end = if flags & FLAG_ROOT != 0 { size.saturating_sub(INFO_SIZE) } else { size };
        let esz = if flags & FLAG_FIXED_KV != 0 { 4 } else { 8 };
        let toc_bytes = nkeys.checked_mul(esz).ok_or(Error::Format("bad B-tree key count"))?;
        if toc.checked_add(toc_bytes).map_or(true, |e| e > key_area) || key_area > val_end {
            return Err(Error::Format("B-tree node layout out of bounds"));
        }
        Ok(Self { data, flags, nkeys, key_area, val_end, toc, key_size, val_size })
    }

    fn is_leaf(&self) -> bool {
        self.flags & FLAG_LEAF != 0
    }

    fn entry(&self, i: usize) -> Result<(&[u8], &[u8])> {
        let d = &self.data[..];
        let (koff, klen, voff, vlen) = if self.flags & FLAG_FIXED_KV != 0 {
            let t = self.toc + i * 4;
            let vl = if self.is_leaf() { self.val_size } else { 8 };
            (u16le(d, t)? as usize, self.key_size, u16le(d, t + 2)? as usize, vl)
        } else {
            let t = self.toc + i * 8;
            (
                u16le(d, t)? as usize,
                u16le(d, t + 2)? as usize,
                u16le(d, t + 4)? as usize,
                u16le(d, t + 6)? as usize,
            )
        };
        let key = slice(d, self.key_area + koff, klen)?;
        let vstart = self.val_end.checked_sub(voff).ok_or(Error::Format("B-tree value offset out of bounds"))?;
        let val = slice(d, vstart, vlen)?;
        Ok((key, val))
    }
}

/// Walk all entries classified `Equal` by `cmp`, in key order. `visit`
/// returns `false` to stop early. `read_child` fetches a child node's block
/// from the pointer stored in an index node.
pub fn walk(
    root: Arc<Vec<u8>>,
    read_child: &dyn Fn(u64) -> Result<Arc<Vec<u8>>>,
    cmp: &dyn Fn(&[u8]) -> Ordering,
    visit: &mut dyn FnMut(&[u8], &[u8]) -> Result<bool>,
) -> Result<()> {
    let size = root.len();
    if size < INFO_SIZE + NODE_HEADER {
        return Err(Error::Format("B-tree block too small"));
    }
    if u16le(&root, OBJ_HEADER)? & FLAG_ROOT == 0 {
        return Err(Error::Format("B-tree root node missing root flag"));
    }
    let info = size - INFO_SIZE;
    let key_size = u32le(&root, info + 8)? as usize;
    let val_size = u32le(&root, info + 12)? as usize;
    let node = Node::parse(root, key_size, val_size)?;
    walk_node(&node, 0, read_child, key_size, val_size, cmp, visit)?;
    Ok(())
}

fn walk_node(
    node: &Node,
    depth: usize,
    read_child: &dyn Fn(u64) -> Result<Arc<Vec<u8>>>,
    ksz: usize,
    vsz: usize,
    cmp: &dyn Fn(&[u8]) -> Ordering,
    visit: &mut dyn FnMut(&[u8], &[u8]) -> Result<bool>,
) -> Result<bool> {
    if depth > MAX_DEPTH {
        return Err(Error::Format("B-tree too deep"));
    }
    if node.is_leaf() {
        for i in 0..node.nkeys {
            let (k, v) = node.entry(i)?;
            match cmp(k) {
                Ordering::Less => {}
                Ordering::Greater => return Ok(false),
                Ordering::Equal => {
                    if !visit(k, v)? {
                        return Ok(false);
                    }
                }
            }
        }
        return Ok(true);
    }
    for i in 0..node.nkeys {
        let (k, v) = node.entry(i)?;
        if cmp(k) == Ordering::Greater {
            return Ok(false);
        }
        // Child i spans [key_i, key_{i+1}); skip it if that all lies before the range.
        if i + 1 < node.nkeys {
            let (k2, _) = node.entry(i + 1)?;
            if cmp(k2) == Ordering::Less {
                continue;
            }
        }
        let child = Node::parse(read_child(u64le(v, 0)?)?, ksz, vsz)?;
        if !walk_node(&child, depth + 1, read_child, ksz, vsz, cmp, visit)? {
            return Ok(false);
        }
    }
    Ok(true)
}

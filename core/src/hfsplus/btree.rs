//! Generic HFS+ B-tree node access.

use super::fork::{read_extents, Extent};
use crate::device::BlockSource;
use crate::util::{u16be, u32be};
use crate::{Error, Result};

pub const KIND_LEAF: i8 = -1;
pub const KIND_INDEX: i8 = 0;

pub struct Node {
    data: Vec<u8>,
    offs: Vec<usize>,
}

impl Node {
    fn parse(data: Vec<u8>) -> Result<Self> {
        let size = data.len();
        let n = u16be(&data, 10)? as usize;
        let table = 2 * (n + 1);
        if 14 + table > size {
            return Err(Error::Format("B-tree node record table overflow"));
        }
        let mut offs = Vec::with_capacity(n + 1);
        for i in 0..=n {
            offs.push(u16be(&data, size - 2 * (i + 1))? as usize);
        }
        let mut prev = 14;
        for &o in &offs {
            if o < prev || o > size - table {
                return Err(Error::Format("bad B-tree record offset"));
            }
            prev = o;
        }
        Ok(Self { data, offs })
    }

    pub fn kind(&self) -> i8 {
        self.data[8] as i8
    }
    pub fn flink(&self) -> u32 {
        u32be(&self.data, 0).unwrap_or(0)
    }
    pub fn num_records(&self) -> usize {
        self.offs.len() - 1
    }
    pub fn record(&self, i: usize) -> &[u8] {
        &self.data[self.offs[i]..self.offs[i + 1]]
    }
}

/// Length of a key including its u16 length field, padded to even.
/// The record payload starts at this offset.
pub fn key_span(rec: &[u8]) -> Result<usize> {
    let klen = u16be(rec, 0)? as usize;
    let span = (2 + klen + 1) & !1;
    if span > rec.len() {
        return Err(Error::Format("B-tree key exceeds record"));
    }
    Ok(span)
}

pub struct BTree<'a> {
    src: &'a dyn BlockSource,
    block_size: u64,
    extents: Vec<Extent>,
    logical_size: u64,
    pub node_size: usize,
    pub root: u32,
    pub depth: u16,
    pub total_nodes: u32,
}

impl<'a> BTree<'a> {
    pub fn open(
        src: &'a dyn BlockSource,
        block_size: u64,
        extents: Vec<Extent>,
        logical_size: u64,
    ) -> Result<Self> {
        let mut head = [0u8; 64];
        let got = read_extents(src, block_size, &extents, logical_size, 0, &mut head)?;
        if got < head.len() {
            return Err(Error::Format("B-tree file too small"));
        }
        let node_size = u16be(&head, 32)? as usize;
        if !node_size.is_power_of_two() || !(512..=32768).contains(&node_size) {
            return Err(Error::Format("bad B-tree node size"));
        }
        Ok(Self {
            src,
            block_size,
            extents,
            logical_size,
            node_size,
            depth: u16be(&head, 14)?,
            root: u32be(&head, 16)?,
            total_nodes: u32be(&head, 36)?,
        })
    }

    pub fn node(&self, n: u32) -> Result<Node> {
        if n >= self.total_nodes {
            return Err(Error::Format("B-tree node number out of range"));
        }
        let mut buf = vec![0u8; self.node_size];
        let off = n as u64 * self.node_size as u64;
        let got =
            read_extents(self.src, self.block_size, &self.extents, self.logical_size, off, &mut buf)?;
        if got < buf.len() {
            return Err(Error::Format("short B-tree node read"));
        }
        Node::parse(buf)
    }

    /// Descend to a leaf. `pick` is given each index record and returns true
    /// if that record's key is <= the search key. The last such child is
    /// followed (or the first, if none qualify).
    pub fn descend(&self, mut pick: impl FnMut(&[u8]) -> Result<bool>) -> Result<Node> {
        if self.root == 0 {
            return Err(Error::Format("empty B-tree"));
        }
        let mut n = self.root;
        for _ in 0..(self.depth as usize + 2) {
            let node = self.node(n)?;
            match node.kind() {
                KIND_LEAF => return Ok(node),
                KIND_INDEX => {
                    if node.num_records() == 0 {
                        return Err(Error::Format("empty index node"));
                    }
                    let mut chosen = 0;
                    for i in 0..node.num_records() {
                        if pick(node.record(i))? {
                            chosen = i;
                        } else if i > 0 {
                            break;
                        }
                    }
                    let rec = node.record(chosen);
                    n = u32be(rec, key_span(rec)?)?;
                }
                _ => return Err(Error::Format("unexpected B-tree node kind")),
            }
        }
        Err(Error::Format("B-tree deeper than header claims"))
    }
}

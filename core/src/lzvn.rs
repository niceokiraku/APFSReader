// SPDX-License-Identifier: BSD-3-Clause
// Copyright (c) 2015-2016, Apple Inc. All rights reserved.
//
// LZVN decoder, a Rust port of the decoder in Apple's reference LZFSE
// implementation (lzvn_decode_base.c), via its Go port in go-apfs-v2. The
// BSD-3-Clause licence text is in THIRD_PARTY_LICENSES.md at the repository
// root; redistributions must retain it.

//! Raw LZVN streams, as stored in decmpfs chunks (no LZFSE container header).

use crate::{Error, Result};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    SmallD,  // LLMMMDDD DDDDDDDD literal: small distance
    MediumD, // 101LLMMM DDDDDDMM DDDDDDDD literal: medium distance
    LargeD,  // LLMMM111 DDDDDDDD DDDDDDDD literal: large distance
    PrevD,   // LLMMM110 literal: previous distance
    SmallM,  // 1111MMMM: match at previous distance
    LargeM,  // 11110000 MMMMMMMM: match at previous distance
    SmallL,  // 1110LLLL literal
    LargeL,  // 11100000 LLLLLLLL literal
    Nop,     // 00001110, 00010110
    Eos,     // 00000110 + 7 bytes
    Undefined,
}

fn classify(op: u8) -> Op {
    match op {
        6 => Op::Eos,
        14 | 22 => Op::Nop,
        0xa0..=0xbf => Op::MediumD,
        0xe0 => Op::LargeL,
        0xe1..=0xef => Op::SmallL,
        0xf0 => Op::LargeM,
        0xf1..=0xff => Op::SmallM,
        0x70..=0x7f | 0xd0..=0xdf => Op::Undefined,
        _ if op & 7 == 7 => Op::LargeD,
        // 30, 38, ..., 62 are undefined; 70 and up are previous-distance.
        _ if op & 7 == 6 => {
            if op < 64 {
                Op::Undefined
            } else {
                Op::PrevD
            }
        }
        _ => Op::SmallD,
    }
}

/// Decode one LZVN stream. `expected` is the exact decoded size; anything
/// else (truncated input, bad opcode, bad distance, wrong length) is an error.
pub fn decode(src: &[u8], expected: usize) -> Result<Vec<u8>> {
    const BAD: Error = Error::Format("corrupt LZVN stream");
    let mut dst = vec![0u8; expected];
    let (mut i, mut out, mut d) = (0usize, 0usize, 0usize);
    let mut finished = false;

    while i < src.len() {
        let opc = src[i];
        let left = src.len() - i;
        let (l, m, opc_len);
        match classify(opc) {
            Op::SmallD => {
                opc_len = 2;
                l = (opc >> 6) as usize;
                m = ((opc >> 3) & 7) as usize + 3;
                if left <= opc_len + l {
                    return Err(BAD);
                }
                d = ((opc & 7) as usize) << 8 | src[i + 1] as usize;
            }
            Op::MediumD => {
                opc_len = 3;
                l = ((opc >> 3) & 3) as usize;
                if left <= opc_len + l {
                    return Err(BAD);
                }
                let opc23 = u16::from_le_bytes([src[i + 1], src[i + 2]]) as usize;
                m = (((opc & 7) as usize) << 2 | (opc23 & 3)) + 3;
                d = opc23 >> 2;
            }
            Op::LargeD => {
                opc_len = 3;
                l = (opc >> 6) as usize;
                m = ((opc >> 3) & 7) as usize + 3;
                if left <= opc_len + l {
                    return Err(BAD);
                }
                d = u16::from_le_bytes([src[i + 1], src[i + 2]]) as usize;
            }
            Op::PrevD => {
                opc_len = 1;
                l = (opc >> 6) as usize;
                m = ((opc >> 3) & 7) as usize + 3;
                if left <= opc_len + l {
                    return Err(BAD);
                }
            }
            Op::SmallM => {
                opc_len = 1;
                l = 0;
                m = (opc & 0xf) as usize;
                if left <= opc_len {
                    return Err(BAD);
                }
            }
            Op::LargeM => {
                opc_len = 2;
                l = 0;
                if left <= opc_len {
                    return Err(BAD);
                }
                m = src[i + 1] as usize + 16;
            }
            Op::SmallL => {
                opc_len = 1;
                m = 0;
                l = (opc & 0xf) as usize;
                if left <= opc_len + l {
                    return Err(BAD);
                }
            }
            Op::LargeL => {
                opc_len = 2;
                m = 0;
                if left <= 2 {
                    return Err(BAD);
                }
                l = src[i + 1] as usize + 16;
                if left <= opc_len + l {
                    return Err(BAD);
                }
            }
            Op::Nop => {
                if left <= 1 {
                    return Err(BAD);
                }
                i += 1;
                continue;
            }
            Op::Eos => {
                if left < 8 {
                    return Err(BAD);
                }
                finished = true;
                break;
            }
            Op::Undefined => return Err(BAD),
        }

        if l > expected - out {
            return Err(BAD);
        }
        let lit = i + opc_len;
        dst[out..out + l].copy_from_slice(&src[lit..lit + l]);
        if m == 0 {
            i = lit + l;
            out += l;
            continue;
        }
        let pos = out + l;
        // A match may not reach before the start of the output, nor have a
        // zero distance.
        if d > pos || d == 0 || m > expected - pos {
            return Err(BAD);
        }
        for k in 0..m {
            dst[pos + k] = dst[pos + k - d];
        }
        i = lit + l;
        out = pos + m;
    }

    if out != expected || (!finished && i < src.len()) {
        return Err(BAD);
    }
    Ok(dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_then_end_of_stream() {
        // SmallL with 3 literals ("abc"), then the 8-byte end-of-stream marker.
        let mut s = vec![0xE3, b'a', b'b', b'c'];
        s.extend_from_slice(&[0x06, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(decode(&s, 3).unwrap(), b"abc");
    }

    #[test]
    fn match_copies_earlier_output() {
        // Literal "ab", then a SmallM-style match needs a previous distance; use
        // SmallD: L=2 (bits 11), M=3 (000+3), D=2 -> output "ababab"? D=2, M=3.
        // opcode = LL=01 (1 literal)... build explicitly: 1 literal 'x', then
        // match 3 bytes at distance 1 -> "xxxx".
        // SmallD: opc = (L<<6) | ((M-3)<<3) | (D>>8), next byte = D&0xff.
        let mut s = vec![(1 << 6) | 0, 1, b'x'];
        s.extend_from_slice(&[0x06, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(decode(&s, 4).unwrap(), b"xxxx");
    }

    #[test]
    fn rejects_truncated_and_wrong_length() {
        assert!(decode(&[0xE3, b'a'], 3).is_err());
        let mut s = vec![0xE1, b'a'];
        s.extend_from_slice(&[0x06, 0, 0, 0, 0, 0, 0, 0]);
        assert!(decode(&s, 2).is_err());
        assert!(decode(&[0x70], 1).is_err());
    }
}

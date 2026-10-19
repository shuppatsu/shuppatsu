//! The game's byte-oriented LZ codec (docs/re/loaders.md §4).
//!
//! Token grammar, shared by model parts and `cg.rom` tiles:
//! - `b < 0x80`: back-reference of `b + 1` items from `pos - (next_byte + 1)`. Overlap is allowed,
//!   so a distance of 1 acts as run-length encoding.
//! - `b >= 0x80`: `(b & 0x7f) + 1` literal items follow.
//!
//! Model parts wrap it in a multi-pass block (`lz_unpack_multipass` @0x425af0): each pass is
//! `i32 out_len` + tokens, and the output of one pass is the input of the next.

use crate::error::{FormatError, Result};
use crate::reader::Bytes;

/// Marker byte at offset 4 of every packed part block (constant in all 13087 parts on the disc).
pub const PART_BLOCK_MARKER: u8 = 0x73;

/// Decodes one token stream from `input`, starting at `pos`, until `out_len` bytes are produced.
/// Returns the output and the input position after the last token consumed.
pub fn decode_tokens(input: &[u8], mut pos: usize, out_len: usize) -> Result<(Vec<u8>, usize)> {
    let mut out = Vec::with_capacity(out_len);
    let byte = |p: usize| -> Result<u8> {
        input.get(p).copied().ok_or(FormatError::Truncated {
            what: "LZ stream",
            offset: p,
            need: 1,
            len: input.len(),
        })
    };
    while out.len() < out_len {
        let token = byte(pos)?;
        pos += 1;
        if token < 0x80 {
            let len = token as usize + 1;
            let dist = byte(pos)? as usize + 1;
            pos += 1;
            if dist > out.len() {
                return Err(FormatError::invalid(
                    "LZ stream",
                    format!("back-reference distance {dist} at output {}", out.len()),
                ));
            }
            for _ in 0..len {
                out.push(out[out.len() - dist]);
            }
        } else {
            let len = (token & 0x7f) as usize + 1;
            let lit = input.get(pos..pos + len).ok_or(FormatError::Truncated {
                what: "LZ literal run",
                offset: pos,
                need: len,
                len: input.len(),
            })?;
            out.extend_from_slice(lit);
            pos += len;
        }
    }
    // The last token may overshoot; the original writes exactly `out_len` bytes.
    out.truncate(out_len);
    Ok((out, pos))
}

/// One pass: `i32 out_len` followed by a token stream.
pub fn decode_pass(input: &[u8]) -> Result<Vec<u8>> {
    let out_len = Bytes::new(input, "LZ pass").i32(0)?;
    let out_len = usize::try_from(out_len)
        .map_err(|_| FormatError::invalid("LZ pass", format!("negative length {out_len}")))?;
    Ok(decode_tokens(input, 4, out_len)?.0)
}

/// Unpacks a part block: u32 unpacked size, u8 0x73, u8 passes, u16 0, then data at +8.
/// With 0 passes the data is stored raw.
pub fn unpack_block(block: &[u8]) -> Result<Vec<u8>> {
    let b = Bytes::new(block, "part block");
    let size = b.usize32(0)?;
    let marker = b.u8(4)?;
    if marker != PART_BLOCK_MARKER {
        return Err(FormatError::BadMagic {
            what: "part block",
            found: vec![marker],
        });
    }
    let passes = b.u8(5)?;
    let body = b.slice(8, block.len().saturating_sub(8))?;
    if passes == 0 {
        return Ok(b.slice(8, size)?.to_vec());
    }
    let mut data = decode_pass(body)?;
    for _ in 1..passes {
        data = decode_pass(&data)?;
    }
    if data.len() != size {
        return Err(FormatError::invalid(
            "part block",
            format!("unpacked {} bytes, header says {size}", data.len()),
        ));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_and_overlapping_copy() {
        // literal "ab", then copy 5 from distance 2 -> "abababa"
        let stream = [0x81, b'a', b'b', 0x04, 0x01];
        let (out, end) = decode_tokens(&stream, 0, 7).unwrap();
        assert_eq!(out, b"abababa");
        assert_eq!(end, stream.len());
    }

    #[test]
    fn rejects_bad_distance() {
        assert!(decode_tokens(&[0x00, 0x00], 0, 1).is_err());
    }

    #[test]
    fn block_two_passes() {
        // Inner pass produces "xxxx"; outer pass produces the inner pass bytes literally.
        let mut inner = 4i32.to_le_bytes().to_vec();
        inner.extend_from_slice(&[0x80, b'x', 0x02, 0x00]); // 'x' then copy 3 at dist 1
        let mut outer = (inner.len() as i32).to_le_bytes().to_vec();
        outer.push(0x80 | (inner.len() as u8 - 1));
        outer.extend_from_slice(&inner);
        let mut block = 4u32.to_le_bytes().to_vec();
        block.extend_from_slice(&[PART_BLOCK_MARKER, 2, 0, 0]);
        block.extend_from_slice(&outer);
        assert_eq!(unpack_block(&block).unwrap(), b"xxxx");
    }

    #[test]
    fn block_raw() {
        let mut block = 3u32.to_le_bytes().to_vec();
        block.extend_from_slice(&[PART_BLOCK_MARKER, 0, 0, 0, 1, 2, 3, 0]);
        assert_eq!(unpack_block(&block).unwrap(), [1, 2, 3]);
    }
}

//! 2D graphics: `cg.rom` tiles, `tbl.rom` tilemaps and the palettes kept in the exe
//! (docs/re/loaders.md §4b and §9, renderer.md §11).
//!
//! - `cg.rom`: u32 count, count × u32 offset, then tiles. A tile is one type byte and a stream
//!   giving 16×16 palette indices, row-major; index 0 is transparent.
//!   Type 1 is RLE, type 2 the game's LZ grammar applied to pixels.
//! - `tbl.rom`: a `ROM2` of tilemaps, each `u8 w, u8 h, w·h × u16 tile id` (0 = empty).
//! - Palettes: 256 × u32 colours each, copied from blobs in the exe into a 128-entry table
//!   (`0x436720` / `0x436770`). Colour byte order: B, R, G.

use crate::error::{FormatError, Result};
use crate::lz;
use crate::reader::Bytes;
use crate::rom2::Rom2;

pub const TILE_SIZE: usize = 16;
pub const TILE_PIXELS: usize = TILE_SIZE * TILE_SIZE;

/// `cg.rom`: offset table over compressed tiles.
#[derive(Debug, Clone)]
pub struct CgFile<'a> {
    data: &'a [u8],
    offsets: Vec<usize>,
}

impl<'a> CgFile<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let b = Bytes::new(data, "cg.rom");
        let count = b.usize32(0)?;
        if count > data.len() / 4 {
            return Err(FormatError::invalid(
                "cg.rom",
                format!("count {count} too large"),
            ));
        }
        let offsets = (0..count)
            .map(|i| b.usize32(4 + i * 4))
            .collect::<Result<Vec<_>>>()?;
        if let Some(&bad) = offsets.iter().find(|&&o| o >= data.len()) {
            return Err(FormatError::invalid(
                "cg.rom",
                format!("tile offset {bad:#x} past EOF"),
            ));
        }
        Ok(Self { data, offsets })
    }

    pub fn len(&self) -> usize {
        self.offsets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.offsets.is_empty()
    }

    /// Decodes tile `index` into 256 palette indices.
    pub fn tile(&self, index: usize) -> Result<[u8; TILE_PIXELS]> {
        let start = *self
            .offsets
            .get(index)
            .ok_or_else(|| FormatError::invalid("cg.rom", format!("no tile {index}")))?;
        decode_tile(&self.data[start..])
    }
}

/// Decodes one tile stream (type byte + data).
pub fn decode_tile(stream: &[u8]) -> Result<[u8; TILE_PIXELS]> {
    let kind = *stream.first().ok_or(FormatError::Truncated {
        what: "cg tile",
        offset: 0,
        need: 1,
        len: 0,
    })?;
    let pixels = match kind {
        1 => decode_rle(stream, 1)?,
        2 => lz::decode_tokens(stream, 1, TILE_PIXELS)?.0,
        _ => {
            return Err(FormatError::invalid(
                "cg tile",
                format!("unknown type {kind}"),
            ));
        }
    };
    let mut out = [0u8; TILE_PIXELS];
    out.copy_from_slice(&pixels);
    Ok(out)
}

/// Type 1: `b < 0x80` → run of `b + 1` copies of the next byte; `b >= 0x80` → `(b & 0x7f) + 1`
/// literal bytes.
fn decode_rle(stream: &[u8], mut pos: usize) -> Result<Vec<u8>> {
    let b = Bytes::new(stream, "cg RLE tile");
    let mut out = Vec::with_capacity(TILE_PIXELS);
    while out.len() < TILE_PIXELS {
        let token = b.u8(pos)?;
        pos += 1;
        if token < 0x80 {
            let value = b.u8(pos)?;
            pos += 1;
            out.extend(std::iter::repeat_n(value, token as usize + 1));
        } else {
            let len = (token & 0x7f) as usize + 1;
            out.extend_from_slice(b.slice(pos, len)?);
            pos += len;
        }
    }
    out.truncate(TILE_PIXELS);
    Ok(out)
}

/// A `tbl.rom` tilemap: `width × height` tiles of 16×16 pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tilemap {
    pub width: u8,
    pub height: u8,
    /// Row-major cg tile ids; 0 = empty cell.
    pub tiles: Vec<u16>,
}

impl Tilemap {
    pub fn parse(entry: &[u8]) -> Result<Self> {
        let b = Bytes::new(entry, "tbl.rom tilemap");
        let width = b.u8(0)?;
        let height = b.u8(1)?;
        let n = width as usize * height as usize;
        let raw = b.slice(2, n * 2)?;
        if entry.len() != 2 + n * 2 {
            return Err(FormatError::invalid(
                "tbl.rom tilemap",
                format!("size {} for {width}x{height}", entry.len()),
            ));
        }
        let tiles = raw
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        Ok(Self {
            width,
            height,
            tiles,
        })
    }

    /// Renders to palette indices (`width·16 × height·16`, row-major).
    pub fn render_indices(&self, cg: &CgFile<'_>) -> Result<Vec<u8>> {
        let w = self.width as usize * TILE_SIZE;
        let mut out = vec![0u8; w * self.height as usize * TILE_SIZE];
        for (i, &id) in self.tiles.iter().enumerate() {
            if id == 0 {
                continue;
            }
            let tile = cg.tile(id as usize)?;
            let (tx, ty) = (i % self.width as usize, i / self.width as usize);
            for row in 0..TILE_SIZE {
                let dst = (ty * TILE_SIZE + row) * w + tx * TILE_SIZE;
                out[dst..dst + TILE_SIZE]
                    .copy_from_slice(&tile[row * TILE_SIZE..(row + 1) * TILE_SIZE]);
            }
        }
        Ok(out)
    }
}

/// All tilemaps of `tbl.rom`.
pub fn parse_tbl(data: &[u8]) -> Result<Vec<Tilemap>> {
    Rom2::parse(data)?.iter().map(Tilemap::parse).collect()
}

/// A 256-colour palette as stored in the exe: u32 per colour, bytes B, R, G, (unused).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette(pub [u32; 256]);

impl Palette {
    pub const BYTES: usize = 256 * 4;

    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let b = Bytes::new(bytes, "palette");
        let mut colours = [0u32; 256];
        for (i, c) in colours.iter_mut().enumerate() {
            *c = b.u32(i * 4)?;
        }
        Ok(Self(colours))
    }

    /// RGBA8 for palette index `i`; index 0 is transparent (sprite colour key).
    pub fn rgba(&self, i: u8) -> [u8; 4] {
        if i == 0 {
            return [0, 0, 0, 0];
        }
        let [b, r, g, _] = self.0[i as usize].to_le_bytes();
        [r, g, b, 255]
    }

    pub fn indices_to_rgba8(&self, indices: &[u8]) -> Vec<u8> {
        indices.iter().flat_map(|&i| self.rgba(i)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rle_tile() {
        // 128 pixels of 5, then 128 of 0, as four 64-pixel runs (token 63 = run of 64).
        let mut s = vec![1];
        for v in [5u8, 5, 0, 0] {
            s.extend_from_slice(&[63, v]);
        }
        let t = decode_tile(&s).unwrap();
        assert!(t[..128].iter().all(|&p| p == 5));
        assert!(t[128..].iter().all(|&p| p == 0));
    }

    #[test]
    fn lz_tile() {
        // literal 1, then copy 255 at distance 1 (two tokens: 127+1 and 126+1)
        let s = [2, 0x80, 9, 0x7f, 0, 0x7e, 0];
        assert_eq!(decode_tile(&s).unwrap(), [9u8; 256]);
    }

    #[test]
    fn tilemap_size_check() {
        assert!(Tilemap::parse(&[1, 1, 3, 0]).is_ok());
        assert!(Tilemap::parse(&[1, 1, 3, 0, 0]).is_err());
    }

    #[test]
    fn palette_byte_order() {
        let mut bytes = vec![0u8; Palette::BYTES];
        bytes[4..8].copy_from_slice(&[0x10, 0x20, 0x30, 0]); // index 1: B=0x10 R=0x20 G=0x30
        let p = Palette::parse(&bytes).unwrap();
        assert_eq!(p.rgba(1), [0x20, 0x30, 0x10, 255]);
        assert_eq!(p.rgba(0), [0, 0, 0, 0]);
    }
}

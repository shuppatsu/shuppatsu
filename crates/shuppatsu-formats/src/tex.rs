//! `tex*.rom`: texture pages plus texture-rectangle records (docs/re/loaders.md §3).
//!
//! ```text
//! u16 rect_count, u16 page_count
//! rect_count × { u16 rect_id, u16 page, u8 u, u8 v, u8 w_minus_1, u8 h_minus_1 }
//! page_count × { u16 width, u16 height, width·height × u16 RGB565 }
//! ```
//! No magic; the walk ending exactly at EOF is the format check (loader @0x432200).
//! Pixels are uploaded unconverted as RGB565, and colour 0x0000 is the transparent colour key
//! (renderer.md §9).
//!
//! Orientation: pages are stored **bottom-up** (the first row is the bottom of the picture). The
//! loader copies row *i* to surface row *i* unchanged (0x432200), and model UVs address that
//! layout, so a renderer must keep the stored row order. Flip only for human viewing
//! ([`TexPage::to_rgba8_upright`]).

use crate::error::{FormatError, Result};
use crate::reader::Bytes;

/// A texture rectangle: where a model group's texture sits inside its page. Model parts
/// reference these by `id` (global; a later file overrides an earlier one with the same id).
///
/// All four placement fields are in units of 1/256 of the page, whatever the page's pixel size
/// (16..512 px), because D3D texture coordinates are normalised. `w` and `h` are stored as
/// size − 1, so `w = 255` spans the whole page width (docs/re/models.md).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TexRect {
    pub id: u16,
    /// Page index within the file it came from (the game adds a per-file page base).
    pub page: u16,
    pub u: u8,
    pub v: u8,
    pub w: u8,
    pub h: u8,
}

impl TexRect {
    /// Maps a stored rect-relative UV (1/256 units) to a normalised page UV (1.0 = page size).
    ///
    /// Reproduces the game exactly (exe 0x432a41..0x432a90):
    /// `u' = ((w + 1) * u/256 + rect.u) / 256` but `v' = (h * v/256 + rect.v) / 256`;
    /// the V axis uses `h`, not `h + 1`. Results outside 0..1 are intended to wrap
    /// (D3D's default WRAP addressing; tiled textures rely on it).
    pub fn map_uv(&self, uv: [i16; 2]) -> [f32; 2] {
        const S: f32 = 1.0 / 256.0;
        let lu = f32::from(uv[0]) * S;
        let lv = f32::from(uv[1]) * S;
        [
            ((f32::from(self.w) + 1.0) * lu + f32::from(self.u)) * S,
            (f32::from(self.h) * lv + f32::from(self.v)) * S,
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TexPage {
    pub width: u16,
    pub height: u16,
    /// Row-major RGB565 texels.
    pub pixels: Vec<u16>,
}

impl TexPage {
    /// RGBA8 conversion: 5/6-bit channels expanded by bit replication, `0x0000` fully
    /// transparent (the colour key), everything else opaque.
    pub fn to_rgba8(&self) -> Vec<u8> {
        self.pixels
            .iter()
            .flat_map(|&p| rgb565_to_rgba8(p))
            .collect()
    }

    /// Like [`Self::to_rgba8`] but with the rows reversed, so the picture is upright.
    pub fn to_rgba8_upright(&self) -> Vec<u8> {
        self.pixels
            .chunks(usize::from(self.width).max(1))
            .rev()
            .flatten()
            .flat_map(|&p| rgb565_to_rgba8(p))
            .collect()
    }
}

pub fn rgb565_to_rgba8(p: u16) -> [u8; 4] {
    if p == 0 {
        return [0, 0, 0, 0];
    }
    let r = ((p >> 11) & 0x1f) as u8;
    let g = ((p >> 5) & 0x3f) as u8;
    let b = (p & 0x1f) as u8;
    [
        (r << 3) | (r >> 2),
        (g << 2) | (g >> 4),
        (b << 3) | (b >> 2),
        255,
    ]
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TexFile {
    pub rects: Vec<TexRect>,
    pub pages: Vec<TexPage>,
}

impl TexFile {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let b = Bytes::new(data, "tex.rom");
        let rect_count = b.u16(0)? as usize;
        let page_count = b.u16(2)? as usize;
        let mut pos = 4;
        let mut rects = Vec::with_capacity(rect_count);
        for _ in 0..rect_count {
            let r = b.slice(pos, 8)?;
            let rect = TexRect {
                id: u16::from_le_bytes([r[0], r[1]]),
                page: u16::from_le_bytes([r[2], r[3]]),
                u: r[4],
                v: r[5],
                w: r[6],
                h: r[7],
            };
            if rect.page as usize >= page_count {
                return Err(FormatError::invalid(
                    "tex.rom",
                    format!(
                        "rect {} references page {} of {page_count}",
                        rect.id, rect.page
                    ),
                ));
            }
            rects.push(rect);
            pos += 8;
        }
        let mut pages = Vec::with_capacity(page_count);
        for _ in 0..page_count {
            let width = b.u16(pos)?;
            let height = b.u16(pos + 2)?;
            let n = width as usize * height as usize;
            let raw = b.slice(pos + 4, n * 2)?;
            let pixels = raw
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&c| u16::from_le_bytes(c))
                .collect();
            pages.push(TexPage {
                width,
                height,
                pixels,
            });
            pos += 4 + n * 2;
        }
        if pos != data.len() {
            return Err(FormatError::invalid(
                "tex.rom",
                format!("{} trailing bytes after the last page", data.len() - pos),
            ));
        }
        Ok(Self { rects, pages })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_file() {
        let mut f = vec![1, 0, 1, 0]; // 1 rect, 1 page
        f.extend_from_slice(&[7, 0, 0, 0, 1, 0, 0, 1]); // id 7, page 0, u1 v0, w0 h1
        f.extend_from_slice(&[1, 0, 2, 0]); // 1x2 page
        f.extend_from_slice(&[0xff, 0xff, 0, 0]);
        let t = TexFile::parse(&f).unwrap();
        assert_eq!(
            t.rects[0],
            TexRect {
                id: 7,
                page: 0,
                u: 1,
                v: 0,
                w: 0,
                h: 1
            }
        );
        assert_eq!(t.pages[0].pixels, [0xffff, 0]);
        assert_eq!(t.pages[0].to_rgba8(), [255, 255, 255, 255, 0, 0, 0, 0]);
        f.push(0);
        assert!(TexFile::parse(&f).is_err());
    }

    #[test]
    fn rgb565_expansion() {
        assert_eq!(rgb565_to_rgba8(0xf800), [255, 0, 0, 255]);
        assert_eq!(rgb565_to_rgba8(0x07e0), [0, 255, 0, 255]);
        assert_eq!(rgb565_to_rgba8(0x0001), [0, 0, 8, 255]);
    }
}

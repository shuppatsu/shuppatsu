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

use crate::error::{FormatError, Result};
use crate::reader::Bytes;

/// A placed rectangle inside a texture page. Model parts reference these by `id`
/// (a global id; a later file overrides an earlier one with the same id).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TexRect {
    pub id: u16,
    /// Page index within this file.
    pub page: u16,
    pub u: u8,
    pub v: u8,
    /// Width in texels (stored as width − 1).
    pub width: u16,
    /// Height in texels (stored as height − 1).
    pub height: u16,
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
                width: r[6] as u16 + 1,
                height: r[7] as u16 + 1,
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
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
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
        f.extend_from_slice(&[7, 0, 0, 0, 1, 0, 0, 1]); // id 7, page 0, u1 v0, 1x2
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
                width: 1,
                height: 2
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

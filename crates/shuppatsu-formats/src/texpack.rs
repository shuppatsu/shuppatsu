//! Texture packs: replacement images for the 3D texture pages (`tex*.rom`), the port's own
//! format for the `texture_pack` enhancement (docs/texture-packs.md). Not an original format.
//!
//! A pack is a directory with one image per page it replaces:
//!
//! ```text
//! <pack>/<tex file stem, lowercase>/page<NN>.qoi     e.g. texr1/page03.qoi, textr2/page00.qoi
//! ```
//!
//! * The image is [QOI](https://qoiformat.org/qoi-specification.pdf) with 4 channels,
//!   non-premultiplied RGBA. Alpha is the transparency (the original's colour key, RGB565
//!   `0x0000`, is not used: black with alpha 255 is opaque black). Transparent texels may keep
//!   a colour, which smooth filtering then blends towards instead of black.
//! * Rows are in the **stored order** of `tex*.rom` (the first row is the bottom of the
//!   picture), the order the game uploads and its UVs address ([`crate::tex`]). A pack image
//!   therefore looks upside down in an image viewer.
//! * The size is the original page's times a whole factor (1, 2, 3, …), the same factor on
//!   both axes ([`scale_of`]). UVs are page-normalised, so a larger page needs no other change.
//! * A page without a file keeps the original texture.

use crate::error::{FormatError, Result};

const WHAT: &str = "qoi";

/// An RGBA8 image, rows top to bottom as stored (no flipping here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The pack-relative path of page `page` of the texture file `tex_name` (`texR1.rom` →
/// `texr1/page03.qoi`), with `/` as separator.
pub fn page_path(tex_name: &str, page: usize) -> String {
    let stem = tex_name
        .rsplit_once('.')
        .map_or(tex_name, |(s, _)| s)
        .to_ascii_lowercase();
    format!("{stem}/page{page:02}.qoi")
}

/// The whole factor that takes an `orig` page (width, height) to `size`, if there is one.
pub fn scale_of(orig: (u32, u32), size: (u32, u32)) -> Option<u32> {
    if orig.0 == 0 || orig.1 == 0 || !size.0.is_multiple_of(orig.0) {
        return None;
    }
    let s = size.0 / orig.0;
    (s >= 1 && size.1 == orig.1 * s).then_some(s)
}

const OP_INDEX: u8 = 0x00;
const OP_DIFF: u8 = 0x40;
const OP_LUMA: u8 = 0x80;
const OP_RUN: u8 = 0xc0;
const OP_RGB: u8 = 0xfe;
const OP_RGBA: u8 = 0xff;
const END: [u8; 8] = [0, 0, 0, 0, 0, 0, 0, 1];
/// Larger images are refused (a page is at most 512 px; ×16 is far beyond any pack).
const MAX_SIDE: u32 = 16384;

fn hash(p: [u8; 4]) -> usize {
    let [r, g, b, a] = p.map(usize::from);
    (r * 3 + g * 5 + b * 7 + a * 11) % 64
}

/// Encodes `img` as a 4-channel QOI file (colour space 0, sRGB with linear alpha).
pub fn encode_qoi(img: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::with_capacity(14 + img.rgba.len() / 2 + 8);
    out.extend_from_slice(b"qoif");
    out.extend_from_slice(&img.width.to_be_bytes());
    out.extend_from_slice(&img.height.to_be_bytes());
    out.extend_from_slice(&[4, 0]);
    let mut index = [[0u8; 4]; 64];
    let mut prev = [0u8, 0, 0, 255];
    let mut run = 0u8;
    let pixels = img.rgba.as_chunks::<4>().0;
    for (i, &px) in pixels.iter().enumerate() {
        if px == prev {
            run += 1;
            if run == 62 || i + 1 == pixels.len() {
                out.push(OP_RUN | (run - 1));
                run = 0;
            }
            continue;
        }
        if run > 0 {
            out.push(OP_RUN | (run - 1));
            run = 0;
        }
        let h = hash(px);
        if index[h] == px {
            out.push(OP_INDEX | h as u8);
        } else {
            index[h] = px;
            if px[3] == prev[3] {
                let dr = px[0].wrapping_sub(prev[0]) as i8;
                let dg = px[1].wrapping_sub(prev[1]) as i8;
                let db = px[2].wrapping_sub(prev[2]) as i8;
                let (dr_dg, db_dg) = (dr.wrapping_sub(dg), db.wrapping_sub(dg));
                if (-2..2).contains(&dr) && (-2..2).contains(&dg) && (-2..2).contains(&db) {
                    out.push(
                        OP_DIFF | ((dr + 2) as u8) << 4 | ((dg + 2) as u8) << 2 | (db + 2) as u8,
                    );
                } else if (-32..32).contains(&dg)
                    && (-8..8).contains(&dr_dg)
                    && (-8..8).contains(&db_dg)
                {
                    out.push(OP_LUMA | (dg + 32) as u8);
                    out.push(((dr_dg + 8) as u8) << 4 | (db_dg + 8) as u8);
                } else {
                    out.extend_from_slice(&[OP_RGB, px[0], px[1], px[2]]);
                }
            } else {
                out.extend_from_slice(&[OP_RGBA, px[0], px[1], px[2], px[3]]);
            }
        }
        prev = px;
    }
    out.extend_from_slice(&END);
    out
}

/// Decodes a QOI file (3 or 4 channels) to RGBA8.
pub fn decode_qoi(data: &[u8]) -> Result<RgbaImage> {
    if data.len() < 14 + END.len() {
        return Err(FormatError::Truncated {
            what: WHAT,
            offset: 0,
            need: 14 + END.len(),
            len: data.len(),
        });
    }
    if &data[..4] != b"qoif" {
        return Err(FormatError::BadMagic {
            what: WHAT,
            found: data[..4].to_vec(),
        });
    }
    let be = |o: usize| u32::from_be_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
    let (width, height) = (be(4), be(8));
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(FormatError::invalid(WHAT, format!("size {width}×{height}")));
    }
    if !matches!(data[12], 3 | 4) {
        return Err(FormatError::invalid(WHAT, format!("{} channels", data[12])));
    }
    let n = width as usize * height as usize;
    let mut rgba = Vec::with_capacity(n * 4);
    let mut index = [[0u8; 4]; 64];
    let mut px = [0u8, 0, 0, 255];
    let body = &data[14..data.len() - END.len()];
    let mut pos = 0;
    let truncated = |pos: usize| FormatError::Truncated {
        what: WHAT,
        offset: 14 + pos,
        need: 1,
        len: data.len(),
    };
    let byte = |pos: &mut usize| -> Result<u8> {
        let b = *body.get(*pos).ok_or_else(|| truncated(*pos))?;
        *pos += 1;
        Ok(b)
    };
    while rgba.len() < n * 4 {
        let op = byte(&mut pos)?;
        let mut run = 1;
        match op {
            OP_RGB => {
                for c in px.iter_mut().take(3) {
                    *c = byte(&mut pos)?;
                }
            }
            OP_RGBA => {
                for c in &mut px {
                    *c = byte(&mut pos)?;
                }
            }
            _ => match op & 0xc0 {
                OP_INDEX => px = index[usize::from(op & 0x3f)],
                OP_DIFF => {
                    px[0] = px[0].wrapping_add((op >> 4) & 3).wrapping_sub(2);
                    px[1] = px[1].wrapping_add((op >> 2) & 3).wrapping_sub(2);
                    px[2] = px[2].wrapping_add(op & 3).wrapping_sub(2);
                }
                OP_LUMA => {
                    let b2 = byte(&mut pos)?;
                    let dg = (op & 0x3f).wrapping_sub(32);
                    px[0] = px[0].wrapping_add(dg.wrapping_add(b2 >> 4).wrapping_sub(8));
                    px[1] = px[1].wrapping_add(dg);
                    px[2] = px[2].wrapping_add(dg.wrapping_add(b2 & 0x0f).wrapping_sub(8));
                }
                _ => run = usize::from(op & 0x3f) + 1,
            },
        }
        index[hash(px)] = px;
        for _ in 0..run.min(n - rgba.len() / 4) {
            rgba.extend_from_slice(&px);
        }
    }
    Ok(RgbaImage {
        width,
        height,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic page with every QOI op in it: runs, repeats (index), small and luma
    /// differences, big jumps and alpha changes.
    fn synthetic(w: u32, h: u32) -> RgbaImage {
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let px = match (x / 4 + y) % 5 {
                    0 => [0, 0, 0, 0],
                    1 => [x as u8, y as u8, 7, 255],
                    2 => [(x * 37) as u8, (y * 91) as u8, (x * y) as u8, 255],
                    3 => [200, 10 + (x % 3) as u8, 40, 255],
                    _ => [12, 34, 56, (x * 8) as u8],
                };
                rgba.extend_from_slice(&px);
            }
        }
        RgbaImage {
            width: w,
            height: h,
            rgba,
        }
    }

    #[test]
    fn qoi_round_trip() {
        for (w, h) in [(1, 1), (16, 16), (64, 32), (200, 3)] {
            let img = synthetic(w, h);
            let enc = encode_qoi(&img);
            assert_eq!(&enc[..4], b"qoif");
            assert!(enc.ends_with(&END));
            assert_eq!(decode_qoi(&enc).unwrap(), img, "{w}×{h}");
        }
        // A long run crosses the 62-pixel limit.
        let flat = RgbaImage {
            width: 100,
            height: 2,
            rgba: [9, 9, 9, 255].repeat(200),
        };
        let enc = encode_qoi(&flat);
        assert!(enc.len() < 14 + 8 + 10);
        assert_eq!(decode_qoi(&enc).unwrap(), flat);
    }

    #[test]
    fn qoi_rejects_bad_input() {
        let enc = encode_qoi(&synthetic(8, 8));
        assert!(matches!(
            decode_qoi(&enc[..30]),
            Err(FormatError::Truncated { .. })
        ));
        let mut bad = enc.clone();
        bad[0] = b'x';
        assert!(matches!(
            decode_qoi(&bad),
            Err(FormatError::BadMagic { .. })
        ));
        let mut zero = enc;
        zero[4..8].copy_from_slice(&0u32.to_be_bytes());
        assert!(decode_qoi(&zero).is_err());
    }

    #[test]
    fn paths_and_scales() {
        assert_eq!(page_path("texR1.rom", 3), "texr1/page03.qoi");
        assert_eq!(page_path("TEXTR2.ROM", 12), "textr2/page12.qoi");
        assert_eq!(scale_of((64, 64), (256, 256)), Some(4));
        assert_eq!(scale_of((256, 128), (512, 256)), Some(2));
        assert_eq!(scale_of((64, 64), (64, 64)), Some(1));
        assert_eq!(scale_of((64, 64), (256, 128)), None, "aspect changed");
        assert_eq!(scale_of((64, 64), (96, 96)), None, "not a whole factor");
        assert_eq!(scale_of((64, 64), (32, 32)), None, "smaller");
    }
}

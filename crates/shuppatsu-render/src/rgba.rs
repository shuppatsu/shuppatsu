//! RGBA8 images of the port's own art (the logo in `assets/logo/`), stored run-length packed so
//! they can be embedded in the binary without an image decoder.
//!
//! Packed layout (all little-endian): the magic `SHPK`, width and height as `u16`, then packets
//! over the pixels in row order (runs may cross rows). A control byte `c < 0x80` is followed by
//! `c + 1` literal pixels; `c ≥ 0x80` by one pixel repeated `c − 0x7e` (2..=129) times. Pixels
//! are 4 bytes R, G, B, A, not premultiplied. `scripts/build-logo.sh` writes them through
//! `shuppatsu-tools pack-rgba`.

/// The first four bytes of a packed image.
pub const MAGIC: [u8; 4] = *b"SHPK";
const MAX_LITERAL: usize = 0x80;
const MAX_RUN: usize = 0x81;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum UnpackError {
    #[error("not a packed RGBA image (bad magic or short header)")]
    Header,
    #[error("packed RGBA image ends early: {0} of {1} pixels")]
    Truncated(usize, usize),
    #[error("packed RGBA image has {0} bytes after its last pixel")]
    Trailing(usize),
}

/// A straight-alpha RGBA8 image, rows top to bottom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl RgbaImage {
    /// Unpacks the [module layout](self).
    pub fn unpack(bytes: &[u8]) -> Result<Self, UnpackError> {
        if bytes.len() < 8 || bytes[..4] != MAGIC {
            return Err(UnpackError::Header);
        }
        let width = u32::from(u16::from_le_bytes([bytes[4], bytes[5]]));
        let height = u32::from(u16::from_le_bytes([bytes[6], bytes[7]]));
        let total = width as usize * height as usize;
        let mut rgba = Vec::with_capacity(total * 4);
        let mut at = 8;
        let short = |rgba: &Vec<u8>| UnpackError::Truncated(rgba.len() / 4, total);
        while rgba.len() < total * 4 {
            let c = usize::from(*bytes.get(at).ok_or_else(|| short(&rgba))?);
            at += 1;
            let (count, len) = if c < 0x80 {
                (c + 1, (c + 1) * 4)
            } else {
                (c - 0x7e, 4)
            };
            let data = bytes.get(at..at + len).ok_or_else(|| short(&rgba))?;
            at += len;
            if count * 4 > total * 4 - rgba.len() {
                return Err(UnpackError::Trailing(count * 4 - (total * 4 - rgba.len())));
            }
            if c < 0x80 {
                rgba.extend_from_slice(data);
            } else {
                for _ in 0..count {
                    rgba.extend_from_slice(data);
                }
            }
        }
        if at != bytes.len() {
            return Err(UnpackError::Trailing(bytes.len() - at));
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }

    /// Packs the image (the [module layout](self)); fully transparent pixels are stored as
    /// transparent black so they form runs. Panics if a side exceeds `u16`.
    pub fn pack(&self) -> Vec<u8> {
        let w = u16::try_from(self.width).expect("width fits u16");
        let h = u16::try_from(self.height).expect("height fits u16");
        let px: Vec<[u8; 4]> = self
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| if p[3] == 0 { [0; 4] } else { *p })
            .collect();
        let mut out = MAGIC.to_vec();
        out.extend(w.to_le_bytes());
        out.extend(h.to_le_bytes());
        let mut literal: Vec<[u8; 4]> = Vec::new();
        let flush = |literal: &mut Vec<[u8; 4]>, out: &mut Vec<u8>| {
            for chunk in literal.chunks(MAX_LITERAL) {
                out.push((chunk.len() - 1) as u8);
                out.extend(chunk.iter().flatten());
            }
            literal.clear();
        };
        let mut i = 0;
        while i < px.len() {
            let run = px[i..]
                .iter()
                .take(MAX_RUN)
                .take_while(|&&p| p == px[i])
                .count();
            if run >= 2 {
                flush(&mut literal, &mut out);
                out.push((run + 0x7e) as u8);
                out.extend(px[i]);
                i += run;
            } else {
                literal.push(px[i]);
                i += 1;
            }
        }
        flush(&mut literal, &mut out);
        out
    }

    /// The image scaled to `w`×`h` by area averaging (each output pixel is the
    /// coverage-weighted mean of the source pixels under it, in premultiplied alpha). Meant for
    /// shrinking; enlarging repeats pixels with blended edges.
    pub fn scaled(&self, w: u32, h: u32) -> Self {
        let (sw, sh) = (self.width as usize, self.height as usize);
        let (dw, dh) = (w as usize, h as usize);
        if (sw, sh) == (dw, dh) || sw == 0 || sh == 0 || dw == 0 || dh == 0 {
            let rgba = if (sw, sh) == (dw, dh) {
                self.rgba.clone()
            } else {
                vec![0; dw * dh * 4]
            };
            return Self {
                width: w,
                height: h,
                rgba,
            };
        }
        // Premultiplied source, so transparent pixels don't darken the edges.
        let pre: Vec<[f32; 4]> = self
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                let a = f32::from(p[3]) / 255.0;
                [
                    f32::from(p[0]) * a,
                    f32::from(p[1]) * a,
                    f32::from(p[2]) * a,
                    f32::from(p[3]),
                ]
            })
            .collect();
        let xs = spans(sw, dw);
        let ys = spans(sh, dh);
        // Horizontal pass, then vertical.
        let mut rows = vec![[0f32; 4]; dw * sh];
        for y in 0..sh {
            for (dx, span) in xs.iter().enumerate() {
                let mut acc = [0f32; 4];
                for &(sx, wt) in span {
                    let p = pre[y * sw + sx];
                    for k in 0..4 {
                        acc[k] += p[k] * wt;
                    }
                }
                rows[y * dw + dx] = acc;
            }
        }
        let mut rgba = Vec::with_capacity(dw * dh * 4);
        for span in &ys {
            for dx in 0..dw {
                let mut acc = [0f32; 4];
                for &(sy, wt) in span {
                    let p = rows[sy * dw + dx];
                    for k in 0..4 {
                        acc[k] += p[k] * wt;
                    }
                }
                let a = acc[3];
                let un = |v: f32| {
                    if a <= 0.0 {
                        0
                    } else {
                        (v * 255.0 / a).round().clamp(0.0, 255.0) as u8
                    }
                };
                rgba.extend([
                    un(acc[0]),
                    un(acc[1]),
                    un(acc[2]),
                    a.round().clamp(0.0, 255.0) as u8,
                ]);
            }
        }
        Self {
            width: w,
            height: h,
            rgba,
        }
    }
}

/// For each of `dst` output pixels, the source pixels it covers and their weights (summing
/// to 1) when `src` pixels are mapped onto `dst`.
fn spans(src: usize, dst: usize) -> Vec<Vec<(usize, f32)>> {
    let k = src as f64 / dst as f64;
    (0..dst)
        .map(|d| {
            let (a, b) = (d as f64 * k, (d + 1) as f64 * k);
            let mut out = Vec::new();
            let mut s = a.floor() as usize;
            while (s as f64) < b && s < src {
                let cover = (b.min(s as f64 + 1.0) - a.max(s as f64)) / (b - a);
                if cover > 0.0 {
                    out.push((s, cover as f32));
                }
                s += 1;
            }
            out
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> RgbaImage {
        let rgba = (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .flat_map(|(x, y)| f(x, y))
            .collect();
        RgbaImage {
            width: w,
            height: h,
            rgba,
        }
    }

    #[test]
    fn pack_round_trips() {
        let img = image(300, 7, |x, y| {
            if x < 150 {
                [1, 2, 3, 255]
            } else {
                [(x * y) as u8, x as u8, 9, (x % 3) as u8 * 100]
            }
        });
        let packed = img.pack();
        assert_eq!(&packed[..8], b"SHPK\x2c\x01\x07\x00");
        let back = RgbaImage::unpack(&packed).unwrap();
        // Transparent pixels come back as transparent black.
        let want: Vec<u8> = img
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| if p[3] == 0 { [0; 4] } else { *p })
            .collect();
        assert_eq!((back.width, back.height), (300, 7));
        assert_eq!(back.rgba, want);
        // A flat image is a few runs of 129.
        let flat = image(129, 2, |_, _| [5, 5, 5, 5]).pack();
        assert_eq!(&flat[8..], &[0xff, 5, 5, 5, 5, 0xff, 5, 5, 5, 5]);
    }

    #[test]
    fn unpack_rejects_damage() {
        let packed = image(4, 4, |x, _| [x as u8, 0, 0, 255]).pack();
        assert_eq!(RgbaImage::unpack(b"SHP"), Err(UnpackError::Header));
        assert!(matches!(
            RgbaImage::unpack(&packed[..packed.len() - 1]),
            Err(UnpackError::Truncated(..))
        ));
        let mut long = packed.clone();
        long.push(0);
        assert_eq!(RgbaImage::unpack(&long), Err(UnpackError::Trailing(1)));
    }

    #[test]
    fn scaled_averages_area() {
        // 4×1 → 2×1: pairs averaged; transparent pixels don't darken the opaque one.
        let img = image(4, 1, |x, _| match x {
            0 => [200, 0, 0, 255],
            1 => [100, 0, 0, 255],
            2 => [0, 0, 0, 0],
            _ => [0, 80, 0, 255],
        });
        let s = img.scaled(2, 1);
        assert_eq!(s.rgba, [150, 0, 0, 255, 0, 80, 0, 128]);
        // 4 → 3: each output covers 4/3 source pixels.
        let ramp = image(4, 1, |x, _| [(x * 40) as u8, 0, 0, 255]).scaled(3, 1);
        assert_eq!(
            ramp.rgba.chunks(4).map(|p| p[0]).collect::<Vec<_>>(),
            [10, 60, 110]
        );
        assert_eq!(img.scaled(4, 1), img);
    }
}

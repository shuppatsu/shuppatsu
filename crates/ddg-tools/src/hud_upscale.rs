//! `ddg-tools upscale-sprites`: contact sheets that compare upscalers on the 2D HUD sprites
//! (`cg.rom` tiles, `tbl.rom` tilemaps, the exe's palettes; docs/re/hud.md), and offline
//! recompositions of whole in-game frames with each method. Everything written is derived
//! from the user's game data and goes to the gitignored `out/`.
//!
//! A sprite is palette indices with index 0 transparent. Methods:
//! * index-preserving (`nearest`, `scalenx`, `xbrz-indexed`): scale the indices and look the
//!   colours up afterwards, so palette swaps, fades and animated entries keep working;
//! * colour methods (`xbrz`, `esrgan`, `waifu2x`): the transparent pixels get the colours of
//!   their opaque neighbours (dilation), the colour is upscaled, and the alpha is rebuilt from
//!   the index-0 mask alone (xBRZ on black/white, threshold one half), as in
//!   `upscale-textures`: no halos, binary transparency;
//! * `sharp-bilinear`: what the overlay does with the original art (overlay.rs), for reference.

use crate::export::GameDirs;
use crate::upscale::{self, Method, Options, Tile, fill_keyed, scale_mask};
use anyhow::{Context, Result, bail};
use ddg_formats::cab_hud::CabTables;
use ddg_formats::cg::{self, CgFile, Palette, TILE_SIZE, Tilemap};
use ddg_formats::exe::Exe;
use ddg_formats::hud::HudTables;
use ddg_render::sprites::xbrz;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const USAGE: &str = "\
upscale-sprites [--out DIR] [--frame <shot.layer.txt>]... [--no-external]
                [--esrgan PATH] [--waifu2x PATH]
  contact sheets of the HUD sprites upscaled by every method (default --out
  out/explain/upscale_hud). --frame recomposes a headless shot taken with DDG_SHOT_LAYER=1
  (`ddg-game play … --shot T:F.ppm` writes F.layer.txt and F.world.ppm) with each method.
  The AI methods need realesrgan-ncnn-vulkan and waifu2x-converter-cpp on PATH (or the
  flags), e.g. `nix shell nixpkgs#realesrgan-ncnn-vulkan nixpkgs#waifu2x-converter-cpp`;
  --no-external leaves them out.";

/// Sheet cell background: a light sky grey, so dark and light halos both show.
const CELL_BG: [u8; 3] = [0x9a, 0xac, 0xbe];
const SHEET_BG: [u8; 3] = [0x1c, 0x1f, 0x24];
const LABEL: [u8; 3] = [0xe8, 0xe8, 0xe8];
/// Index 0's colour for the decisions of `xbrz-indexed`: unlike any art colour.
const KEY_COLOUR: [u8; 3] = [255, 0, 255];
/// Gap between sprites of a group and around cells, in original pixels.
const GAP: usize = 4;
/// The widest row of a group's montage, in original pixels (wider sprites get a row each).
const ROW_WIDTH: usize = 224;

/// One upscaling method of the comparison.
#[derive(Clone, Debug, PartialEq)]
pub enum Up {
    Nearest(u32),
    /// The overlay's sharp bilinear (overlay.rs): the canvas at multiple `canvas` with
    /// nearest neighbour, presented `num/den` of that size.
    SharpBilinear {
        canvas: u32,
        num: u32,
        den: u32,
    },
    Xbrz(u32),
    XbrzIndexed(u32),
    /// Scale2x (AdvMAME2x) on the indices; ×4 is Scale2x twice.
    ScaleNx(u32),
    Esrgan(&'static str, u32),
    Waifu2x(u8, u32),
}

impl Up {
    /// Output pixels per original pixel (rounded for sharp bilinear).
    pub fn factor(&self) -> f64 {
        match *self {
            Self::SharpBilinear { canvas, num, den } => {
                f64::from(canvas) * f64::from(num) / f64::from(den)
            }
            Self::Nearest(s)
            | Self::Xbrz(s)
            | Self::XbrzIndexed(s)
            | Self::ScaleNx(s)
            | Self::Esrgan(_, s)
            | Self::Waifu2x(_, s) => f64::from(s),
        }
    }

    pub fn external(&self) -> bool {
        matches!(self, Self::Esrgan(..) | Self::Waifu2x(..))
    }

    /// File-name part.
    pub fn name(&self) -> String {
        match self {
            Self::Nearest(s) => format!("nearest{s}"),
            Self::SharpBilinear { canvas, num, den } => {
                format!("sharpbilinear{canvas}x{num}-{den}")
            }
            Self::Xbrz(s) => format!("xbrz{s}"),
            Self::XbrzIndexed(s) => format!("xbrzidx{s}"),
            Self::ScaleNx(s) => format!("scale{s}x"),
            Self::Esrgan(m, s) => format!("{m}-{s}"),
            Self::Waifu2x(n, s) => format!("waifu2x-n{n}-{s}"),
        }
    }

    /// Column header (the sheet font has capitals, digits and a few signs).
    pub fn label(&self) -> String {
        match self {
            Self::Nearest(s) => format!("NEAREST X{s}"),
            Self::SharpBilinear { canvas, num, den } => format!(
                "SHARP BILINEAR X{:.2} (CANVAS X{canvas} AT 1600P)",
                f64::from(*canvas) * f64::from(*num) / f64::from(*den)
            ),
            Self::Xbrz(s) => format!("XBRZ X{s}"),
            Self::XbrzIndexed(s) => format!("XBRZ INDEXED X{s}"),
            Self::ScaleNx(s) => format!("SCALE{s}X"),
            Self::Esrgan(m, s) => format!("{} X{s}", m.to_ascii_uppercase()),
            Self::Waifu2x(n, s) => format!("WAIFU2X ART NOISE {n} X{s}"),
        }
    }
}

/// The ×2 and ×4 columns of the sheets.
fn methods_x2() -> Vec<Up> {
    vec![
        Up::Nearest(2),
        Up::Xbrz(2),
        Up::XbrzIndexed(2),
        Up::ScaleNx(2),
        Up::Esrgan("realesr-animevideov3", 2),
        Up::Waifu2x(0, 2),
        Up::Waifu2x(1, 2),
    ]
}

fn methods_x4() -> Vec<Up> {
    vec![
        Up::Nearest(4),
        Up::SharpBilinear {
            canvas: 4,
            num: 5,
            den: 6,
        },
        Up::Xbrz(4),
        Up::XbrzIndexed(4),
        Up::ScaleNx(4),
        Up::Esrgan("realesrgan-x4plus", 4),
        Up::Esrgan("realesrgan-x4plus-anime", 4),
        Up::Esrgan("realesr-animevideov3", 4),
        Up::Waifu2x(0, 4),
        Up::Waifu2x(1, 4),
    ]
}

/// A sprite instance: indices (0 = transparent) and the palette it is drawn with.
#[derive(Clone, Debug, PartialEq)]
pub struct Art {
    pub w: usize,
    pub h: usize,
    pub idx: Vec<u8>,
    pub pal: Palette,
}

impl Art {
    fn colour(&self, i: u8) -> [u8; 3] {
        let [r, g, b, _] = self.pal.rgba(i);
        [r, g, b]
    }

    fn rgba(&self, idx: &[u8]) -> Vec<[u8; 4]> {
        idx.iter().map(|&i| self.pal.rgba(i)).collect()
    }
}

/// Straight-alpha RGBA, sRGB.
#[derive(Clone, Debug, PartialEq)]
pub struct Img {
    pub w: usize,
    pub h: usize,
    pub px: Vec<[u8; 4]>,
}

impl Img {
    fn new(w: usize, h: usize, fill: [u8; 4]) -> Self {
        Self {
            w,
            h,
            px: vec![fill; w * h],
        }
    }

    /// Alpha-blends `src` at (x, y) (straight alpha, in sRGB values: only for the sheets).
    fn over(&mut self, src: &Img, x: i64, y: i64) {
        for sy in 0..src.h {
            let ty = y + sy as i64;
            if !(0..self.h as i64).contains(&ty) {
                continue;
            }
            for sx in 0..src.w {
                let tx = x + sx as i64;
                if !(0..self.w as i64).contains(&tx) {
                    continue;
                }
                let s = src.px[sy * src.w + sx];
                let d = &mut self.px[ty as usize * self.w + tx as usize];
                let a = u32::from(s[3]);
                for c in 0..3 {
                    d[c] = ((u32::from(s[c]) * a + u32::from(d[c]) * (255 - a) + 127) / 255) as u8;
                }
                d[3] = 255;
            }
        }
    }

    fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, c: [u8; 3]) {
        for yy in y..(y + h).min(self.h) {
            for xx in x..(x + w).min(self.w) {
                self.px[yy * self.w + xx] = [c[0], c[1], c[2], 255];
            }
        }
    }

    fn crop(&self, x: usize, y: usize, w: usize, h: usize) -> Img {
        let mut out = Img::new(w, h, [0; 4]);
        for yy in 0..h.min(self.h.saturating_sub(y)) {
            for xx in 0..w.min(self.w.saturating_sub(x)) {
                out.px[yy * w + xx] = self.px[(y + yy) * self.w + x + xx];
            }
        }
        out
    }

    fn magnify(&self, k: usize) -> Img {
        let mut out = Img::new(self.w * k, self.h * k, [0; 4]);
        for (i, p) in out.px.iter_mut().enumerate() {
            let (x, y) = (i % (self.w * k) / k, i / (self.w * k) / k);
            *p = self.px[y * self.w + x];
        }
        out
    }

    fn save(&self, path: &Path) -> Result<()> {
        let raw: Vec<u8> = self.px.iter().flatten().copied().collect();
        image::RgbaImage::from_raw(self.w as u32, self.h as u32, raw)
            .context("image size")?
            .save(path)
            .with_context(|| format!("writing {}", path.display()))
    }
}

/// Nearest-neighbour enlargement of row-major values.
fn nearest<T: Copy>(src: &[T], w: usize, h: usize, s: usize) -> Vec<T> {
    let ow = w * s;
    (0..ow * h * s)
        .map(|i| src[(i / ow / s) * w + (i % ow) / s])
        .collect()
}

/// Scale2x (AdvMAME2x) on exact values: each pixel becomes 2×2, a corner takes the
/// neighbours' value where two of them agree across it and the other two differ.
pub fn scale2x<T: Copy + PartialEq>(src: &[T], w: usize, h: usize) -> Vec<T> {
    let at = |x: i64, y: i64| {
        src[y.clamp(0, h as i64 - 1) as usize * w + x.clamp(0, w as i64 - 1) as usize]
    };
    let ow = w * 2;
    let mut out = Vec::with_capacity(ow * h * 2);
    out.resize(ow * h * 2, src[0]);
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let e = at(x, y);
            let (b, d, f, hh) = (at(x, y - 1), at(x - 1, y), at(x + 1, y), at(x, y + 1));
            let (mut e0, mut e1, mut e2, mut e3) = (e, e, e, e);
            if b != hh && d != f {
                if d == b {
                    e0 = d;
                }
                if b == f {
                    e1 = f;
                }
                if d == hh {
                    e2 = d;
                }
                if hh == f {
                    e3 = f;
                }
            }
            let o = (y as usize * 2) * ow + x as usize * 2;
            out[o] = e0;
            out[o + 1] = e1;
            out[o + ow] = e2;
            out[o + ow + 1] = e3;
        }
    }
    out
}

fn srgb_to_linear(c: u8) -> f32 {
    let c = f32::from(c) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round() as u8
}

/// The overlay's presentation (overlay.rs shader): `src` (sRGB, straight alpha) shown at
/// `dw`×`dh`, sharp bilinear (nearest neighbour to the smallest whole multiple at least the
/// target, then one bilinear tap), with the alpha test at one half. Returns premultiplied
/// linear RGBA.
pub fn present(src: &Img, dw: usize, dh: usize) -> Vec<[f32; 4]> {
    let lut: Vec<f32> = (0..=255).map(srgb_to_linear).collect();
    let fetch = |x: f32, y: f32| -> [f32; 4] {
        let xi = (x as i64).clamp(0, src.w as i64 - 1) as usize;
        let yi = (y as i64).clamp(0, src.h as i64 - 1) as usize;
        let p = src.px[yi * src.w + xi];
        if p[3] < 128 {
            return [0.0; 4];
        }
        [
            lut[usize::from(p[0])],
            lut[usize::from(p[1])],
            lut[usize::from(p[2])],
            1.0,
        ]
    };
    let (tw, th) = (src.w as f32, src.h as f32);
    let p = [
        ((dw as f32 / tw) - 1e-4).ceil().max(1.0),
        ((dh as f32 / th) - 1e-4).ceil().max(1.0),
    ];
    let mut out = vec![[0.0; 4]; dw * dh];
    for j in 0..dh {
        for i in 0..dw {
            let x = [
                (i as f32 + 0.5) / dw as f32 * tw,
                (j as f32 + 0.5) / dh as f32 * th,
            ];
            let mut q = [0.0; 2];
            for k in 0..2 {
                let base = x[k].floor();
                let d = x[k] - base - 0.5;
                let range = 0.5 - 0.5 / p[k];
                q[k] = base + (d - d.clamp(-range, range)) * p[k];
            }
            let (ix, iy) = (q[0].floor(), q[1].floor());
            let (wx, wy) = (q[0] - ix, q[1] - iy);
            let (a, b) = (fetch(ix, iy), fetch(ix + 1.0, iy));
            let (c, d) = (fetch(ix, iy + 1.0), fetch(ix + 1.0, iy + 1.0));
            let o = &mut out[j * dw + i];
            for ch in 0..4 {
                let top = a[ch] + (b[ch] - a[ch]) * wx;
                let bottom = c[ch] + (d[ch] - c[ch]) * wx;
                o[ch] = top + (bottom - top) * wy;
            }
        }
    }
    out
}

/// Premultiplied linear RGBA back to straight sRGB.
fn unpremultiply(px: &[[f32; 4]], w: usize, h: usize) -> Img {
    Img {
        w,
        h,
        px: px
            .iter()
            .map(|p| {
                if p[3] <= 0.0 {
                    return [0; 4];
                }
                [
                    linear_to_srgb(p[0] / p[3]),
                    linear_to_srgb(p[1] / p[3]),
                    linear_to_srgb(p[2] / p[3]),
                    (p[3] * 255.0).round() as u8,
                ]
            })
            .collect(),
    }
}

struct Tools {
    work: PathBuf,
    esrgan: Option<PathBuf>,
    waifu2x: Option<PathBuf>,
}

/// Every art upscaled by `m` (straight-alpha RGBA). External tools run once for all.
fn upscale_arts(arts: &[Art], m: &Up, tools: &Tools) -> Result<Vec<Img>> {
    let by_index = |f: &dyn Fn(&Art) -> (usize, Vec<u8>)| -> Vec<Img> {
        arts.iter()
            .map(|a| {
                let (s, idx) = f(a);
                Img {
                    w: a.w * s,
                    h: a.h * s,
                    px: a.rgba(&idx),
                }
            })
            .collect()
    };
    match *m {
        Up::Nearest(s) => {
            let s = s as usize;
            Ok(by_index(&|a| (s, nearest(&a.idx, a.w, a.h, s))))
        }
        Up::ScaleNx(s) => Ok(by_index(&|a| {
            let (mut idx, mut w, mut h, mut k) = (a.idx.clone(), a.w, a.h, 1);
            while k < s as usize {
                idx = scale2x(&idx, w, h);
                (w, h, k) = (w * 2, h * 2, k * 2);
            }
            (k, idx)
        })),
        Up::XbrzIndexed(s) => Ok(by_index(&|a| {
            let colours: Vec<[u8; 3]> = a
                .idx
                .iter()
                .map(|&i| if i == 0 { KEY_COLOUR } else { a.colour(i) })
                .collect();
            (
                s as usize,
                xbrz::scale_indexed(&colours, &a.idx, a.w, a.h, s as usize),
            )
        })),
        Up::SharpBilinear { canvas, num, den } => Ok(arts
            .iter()
            .map(|a| {
                let c = canvas as usize;
                let src = Img {
                    w: a.w * c,
                    h: a.h * c,
                    px: a.rgba(&nearest(&a.idx, a.w, a.h, c)),
                };
                let dw = (src.w * num as usize).div_ceil(den as usize);
                let dh = (src.h * num as usize).div_ceil(den as usize);
                unpremultiply(&present(&src, dw, dh), dw, dh)
            })
            .collect()),
        Up::Xbrz(s) | Up::Esrgan(_, s) | Up::Waifu2x(_, s) => {
            let (method, tool, pad) = match *m {
                Up::Xbrz(_) => (Method::Xbrz, None, 2),
                Up::Esrgan(model, _) => (
                    Method::Esrgan {
                        model: model.to_owned(),
                    },
                    tools.esrgan.clone(),
                    8,
                ),
                Up::Waifu2x(noise, _) => (Method::Waifu2x { noise }, tools.waifu2x.clone(), 8),
                _ => unreachable!(),
            };
            let tiles: Vec<Tile> = arts.iter().map(|a| keyed_tile(a, pad)).collect();
            let opts = Options {
                method,
                scale: s,
                out: tools.work.join(m.name()),
                smooth_mask: true,
                pad: pad as u32,
                only: None,
                tool,
                keep_work: false,
            };
            let refs: Vec<&Tile> = tiles.iter().collect();
            let rgb = upscale::upscale_tiles(&opts, &refs)?;
            let _ = std::fs::remove_dir_all(&opts.out);
            Ok(tiles
                .iter()
                .zip(arts)
                .zip(rgb)
                .map(|((t, a), rgb)| {
                    let mask = scale_mask(&t.opaque, t.w, t.h, s, true);
                    unpad(&rgb, &mask, t.w as usize * s as usize, a, pad, s as usize)
                })
                .collect())
        }
    }
}

/// The art with `pad` transparent pixels around it, transparent pixels coloured by dilation.
fn keyed_tile(a: &Art, pad: usize) -> Tile {
    let (w, h) = (a.w + 2 * pad, a.h + 2 * pad);
    let mut t = Tile {
        w: w as u32,
        h: h as u32,
        rgb: vec![[0; 3]; w * h],
        opaque: vec![false; w * h],
    };
    for y in 0..a.h {
        for x in 0..a.w {
            let i = a.idx[y * a.w + x];
            if i != 0 {
                let o = (y + pad) * w + x + pad;
                t.rgb[o] = a.colour(i);
                t.opaque[o] = true;
            }
        }
    }
    fill_keyed(&mut t);
    t
}

/// The upscaled colour and rebuilt mask without the padding.
fn unpad(rgb: &[[u8; 3]], mask: &[bool], tw: usize, a: &Art, pad: usize, s: usize) -> Img {
    let (w, h) = (a.w * s, a.h * s);
    let mut px = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            let i = (y + pad * s) * tw + x + pad * s;
            let c = rgb[i];
            px.push([c[0], c[1], c[2], if mask[i] { 255 } else { 0 }]);
        }
    }
    Img { w, h, px }
}

/// Palette indices of the tile rectangle `(col, row, w, h)` of `map` (`w`/`h` ≤ 0: to the
/// map's edge).
fn map_indices(map: &Tilemap, cg: &CgFile<'_>, (c0, r0, w, h): (i32, i32, i32, i32)) -> Art {
    let (mw, mh) = (i32::from(map.width), i32::from(map.height));
    let (c0, r0) = (c0.max(0), r0.max(0));
    let w = if w <= 0 { mw - c0 } else { w }.max(0) as usize;
    let h = if h <= 0 { mh - r0 } else { h }.max(0) as usize;
    let (pw, ph) = (w * TILE_SIZE, h * TILE_SIZE);
    let mut idx = vec![0u8; pw * ph];
    for ty in 0..h {
        for tx in 0..w {
            let (mc, mr) = (c0 + tx as i32, r0 + ty as i32);
            if mc >= mw || mr >= mh {
                continue;
            }
            let id = map.tiles[(mr * mw + mc) as usize];
            let Some(tile) = (id != 0).then(|| cg.tile(usize::from(id)).ok()).flatten() else {
                continue;
            };
            for py in 0..TILE_SIZE {
                let d = (ty * TILE_SIZE + py) * pw + tx * TILE_SIZE;
                idx[d..d + TILE_SIZE].copy_from_slice(&tile[py * TILE_SIZE..(py + 1) * TILE_SIZE]);
            }
        }
    }
    Art {
        w: pw,
        h: ph,
        idx,
        pal: Palette([0; 256]),
    }
}

/// Removes fully transparent rows and columns at the edges (tilemaps are padded to 16 px).
fn trim(a: &Art) -> Art {
    let opaque = |x: usize, y: usize| a.idx[y * a.w + x] != 0;
    let cols: Vec<usize> = (0..a.w)
        .filter(|&x| (0..a.h).any(|y| opaque(x, y)))
        .collect();
    let rows: Vec<usize> = (0..a.h)
        .filter(|&y| (0..a.w).any(|x| opaque(x, y)))
        .collect();
    let (Some(&x0), Some(&x1), Some(&y0), Some(&y1)) =
        (cols.first(), cols.last(), rows.first(), rows.last())
    else {
        return a.clone();
    };
    let (w, h) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut idx = Vec::with_capacity(w * h);
    for y in y0..=y1 {
        idx.extend_from_slice(&a.idx[y * a.w + x0..y * a.w + x1 + 1]);
    }
    Art {
        w,
        h,
        idx,
        pal: a.pal.clone(),
    }
}

/// The game data the sheets need.
struct Data<'a> {
    cg: CgFile<'a>,
    maps: Vec<Tilemap>,
    palettes: HashMap<u8, Palette>,
    hud: HudTables,
    cab: CabTables,
}

impl Data<'_> {
    /// The whole map `tbl` in palette `pal` (with `edit` applied to the palette), trimmed.
    fn art(&self, tbl: u16, pal: u8, edit: &dyn Fn(&mut Palette)) -> Result<Art> {
        let map = self
            .maps
            .get(usize::from(tbl))
            .with_context(|| format!("no tilemap {tbl:#x}"))?;
        let mut a = map_indices(map, &self.cg, (0, 0, 0, 0));
        let mut p = self
            .palettes
            .get(&pal)
            .with_context(|| format!("palette {pal} is not set at init"))?
            .clone();
        edit(&mut p);
        a.pal = p;
        Ok(trim(&a))
    }
}

/// A row of the sheets: a name and its sprites.
struct Group {
    name: &'static str,
    arts: Vec<Art>,
}

fn groups(d: &Data<'_>) -> Result<Vec<Group>> {
    let none: &dyn Fn(&mut Palette) = &|_| {};
    let ids = |list: &[u16], pal: u8| -> Result<Vec<Art>> {
        list.iter().map(|&t| d.art(t, pal, none)).collect()
    };
    // Seven-segment readout showing "123" (0x416d10: entries 0xdb.., lit 0xff0000 in the
    // B, R, G order = green, unlit 0x555555; masks read from the exe).
    let masks = d.hud.segment_masks;
    let segments = move |p: &mut Palette| {
        for (digit, v) in [1usize, 2, 3].iter().enumerate() {
            for k in 0..7 {
                let lit = masks[*v] & (1 << k) != 0;
                p.0[0xdb + digit * 7 + k] = if lit { 0x00ff_0000 } else { 0x0055_5555 };
            }
        }
    };
    // Route strip: three segments run (green), the rest olive (0x466b70 / 0x466b10).
    let (green, olive) = (d.cab.strip_green, d.cab.strip_olive);
    let strip = move |p: &mut Palette| {
        for i in 0..16 {
            p.0[0x50 + i] = if i < 3 { green[i] } else { olive[i] };
        }
    };
    let plates: Vec<u16> = d
        .hud
        .stage_station_ids
        .first()
        .into_iter()
        .flatten()
        .filter_map(|&id| u16::try_from(d.cab.station_plates[usize::from(id)]).ok())
        .take(3)
        .collect();
    let names: Vec<u16> = d.hud.stage_station_ids[0]
        .iter()
        .take(1)
        .flat_map(|&id| {
            let n = &d.hud.station_names[usize::from(id)];
            [n.plain, n.highlighted]
        })
        .collect();
    let mut clock: Vec<Art> = ids(&d.hud.clock_digits, 5)?;
    clock.extend(ids(&d.hud.clock_colon_digits[..3], 6)?);
    clock.extend(ids(&[0x206, 0x207], 4)?);
    let mut remaining = ids(&d.hud.time_digits, 5)?;
    remaining.extend(ids(&[0x40e, 0x261, 0x262], 4)?);
    let mut distance = ids(&d.hud.distance_digits, 4)?;
    distance.extend(ids(&[0x58, 0x59, 0x5a, 0x5e, 0x2c7], 4)?);
    let mut speed = ids(&[0x54a], 4)?;
    speed.push(d.art(0x53, 4, &segments)?);
    let mut stations = ids(&plates, 4)?;
    stations.extend(ids(&[0x4b8], 4)?);
    stations.extend(ids(&names, 4)?);
    stations.extend(ids(&[0x70, 0x6c, 0x71, 0x6f, 0x222], 4)?);
    let mut board = vec![d.art(0x327, 4, &strip)?, d.art(0x32a, 4, &strip)?];
    board.extend(ids(&[0x38f, 0x395], 4)?);
    let mut handles = ids(
        &[
            d.hud.brake_sprites[0],
            d.hud.brake_sprites[4],
            d.hud.brake_sprites[9],
        ],
        4,
    )?;
    handles.extend(ids(
        &[
            d.hud.mascon_sprites[0],
            d.hud.mascon_sprites[5],
            0x553,
            0x53e,
        ],
        4,
    )?);
    let mut navigator = ids(&[0x157, 0x159, 0x148, 0x45a], 4)?;
    navigator.extend(ids(&[0x212, 0x219], 8)?);
    navigator.extend(ids(&[0x2b0], 0x10)?);
    let mut messages = ids(&[0x1b0, 0x1ad, 0x1b4, 0x1b5, 0x1bd], 5)?;
    messages.extend(ids(&[0x252], 4)?);
    Ok(vec![
        Group {
            name: "SPEEDOMETER",
            arts: speed,
        },
        Group {
            name: "CLOCK DIGITS",
            arts: clock,
        },
        Group {
            name: "REMAINING TIME",
            arts: remaining,
        },
        Group {
            name: "STOP DISTANCE",
            arts: distance,
        },
        Group {
            name: "SIGNALS AND LAMPS",
            arts: ids(
                &[
                    0x47c, 0x476, 0x470, 0x486, 0x44b, 0x20e, 0x211, 0x54e, 0x54f,
                ],
                4,
            )?,
        },
        Group {
            name: "SIGNS",
            arts: ids(&[0x45b, 0x45c, 0x46f, 0x46e, 0x460, 0x48a], 4)?,
        },
        Group {
            name: "STATION PLATES",
            arts: stations,
        },
        Group {
            name: "VEHICLE",
            arts: vec![d.art(0x68, 10, none)?, d.art(0x69, 4, none)?],
        },
        Group {
            name: "BRAKE AND NOTCH",
            arts: handles,
        },
        Group {
            name: "ROUTE BOARD",
            arts: board,
        },
        Group {
            name: "NAVIGATOR",
            arts: navigator,
        },
        Group {
            name: "MESSAGES",
            arts: messages,
        },
    ])
}

/// Places sprites left to right in rows at most [`ROW_WIDTH`] wide: positions and the
/// montage size, in original pixels.
pub fn montage(sizes: &[(usize, usize)]) -> (Vec<(usize, usize)>, usize, usize) {
    let (mut x, mut y, mut row_h, mut w) = (0, 0, 0, 0);
    let mut pos = Vec::with_capacity(sizes.len());
    for &(sw, sh) in sizes {
        if x > 0 && x + sw > ROW_WIDTH {
            y += row_h + GAP;
            (x, row_h) = (0, 0);
        }
        pos.push((x, y));
        x += sw + GAP;
        w = w.max(x - GAP);
        row_h = row_h.max(sh);
    }
    (pos, w, y + row_h)
}

// ---- Labels: a 3×5 pixel font --------------------------------------------------------------

/// Rows top to bottom, 3 bits each (left = high bit).
fn glyph(c: char) -> [u8; 5] {
    match c {
        'A' => [2, 5, 7, 5, 5],
        'B' => [6, 5, 6, 5, 6],
        'C' => [3, 4, 4, 4, 3],
        'D' => [6, 5, 5, 5, 6],
        'E' => [7, 4, 6, 4, 7],
        'F' => [7, 4, 6, 4, 4],
        'G' => [3, 4, 5, 5, 3],
        'H' => [5, 5, 7, 5, 5],
        'I' => [7, 2, 2, 2, 7],
        'J' => [1, 1, 1, 5, 2],
        'K' => [5, 5, 6, 5, 5],
        'L' => [4, 4, 4, 4, 7],
        'M' => [5, 7, 7, 5, 5],
        'N' => [6, 5, 5, 5, 5],
        'O' => [2, 5, 5, 5, 2],
        'P' => [6, 5, 6, 4, 4],
        'Q' => [2, 5, 5, 6, 3],
        'R' => [6, 5, 6, 5, 5],
        'S' => [3, 4, 2, 1, 6],
        'T' => [7, 2, 2, 2, 2],
        'U' => [5, 5, 5, 5, 7],
        'V' => [5, 5, 5, 5, 2],
        'W' => [5, 5, 7, 7, 5],
        'X' => [5, 5, 2, 5, 5],
        'Y' => [5, 5, 2, 2, 2],
        'Z' => [7, 1, 2, 4, 7],
        '0' => [7, 5, 5, 5, 7],
        '1' => [2, 6, 2, 2, 7],
        '2' => [6, 1, 2, 4, 7],
        '3' => [6, 1, 2, 1, 6],
        '4' => [5, 5, 7, 1, 1],
        '5' => [7, 4, 6, 1, 6],
        '6' => [3, 4, 6, 5, 2],
        '7' => [7, 1, 2, 2, 2],
        '8' => [2, 5, 2, 5, 2],
        '9' => [2, 5, 3, 1, 6],
        '-' => [0, 0, 7, 0, 0],
        '.' => [0, 0, 0, 0, 2],
        ':' => [0, 2, 0, 2, 0],
        '/' => [1, 1, 2, 4, 4],
        '(' => [2, 4, 4, 4, 2],
        ')' => [2, 1, 1, 1, 2],
        _ => [0; 5],
    }
}

const TEXT_SCALE: usize = 3;
const TEXT_H: usize = 5 * TEXT_SCALE;

/// Draws `text` with its top-left at (x, y); returns the width.
fn text(img: &mut Img, x: usize, y: usize, s: &str, c: [u8; 3]) -> usize {
    let k = TEXT_SCALE;
    for (n, ch) in s.chars().enumerate() {
        let g = glyph(ch.to_ascii_uppercase());
        for (row, bits) in g.iter().enumerate() {
            for col in 0..3 {
                if bits & (4 >> col) != 0 {
                    img.fill_rect(x + n * 4 * k + col * k, y + row * k, k, k, c);
                }
            }
        }
    }
    s.chars().count() * 4 * k
}

// ---- Sheets --------------------------------------------------------------------------------

/// One sheet: a row per group, a column per method, every cell at its method's scale (1:1).
fn sheet(groups: &[Group], methods: &[Up], results: &[Vec<Img>], cell_scale: usize) -> Img {
    let label_w = 20 * 4 * TEXT_SCALE;
    let pad = GAP * cell_scale;
    let layouts: Vec<_> = groups
        .iter()
        .map(|g| montage(&g.arts.iter().map(|a| (a.w, a.h)).collect::<Vec<_>>()))
        .collect();
    let col_w = layouts.iter().map(|l| l.1).max().unwrap_or(0) * cell_scale + 2 * pad;
    let header = TEXT_H + 2 * pad;
    let row_h: Vec<usize> = layouts
        .iter()
        .map(|l| (l.2 * cell_scale + 2 * pad).max(TEXT_H + 2 * pad))
        .collect();
    let width = label_w + methods.len() * (col_w + pad) + pad;
    let height = header + row_h.iter().map(|h| h + pad).sum::<usize>();
    let mut img = Img::new(width, height, [SHEET_BG[0], SHEET_BG[1], SHEET_BG[2], 255]);
    for (mi, m) in methods.iter().enumerate() {
        let x = label_w + mi * (col_w + pad);
        text(&mut img, x, pad, &m.label(), LABEL);
    }
    let mut first = 0;
    let mut y = header;
    for (gi, g) in groups.iter().enumerate() {
        text(&mut img, pad, y + pad, g.name, LABEL);
        let (pos, _, _) = &layouts[gi];
        for (mi, m) in methods.iter().enumerate() {
            let x = label_w + mi * (col_w + pad);
            img.fill_rect(x, y, col_w, row_h[gi], CELL_BG);
            let f = m.factor();
            for (ai, &(px, py)) in pos.iter().enumerate() {
                let up = &results[mi][first + ai];
                let ox = x + pad + (px as f64 * f).round() as usize;
                let oy = y + pad + (py as f64 * f).round() as usize;
                img.over(up, ox as i64, oy as i64);
            }
        }
        first += g.arts.len();
        y += row_h[gi] + pad;
    }
    img
}

/// A zoomed crop: art `art` (index into the flat list), rectangle in original pixels.
struct Zoom {
    name: &'static str,
    art: usize,
    rect: (usize, usize, usize, usize),
}

/// Crops of the ×4 results, magnified twice more with nearest neighbour.
fn zoom_sheet(zooms: &[Zoom], methods: &[Up], results: &[Vec<Img>]) -> Img {
    const MAG: usize = 2;
    let label_w = 14 * 4 * TEXT_SCALE;
    let pad = 12;
    let cell = |z: &Zoom| (z.rect.2 * 4 * MAG, z.rect.3 * 4 * MAG);
    let col_w = zooms.iter().map(|z| cell(z).0).max().unwrap_or(0) + 2 * pad;
    let header = TEXT_H + 2 * pad;
    let width = label_w + methods.len() * (col_w + pad) + pad;
    let height = header + zooms.iter().map(|z| cell(z).1 + 3 * pad).sum::<usize>();
    let mut img = Img::new(width, height, [SHEET_BG[0], SHEET_BG[1], SHEET_BG[2], 255]);
    for (mi, m) in methods.iter().enumerate() {
        let x = label_w + mi * (col_w + pad);
        let short: String = m.label().chars().take(26).collect();
        text(&mut img, x, pad, &short, LABEL);
    }
    let mut y = header;
    for z in zooms {
        let (cw, ch) = cell(z);
        text(&mut img, pad, y + pad, z.name, LABEL);
        for (mi, m) in methods.iter().enumerate() {
            let x = label_w + mi * (col_w + pad);
            img.fill_rect(x, y, cw + 2 * pad, ch + 2 * pad, CELL_BG);
            let f = m.factor();
            let (rx, ry, rw, rh) = z.rect;
            let up = &results[mi][z.art];
            let crop = up.crop(
                (rx as f64 * f).round() as usize,
                (ry as f64 * f).round() as usize,
                (rw as f64 * f).round() as usize,
                (rh as f64 * f).round() as usize,
            );
            img.over(&crop.magnify(MAG), (x + pad) as i64, (y + pad) as i64);
        }
        y += ch + 3 * pad;
    }
    img
}

// ---- Frames --------------------------------------------------------------------------------

/// A sprite layer dumped by `ddg-game` (`DDG_SHOT_LAYER`, shot.rs `layer_dump`).
#[derive(Debug, Default, PartialEq)]
pub struct LayerDump {
    /// Canvas size in pixels and its multiple of 640×480.
    pub canvas: (usize, usize),
    pub scale: usize,
    pub shake: (i32, i32),
    pub widgets: usize,
    pub slots: Vec<DumpSlot>,
    pub palettes: HashMap<u8, Palette>,
    pub fans: Vec<DumpFan>,
}

#[derive(Debug, PartialEq)]
pub struct DumpSlot {
    pub tbl: u16,
    pub x: i32,
    pub y: i32,
    pub rect: (i32, i32, i32, i32),
    pub pal: u8,
    /// Anchor offset in canvas pixels.
    pub shift: i32,
}

#[derive(Debug, PartialEq)]
pub struct DumpFan {
    pub shift: i32,
    pub argb: u32,
    pub points: Vec<(i32, i32)>,
}

pub fn parse_dump(text: &str) -> Result<LayerDump> {
    let mut d = LayerDump::default();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        let num = |i: usize| -> Result<i64> {
            f.get(i)
                .with_context(|| format!("short line: {line}"))?
                .parse::<i64>()
                .with_context(|| format!("bad number in: {line}"))
        };
        match f.first().copied() {
            Some("canvas") => {
                d.canvas = (num(1)? as usize, num(2)? as usize);
                d.scale = num(4)? as usize;
                d.shake = (num(8)? as i32, num(9)? as i32);
            }
            Some("widgets") => d.widgets = num(1)? as usize,
            Some("slot") => d.slots.push(DumpSlot {
                tbl: u16::try_from(num(3)?)?,
                x: num(5)? as i32,
                y: num(7)? as i32,
                rect: (
                    num(9)? as i32,
                    num(10)? as i32,
                    num(11)? as i32,
                    num(12)? as i32,
                ),
                pal: u8::try_from(num(14)?)?,
                shift: num(18)? as i32,
            }),
            Some("palette") => {
                let p = u8::try_from(num(1)?)?;
                if f.len() != 258 {
                    bail!("palette {p}: {} colours", f.len() - 2);
                }
                let mut c = [0u32; 256];
                for (i, v) in c.iter_mut().enumerate() {
                    *v = u32::from_str_radix(f[2 + i], 16)?;
                }
                d.palettes.insert(p, Palette(c));
            }
            Some("fan") => d.fans.push(DumpFan {
                shift: num(1)? as i32,
                argb: u32::from_str_radix(f.get(3).context("fan colour")?, 16)?,
                points: (4..f.len())
                    .step_by(2)
                    .map(|i| Ok((num(i)? as i32, num(i + 1)? as i32)))
                    .collect::<Result<_>>()?,
            }),
            Some(other) if !other.starts_with('#') => bail!("unknown dump line: {line}"),
            _ => {}
        }
    }
    if d.scale == 0 {
        bail!("dump has no canvas line");
    }
    Ok(d)
}

/// The arts of a dump's slots, in draw order.
fn dump_arts(d: &LayerDump, data: &Data<'_>) -> Result<Vec<Art>> {
    d.slots
        .iter()
        .map(|s| {
            let map = data
                .maps
                .get(usize::from(s.tbl))
                .with_context(|| format!("no tilemap {:#x}", s.tbl))?;
            let mut a = map_indices(map, &data.cg, s.rect);
            a.pal = d
                .palettes
                .get(&s.pal)
                .with_context(|| format!("palette {} not in the dump", s.pal))?
                .clone();
            Ok(a)
        })
        .collect()
}

/// The canvas at `f` pixels per original pixel with the slots' upscaled images and the fans.
fn compose(d: &LayerDump, ups: &[Img], f: usize) -> Img {
    let k = f as f64 / d.scale as f64;
    let (w, h) = (
        (d.canvas.0 as f64 * k).round() as usize,
        (d.canvas.1 as f64 * k).round() as usize,
    );
    let mut img = Img::new(w, h, [0; 4]);
    for (s, up) in d.slots.iter().zip(ups) {
        let x = i64::from(s.x + d.shake.0) * f as i64 + (f64::from(s.shift) * k).round() as i64;
        let y = i64::from(s.y + d.shake.1) * f as i64;
        // Binary alpha: replace, as the blit does.
        for sy in 0..up.h {
            for sx in 0..up.w {
                let p = up.px[sy * up.w + sx];
                let (tx, ty) = (x + sx as i64, y + sy as i64);
                if p[3] >= 128 && (0..w as i64).contains(&tx) && (0..h as i64).contains(&ty) {
                    img.px[ty as usize * w + tx as usize] = [p[0], p[1], p[2], 255];
                }
            }
        }
    }
    for fan in &d.fans {
        let pts: Vec<[f64; 2]> = fan
            .points
            .iter()
            .map(|&(x, y)| {
                [
                    f64::from(x + d.shake.0) * f as f64 + f64::from(fan.shift) * k,
                    f64::from(y + d.shake.1) * f as f64,
                ]
            })
            .collect();
        let [_, r, g, b] = fan.argb.to_be_bytes();
        for i in 1..pts.len().saturating_sub(1) {
            fill_triangle(&mut img, [pts[0], pts[i], pts[i + 1]], [r, g, b, 255]);
        }
    }
    img
}

/// Pixels whose centre is inside the triangle (either winding).
fn fill_triangle(img: &mut Img, t: [[f64; 2]; 3], c: [u8; 4]) {
    let edge = |a: [f64; 2], b: [f64; 2], p: [f64; 2]| {
        (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
    };
    let x0 = t
        .iter()
        .map(|p| p[0])
        .fold(f64::MAX, f64::min)
        .floor()
        .max(0.0) as usize;
    let x1 = t
        .iter()
        .map(|p| p[0])
        .fold(f64::MIN, f64::max)
        .ceil()
        .min(img.w as f64) as usize;
    let y0 = t
        .iter()
        .map(|p| p[1])
        .fold(f64::MAX, f64::min)
        .floor()
        .max(0.0) as usize;
    let y1 = t
        .iter()
        .map(|p| p[1])
        .fold(f64::MIN, f64::max)
        .ceil()
        .min(img.h as f64) as usize;
    for y in y0..y1 {
        for x in x0..x1 {
            let p = [x as f64 + 0.5, y as f64 + 0.5];
            let e = [
                edge(t[0], t[1], p),
                edge(t[1], t[2], p),
                edge(t[2], t[0], p),
            ];
            if e.iter().all(|&v| v >= 0.0) || e.iter().all(|&v| v <= 0.0) {
                img.px[y * img.w + x] = c;
            }
        }
    }
}

/// The canvas over `world` (`ww`×`wh` RGB) as the overlay draws it in smooth mode: fitted
/// with one scale factor and centred, sharp bilinear, premultiplied blending in linear light.
fn present_over(canvas: &Img, world: &[u8], ww: usize, wh: usize) -> Img {
    let k = (ww as f64 / canvas.w as f64).min(wh as f64 / canvas.h as f64);
    let (dw, dh) = (
        (canvas.w as f64 * k).round() as usize,
        (canvas.h as f64 * k).round() as usize,
    );
    let (ox, oy) = ((ww - dw.min(ww)) / 2, (wh - dh.min(wh)) / 2);
    let top = present(canvas, dw, dh);
    let mut out = Img::new(ww, wh, [0, 0, 0, 255]);
    for y in 0..wh {
        for x in 0..ww {
            let i = (y * ww + x) * 3;
            let mut c = [
                srgb_to_linear(world[i]),
                srgb_to_linear(world[i + 1]),
                srgb_to_linear(world[i + 2]),
            ];
            if (ox..ox + dw).contains(&x) && (oy..oy + dh).contains(&y) {
                let t = top[(y - oy) * dw + x - ox];
                for ch in 0..3 {
                    c[ch] = t[ch] + c[ch] * (1.0 - t[3]);
                }
            }
            out.px[y * ww + x] = [
                linear_to_srgb(c[0]),
                linear_to_srgb(c[1]),
                linear_to_srgb(c[2]),
                255,
            ];
        }
    }
    out
}

/// A binary PPM (P6) as written by `ddg-game` shots.
fn read_ppm(path: &Path) -> Result<(usize, usize, Vec<u8>)> {
    let data = std::fs::read(path).with_context(|| path.display().to_string())?;
    let mut fields = Vec::new();
    let mut pos = 0;
    while fields.len() < 4 {
        while pos < data.len() && data[pos].is_ascii_whitespace() {
            pos += 1;
        }
        let start = pos;
        while pos < data.len() && !data[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if start == pos {
            bail!("{}: truncated header", path.display());
        }
        fields.push(String::from_utf8_lossy(&data[start..pos]).into_owned());
    }
    if fields[0] != "P6" || fields[3] != "255" {
        bail!("{}: not an 8-bit P6 PPM", path.display());
    }
    let (w, h): (usize, usize) = (fields[1].parse()?, fields[2].parse()?);
    let px = data
        .get(pos + 1..pos + 1 + w * h * 3)
        .with_context(|| format!("{}: short pixel data", path.display()))?;
    Ok((w, h, px.to_vec()))
}

// ---- Command -------------------------------------------------------------------------------

pub fn run(args: &[String]) -> Result<()> {
    let mut out = PathBuf::from("out/explain/upscale_hud");
    let mut frames = Vec::new();
    let mut external = true;
    let mut esrgan = None;
    let mut waifu2x = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .with_context(|| format!("{a} needs a value\n\n{USAGE}"))
        };
        match a.as_str() {
            "--out" => out = PathBuf::from(val()?),
            "--frame" => frames.push(PathBuf::from(val()?)),
            "--no-external" => external = false,
            "--esrgan" => esrgan = Some(PathBuf::from(val()?)),
            "--waifu2x" => waifu2x = Some(PathBuf::from(val()?)),
            other => bail!("unknown option {other}\n\n{USAGE}"),
        }
    }
    std::fs::create_dir_all(&out)?;
    let dirs = GameDirs::from_env()?;
    let exe_data = dirs.read_exe()?;
    let exe = Exe::parse(&exe_data)?;
    let cg_data = dirs.read_data("cg.rom")?;
    let data = Data {
        cg: CgFile::parse(&cg_data)?,
        maps: cg::parse_tbl(&dirs.read_data("tbl.rom")?)?,
        palettes: cg::init_palettes(&exe)?.into_iter().collect(),
        hud: HudTables::read(&exe)?,
        cab: CabTables::read(&exe)?,
    };
    let tools = Tools {
        work: out.join(".work"),
        esrgan,
        waifu2x,
    };
    let keep = |m: &Up| external || !m.external();

    let groups = groups(&data)?;
    let arts: Vec<Art> = groups.iter().flat_map(|g| g.arts.iter().cloned()).collect();
    eprintln!("{} groups, {} sprites", groups.len(), arts.len());
    let run_all = |methods: Vec<Up>, arts: &[Art]| -> (Vec<Up>, Vec<Vec<Img>>) {
        let mut ok = (Vec::new(), Vec::new());
        for m in methods.into_iter().filter(keep) {
            let t = std::time::Instant::now();
            match upscale_arts(arts, &m, &tools) {
                Ok(r) => {
                    eprintln!("{}: {:.1?}", m.name(), t.elapsed());
                    ok.0.push(m);
                    ok.1.push(r);
                }
                Err(e) => eprintln!("{}: skipped ({e:#})", m.name()),
            }
        }
        ok
    };

    let (m2, r2) = run_all(methods_x2(), &arts);
    sheet(&groups, &m2, &r2, 2).save(&out.join("sheet_x2.png"))?;
    let (m4, r4) = run_all(methods_x4(), &arts);
    sheet(&groups, &m4, &r4, 4).save(&out.join("sheet_x4.png"))?;
    // The same rows one file each, easier to view at 1:1.
    let rows = out.join("rows");
    std::fs::create_dir_all(&rows)?;
    let mut off = 0;
    for g in &groups {
        let n = g.arts.len();
        for (methods, results, s) in [(&m2, &r2, 2), (&m4, &r4, 4)] {
            let sub: Vec<Vec<Img>> = results.iter().map(|r| r[off..off + n].to_vec()).collect();
            let file = format!("{}_x{s}.png", g.name.to_ascii_lowercase().replace(' ', "_"));
            sheet(std::slice::from_ref(g), methods, &sub, s).save(&rows.join(file))?;
        }
        off += n;
    }
    let base = |name: &str| -> usize {
        let mut n = 0;
        for g in &groups {
            if g.name == name {
                return n;
            }
            n += g.arts.len();
        }
        0
    };
    let zooms = [
        Zoom {
            name: "DIAL",
            art: base("SPEEDOMETER"),
            rect: (8, 4, 56, 48),
        },
        Zoom {
            name: "7-SEGMENT",
            art: base("SPEEDOMETER") + 1,
            rect: (40, 4, 64, 40),
        },
        Zoom {
            name: "CLOCK DIGIT",
            art: base("CLOCK DIGITS") + 2,
            rect: (0, 0, 24, 24),
        },
        Zoom {
            name: "TIME DIGIT",
            art: base("REMAINING TIME") + 3,
            rect: (0, 0, 40, 40),
        },
        Zoom {
            name: "KANJI PLATE",
            art: base("STATION PLATES"),
            rect: (0, 36, 32, 60),
        },
        Zoom {
            name: "KANJI BANNER",
            art: base("STATION PLATES") + 5,
            rect: (0, 0, 56, 32),
        },
        Zoom {
            name: "NAVIGATOR",
            art: base("NAVIGATOR"),
            rect: (0, 0, 56, 56),
        },
        Zoom {
            name: "MASCOT",
            art: base("NAVIGATOR") + 4,
            rect: (0, 0, 56, 56),
        },
    ];
    for z in &zooms {
        let a = &arts[z.art];
        eprintln!("zoom {}: art {}×{}", z.name, a.w, a.h);
    }
    let zooms: Vec<Zoom> = zooms
        .into_iter()
        .map(|mut z| {
            let a = &arts[z.art];
            let (x, y, w, h) = z.rect;
            z.rect = (
                x.min(a.w - 1),
                y.min(a.h - 1),
                w.min(a.w - x.min(a.w - 1)),
                h.min(a.h - y.min(a.h - 1)),
            );
            z
        })
        .collect();
    zoom_sheet(&zooms, &m4, &r4).save(&out.join("zoom_x4.png"))?;

    for path in &frames {
        frame(path, &data, &out, &tools, &keep)?;
    }
    println!("written to {}", out.display());
    Ok(())
}

/// Recomposes one dumped frame with every method (`<stem>_<method>.png`, window size) and a
/// sheet of crops of the HUD corners at 1:1 (`<stem>_crops.png`).
fn frame(
    path: &Path,
    data: &Data<'_>,
    out: &Path,
    tools: &Tools,
    keep: &dyn Fn(&Up) -> bool,
) -> Result<()> {
    let src = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
    let dump = parse_dump(&src)?;
    if dump.widgets > 0 {
        eprintln!(
            "{}: {} text widgets are not recomposed (shoot with --lang ja)",
            path.display(),
            dump.widgets
        );
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("frame");
    let stem = name.strip_suffix(".layer.txt").unwrap_or(name);
    let world_path = path.with_file_name(format!("{stem}.world.ppm"));
    let (ww, wh, world) = read_ppm(&world_path)?;
    let arts = dump_arts(&dump, data)?;
    let dir = out.join("frames");
    std::fs::create_dir_all(&dir)?;
    // The runtime's own multiple first (what the game draws now), then the ×4 methods.
    let mut methods = vec![Up::Nearest(dump.scale as u32)];
    methods.extend(
        methods_x4()
            .into_iter()
            .filter(|m| !matches!(m, Up::SharpBilinear { .. })),
    );
    let mut shots = Vec::new();
    for m in methods.into_iter().filter(|m| keep(m)) {
        let ups = match upscale_arts(&arts, &m, tools) {
            Ok(u) => u,
            Err(e) => {
                eprintln!("{}: skipped ({e:#})", m.name());
                continue;
            }
        };
        let canvas = compose(&dump, &ups, m.factor() as usize);
        let img = present_over(&canvas, &world, ww, wh);
        img.save(&dir.join(format!("{stem}_{}.png", m.name())))?;
        eprintln!("{stem}: {}", m.name());
        shots.push((m, img));
    }
    // Crops at 1:1: the top-left clocks, the top-right plates (the black panel's height), the
    // speedometer and the bottom centre (navigator hint).
    let crops = [
        (0, 0, ww * 3 / 10, wh * 113 / 480),
        (ww * 7 / 10, 0, ww * 3 / 10, wh * 113 / 480),
        (0, wh * 11 / 16, ww / 5, wh * 5 / 16),
        (ww * 3 / 10, wh * 11 / 16, ww * 2 / 5, wh * 5 / 16),
    ];
    let pad = 12;
    let label_h = TEXT_H + 2 * pad;
    let row_h = crops.iter().map(|c| c.3).max().unwrap_or(0);
    let width = crops.iter().map(|c| c.2 + pad).sum::<usize>() + pad;
    let mut sheet = Img::new(
        width,
        shots.len() * (row_h + label_h),
        [SHEET_BG[0], SHEET_BG[1], SHEET_BG[2], 255],
    );
    for (i, (m, img)) in shots.iter().enumerate() {
        let y = i * (row_h + label_h);
        let label = if i == 0 {
            format!("{} (THE GAME NOW)", m.label())
        } else {
            m.label()
        };
        text(&mut sheet, pad, y + pad, &label, LABEL);
        let mut x = pad;
        for &(cx, cy, cw, ch) in &crops {
            sheet.over(&img.crop(cx, cy, cw, ch), x as i64, (y + label_h) as i64);
            x += cw + pad;
        }
    }
    sheet.save(&out.join(format!("{stem}_crops.png")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale2x_rounds_a_diagonal_and_keeps_values() {
        // 1 above the anti-diagonal, 2 below: Scale2x fills the inner corners.
        let src = [1u8, 1, 2, 1, 2, 2, 2, 2, 2];
        let out = scale2x(&src, 3, 3);
        assert_eq!(out.len(), 36);
        assert!(out.iter().all(|&v| v == 1 || v == 2));
        // Centre pixel (2) has 1 above and left: its top-left corner becomes 1.
        assert_eq!(out[2 * 6 + 2], 1);
        assert_eq!(out[3 * 6 + 3], 2);
        // A flat image stays flat.
        assert_eq!(scale2x(&[7u8; 4], 2, 2), vec![7u8; 16]);
    }

    #[test]
    fn present_at_a_whole_multiple_is_nearest() {
        let src = Img {
            w: 2,
            h: 1,
            px: vec![[255, 0, 0, 255], [0, 0, 255, 255]],
        };
        let out = unpremultiply(&present(&src, 6, 3), 6, 3);
        for y in 0..3 {
            assert_eq!(out.px[y * 6], [255, 0, 0, 255]);
            assert_eq!(out.px[y * 6 + 2], [255, 0, 0, 255]);
            assert_eq!(out.px[y * 6 + 3], [0, 0, 255, 255]);
        }
    }

    #[test]
    fn present_tests_alpha_and_blends_edges_only() {
        // Transparent texels with a colour do not leak (alpha test before the blend).
        let src = Img {
            w: 2,
            h: 1,
            px: vec![[255, 255, 255, 255], [0, 255, 0, 0]],
        };
        let out = present(&src, 5, 1);
        assert_eq!(out[0], [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(out[4], [0.0; 4]);
        // The edge pixel is a partial white, never green.
        let edge = out[2];
        assert!(edge[3] > 0.0 && edge[3] < 1.0 && edge[1] == edge[0]);
    }

    #[test]
    fn montage_wraps_rows() {
        let (pos, w, h) = montage(&[(100, 10), (100, 20), (100, 5)]);
        assert_eq!(pos, vec![(0, 0), (104, 0), (0, 24)]);
        assert_eq!((w, h), (204, 29));
    }

    #[test]
    fn trim_drops_transparent_edges() {
        let a = Art {
            w: 4,
            h: 3,
            idx: vec![0, 0, 0, 0, 0, 5, 6, 0, 0, 0, 0, 0],
            pal: Palette([0; 256]),
        };
        let t = trim(&a);
        assert_eq!((t.w, t.h, t.idx), (2, 1, vec![5, 6]));
    }

    #[test]
    fn colour_methods_rebuild_alpha_from_the_key_mask() {
        // A 2×2 opaque block in a 4×4 sprite; xBRZ colour, mask from index 0 only.
        let mut pal = [0u32; 256];
        pal[1] = 0x00_00_ff_00; // B, R, G bytes: red
        let mut idx = vec![0u8; 16];
        for i in [5, 6, 9, 10] {
            idx[i] = 1;
        }
        let a = Art {
            w: 4,
            h: 4,
            idx,
            pal: Palette(pal),
        };
        let tools = Tools {
            work: PathBuf::from("unused"),
            esrgan: None,
            waifu2x: None,
        };
        let up = &upscale_arts(std::slice::from_ref(&a), &Up::Xbrz(2), &tools).unwrap()[0];
        assert_eq!((up.w, up.h), (8, 8));
        // Opaque pixels are pure red (no black from the key), transparent ones alpha 0.
        for p in &up.px {
            assert!(p[3] == 0 || p[3] == 255);
            if p[3] == 255 {
                assert_eq!(&p[..3], &[255, 0, 0]);
            }
        }
        assert_eq!(up.px[0][3], 0);
        assert_eq!(up.px[3 * 8 + 3][3], 255);
    }

    #[test]
    fn indexed_methods_only_produce_source_indices() {
        let mut pal = [0u32; 256];
        pal[1] = 0x00_ff_ff_ff;
        pal[2] = 0x00_00_00_00;
        let idx: Vec<u8> = (0..36)
            .map(|i| if i % 6 <= i / 6 { 1 } else { 2 })
            .collect();
        let a = Art {
            w: 6,
            h: 6,
            idx,
            pal: Palette(pal),
        };
        let colours: Vec<[u8; 3]> = a.idx.iter().map(|&i| a.colour(i)).collect();
        let out = xbrz::scale_indexed(&colours, &a.idx, 6, 6, 4);
        assert!(out.iter().all(|&i| i == 1 || i == 2));
        // Unlike nearest neighbour, the staircase gets sub-pixel steps.
        assert_ne!(out, nearest(&a.idx, 6, 6, 4));
    }

    #[test]
    fn dump_round_trip() {
        let mut text = String::from(
            "canvas 2560 1440 scale 3 wide 640 shake 0 0\nwidgets 0\n\
             slot 129 tbl 1354 x 16 y 345 rect 0 0 0 0 pal 4 layer 3 shift 0\n\
             fan 0 argb ff000000 84 389 80 351 76 389\npalette 4",
        );
        for i in 0..256 {
            text.push_str(&format!(" {i:08x}"));
        }
        let d = parse_dump(&text).unwrap();
        assert_eq!((d.canvas, d.scale), ((2560, 1440), 3));
        assert_eq!(d.slots[0].tbl, 1354);
        assert_eq!(d.fans[0].points, vec![(84, 389), (80, 351), (76, 389)]);
        assert_eq!(d.palettes[&4].0[255], 255);
        assert!(parse_dump("slot 1").is_err());
    }
}

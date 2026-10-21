//! The 2D layer composed on the GPU (docs/performance.md, "GPU 2D compositing"): the CPU only
//! lists the layer's draws ([`GpuFrame`]), and a compute shader (`gpu2d.rs`) works out every
//! canvas pixel from them, with the same integer arithmetic as the CPU compose
//! ([`SpriteLayer::compose`]), which stays the reference.
//!
//! Palette-indexed art is drawn from indices on the GPU: tiles and tilemaps ([`BankArt`]),
//! upscaled images, paper fills and translated text, coloured with the palettes of the frame,
//! so a palette fade only changes the palettes. The port's widgets whose pixels come from
//! floating-point maths on the CPU (label text, legends, resampled sprites, triangles, pictures)
//! are rasterised by the same CPU code into small images ("rasters") that do not depend on
//! what is under them, kept while they stay the same; the shader only blends them, in integer
//! arithmetic (or, for resampled sprites, the CPU's exact `f32` steps).
//!
//! Each draw is a record of [`DRAW_WORDS`] words: its kind, the canvas box it may change, the
//! columns it keeps out of on some rows (a framed slot, [`SpriteLayer::frame`]), and its
//! parameters. Every pixel applies the draws covering it in the CPU compose's order.

use super::legend::{self, LegendPart, Recorder};
use super::paper::PaperFill;
use super::*;
use crate::text::RenderedPart;
use std::sync::Weak;

/// Words per draw record.
pub const DRAW_WORDS: usize = 32;
/// The first parameter word of a record.
const PARAM: usize = 9;

/// Draw kinds (word 0 of a record); the shader (`gpu2d.wgsl`) has the same numbers.
pub mod kind {
    /// A slot's tiles or upscaled image.
    pub const SLOT: u32 = 1;
    /// A slot's paper fill (`wide_menus`).
    pub const PAPER: u32 = 2;
    /// One part of the translated text over a slot.
    pub const PART: u32 = 3;
    /// A rectangle in one colour.
    pub const FILL: u32 = 4;
    /// The pixels of a byte mask in one colour (triangles, fans).
    pub const MASK: u32 = 5;
    /// A label text widget: backdrop, outline and letters.
    pub const TEXT: u32 = 6;
    /// One group of a legend's blends.
    pub const BLEND: u32 = 7;
    /// An RGBA picture blended over the canvas.
    pub const IMAGE: u32 = 8;
    /// A resampled sprite widget.
    pub const RESAMPLE: u32 = 9;
}

/// Data a draw reads, uploaded once and shared by identity.
#[derive(Clone)]
pub enum Blob {
    Bytes(Arc<[u8]>),
    Words(Arc<[u32]>),
    /// Upscaled palette indices (`hud_upscale`), one byte per canvas pixel.
    Hires(Arc<Hires>),
}

impl Blob {
    /// Identifies the data: its address and length (the data is kept alive while in use).
    pub fn id(&self) -> (usize, usize) {
        match self {
            Blob::Bytes(a) => (a.as_ptr() as usize, a.len()),
            Blob::Words(a) => (a.as_ptr() as usize, a.len() * 4),
            Blob::Hires(h) => (Arc::as_ptr(h) as usize, h.idx.len()),
        }
    }

    /// The bytes to upload (words little-endian).
    pub fn bytes(&self) -> &[u8] {
        match self {
            Blob::Bytes(a) => a,
            Blob::Words(a) => bytemuck::cast_slice(a),
            Blob::Hires(h) => &h.idx,
        }
    }
}

/// The bank's tiles and tilemaps as words: tile `i`'s 256 indices at word `64 i` (four per
/// word, little-endian), then every tilemap's cells, one tile id per word.
pub struct BankArt {
    /// [`SpriteBank`]'s art id.
    pub id: u64,
    pub words: Vec<u32>,
    pub tiles: u32,
    /// The first word of each tilemap.
    pub maps: Vec<u32>,
}

impl BankArt {
    fn new(bank: &SpriteBank) -> Self {
        let mut words = vec![0u32; bank.tiles.len() * TILE_PIXELS / 4];
        for (i, t) in bank.tiles.iter().enumerate() {
            if let Some(t) = t {
                for (k, w) in t.as_chunks::<4>().0.iter().enumerate() {
                    words[i * TILE_PIXELS / 4 + k] = u32::from_le_bytes(*w);
                }
            }
        }
        let mut maps = Vec::with_capacity(bank.tilemaps.len());
        for m in &bank.tilemaps {
            maps.push(words.len() as u32);
            words.extend(m.tiles.iter().map(|&t| u32::from(t)));
        }
        Self {
            id: bank.art,
            words,
            tiles: bank.tiles.len() as u32,
            maps,
        }
    }
}

/// One composed 2D frame for the GPU: what [`SpriteLayer::compose`] would draw.
pub struct GpuFrame {
    /// Canvas size in pixels and the canvas multiple.
    pub size: (u32, u32),
    pub scale: u32,
    /// [`SpriteLayer::put_backdrop`]: the colour (`r | g << 8 | b << 16`) and the canvas
    /// columns `[x0, x1)` and rows `[0, y1)` it goes under.
    pub backdrop: Option<[u32; 4]>,
    pub draws: Vec<[u32; DRAW_WORDS]>,
    pub blobs: Vec<Blob>,
    /// `(draw, word, blob)`: the word is set to the blob's first word in the GPU's blob buffer.
    pub patches: Vec<(u32, u8, u32)>,
    pub palettes: Vec<Option<Palette>>,
    pub art: Arc<BankArt>,
}

impl GpuFrame {
    /// Whether `other` draws exactly the same pixels from the same data.
    pub fn same(&self, other: &GpuFrame) -> bool {
        self.size == other.size
            && self.scale == other.scale
            && self.backdrop == other.backdrop
            && self.draws == other.draws
            && self.patches == other.patches
            && self.blobs.len() == other.blobs.len()
            && self
                .blobs
                .iter()
                .zip(&other.blobs)
                .all(|(a, b)| a.id() == b.id())
            && Arc::ptr_eq(&self.art, &other.art)
            && self.palettes == other.palettes
    }
}

/// Rasters and converted data kept between frames (by their inputs), so a frame only builds
/// what changed.
#[derive(Default)]
pub struct GpuCache {
    art: Option<Arc<BankArt>>,
    paper: Vec<(Weak<PaperFill>, Blob)>,
    /// Translated text parts by the address of their list, for one bank generation: the
    /// regions (four words each) and each part's pixels.
    parts_gen: u64,
    parts: Vec<(usize, Blob, Vec<PartBlobs>)>,
    text: Vec<(TextKey, Blob)>,
    legends: Vec<(LegendKey, Arc<[LegendGroup]>)>,
    sprites: Vec<(SpriteKey, Arc<Resampled>)>,
    /// Colours of resampled sprites, by sprite (its address) and palette.
    sprite_colours: Vec<(ColoursKey, Blob)>,
    images: Vec<(ImageKey, Blob)>,
}

#[derive(PartialEq)]
struct TextKey {
    generation: u64,
    text: String,
    style: TextStyle,
    size: (usize, usize),
    scale: usize,
    backdrop: bool,
    outline: bool,
}

#[derive(PartialEq)]
struct LegendKey {
    generation: u64,
    parts: Vec<LegendPart>,
    style: TextStyle,
    rgb: legend::Rgb,
    backdrop: Option<legend::Rgb>,
    rect: [usize; 4],
    canvas: (usize, usize),
}

/// One group of a legend's blends: its colour, box and coverage bytes over the box.
struct LegendGroup {
    rgb: legend::Rgb,
    bbox: [usize; 4],
    cov: Blob,
}

/// A resampled sprite (the address of its [`Resampled`]) and a palette's colours.
type ColoursKey = (usize, Vec<[u8; 4]>);

#[derive(PartialEq)]
struct SpriteKey {
    generation: u64,
    tilemap: u16,
    src: [usize; 4],
    z: u32,
}

/// A resampled sprite widget independent of its palette: for each canvas pixel of the
/// `w × h` sprite, the id of its list of art pixels (palette index, weight) in the order
/// [`resample_at`] sums them (`u32::MAX`: none), and the distinct lists.
struct Resampled {
    ids: Blob,
    w: usize,
    lists: Vec<Vec<(u8, f32)>>,
    z: f32,
}

impl Resampled {
    fn new(art: &Art<'_>, z: f32, (w, h): (usize, usize)) -> Self {
        let [ax, ay, sw, sh] = art.rect;
        let mut known: std::collections::HashMap<Vec<(u8, u32)>, u32> = Default::default();
        let mut lists = Vec::new();
        let mut ids = vec![u32::MAX; w * h];
        for ry in 0..h {
            let rows: Vec<(usize, f32)> = footprint(ry as i64, z, sh).collect();
            for rx in 0..w {
                let mut list = Vec::new();
                for (col, wx) in footprint(rx as i64, z, sw) {
                    for &(row, wy) in &rows {
                        let index = art.px[(ay + row) * art.stride + ax + col];
                        if index != 0 {
                            list.push((index, wx * wy));
                        }
                    }
                }
                if list.is_empty() {
                    continue;
                }
                let key: Vec<(u8, u32)> = list.iter().map(|&(i, w)| (i, w.to_bits())).collect();
                ids[ry * w + rx] = *known.entry(key).or_insert_with(|| {
                    lists.push(list);
                    (lists.len() - 1) as u32
                });
            }
        }
        Self {
            ids: Blob::Words(ids.into()),
            w,
            lists,
            z,
        }
    }

    /// Per list, as [`resample_at`] works it out with `colours`: the colour and the coverage
    /// rounded to a byte, the coverage, its complement (four words).
    fn colours(&self, colours: &[[u8; 4]]) -> Blob {
        let area = 1.0 / (self.z * self.z);
        let mut out = Vec::with_capacity(self.lists.len() * 4);
        for list in &self.lists {
            let mut sum = [0.0f32; 4];
            for &(index, w) in list {
                for (s, v) in sum.iter_mut().zip(colours[usize::from(index)]) {
                    *s += f32::from(v) * w;
                }
            }
            if sum[3] <= 0.0 {
                out.extend([0, 0, 0, 0]);
                continue;
            }
            let a = (sum[3] / 255.0 / area).min(1.0);
            let c = [0, 1, 2].map(|i| (sum[i] * 255.0 / sum[3]).round() as u8);
            let a255 = (a * 255.0).round() as u8;
            out.extend([
                pack([c[0], c[1], c[2], a255]),
                a.to_bits(),
                (1.0 - a).to_bits(),
                1,
            ]);
        }
        Blob::Words(out.into())
    }
}

struct ImageKey {
    image: Arc<WidgetImage>,
    size: (u32, u32),
}

impl PartialEq for ImageKey {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.image, &other.image) && self.size == other.size
    }
}

/// Looks `key` up in a small most-recently-used list of at most `cap` entries, making it with
/// `make` when absent.
fn cached<K: PartialEq, V: Clone>(
    list: &mut Vec<(K, V)>,
    key: K,
    cap: usize,
    make: impl FnOnce() -> V,
) -> V {
    if let Some(i) = list.iter().position(|(k, _)| *k == key) {
        let e = list.remove(i);
        let v = e.1.clone();
        list.push(e);
        return v;
    }
    let v = make();
    if list.len() >= cap {
        list.remove(0);
    }
    list.push((key, v.clone()));
    v
}

/// Packs RGBA bytes into a word (`r` lowest).
fn pack(c: [u8; 4]) -> u32 {
    u32::from_le_bytes(c)
}

/// The records being built.
struct Builder {
    draws: Vec<[u32; DRAW_WORDS]>,
    blobs: Vec<Blob>,
    patches: Vec<(u32, u8, u32)>,
}

impl Builder {
    /// Adds a draw of `kind` over `bbox` with parameters `params` (word [`PARAM`] on); the
    /// parameters listed in `blobs` are patched to their blob's place. `frame` is
    /// `[x0, x1, y0, y1]`: rows `y0..y1` are only drawn in columns `x0..x1`.
    fn push(
        &mut self,
        kind: u32,
        bbox: [usize; 4],
        frame: [u32; 4],
        params: &[u32],
        blobs: &[(usize, Blob)],
    ) {
        if bbox[0] >= bbox[2] || bbox[1] >= bbox[3] {
            return;
        }
        let mut d = [0u32; DRAW_WORDS];
        d[0] = kind;
        for (k, v) in bbox.iter().enumerate() {
            d[1 + k] = *v as u32;
        }
        d[5..9].copy_from_slice(&frame);
        d[PARAM..PARAM + params.len()].copy_from_slice(params);
        let n = self.draws.len() as u32;
        for (word, blob) in blobs {
            let id = blob.id();
            let i = match self.blobs.iter().position(|b| b.id() == id) {
                Some(i) => i,
                None => {
                    self.blobs.push(blob.clone());
                    self.blobs.len() - 1
                }
            };
            self.patches.push((n, (PARAM + word) as u8, i as u32));
        }
        self.draws.push(d);
    }
}

/// `v` as a word (two's complement for negative values).
fn w(v: i32) -> u32 {
    v as u32
}

/// The bounding box of `pixels` (`(x, y)`), `[x0, y0, x1, y1]`.
fn bounds(pixels: impl Iterator<Item = (usize, usize)>) -> Option<[usize; 4]> {
    pixels.fold(None, |b, (x, y)| {
        Some(match b {
            None => [x, y, x + 1, y + 1],
            Some([x0, y0, x1, y1]) => [x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1)],
        })
    })
}

/// A byte mask of `pixels` over their bounding box: the box and the mask.
fn mask_of(pixels: &[(usize, usize)]) -> Option<([usize; 4], Blob)> {
    let b = bounds(pixels.iter().copied())?;
    let w = b[2] - b[0];
    let mut m = vec![0u8; w * (b[3] - b[1])];
    for &(x, y) in pixels {
        m[(y - b[1]) * w + x - b[0]] = 1;
    }
    Some((b, Blob::Bytes(m.into())))
}

impl SpriteLayer {
    /// The GPU frame of this layer, from the slots as [`SpriteLayer::prepare`] prepared them
    /// (for the whole canvas `c`), with `backdrop` under it ([`SpriteLayer::put_backdrop`]).
    pub(super) fn gpu_frame(
        &self,
        bank: &SpriteBank,
        slots: &[Option<SlotDraw<'_>>],
        c: Canvas,
        cache: &mut GpuCache,
        backdrop: Option<[u8; 3]>,
    ) -> GpuFrame {
        let art = match &cache.art {
            Some(a) if a.id == bank.art => a.clone(),
            _ => {
                let a = Arc::new(BankArt::new(bank));
                cache.art = Some(a.clone());
                a
            }
        };
        if cache.parts_gen != bank.generation {
            cache.parts.clear();
            cache.parts_gen = bank.generation;
        }
        cache.paper.retain(|(p, _)| p.strong_count() > 0);
        let mut b = Builder {
            draws: Vec::new(),
            blobs: Vec::new(),
            patches: Vec::new(),
        };
        let (sx, sy) = (i32::from(self.shake.0), i32::from(self.shake.1));
        for layer in (0..LAYERS).rev() {
            for i in (0..SLOT_COUNT).rev() {
                if self.slots[i].layer != layer {
                    continue;
                }
                let Some(d) = &slots[i] else { continue };
                let framed = self.wide > 0 && self.framed.contains(&i);
                self.gpu_slot(&art, i, d, framed, c, cache, &mut b);
            }
            for wd in self.widgets.iter().filter(|w| w.layer == layer) {
                let shift = self.anchor_shift(wd.anchor);
                self.gpu_widget(bank, wd, sx, sy, shift, c, cache, &mut b);
            }
        }
        for fan in &self.fans {
            let shift = self.anchor_shift(fan.anchor);
            let n = usize::from(fan.count);
            let s = c.s as f32;
            let pts: Vec<[f32; 2]> = fan.points[..n]
                .iter()
                .map(|p| {
                    [
                        (p[0] + sx) as f32 * s + shift as f32,
                        (p[1] + sy) as f32 * s,
                    ]
                })
                .collect();
            let mut px = Vec::new();
            for k in 1..n.saturating_sub(1) {
                triangle_cover([pts[0], pts[k], pts[k + 1]], c.clip, |x, y| px.push((x, y)));
            }
            let [_, r, g, bl] = fan.argb.to_be_bytes();
            if let Some((bbox, m)) = mask_of(&px) {
                let params = [pack([r, g, bl, 255]), w(bbox[0] as i32), w(bbox[1] as i32)];
                b.push(kind::MASK, bbox, [0; 4], &params, &[(3, m)]);
            }
        }
        let backdrop = backdrop.map(|[r, g, bl]| {
            let s = self.scale.max(1);
            let x0 = (self.wide / 2).min(c.w);
            let x1 = (self.wide / 2 + CANVAS_W * s).min(c.w);
            let y1 = (CANVAS_H * s).min(c.h);
            [pack([r, g, bl, 0]), x0 as u32, x1 as u32, y1 as u32]
        });
        GpuFrame {
            size: (c.w as u32, c.h as u32),
            scale: c.s as u32,
            backdrop,
            draws: b.draws,
            blobs: b.blobs,
            patches: b.patches,
            palettes: self.palettes.clone(),
            art,
        }
    }

    /// The records of slot `i`: its tiles (one record per run of columns), its paper fill and
    /// its translated text, in the order [`SlotDraw::draw`] draws them.
    #[allow(clippy::too_many_arguments)]
    fn gpu_slot(
        &self,
        art: &BankArt,
        i: usize,
        d: &SlotDraw<'_>,
        framed: bool,
        c: Canvas,
        cache: &mut GpuCache,
        b: &mut Builder,
    ) {
        let Some(bbox) = d.bbox(c) else { return };
        let s = c.s as i32;
        let frame = if framed {
            let top = d.sy0 * s;
            let rows = d.rows * TILE_SIZE as i32 * s;
            let (x0, x1) = (
                d.shift.clamp(0, c.w as i32),
                (d.shift + (CANVAS_W * c.s) as i32).clamp(0, c.w as i32),
            );
            [
                w(x0),
                w(x1),
                w(top.clamp(0, c.h as i32)),
                w((top + rows).clamp(0, c.h as i32)),
            ]
        } else {
            [0; 4]
        };
        let slot = &self.slots[i];
        let tbl = slot.tilemap as usize;
        let (mw, mh) = (i32::from(d.map.width), i32::from(d.map.height));
        // Translated text: its regions (for the art it covers) and its parts.
        let parts = d.text.map(|parts| {
            let key = parts.as_ptr() as usize;
            match cache.parts.iter().find(|(k, _, _)| *k == key) {
                Some((_, rects, blobs)) => (rects.clone(), blobs.clone()),
                None => {
                    let rects: Vec<u32> =
                        parts.iter().flat_map(|p| p.region.map(u32::from)).collect();
                    let rects = Blob::Words(rects.into());
                    let blobs: Vec<PartBlobs> = parts.iter().map(part_blobs).collect();
                    cache.parts.push((key, rects.clone(), blobs.clone()));
                    (rects, blobs)
                }
            }
        });
        let (repeat_start, repeat_period) = d.repeat.map_or((0, 0), |r| (r.start, r.period));
        let (wide, first) = match d.view {
            View::Whole => (0, 0),
            View::Wide { first, .. } => (1, first),
        };
        let mut cols = d.cols.clone();
        cols.sort_unstable();
        let mut k = 0;
        while k < cols.len() {
            // A run of consecutive columns.
            let mut e = k + 1;
            while e < cols.len() && cols[e] == cols[e - 1] + 1 {
                e += 1;
            }
            let mut params = [0u32; 23];
            params[..15].copy_from_slice(&[
                w(d.sx0),
                w(d.sy0),
                w(d.shift),
                w(cols[k]),
                w(cols[e - 1] + 1),
                w(d.h),
                w(d.w),
                w(d.c0),
                w(d.r0),
                w(mw),
                w(mh),
                art.maps.get(tbl).copied().unwrap_or(0),
                u32::from(slot.palette),
                w(repeat_start),
                w(repeat_period),
            ]);
            let mut blobs = Vec::new();
            if let Some(hi) = &d.hires {
                params[16] = hi.w as u32;
                params[17] = wide;
                params[18] = w(first);
                params[21] = 1;
                params[22] = hi.h as u32;
                blobs.push((15, Blob::Hires(hi.clone())));
            }
            if let Some((rects, list)) = &parts {
                params[20] = list.len() as u32;
                blobs.push((19, rects.clone()));
            }
            b.push(kind::SLOT, bbox, frame, &params, &blobs);
            k = e;
        }
        if let Some(p) = &d.paper {
            let blob = match cache
                .paper
                .iter()
                .find(|(q, _)| std::ptr::eq(q.as_ptr(), Arc::as_ptr(p)))
            {
                Some((_, blob)) => blob.clone(),
                None => {
                    let (_, _, _, sides) = p.sides();
                    let words: Vec<u32> = sides
                        .iter()
                        .flat_map(|(_, _, px)| px.iter().map(|&c| pack(c)))
                        .collect();
                    let blob = Blob::Words(words.into());
                    cache.paper.push((Arc::downgrade(p), blob.clone()));
                    blob
                }
            };
            let (width, height, ramp, [(lf, lc, lpx), (rf, rc, _)]) = p.sides();
            let (a, bb) = p.inner();
            let left_end = ((d.sx0 + a) * s + d.shift).clamp(0, c.w as i32);
            let right_start = ((d.sx0 + bb) * s + d.shift).clamp(0, c.w as i32);
            let params = [
                w(d.sx0),
                w(d.sy0),
                w(d.shift),
                w(left_end),
                w(right_start),
                w(width),
                w(height),
                w(ramp),
                0,
                w(lf),
                lc as u32,
                lpx.len() as u32,
                w(rf),
                rc as u32,
            ];
            b.push(kind::PAPER, bbox, frame, &params, &[(8, blob)]);
        }
        if let (Some(list), Some((_, blobs))) = (d.text, &parts) {
            let (ox, oy) = (d.sx0 * s + d.shift, d.sy0 * s);
            for (p, blob) in list.iter().zip(blobs) {
                let [rx, ry, rw, rh] = p.region.map(i32::from);
                let pb = clamp_box([ox + rx, oy + ry, ox + rx + rw, oy + ry + rh], c);
                let Some(pb) = pb else { continue };
                // Words 6, 8 and 9: the base, coverage and (flag 1 in word 10) text colour
                // blobs; without colours per pixel, word 10 holds the colour << 8.
                let params = [
                    w(ox),
                    w(oy),
                    w(rx),
                    w(ry),
                    w(rw),
                    w(rh),
                    0,
                    u32::from(slot.palette),
                    0,
                    0,
                    match blob.fg_map {
                        Some(_) => 1,
                        None => u32::from(p.fg) << 8,
                    },
                ];
                let mut data = vec![(6, blob.base.clone()), (8, blob.coverage.clone())];
                if let Some(m) = &blob.fg_map {
                    data.push((9, m.clone()));
                }
                b.push(kind::PART, pb, frame, &params, &data);
            }
        }
    }

    /// The records of a widget, as [`SpriteLayer::draw_widget`] draws it.
    #[allow(clippy::too_many_arguments)]
    fn gpu_widget(
        &self,
        bank: &SpriteBank,
        wd: &Widget,
        sx: i32,
        sy: i32,
        shift: i32,
        c: Canvas,
        cache: &mut GpuCache,
        b: &mut Builder,
    ) {
        let palette = |p: u8| self.palettes.get(usize::from(p)).and_then(Option::as_ref);
        let s = c.s as f32;
        let [x, y, rw, rh] = wd.rect;
        let to_px = |v: f32, shake: i32, offset: i32, max: usize| {
            (((v + shake as f32) * s).round() as i64 + i64::from(offset)).clamp(0, max as i64)
                as usize
        };
        let (x0, x1) = (to_px(x, sx, shift, c.w), to_px(x + rw, sx, shift, c.w));
        let (y0, y1) = (to_px(y, sy, 0, c.h), to_px(y + rh, sy, 0, c.h));
        let shiftf = shift as f32;
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let rect = [x0, y0, x1, y1];
        match &wd.kind {
            WidgetKind::Fill(p) => {
                let Some(pal) = palette(p.palette) else {
                    return;
                };
                b.push(kind::FILL, rect, [0; 4], &[pack(pal.rgba(p.index))], &[]);
            }
            WidgetKind::DownArrow(p) => {
                let Some(pal) = palette(p.palette) else {
                    return;
                };
                let (l, r) = (
                    (x + sx as f32) * s + shiftf,
                    (x + rw + sx as f32) * s + shiftf,
                );
                let (t, bt) = ((y + sy as f32) * s, (y + rh + sy as f32) * s);
                let mut px = Vec::new();
                triangle_cover([[l, t], [r, t], [(l + r) / 2.0, bt]], c.clip, |x, y| {
                    px.push((x, y));
                });
                if let Some((bbox, m)) = mask_of(&px) {
                    let params = [
                        pack(pal.rgba(p.index)),
                        w(bbox[0] as i32),
                        w(bbox[1] as i32),
                    ];
                    b.push(kind::MASK, bbox, [0; 4], &params, &[(3, m)]);
                }
            }
            WidgetKind::Sprite {
                tilemap,
                src,
                scale,
                palette: p,
            } => {
                let (Some(pal), Some(map)) =
                    (palette(*p), bank.tilemaps.get(usize::from(*tilemap)))
                else {
                    return;
                };
                let aw = usize::from(map.width) * TILE_SIZE;
                let ah = usize::from(map.height) * TILE_SIZE;
                let [ax, ay, sw, sh] = src.map(usize::from);
                let (sw, sh) = (sw.min(aw.saturating_sub(ax)), sh.min(ah.saturating_sub(ay)));
                let z = scale * s;
                if z.is_nan() || z <= 0.0 || sw == 0 || sh == 0 {
                    return;
                }
                let (dw, dh) = (sw as f32 * z, sh as f32 * z);
                let cx = ((x + sx as f32 + rw / 2.0) * s + shiftf).round() as i64
                    - (dw / 2.0).floor() as i64;
                let cy =
                    ((y + sy as f32 + rh / 2.0) * s).round() as i64 - (dh / 2.0).floor() as i64;
                let colours: Vec<[u8; 4]> = (0..=255u8).map(|i| pal.rgba(i)).collect();
                // The whole resampled sprite, `dwc × dhc` canvas pixels from its top left.
                let (dwc, dhc) = ((sw as f32 * z).ceil() as i64, (sh as f32 * z).ceil() as i64);
                let key = SpriteKey {
                    generation: bank.generation,
                    tilemap: *tilemap,
                    src: [ax, ay, sw, sh],
                    z: z.to_bits(),
                };
                let sprite = cached(&mut cache.sprites, key, 16, || {
                    let art = bank.map_art(*tilemap);
                    let src = Art {
                        px: &art,
                        stride: aw,
                        rect: [ax, ay, sw, sh],
                    };
                    Arc::new(Resampled::new(&src, z, (dwc as usize, dhc as usize)))
                });
                let ckey = (Arc::as_ptr(&sprite) as usize, colours);
                let table = cached(&mut cache.sprite_colours, ckey, 32, || {
                    sprite.colours(&(0..=255u8).map(|i| pal.rgba(i)).collect::<Vec<_>>())
                });
                // A sprite dropped from the cache may leave its colours behind under an
                // address another one reuses: keep only those of cached sprites.
                cache.sprite_colours.retain(|((p, _), _)| {
                    cache
                        .sprites
                        .iter()
                        .any(|(_, s)| Arc::as_ptr(s) as usize == *p)
                });
                let bbox = [
                    (x0 as i64).max(cx),
                    (y0 as i64).max(cy),
                    (x1 as i64).min(cx + dwc),
                    (y1 as i64).min(cy + dhc),
                ];
                if bbox[0] >= bbox[2] || bbox[1] >= bbox[3] {
                    return;
                }
                let bbox = bbox.map(|v| v as usize);
                let params = [w(cx as i32), w(cy as i32), sprite.w as u32, 0, 0];
                let blobs = [(3, sprite.ids.clone()), (4, table)];
                b.push(kind::RESAMPLE, bbox, [0; 4], &params, &blobs);
            }
            WidgetKind::Image(img) => {
                let (iw, ih) = (img.image.width as f32, img.image.height as f32);
                if iw <= 0.0 || ih <= 0.0 {
                    return;
                }
                let k = (rw * s / iw).min(rh * s / ih);
                let (dw, dh) = ((iw * k).round() as i64, (ih * k).round() as i64);
                if dw <= 0 || dh <= 0 {
                    return;
                }
                let left =
                    ((x + sx as f32) * s + shiftf + (rw * s - dw as f32) / 2.0).round() as i64;
                let top = ((y + sy as f32) * s + (rh * s - dh as f32) / 2.0).round() as i64;
                let key = ImageKey {
                    image: img.clone(),
                    size: (dw as u32, dh as u32),
                };
                let blob = cached(&mut cache.images, key, 8, || {
                    img.with_scaled(dw as u32, dh as u32, |pic| {
                        let words: Vec<u32> = pic
                            .rgba
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .map(|&p| pack(p))
                            .collect();
                        Blob::Words(words.into())
                    })
                });
                let bbox = [
                    (x0 as i64).max(left),
                    (y0 as i64).max(top),
                    (x1 as i64).min(left + dw),
                    (y1 as i64).min(top + dh),
                ];
                if bbox[0] >= bbox[2] || bbox[1] >= bbox[3] {
                    return;
                }
                let (matte, has) = match img.matte {
                    Some([r, g, bl]) => (pack([r, g, bl, 255]), 1),
                    None => (0, 0),
                };
                let params = [w(left as i32), w(top as i32), dw as u32, 0, matte, has];
                b.push(
                    kind::IMAGE,
                    bbox.map(|v| v as usize),
                    [0; 4],
                    &params,
                    &[(3, blob)],
                );
            }
            WidgetKind::Legend { parts, style } => {
                let Some(font) = bank.legend_font.as_ref().or(bank.label_font(style.font)) else {
                    return;
                };
                let [r, g, bl, _] =
                    palette(style.paint.palette).map_or([0xff; 4], |p| p.rgba(style.paint.index));
                let backdrop = style.backdrop.map(|p| {
                    let [r, g, b, _] = palette(p.palette).map_or([0x14; 4], |q| q.rgba(p.index));
                    [r, g, b]
                });
                let cap = style.cap.map_or(f32::MAX, |v| v * s);
                let key = LegendKey {
                    generation: bank.generation,
                    parts: parts.clone(),
                    style: *style,
                    rgb: [r, g, bl],
                    backdrop,
                    rect,
                    canvas: (c.w, c.h),
                };
                let groups = cached(&mut cache.legends, key, 16, || {
                    let mut rec = Recorder {
                        w: c.w,
                        h: c.h,
                        layers: Vec::new(),
                    };
                    legend::draw_legend(
                        parts,
                        cap,
                        style.align,
                        [r, g, bl],
                        backdrop,
                        font,
                        rect,
                        &mut rec,
                    );
                    rec.layers
                        .iter()
                        .filter_map(|l| {
                            let bbox =
                                bounds(l.px.iter().map(|&(x, y, _)| (x as usize, y as usize)))?;
                            let bw = bbox[2] - bbox[0];
                            let mut cov = vec![0u8; bw * (bbox[3] - bbox[1])];
                            for &(x, y, v) in &l.px {
                                cov[(y as usize - bbox[1]) * bw + x as usize - bbox[0]] = v;
                            }
                            Some(LegendGroup {
                                rgb: l.rgb,
                                bbox,
                                cov: Blob::Bytes(cov.into()),
                            })
                        })
                        .collect::<Vec<_>>()
                        .into()
                });
                for g in groups.iter() {
                    let [r, gg, bl] = g.rgb;
                    let params = [
                        pack([r, gg, bl, 0]),
                        w(g.bbox[0] as i32),
                        w(g.bbox[1] as i32),
                        (g.bbox[2] - g.bbox[0]) as u32,
                        0,
                    ];
                    b.push(kind::BLEND, g.bbox, [0; 4], &params, &[(4, g.cov.clone())]);
                }
            }
            WidgetKind::Text { text, style } => {
                let paint = style.paint;
                let (Some(pal), Some(font)) = (palette(paint.palette), bank.label_font(style.font))
                else {
                    return;
                };
                let (mw, mh) = (x1 - x0, y1 - y0);
                let backdrop = style
                    .backdrop
                    .and_then(|b| Some(palette(b.palette)?.rgba(b.index)));
                let outline = style
                    .outline
                    .and_then(|o| Some(palette(o.palette)?.rgba(o.index)));
                let key = TextKey {
                    generation: bank.generation,
                    text: text.clone(),
                    style: *style,
                    size: (mw, mh),
                    scale: c.s,
                    backdrop: backdrop.is_some(),
                    outline: outline.is_some(),
                };
                let blob = cached(&mut cache.text, key, 64, || {
                    let l = TextLayers::new(
                        font,
                        text,
                        style,
                        (mw, mh),
                        c.s,
                        backdrop.is_some(),
                        outline.is_some(),
                    );
                    let mut words: Vec<u32> = l.mask.iter().map(|&v| u32::from(v)).collect();
                    if let Some(near) = &l.near {
                        for (o, &n) in words.iter_mut().zip(near) {
                            *o |= u32::from(n) << 8;
                        }
                    }
                    if let Some((bx0, bx1)) = l.backdrop {
                        for row in words.chunks_exact_mut(mw.max(1)) {
                            for o in &mut row[bx0..bx1] {
                                *o |= 1 << 9;
                            }
                        }
                    }
                    Blob::Words(words.into())
                });
                let flags = u32::from(outline.is_some()) | u32::from(backdrop.is_some()) << 1;
                let params = [
                    w(x0 as i32),
                    w(y0 as i32),
                    mw as u32,
                    0,
                    pack(pal.rgba(paint.index)),
                    outline.map_or(0, pack),
                    backdrop.map_or(0, pack),
                    flags,
                ];
                b.push(kind::TEXT, rect, [0; 4], &params, &[(3, blob)]);
            }
        }
    }
}

/// A translated text part's pixels as the GPU reads them: its base indices and coverage, and
/// its per-pixel text colours when it has them, shared with the part (nothing is copied).
#[derive(Clone)]
struct PartBlobs {
    base: Blob,
    coverage: Blob,
    fg_map: Option<Blob>,
}

fn part_blobs(p: &RenderedPart) -> PartBlobs {
    PartBlobs {
        base: Blob::Bytes(p.base.clone()),
        coverage: Blob::Bytes(p.coverage.clone()),
        fg_map: (!p.fg_map.is_empty()).then(|| Blob::Bytes(p.fg_map.clone())),
    }
}

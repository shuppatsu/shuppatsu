//! The original's 2D layers over the 3D view (docs/re/hud.md §1.1, §6): the software sprite
//! layer and the rect layer, composed on the CPU into a 640×480 RGBA canvas exactly as the
//! original blits them into its back buffer. The canvas is then drawn as one overlay texture.

use crate::text::{SubRect, TextOverlay, draw_parts};
use ddg_formats::cg::{CgFile, Palette, TILE_PIXELS, TILE_SIZE, Tilemap};

/// The HUD design size: native pixels of resolution mode 1 (hud.md §1.1).
pub const CANVAS_W: usize = 640;
pub const CANVAS_H: usize = 480;
/// Sprite slot records at 0x816f88 (207 × 16 bytes).
pub const SLOT_COUNT: usize = 207;
/// Palette table entries (`0x829d5c + p·0x400`).
pub const PALETTE_COUNT: usize = 128;
/// Rect layer capacity (0x816b00, 16 fans of ≤ 4 points).
pub const MAX_FANS: usize = 16;
const LAYERS: u8 = 4;

/// One sprite slot (hud.md §1.1 record layout).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Slot {
    pub x: i16,
    pub y: i16,
    /// Source sub-rectangle in tiles; `w`/`h` ≤ 0 means the whole map.
    pub col: i8,
    pub row: i8,
    pub w: i8,
    pub h: i8,
    pub palette: u8,
    /// 0..=3; layer 3 is drawn first, layer 0 on top.
    pub layer: u8,
    /// `tbl.rom` tilemap id, or −1 when hidden.
    pub tilemap: i16,
    /// Survives the per-tick reset (0x4316b0) when set.
    pub keep: bool,
}

impl Slot {
    /// The cleared state (0x431790): off-screen, layer 3, no image.
    pub const CLEAR: Self = Self {
        x: 0x800,
        y: 0x800,
        col: 0,
        row: 0,
        w: 0,
        h: 0,
        palette: 0,
        layer: 3,
        tilemap: -1,
        keep: false,
    };

    /// Everything set at once (0x4316e0); the source rect is the whole map.
    pub fn new(tilemap: i16, x: i16, y: i16, palette: u8, layer: u8, keep: bool) -> Self {
        Self {
            x,
            y,
            palette,
            layer,
            tilemap,
            keep,
            ..Self::CLEAR
        }
    }
}

/// An untextured fan of up to four integer points in canvas pixels, ARGB colour (0x431950).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fan {
    pub points: [[i32; 2]; 4],
    pub count: u8,
    pub argb: u32,
}

/// Decoded tiles and tilemaps (`cg.rom`, `tbl.rom`).
pub struct SpriteBank {
    tiles: Vec<Option<Box<[u8; TILE_PIXELS]>>>,
    pub tilemaps: Vec<Tilemap>,
    /// Opt-in translated text drawn instead of the art of some sprites (`text.rs`); `None`
    /// draws the original art everywhere.
    text: Option<TextOverlay>,
}

impl SpriteBank {
    /// Decodes every tile up front; tiles that fail to decode are drawn as empty.
    pub fn new(cg: &CgFile<'_>, tilemaps: Vec<Tilemap>) -> Self {
        let tiles = (0..cg.len())
            .map(|i| cg.tile(i).ok().map(Box::new))
            .collect();
        Self {
            tiles,
            tilemaps,
            text: None,
        }
    }

    fn tile(&self, id: u16) -> Option<&[u8; TILE_PIXELS]> {
        self.tiles.get(usize::from(id))?.as_deref()
    }

    /// Installs (or with `None` removes) the text overrides. Build them with
    /// [`TextOverlay::build`] from this bank.
    pub fn set_text(&mut self, overlay: Option<TextOverlay>) {
        self.text = overlay;
    }

    pub fn text(&self) -> Option<&TextOverlay> {
        self.text.as_ref()
    }

    /// Palette indices of a resolved `(col, row, w, h)` tile rectangle of map `tbl`,
    /// `w·16 × h·16` row-major; empty cells and missing tiles are 0.
    pub(crate) fn render_rect(&self, tbl: u16, (c0, r0, w, h): (u8, u8, u8, u8)) -> Vec<u8> {
        let (pw, ph) = (usize::from(w) * TILE_SIZE, usize::from(h) * TILE_SIZE);
        let mut out = vec![0u8; pw * ph];
        let Some(map) = self.tilemaps.get(usize::from(tbl)) else {
            return out;
        };
        let mw = usize::from(map.width);
        for ty in 0..usize::from(h) {
            for tx in 0..usize::from(w) {
                let (mc, mr) = (usize::from(c0) + tx, usize::from(r0) + ty);
                if mc >= mw || mr >= usize::from(map.height) {
                    continue;
                }
                let id = map.tiles[mr * mw + mc];
                let Some(tile) = (id != 0).then(|| self.tile(id)).flatten() else {
                    continue;
                };
                for py in 0..TILE_SIZE {
                    let dst = (ty * TILE_SIZE + py) * pw + tx * TILE_SIZE;
                    out[dst..dst + TILE_SIZE]
                        .copy_from_slice(&tile[py * TILE_SIZE..(py + 1) * TILE_SIZE]);
                }
            }
        }
        out
    }
}

/// The sprite slots, palettes, rect layer and shake offset of one frame.
#[derive(Clone)]
pub struct SpriteLayer {
    pub slots: [Slot; SLOT_COUNT],
    pub palettes: Vec<Option<Palette>>,
    pub fans: Vec<Fan>,
    /// Global shake offset (signed bytes 0x856358/0x856359).
    pub shake: (i8, i8),
    /// Canvas pixels per original pixel. 1 is the original 640×480; 2 is the enhancement
    /// `ui_scale` (art doubled with nearest neighbour, translated text at full resolution).
    pub scale: usize,
}

impl Default for SpriteLayer {
    fn default() -> Self {
        Self {
            slots: [Slot::CLEAR; SLOT_COUNT],
            palettes: vec![None; PALETTE_COUNT],
            fans: Vec::new(),
            shake: (0, 0),
            scale: 1,
        }
    }
}

impl SpriteLayer {
    /// The per-tick reset (0x4316b0 for slots, 0x431db0 for the rect layer).
    pub fn reset_transient(&mut self) {
        for s in self.slots.iter_mut().filter(|s| !s.keep) {
            *s = Slot::CLEAR;
        }
        self.fans.clear();
    }

    /// Appends a fan; ignored once the list is full (16 fans).
    pub fn add_fan(&mut self, points: &[[i32; 2]], argb: u32) {
        if self.fans.len() >= MAX_FANS || points.len() < 3 {
            return;
        }
        let mut p = [[0; 2]; 4];
        let n = points.len().min(4);
        p[..n].copy_from_slice(&points[..n]);
        self.fans.push(Fan {
            points: p,
            count: n as u8,
            argb,
        });
    }

    /// A transparent canvas of [`SpriteLayer::canvas_size`].
    pub fn new_canvas(&self) -> Vec<u8> {
        let (w, h) = self.canvas_size();
        vec![0; w * h * 4]
    }

    /// Canvas size in pixels for [`SpriteLayer::compose`]: 640×480 times [`SpriteLayer::scale`].
    pub fn canvas_size(&self) -> (usize, usize) {
        let s = self.scale.max(1);
        (CANVAS_W * s, CANVAS_H * s)
    }

    /// Draws the layer into `canvas` ([`SpriteLayer::canvas_size`] RGBA8, cleared to
    /// transparent): sprites in four passes (layer 3 → 0, slot high → low), then the rect layer.
    pub fn compose(&self, bank: &SpriteBank, canvas: &mut [u8]) {
        let (cw, ch) = self.canvas_size();
        assert_eq!(canvas.len(), cw * ch * 4);
        canvas.fill(0);
        let c = Canvas {
            w: cw,
            h: ch,
            s: self.scale.max(1),
        };
        let (sx, sy) = (i32::from(self.shake.0), i32::from(self.shake.1));
        for layer in (0..LAYERS).rev() {
            for slot in self.slots.iter().rev().filter(|s| s.layer == layer) {
                self.blit(bank, slot, sx, sy, c, canvas);
            }
        }
        for fan in &self.fans {
            fill_fan(fan, sx, sy, c, canvas);
        }
    }

    fn blit(&self, bank: &SpriteBank, slot: &Slot, sx: i32, sy: i32, c: Canvas, canvas: &mut [u8]) {
        let Ok(id) = usize::try_from(slot.tilemap) else {
            return;
        };
        let (Some(map), Some(Some(palette))) = (
            bank.tilemaps.get(id),
            self.palettes.get(usize::from(slot.palette)),
        ) else {
            return;
        };
        let (mw, mh) = (i32::from(map.width), i32::from(map.height));
        let (c0, r0) = (i32::from(slot.col).max(0), i32::from(slot.row).max(0));
        let w = if slot.w <= 0 { mw } else { i32::from(slot.w) };
        let h = if slot.h <= 0 { mh } else { i32::from(slot.h) };
        let ts = TILE_SIZE as i32;
        // Translated text replacing (part of) this sprite, if an overlay is installed.
        let text = bank.text.as_ref().and_then(|t| {
            let rect = SubRect {
                col: slot.col,
                row: slot.row,
                w: slot.w,
                h: slot.h,
            };
            let tbl = u16::try_from(slot.tilemap).ok()?;
            // Parts rendered for another scale would not line up; draw the art then.
            (t.scale() == c.s).then_some(())?;
            t.get(&(tbl, rect.resolve(map.width, map.height)?))
        });
        let s = c.s as i32;
        let (sx0, sy0) = (i32::from(slot.x) + sx, i32::from(slot.y) + sy);
        for ty in 0..h {
            for tx in 0..w {
                let (mc, mr) = (c0 + tx, r0 + ty);
                if mc >= mw || mr >= mh {
                    continue;
                }
                let id = map.tiles[(mr * mw + mc) as usize];
                let Some(tile) = (id != 0).then(|| bank.tile(id)).flatten() else {
                    continue;
                };
                let ox = sx0 + tx * ts;
                let oy = sy0 + ty * ts;
                for py in 0..ts {
                    for px in 0..ts {
                        let index = tile[(py * ts + px) as usize];
                        if index == 0 {
                            continue;
                        }
                        let (lx, ly) = ((tx * ts + px) as usize, (ty * ts + py) as usize);
                        let (bx, by) = (lx * c.s, ly * c.s);
                        if text.is_some_and(|parts| parts.iter().any(|p| p.contains(bx, by))) {
                            continue;
                        }
                        // One original pixel becomes an s × s block.
                        let rgba = palette.rgba(index);
                        for dy in 0..s {
                            let y = (oy + py) * s + dy;
                            if !(0..c.h as i32).contains(&y) {
                                continue;
                            }
                            for dx in 0..s {
                                let x = (ox + px) * s + dx;
                                if !(0..c.w as i32).contains(&x) {
                                    continue;
                                }
                                let o = (y as usize * c.w + x as usize) * 4;
                                canvas[o..o + 4].copy_from_slice(&rgba);
                            }
                        }
                    }
                }
            }
        }
        if let Some(parts) = text {
            draw_parts(parts, palette, sx0 * s, sy0 * s, canvas, c.w, c.h);
        }
    }
}

/// Fills a convex fan, sampling pixel centres (D3D's top-left fill convention is approximated
/// by the half-pixel centre test). Opaque; the original's alpha byte is not used (guess).
fn fill_fan(fan: &Fan, sx: i32, sy: i32, c: Canvas, canvas: &mut [u8]) {
    let n = usize::from(fan.count);
    let s = c.s as f32;
    let pts: Vec<[f32; 2]> = fan.points[..n]
        .iter()
        .map(|p| [(p[0] + sx) as f32 * s, (p[1] + sy) as f32 * s])
        .collect();
    let [_, r, g, b] = fan.argb.to_be_bytes();
    for i in 1..n - 1 {
        fill_triangle([pts[0], pts[i], pts[i + 1]], [r, g, b, 255], c, canvas);
    }
}

/// The composed canvas: size in pixels and pixels per original pixel.
#[derive(Clone, Copy, Debug)]
struct Canvas {
    w: usize,
    h: usize,
    s: usize,
}

fn fill_triangle(t: [[f32; 2]; 3], rgba: [u8; 4], c: Canvas, canvas: &mut [u8]) {
    let edge = |a: [f32; 2], b: [f32; 2], p: [f32; 2]| {
        (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
    };
    let area = edge(t[0], t[1], t[2]);
    if area == 0.0 {
        return;
    }
    let min_x = t.iter().map(|p| p[0]).fold(f32::MAX, f32::min).max(0.0) as usize;
    let max_x = (t.iter().map(|p| p[0]).fold(f32::MIN, f32::max).ceil() as usize).min(c.w);
    let min_y = t.iter().map(|p| p[1]).fold(f32::MAX, f32::min).max(0.0) as usize;
    let max_y = (t.iter().map(|p| p[1]).fold(f32::MIN, f32::max).ceil() as usize).min(c.h);
    for y in min_y..max_y {
        for x in min_x..max_x {
            let p = [x as f32 + 0.5, y as f32 + 0.5];
            let w = [
                edge(t[1], t[2], p),
                edge(t[2], t[0], p),
                edge(t[0], t[1], p),
            ];
            if w.iter().all(|&v| v * area >= 0.0) {
                let o = (y * c.w + x) * 4;
                canvas[o..o + 4].copy_from_slice(&rgba);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bank with one 1×1 tilemap whose tile is solid index `v`.
    fn bank(v: u8) -> SpriteBank {
        SpriteBank {
            tiles: vec![None, Some(Box::new([v; TILE_PIXELS]))],
            tilemaps: vec![Tilemap {
                width: 1,
                height: 1,
                tiles: vec![1],
            }],
            text: None,
        }
    }

    fn palette() -> Palette {
        let mut p = [0u32; 256];
        p[1] = 0x00_ff_00_00; // B=0, R=0, G=0xff (bytes B, R, G)
        p[2] = 0x00_00_ff_00; // R=0xff
        Palette(p)
    }

    fn pixel(c: &[u8], x: usize, y: usize) -> [u8; 4] {
        let o = (y * CANVAS_W + x) * 4;
        c[o..o + 4].try_into().unwrap()
    }

    #[test]
    fn layer_zero_and_low_slots_are_on_top() {
        let b = bank(1);
        let mut l = SpriteLayer::default();
        l.palettes[0] = Some(palette());
        let mut red = palette();
        red.0[1] = red.0[2];
        l.palettes[1] = Some(red);
        // Slot 5 (layer 0, green) over slot 3 (layer 1, red), though 3 has the lower number.
        l.slots[5] = Slot::new(0, 10, 10, 0, 0, true);
        l.slots[3] = Slot::new(0, 10, 10, 1, 1, true);
        let mut c = vec![0; CANVAS_W * CANVAS_H * 4];
        l.compose(&b, &mut c);
        assert_eq!(pixel(&c, 10, 10), [0, 255, 0, 255]);
        assert_eq!(pixel(&c, 9, 10), [0, 0, 0, 0]);
        // Same layer: the lower slot wins.
        l.slots[3].layer = 0;
        l.compose(&b, &mut c);
        assert_eq!(pixel(&c, 10, 10), [255, 0, 0, 255]);
    }

    #[test]
    fn clipping_transparency_and_reset() {
        let b = bank(0);
        let mut l = SpriteLayer::default();
        l.palettes[0] = Some(palette());
        l.slots[0] = Slot::new(0, 632, 472, 0, 0, false);
        let mut c = vec![0; CANVAS_W * CANVAS_H * 4];
        l.compose(&b, &mut c); // index 0 everywhere: nothing drawn, no panic at the edge
        assert!(c.iter().all(|&v| v == 0));
        l.reset_transient();
        assert_eq!(l.slots[0], Slot::CLEAR);
    }

    #[test]
    fn text_overlay_replaces_art_inside_the_slot_only() {
        use crate::text::{Style, SubRect, TextOverlay, TextOverride, TextPart, tests::BoxGlyphs};
        // A 4×2-tile map: index 1 (green) everywhere, index 2 (red) in the first tile, so
        // green is dominant and red is the outline colour.
        let mut b = bank(1);
        b.tiles.push(Some(Box::new([2; TILE_PIXELS])));
        b.tilemaps[0] = Tilemap {
            width: 4,
            height: 2,
            tiles: vec![2, 1, 1, 1, 1, 1, 1, 1],
        };
        let o = TextOverride {
            parts: vec![TextPart {
                // The left half only; the right half keeps the art.
                region: Some([0, 0, 32, 32]),
                text: "Hi".into(),
                style: Style::Plain,
                outline: true,
                ..TextPart::default()
            }],
        };
        let overlay = TextOverlay::build(&b, [(0, SubRect::WHOLE, &o)], &BoxGlyphs);
        assert_eq!(overlay.len(), 1);
        b.set_text(Some(overlay));
        let mut l = SpriteLayer::default();
        l.palettes[0] = Some(palette());
        l.slots[0] = Slot::new(0, 100, 100, 0, 0, true);
        let mut c = vec![0; CANVAS_W * CANVAS_H * 4];
        l.compose(&b, &mut c);
        // Outside the slot's 64×32 rectangle nothing is drawn.
        for y in 0..CANVAS_H {
            for x in 0..CANVAS_W {
                if !(100..164).contains(&x) || !(100..132).contains(&y) {
                    assert_eq!(pixel(&c, x, y)[3], 0, "({x}, {y})");
                }
            }
        }
        // The right half is untouched art (green); the left half lost its red tile corner.
        assert_eq!(pixel(&c, 150, 110), [0, 255, 0, 255]);
        assert_eq!(pixel(&c, 100, 100), [0, 0, 0, 0]);
        // The text was drawn in the dominant colour somewhere in the left half.
        let green = (100..132)
            .flat_map(|y| (100..132).map(move |x| (x, y)))
            .filter(|&(x, y)| pixel(&c, x, y) == [0, 255, 0, 255])
            .count();
        assert!(green > 0);
        // A slot showing a different sub-rect of the same map keeps the art.
        l.slots[0].col = 2;
        l.slots[0].w = 2;
        l.compose(&b, &mut c);
        assert_eq!(pixel(&c, 100, 100), [0, 255, 0, 255]);
    }

    #[test]
    fn scale_two_doubles_pixels_and_fans() {
        let b = bank(1);
        let mut l = SpriteLayer {
            scale: 2,
            ..SpriteLayer::default()
        };
        l.palettes[0] = Some(palette());
        l.slots[0] = Slot::new(0, 10, 10, 0, 0, true);
        l.add_fan(
            &[[100, 100], [110, 100], [110, 110], [100, 110]],
            0xff_12_34_56,
        );
        let (w, h) = l.canvas_size();
        assert_eq!((w, h), (1280, 960));
        let mut c = vec![0; w * h * 4];
        l.compose(&b, &mut c);
        let px = |x: usize, y: usize| -> [u8; 4] { c[(y * w + x) * 4..][..4].try_into().unwrap() };
        // The 16×16 tile at (10, 10) covers 20..52 on the 2× canvas.
        assert_eq!(px(20, 20), [0, 255, 0, 255]);
        assert_eq!(px(51, 51), [0, 255, 0, 255]);
        assert_eq!(px(19, 20), [0, 0, 0, 0]);
        assert_eq!(px(52, 52), [0, 0, 0, 0]);
        assert_eq!(px(210, 210), [0x12, 0x34, 0x56, 255]);
        assert_eq!(px(221, 210), [0, 0, 0, 0]);
    }

    #[test]
    fn fans_fill_inside() {
        let mut l = SpriteLayer::default();
        l.add_fan(&[[0, 0], [10, 0], [10, 10], [0, 10]], 0xff_12_34_56);
        let mut c = vec![0; CANVAS_W * CANVAS_H * 4];
        l.compose(&bank(0), &mut c);
        assert_eq!(pixel(&c, 5, 5), [0x12, 0x34, 0x56, 255]);
        assert_eq!(pixel(&c, 10, 5), [0, 0, 0, 0]);
    }
}

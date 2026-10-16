//! The original's 2D layers over the 3D view (docs/re/hud.md §1.1, §6): the software sprite
//! layer and the rect layer, composed on the CPU into a 640×480 RGBA canvas exactly as the
//! original blits them into its back buffer. The canvas is then drawn as one overlay texture.

use crate::label::LabelFont;
use crate::text::{Align, SubRect, TextOverlay, draw_parts};
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
    pub anchor: Anchor,
}

/// Which edge of a wide canvas an element keeps its distance to (enhancement `wide_hud`,
/// docs/re/hud.md §6.4). The design is 640 original pixels wide; on a canvas
/// [`SpriteLayer::wide`] canvas pixels wider, `Centre` elements are shifted by half the extra
/// width (the 4:3 design stays centred), `Left` ones not at all and `Right` ones by all of it.
/// Only positions change: sizes and pixel shapes never do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Anchor {
    #[default]
    Centre,
    Left,
    Right,
}

/// A port-only 2D element (enhancements; the original has none). Widgets are rebuilt every
/// tick by their owner and cleared with the rect layer ([`SpriteLayer::reset_transient`]).
/// They are drawn in the sprite passes, after the slots of the same `layer`, in the order they
/// were added. Geometry is in original 640×480 pixels; fractions address the finer canvas
/// pixels of `SpriteLayer::scale` > 1. The shake offset applies.
#[derive(Clone, Debug, PartialEq)]
pub struct Widget {
    pub layer: u8,
    /// `[x, y, w, h]`; nothing is drawn outside it.
    pub rect: [f32; 4],
    pub kind: WidgetKind,
    pub anchor: Anchor,
}

/// A colour: index `index` of palette slot `palette`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Paint {
    pub palette: u8,
    pub index: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub enum WidgetKind {
    /// The whole rectangle in one colour.
    Fill(Paint),
    /// A triangle pointing down: the rectangle's top edge to the middle of its bottom edge.
    DownArrow(Paint),
    /// The `src` rectangle (`[x, y, w, h]` in art pixels) of a tilemap, `zoom` canvas pixels
    /// per art pixel, centred in the widget's rectangle and clipped to it (never stretched).
    /// Index 0 is transparent, as in a slot.
    Sprite {
        tilemap: u16,
        src: [u16; 4],
        zoom: u8,
        palette: u8,
    },
    /// Text in the rectangle at canvas resolution, uniformly scaled, in the bank's label font
    /// ([`SpriteBank::set_label_font`]), styled by [`TextStyle`].
    Text { text: String, style: TextStyle },
}

/// How a [`WidgetKind::Text`] is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextStyle {
    pub paint: Paint,
    /// Capitals `cap` original pixels high (made smaller only when the text would not fit), or
    /// with `None` the largest size that fits.
    pub cap: Option<f32>,
    /// A one-original-pixel outline under the text in this colour.
    pub outline: Option<Paint>,
    /// A box in this colour behind the text: the rectangle's full height, as wide as the
    /// text plus [`BACKDROP_PAD`] on each side (for art drawn on a box, like the banners).
    pub backdrop: Option<Paint>,
    pub align: Align,
    /// Rotated 90° clockwise (reads top to bottom); `align` then runs from the top.
    pub vertical: bool,
}

/// Horizontal padding of a [`TextStyle::backdrop`], original pixels.
pub const BACKDROP_PAD: usize = 4;

impl TextStyle {
    /// Centred, no outline.
    pub fn plain(paint: Paint, cap: Option<f32>) -> Self {
        Self {
            paint,
            cap,
            outline: None,
            backdrop: None,
            align: Align::Center,
            vertical: false,
        }
    }
}

/// Decoded tiles and tilemaps (`cg.rom`, `tbl.rom`).
pub struct SpriteBank {
    tiles: Vec<Option<Box<[u8; TILE_PIXELS]>>>,
    pub tilemaps: Vec<Tilemap>,
    /// Opt-in translated text drawn instead of the art of some sprites (`text.rs`); `None`
    /// draws the original art everywhere.
    text: Option<TextOverlay>,
    /// The font of [`WidgetKind::Text`] labels; `None` draws no labels.
    labels: Option<LabelFont>,
    /// Enhancement `wide_menus`: backdrops continued sideways on a wide canvas, by tilemap id
    /// ([`SpriteBank::set_wide_fill`]).
    wide_fill: Vec<(u16, Repeat)>,
}

/// A horizontally periodic run of a tilemap's columns: columns `start..` up to the right
/// edge repeat every `period` columns (enhancement `wide_menus`). A wide canvas continues
/// such a backdrop past both edges of the 4:3 design by mapping every column outside the map
/// into the run, so the tiling keeps the phase of the original's centred picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Repeat {
    pub start: i32,
    pub period: i32,
}

impl Repeat {
    /// The shortest period of the columns at the right edge of `map`, found in its tile ids.
    /// The run must check at least half a period of columns against the period before it, so
    /// a map that is not a pattern (a framed panel) gets `None` and stays centred. A uniform
    /// edge (a solid band) has period 1.
    pub fn of(map: &Tilemap) -> Option<Self> {
        let (w, h) = (usize::from(map.width), usize::from(map.height));
        let same =
            |a: usize, b: usize| (0..h).all(|r| map.tiles[r * w + a] == map.tiles[r * w + b]);
        (1..=w / 2).find_map(|p| {
            // Grow the run leftwards from the right edge while column c matches c + p.
            let mut start = w - p;
            while start > 0 && same(start - 1, start - 1 + p) {
                start -= 1;
            }
            let checked = w - p - start;
            (checked > 0 && checked >= p.div_ceil(2)).then_some(Self {
                start: start as i32,
                period: p as i32,
            })
        })
    }

    /// The map column drawn at column `col` (any integer; columns of the run map to
    /// themselves).
    pub fn source(&self, col: i32) -> i32 {
        self.start + (col - self.start).rem_euclid(self.period)
    }
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
            labels: None,
            wide_fill: Vec::new(),
        }
    }

    /// Enhancement `wide_menus`: marks tilemap `tbl` as a backdrop that a wide canvas
    /// continues sideways, repeating its pattern ([`Repeat::of`]), wherever a slot shows the
    /// whole map across the 4:3 design. Returns whether the map has a pattern to repeat; one
    /// without keeps the original's centred picture.
    pub fn set_wide_fill(&mut self, tbl: u16) -> bool {
        let Some(r) = self.tilemaps.get(usize::from(tbl)).and_then(Repeat::of) else {
            return false;
        };
        if !self.wide_fill.iter().any(|&(t, _)| t == tbl) {
            self.wide_fill.push((tbl, r));
        }
        true
    }

    fn wide_fill(&self, tbl: u16) -> Option<Repeat> {
        self.wide_fill
            .iter()
            .find(|&&(t, _)| t == tbl)
            .map(|&(_, r)| r)
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

    /// Installs (or with `None` removes) the font of [`WidgetKind::Text`] labels.
    pub fn set_label_font(&mut self, font: Option<LabelFont>) {
        self.labels = font;
    }

    pub fn has_label_font(&self) -> bool {
        self.labels.is_some()
    }

    /// The width of `text` in the label font with capitals `cap` pixels high (any unit: the
    /// result is in the same one); `None` without a label font.
    pub fn label_width(&self, text: &str, cap: f32) -> Option<f32> {
        Some(self.labels.as_ref()?.width(text, cap))
    }

    /// The palette indices of a whole tilemap: `(width, height, pixels)` in art pixels.
    pub fn art(&self, tbl: u16) -> Option<(usize, usize, Vec<u8>)> {
        let map = self.tilemaps.get(usize::from(tbl))?;
        let (w, h) = (map.width, map.height);
        let px = self.render_rect(tbl, (0, 0, w, h));
        Some((usize::from(w) * TILE_SIZE, usize::from(h) * TILE_SIZE, px))
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
    /// Port-only elements of enhancements; empty in the arcade default.
    pub widgets: Vec<Widget>,
    /// Global shake offset (signed bytes 0x856358/0x856359).
    pub shake: (i8, i8),
    /// Canvas pixels per original pixel. 1 is the original 640×480; 2 is the enhancement
    /// `ui_scale` (art doubled with nearest neighbour, translated text at full resolution).
    pub scale: usize,
    /// Enhancement `wide_hud`: canvas pixels added to the width (0 = the original 4:3 canvas),
    /// so the canvas has the window's shape ([`SpriteLayer::fit_window`]).
    pub wide: usize,
    /// Edge anchor of each slot on a wide canvas; all `Centre` keeps the 4:3 layout centred.
    pub anchors: [Anchor; SLOT_COUNT],
    /// Anchor given to widgets and fans as they are added ([`SpriteLayer::anchored`]).
    pub pen: Anchor,
}

impl Default for SpriteLayer {
    fn default() -> Self {
        Self {
            slots: [Slot::CLEAR; SLOT_COUNT],
            palettes: vec![None; PALETTE_COUNT],
            fans: Vec::new(),
            widgets: Vec::new(),
            shake: (0, 0),
            scale: 1,
            wide: 0,
            anchors: [Anchor::Centre; SLOT_COUNT],
            pen: Anchor::Centre,
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
        self.widgets.clear();
    }

    /// Adds a [`Widget`], anchored as [`SpriteLayer::anchored`] says (else `Centre`).
    pub fn add_widget(&mut self, layer: u8, rect: [f32; 4], kind: WidgetKind) {
        let anchor = self.pen;
        self.widgets.push(Widget {
            layer,
            rect,
            kind,
            anchor,
        });
    }

    /// Runs `f` with widgets and fans added by it anchored to `anchor`.
    pub fn anchored(&mut self, anchor: Anchor, f: impl FnOnce(&mut Self)) {
        let old = std::mem::replace(&mut self.pen, anchor);
        f(self);
        self.pen = old;
    }

    /// Every slot back to `Centre` (the 4:3 layout, centred on a wide canvas).
    pub fn centre_all(&mut self) {
        self.anchors = [Anchor::Centre; SLOT_COUNT];
    }

    /// Enhancement `wide_hud`: widens the canvas to a `width × height` window's shape (never
    /// narrower than 4:3), keeping the height of 480 × [`SpriteLayer::scale`]. Returns whether
    /// the canvas size changed.
    pub fn fit_window(&mut self, width: u32, height: u32) -> bool {
        let s = self.scale.max(1);
        let ch = (CANVAS_H * s) as f64;
        let cw = (ch * f64::from(width) / f64::from(height.max(1))).round() as usize;
        let wide = cw.saturating_sub(CANVAS_W * s);
        std::mem::replace(&mut self.wide, wide) != wide
    }

    /// The horizontal offset of an anchor, in canvas pixels.
    pub fn anchor_shift(&self, anchor: Anchor) -> i32 {
        match anchor {
            Anchor::Left => 0,
            Anchor::Centre => (self.wide / 2) as i32,
            Anchor::Right => self.wide as i32,
        }
    }

    /// The extra width of a wide canvas in original pixels, on each side of the centred 4:3
    /// design: how far `Left` and `Right` elements move away from the centre.
    pub fn side_room(&self) -> f32 {
        (self.wide / 2) as f32 / self.scale.max(1) as f32
    }

    /// Adds `text` in `rect` (original pixels) in `paint`, capitals `cap` original pixels high
    /// (`None`: as large as fits): the free-text path of the label font, drawn at the canvas
    /// resolution of [`SpriteLayer::scale`].
    pub fn add_text(
        &mut self,
        layer: u8,
        rect: [f32; 4],
        text: &str,
        paint: Paint,
        cap: Option<f32>,
    ) {
        self.add_styled_text(layer, rect, text, TextStyle::plain(paint, cap));
    }

    /// [`SpriteLayer::add_text`] with an outline or another alignment.
    pub fn add_styled_text(&mut self, layer: u8, rect: [f32; 4], text: &str, style: TextStyle) {
        let text = text.to_owned();
        self.add_widget(layer, rect, WidgetKind::Text { text, style });
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
            anchor: self.pen,
        });
    }

    /// A transparent canvas of [`SpriteLayer::canvas_size`].
    pub fn new_canvas(&self) -> Vec<u8> {
        let (w, h) = self.canvas_size();
        vec![0; w * h * 4]
    }

    /// Canvas size in pixels for [`SpriteLayer::compose`]: 640×480 times
    /// [`SpriteLayer::scale`], plus [`SpriteLayer::wide`] in width.
    pub fn canvas_size(&self) -> (usize, usize) {
        let s = self.scale.max(1);
        (CANVAS_W * s + self.wide, CANVAS_H * s)
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
            for (i, slot) in self.slots.iter().enumerate().rev() {
                if slot.layer == layer {
                    let shift = self.anchor_shift(self.anchors[i]);
                    self.blit(bank, slot, sx, sy, shift, c, canvas);
                }
            }
            for w in self.widgets.iter().filter(|w| w.layer == layer) {
                let shift = self.anchor_shift(w.anchor);
                self.draw_widget(bank, w, sx, sy, shift, c, canvas);
            }
        }
        for fan in &self.fans {
            fill_fan(fan, sx, sy, self.anchor_shift(fan.anchor), c, canvas);
        }
    }

    /// Draws one slot; `shift` is its anchor's offset in canvas pixels.
    #[allow(clippy::too_many_arguments)]
    fn blit(
        &self,
        bank: &SpriteBank,
        slot: &Slot,
        sx: i32,
        sy: i32,
        shift: i32,
        c: Canvas,
        canvas: &mut [u8],
    ) {
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
        // The columns to draw: the slot's own and, for a backdrop on a wide canvas, its
        // continuation past both edges of the design (enhancement `wide_menus`).
        let mut cols: Vec<(i32, i32)> = (0..w).map(|tx| (tx, c0 + tx)).collect();
        if let Some(rep) = self.backdrop_repeat(bank, slot, mw) {
            // The original pixels the canvas shows: [-shift / s, (c.w - shift) / s).
            let left = (-shift).div_euclid(s);
            let right = (c.w as i32 - shift + s - 1).div_euclid(s);
            let first = (left - sx0).div_euclid(ts);
            let last = (right - sx0 - 1).div_euclid(ts);
            cols.extend((first..0).chain(mw..=last).map(|tx| (tx, rep.source(tx))));
        }
        for ty in 0..h {
            for &(tx, mc) in &cols {
                let mr = r0 + ty;
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
                        // Translated text only covers the slot's own columns.
                        let lx = (tx * ts + px) as usize;
                        let (bx, by) = (lx * c.s, (ty * ts + py) as usize * c.s);
                        if (0..w).contains(&tx)
                            && text.is_some_and(|parts| parts.iter().any(|p| p.contains(bx, by)))
                        {
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
                                let x = (ox + px) * s + dx + shift;
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
            draw_parts(parts, palette, sx0 * s + shift, sy0 * s, canvas, c.w, c.h);
        }
    }

    /// Enhancement `wide_menus`: the repeat of a backdrop slot on a wide canvas. Only a slot
    /// showing a whole marked map ([`SpriteBank::set_wide_fill`]) across the full 640-px
    /// design is continued; everything else keeps its 4:3 place.
    fn backdrop_repeat(&self, bank: &SpriteBank, slot: &Slot, mw: i32) -> Option<Repeat> {
        let whole = slot.col <= 0 && slot.row <= 0 && slot.w <= 0 && slot.h <= 0;
        let x = i32::from(slot.x);
        let spans = x <= 0 && x + mw * TILE_SIZE as i32 >= CANVAS_W as i32;
        if self.wide == 0 || !whole || !spans {
            return None;
        }
        bank.wide_fill(u16::try_from(slot.tilemap).ok()?)
    }
}

impl SpriteLayer {
    #[allow(clippy::too_many_arguments)]
    fn draw_widget(
        &self,
        bank: &SpriteBank,
        w: &Widget,
        sx: i32,
        sy: i32,
        shift: i32,
        c: Canvas,
        canvas: &mut [u8],
    ) {
        let palette = |p: u8| self.palettes.get(usize::from(p)).and_then(Option::as_ref);
        // The rectangle in canvas pixels, clipped to the canvas.
        let s = c.s as f32;
        let [x, y, rw, rh] = w.rect;
        let to_px = |v: f32, shake: i32, offset: i32, max: usize| {
            (((v + shake as f32) * s).round() as i64 + i64::from(offset)).clamp(0, max as i64)
                as usize
        };
        let (x0, x1) = (to_px(x, sx, shift, c.w), to_px(x + rw, sx, shift, c.w));
        let (y0, y1) = (to_px(y, sy, 0, c.h), to_px(y + rh, sy, 0, c.h));
        let shift = shift as f32;
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let put = |canvas: &mut [u8], px: usize, py: usize, rgba: [u8; 4]| {
            let o = (py * c.w + px) * 4;
            canvas[o..o + 4].copy_from_slice(&rgba);
        };
        match &w.kind {
            WidgetKind::Fill(p) => {
                let Some(pal) = palette(p.palette) else {
                    return;
                };
                let rgba = pal.rgba(p.index);
                for py in y0..y1 {
                    for px in x0..x1 {
                        put(canvas, px, py, rgba);
                    }
                }
            }
            WidgetKind::DownArrow(p) => {
                let Some(pal) = palette(p.palette) else {
                    return;
                };
                let rgba = pal.rgba(p.index);
                let (l, r) = (
                    (x + sx as f32) * s + shift,
                    (x + rw + sx as f32) * s + shift,
                );
                let (t, b) = ((y + sy as f32) * s, (y + rh + sy as f32) * s);
                let tri = [[l, t], [r, t], [(l + r) / 2.0, b]];
                fill_triangle(tri, rgba, c, canvas);
            }
            WidgetKind::Sprite {
                tilemap,
                src,
                zoom,
                palette: p,
            } => {
                let (Some(pal), Some(map)) =
                    (palette(*p), bank.tilemaps.get(usize::from(*tilemap)))
                else {
                    return;
                };
                let art = bank.render_rect(*tilemap, (0, 0, map.width, map.height));
                let aw = usize::from(map.width) * TILE_SIZE;
                let ah = usize::from(map.height) * TILE_SIZE;
                let [ax, ay, sw, sh] = src.map(usize::from);
                let (sw, sh) = (sw.min(aw.saturating_sub(ax)), sh.min(ah.saturating_sub(ay)));
                let z = usize::from((*zoom).max(1));
                // Centred in the unclipped rectangle (in signed canvas pixels).
                let cx =
                    ((x + sx as f32 + rw / 2.0) * s + shift).round() as i64 - (sw * z / 2) as i64;
                let cy = ((y + sy as f32 + rh / 2.0) * s).round() as i64 - (sh * z / 2) as i64;
                for py in y0..y1 {
                    let ly = py as i64 - cy;
                    if ly < 0 || ly >= (sh * z) as i64 {
                        continue;
                    }
                    let row = ay + ly as usize / z;
                    for px in x0..x1 {
                        let lx = px as i64 - cx;
                        if lx < 0 || lx >= (sw * z) as i64 {
                            continue;
                        }
                        let index = art[row * aw + ax + lx as usize / z];
                        if index != 0 {
                            put(canvas, px, py, pal.rgba(index));
                        }
                    }
                }
            }
            WidgetKind::Text { text, style } => {
                let paint = style.paint;
                let (Some(pal), Some(font)) = (palette(paint.palette), bank.labels.as_ref()) else {
                    return;
                };
                let (mw, mh) = (x1 - x0, y1 - y0);
                let mask = font.mask(
                    text,
                    mw,
                    mh,
                    style.cap.map(|v| v * s),
                    style.align,
                    style.vertical,
                );
                let backdrop = style
                    .backdrop
                    .and_then(|b| Some(palette(b.palette)?.rgba(b.index)));
                let inked = (0..mw).filter(|&px| (0..mh).any(|py| mask[py * mw + px] > 0));
                if let (Some(b), Some(first), Some(last)) =
                    (backdrop, inked.clone().min(), inked.max())
                {
                    let pad = BACKDROP_PAD * c.s;
                    let (bx0, bx1) = (first.saturating_sub(pad), (last + 1 + pad).min(mw));
                    for py in 0..mh {
                        for px in bx0..bx1 {
                            put(canvas, x0 + px, y0 + py, b);
                        }
                    }
                }
                let outline = style
                    .outline
                    .and_then(|o| Some(palette(o.palette)?.rgba(o.index)));
                if let Some(o) = outline {
                    // One original pixel around the solid part of the text, under it.
                    let r = c.s as isize;
                    let solid = |px: isize, py: isize| {
                        (0..mw as isize).contains(&px)
                            && (0..mh as isize).contains(&py)
                            && mask[py as usize * mw + px as usize] >= 128
                    };
                    for py in 0..mh as isize {
                        for px in 0..mw as isize {
                            if (-r..=r).any(|dy| (-r..=r).any(|dx| solid(px + dx, py + dy))) {
                                put(canvas, x0 + px as usize, y0 + py as usize, o);
                            }
                        }
                    }
                }
                let fg = pal.rgba(paint.index);
                for py in 0..mh {
                    for px in 0..mw {
                        let cov = mask[py * mw + px];
                        if cov == 0 {
                            continue;
                        }
                        let o = ((y0 + py) * c.w + x0 + px) * 4;
                        let under: [u8; 4] = canvas[o..o + 4].try_into().expect("4 bytes");
                        let out = if under[3] == 0 {
                            // Over transparency the overlay's alpha test decides.
                            [fg[0], fg[1], fg[2], cov]
                        } else {
                            let t = u16::from(cov);
                            let mix = |a: u8, b: u8| {
                                ((u16::from(a) * (255 - t) + u16::from(b) * t) / 255) as u8
                            };
                            [
                                mix(under[0], fg[0]),
                                mix(under[1], fg[1]),
                                mix(under[2], fg[2]),
                                255,
                            ]
                        };
                        canvas[o..o + 4].copy_from_slice(&out);
                    }
                }
            }
        }
    }
}

/// Fills a convex fan, sampling pixel centres (D3D's top-left fill convention is approximated
/// by the half-pixel centre test). Opaque; the original's alpha byte is not used (guess).
fn fill_fan(fan: &Fan, sx: i32, sy: i32, shift: i32, c: Canvas, canvas: &mut [u8]) {
    let n = usize::from(fan.count);
    let s = c.s as f32;
    // Uniform in x and y: the anchor only translates.
    let pts: Vec<[f32; 2]> = fan.points[..n]
        .iter()
        .map(|p| {
            [
                (p[0] + sx) as f32 * s + shift as f32,
                (p[1] + sy) as f32 * s,
            ]
        })
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
            labels: None,
            wide_fill: Vec::new(),
        }
    }

    #[test]
    fn grouped_parts_share_the_smallest_size() {
        use crate::text::{Style, SubRect, TextOverlay, TextOverride, TextPart, tests::BoxGlyphs};
        let b = bank(3);
        // Two words in the two halves of one 16×16 sprite, as in a callout cell.
        let parts = |group: Option<&str>| TextOverride {
            parts: ["Off", "Accelerate"]
                .iter()
                .enumerate()
                .map(|(i, t)| TextPart {
                    region: Some([0, 8 * i as u16, 16, 8]),
                    text: (*t).into(),
                    style: Style::Plain,
                    group: group.map(Into::into),
                    ..TextPart::default()
                })
                .collect(),
        };
        // Inked rows of each part's coverage.
        let rows = |o: &TextOverride| -> Vec<usize> {
            let overlay = TextOverlay::build_scaled(&b, [(0, SubRect::WHOLE, o)], &BoxGlyphs, 4);
            overlay
                .get(&(0, (0, 0, 1, 1)))
                .unwrap()
                .iter()
                .map(|p| {
                    let w = usize::from(p.region[2]);
                    p.coverage
                        .chunks(w)
                        .filter(|r| r.iter().any(|&c| c > 0))
                        .count()
                })
                .collect()
        };
        let alone = rows(&parts(None));
        assert!(alone[0] > alone[1], "{alone:?}");
        let grouped = rows(&parts(Some("cell")));
        assert_eq!(grouped[0], grouped[1]);
        assert_eq!(grouped[1], alone[1]);
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
    fn widgets_draw_in_their_layer_and_clip() {
        // A 2×1-tile map: index 1 (green) in the left tile, 2 (red) in the right one.
        let mut b = bank(1);
        b.tiles.push(Some(Box::new([2; TILE_PIXELS])));
        b.tilemaps[0] = Tilemap {
            width: 2,
            height: 1,
            tiles: vec![1, 2],
        };
        let mut l = SpriteLayer {
            scale: 2,
            ..SpriteLayer::default()
        };
        l.palettes[0] = Some(palette());
        // A slot on layer 1 under a layer-1 fill, which is under a layer-0 slot.
        l.slots[0] = Slot::new(0, 0, 0, 0, 0, true);
        l.slots[1] = Slot::new(0, 100, 100, 0, 1, true);
        let red = Paint {
            palette: 0,
            index: 2,
        };
        l.add_widget(1, [0.0, 0.0, 120.0, 120.0], WidgetKind::Fill(red));
        // The whole 32×16 map, one canvas pixel per art pixel, centred in a 10×20 rect at
        // (200, 200): 20 canvas pixels wide, so 6 art columns are cut off on each side.
        let sprite = WidgetKind::Sprite {
            tilemap: 0,
            src: [0, 0, 32, 16],
            zoom: 1,
            palette: 0,
        };
        l.add_widget(0, [200.0, 200.0, 10.0, 20.0], sprite);
        let (w, h) = l.canvas_size();
        let mut c = vec![0; w * h * 4];
        l.compose(&b, &mut c);
        let px = |x: usize, y: usize| -> [u8; 4] { c[(y * w + x) * 4..][..4].try_into().unwrap() };
        assert_eq!(px(5, 5), [0, 255, 0, 255], "layer-0 slot over the fill");
        assert_eq!(px(210, 210), [255, 0, 0, 255], "fill over the layer-1 slot");
        assert_eq!(px(239, 239), [255, 0, 0, 255]);
        assert_eq!(px(240, 240), [0, 0, 0, 0]);
        // Sprite: canvas x 400..420, y 400..440; art rows centred at 412..428.
        assert_eq!(px(399, 420), [0, 0, 0, 0]);
        assert_eq!(px(400, 420), [0, 255, 0, 255]);
        assert_eq!(px(409, 420), [0, 255, 0, 255]);
        assert_eq!(px(410, 420), [255, 0, 0, 255]);
        assert_eq!(px(419, 420), [255, 0, 0, 255]);
        assert_eq!(px(420, 420), [0, 0, 0, 0]);
        assert_eq!(px(405, 411), [0, 0, 0, 0]);
        assert_eq!(px(405, 412), [0, 255, 0, 255]);
        // Text without a label font draws nothing; widgets go with the per-tick reset.
        l.add_text(0, [300.0, 300.0, 50.0, 10.0], "Hi", red, None);
        l.compose(&b, &mut c);
        assert!((600..700).all(|x| (600..620).all(|y| c[(y * w + x) * 4 + 3] == 0)));
        l.reset_transient();
        assert!(l.widgets.is_empty());
    }

    #[test]
    fn text_widgets_use_the_label_font() {
        use crate::text::tests::BoxGlyphs;
        let mut b = bank(1);
        b.set_label_font(Some(LabelFont::new(Box::new(BoxGlyphs))));
        let mut l = SpriteLayer::default();
        l.palettes[0] = Some(palette());
        let green = Paint {
            palette: 0,
            index: 1,
        };
        l.add_text(0, [300.0, 300.0, 50.0, 10.0], "Hi", green, Some(5.0));
        let mut c = vec![0; CANVAS_W * CANVAS_H * 4];
        l.compose(&b, &mut c);
        let inside = (300..350)
            .flat_map(|x| (300..310).map(move |y| (x, y)))
            .filter(|&(x, y)| pixel(&c, x, y) == [0, 255, 0, 255])
            .count();
        assert!(inside > 0);
        for y in 0..CANVAS_H {
            for x in 0..CANVAS_W {
                if !(300..350).contains(&x) || !(300..310).contains(&y) {
                    assert_eq!(pixel(&c, x, y)[3], 0, "({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn repeats_are_found_in_the_tile_ids() {
        let map = |width: u8, row: &[u16]| Tilemap {
            width,
            height: 1,
            tiles: row.to_vec(),
        };
        // A pattern of period 3 after a different first block (the 0x501 shape).
        let r = Repeat::of(&map(8, &[9, 9, 9, 1, 2, 3, 1, 2])).unwrap();
        assert_eq!((r.start, r.period), (3, 3));
        assert_eq!([-1, 0, 2, 3, 8, 9].map(|c| r.source(c)), [5, 3, 5, 3, 5, 3]);
        // A uniform edge (a header band) repeats its last column.
        let r = Repeat::of(&map(6, &[4, 7, 7, 1, 1, 1])).unwrap();
        assert_eq!((r.start, r.period, r.source(-3), r.source(6)), (3, 1, 3, 3));
        // A frame is no pattern.
        assert_eq!(Repeat::of(&map(6, &[1, 2, 2, 2, 2, 3])), None);
    }

    #[test]
    fn backdrops_continue_past_the_design_on_a_wide_canvas() {
        // A 40 × 1 map: column c shows tile 1 + c % 2 (period 2), so index 1 or 2.
        let mut b = bank(1);
        b.tiles.push(Some(Box::new([2; TILE_PIXELS])));
        b.tilemaps[0] = Tilemap {
            width: 40,
            height: 1,
            tiles: (0..40).map(|c| 1 + c % 2).collect(),
        };
        assert!(b.set_wide_fill(0));
        let mut l = SpriteLayer::default();
        l.palettes[0] = Some(palette());
        l.fit_window(1280, 720);
        assert_eq!(l.wide, 213);
        l.slots[0] = Slot::new(0, 0, 0, 0, 3, true);
        let (w, h) = l.canvas_size();
        let mut c = vec![0; w * h * 4];
        l.compose(&b, &mut c);
        let px = |x: usize| -> [u8; 4] { c[x * 4..][..4].try_into().unwrap() };
        let (green, red) = ([0, 255, 0, 255], [255, 0, 0, 255]);
        // The design starts at 106: green there, red 16 px to the left (phase kept).
        assert_eq!((px(106), px(105), px(90), px(89)), (green, red, red, green));
        // Column -7 (odd) at the left edge, 46 (even) at the right one.
        assert_eq!((px(0), px(w - 1)), (red, green));
        // A slot that does not span the design, or a part of the map, is not continued.
        l.slots[0].x = 8;
        l.compose(&b, &mut c);
        assert_eq!(c[..4], [0; 4]);
        l.slots[0] = Slot {
            w: 4,
            ..Slot::new(0, 0, 0, 0, 3, true)
        };
        l.compose(&b, &mut c);
        assert_eq!(c[..4], [0; 4]);
    }

    #[test]
    fn anchors_move_elements_to_the_edges_of_a_wide_canvas() {
        let b = bank(1);
        let mut l = SpriteLayer {
            scale: 2,
            ..SpriteLayer::default()
        };
        l.palettes[0] = Some(palette());
        // 16:10 at 2×: 1536 × 960, 256 canvas pixels wider than 4:3.
        assert!(l.fit_window(1280, 800));
        assert!(!l.fit_window(2560, 1600));
        assert_eq!((l.wide, l.canvas_size()), (256, (1536, 960)));
        for (i, x) in [(0, 0), (1, 300), (2, 600)] {
            l.slots[i] = Slot::new(0, x, 10, 0, 0, true);
        }
        l.anchors[0] = Anchor::Left;
        l.anchors[2] = Anchor::Right;
        l.anchored(Anchor::Right, |l| {
            l.add_fan(
                &[[620, 100], [630, 100], [630, 110], [620, 110]],
                0xff_12_34_56,
            )
        });
        l.add_fan(&[[0, 100], [10, 100], [10, 110], [0, 110]], 0xff_12_34_56);
        let (w, h) = l.canvas_size();
        let mut c = vec![0; w * h * 4];
        l.compose(&b, &mut c);
        let at = |c: &[u8], x: usize, y: usize| -> [u8; 4] {
            c[(y * w + x) * 4..][..4].try_into().unwrap()
        };
        let px = |x: usize, y: usize| at(&c, x, y);
        let green = [0, 255, 0, 255];
        // Left: at the canvas edge; centre: +128; right: +256. Each tile stays 32 × 32.
        assert_eq!(px(0, 20), green);
        assert_eq!(px(31, 51), green);
        assert_eq!(px(32, 20), [0; 4]);
        assert_eq!(px(600 + 128, 20), green);
        assert_eq!(px(600 + 127, 20), [0; 4]);
        assert_eq!(px(1200 + 256, 20), green);
        assert_eq!(px(1200 + 256 + 31, 51), green);
        // Fans: the right one keeps its distance to the right edge; the other is centred.
        assert_eq!(px(1240 + 256, 210), [0x12, 0x34, 0x56, 255]);
        assert_eq!(px(128, 210), [0x12, 0x34, 0x56, 255]);
        assert_eq!(px(0, 210), [0; 4]);
        // Back to centred: the 4:3 layout in the middle.
        l.centre_all();
        let mut c2 = vec![0; w * h * 4];
        l.compose(&b, &mut c2);
        assert_eq!(at(&c2, 128, 20), green);
        assert_eq!(at(&c2, 0, 20), [0; 4]);
    }

    #[test]
    fn text_outline_and_backdrop() {
        use crate::text::tests::BoxGlyphs;
        let mut b = bank(1);
        b.set_label_font(Some(LabelFont::new(Box::new(BoxGlyphs))));
        let mut l = SpriteLayer::default();
        l.palettes[0] = Some(palette());
        let (green, red) = (
            Paint {
                palette: 0,
                index: 1,
            },
            Paint {
                palette: 0,
                index: 2,
            },
        );
        let style = TextStyle {
            outline: Some(red),
            align: Align::Left,
            ..TextStyle::plain(green, Some(6.0))
        };
        l.add_styled_text(0, [100.0, 100.0, 200.0, 20.0], "Hi", style);
        let boxed = TextStyle {
            backdrop: Some(red),
            ..TextStyle::plain(green, Some(6.0))
        };
        l.add_styled_text(0, [100.0, 200.0, 200.0, 20.0], "Hi", boxed);
        let mut c = vec![0; CANVAS_W * CANVAS_H * 4];
        l.compose(&b, &mut c);
        let count = |y0: usize, rgba: [u8; 4]| {
            (100..300)
                .flat_map(|x| (y0..y0 + 20).map(move |y| (x, y)))
                .filter(|&(x, y)| pixel(&c, x, y) == rgba)
                .count()
        };
        let (g, r) = ([0, 255, 0, 255], [255, 0, 0, 255]);
        assert!(count(100, g) > 0 && count(100, r) > 0);
        // Left-aligned: nothing in the right half.
        assert!((200..300).all(|x| (100..120).all(|y| pixel(&c, x, y)[3] == 0)));
        // The box spans the full height around the centred text, not the whole rectangle.
        assert_eq!(pixel(&c, 200, 200), r);
        assert_eq!(pixel(&c, 200, 219), r);
        assert_eq!(pixel(&c, 101, 210)[3], 0);
        assert!(count(200, g) > 0);
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

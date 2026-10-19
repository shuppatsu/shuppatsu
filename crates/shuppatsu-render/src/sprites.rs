//! The original's 2D layers over the 3D view (docs/re/hud.md §1.1, §6): the software sprite
//! layer and the rect layer, composed on the CPU into a 640×480 RGBA canvas exactly as the
//! original blits them into its back buffer. The canvas is then drawn as one overlay texture.

use crate::label::LabelFont;
use crate::text::{Align, FontRole, SubRect, TextOverlay, draw_parts};
use hires::{Hires, View};
pub use paper::Paper;
use paper::{PaperCache, PaperFill};
use shuppatsu_formats::cg::{CgFile, Palette, TILE_PIXELS, TILE_SIZE, Tilemap};
use std::sync::{Arc, Mutex};

pub mod hires;
#[cfg(test)]
mod hires_tests;
pub mod legend;
mod paper;
pub mod xbrz;

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
    /// The `src` rectangle (`[x, y, w, h]` in art pixels) of a tilemap, `scale` original
    /// pixels per art pixel (the same in x and y, so never stretched), centred in the widget's
    /// rectangle and clipped to it. Index 0 is transparent, as in a slot. At a whole number of
    /// canvas pixels per art pixel this is the plain nearest-neighbour blit; otherwise each
    /// canvas pixel is the area average of the art under it (the art enlarged with nearest
    /// neighbour and box-filtered), so only edge pixels blend: sharp bilinear when enlarging,
    /// an area downscale when shrinking ([`resample_sprite`]).
    Sprite {
        tilemap: u16,
        src: [u16; 4],
        scale: f32,
        palette: u8,
    },
    /// Text in the rectangle at canvas resolution, uniformly scaled, in the bank's label font
    /// ([`SpriteBank::set_label_font`]), styled by [`TextStyle`].
    Text { text: String, style: TextStyle },
    /// A line of controller glyphs and text ([`legend`]), laid out at canvas resolution in the
    /// label font: `style.paint` colours the text, `style.cap` sizes it (glyphs are
    /// [`legend::GLYPH_PER_CAP`] times taller), `style.align` places the line.
    Legend {
        parts: Vec<legend::LegendPart>,
        style: TextStyle,
    },
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
    /// The label font role: [`FontRole::Display`] uses the display font when one is
    /// installed ([`SpriteBank::set_display_label_font`]), else the body font.
    pub font: FontRole,
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
            font: FontRole::Body,
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
    /// The font of display-role labels ([`TextStyle::font`]); `None` uses `labels`.
    display_labels: Option<LabelFont>,
    /// The font of [`WidgetKind::Legend`] (controller glyphs and their words), independent of
    /// the language: hints are drawn in Japanese too, where `labels` is mostly absent.
    legend_font: Option<LabelFont>,
    /// Enhancement `wide_menus`: backdrops continued sideways on a wide canvas, by tilemap id
    /// ([`SpriteBank::set_wide_fill`], [`SpriteBank::set_wide_paper`]).
    wide_fill: Vec<(u16, WideFill)>,
    /// The paper fills synthesised for [`WideFill::Paper`] maps, one per map ([`PaperCache`]:
    /// kept across palette fades; at art resolution, so one serves every canvas scale).
    paper: Mutex<Vec<PaperCache>>,
    /// Enhancement `hud_upscale`: the art drawn from xBRZ-upscaled images ([`hires`]) at
    /// canvas multiples 2–4; off draws whole-pixel blocks, as the original.
    upscale: bool,
    /// The upscaled images, built on first use.
    hires: Mutex<hires::Cache>,
}

/// How a backdrop continues past the edges of the 4:3 design on a wide canvas (enhancement
/// `wide_menus`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WideFill {
    /// Its tile pattern repeats ([`Repeat`]).
    Repeat(Repeat),
    /// A picture without a pattern continues with paper synthesised from its plain edges
    /// ([`Paper`]).
    Paper(Paper),
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
            display_labels: None,
            legend_font: None,
            wide_fill: Vec::new(),
            paper: Mutex::default(),
            upscale: false,
            hires: Mutex::default(),
        }
    }

    /// Enhancement `hud_upscale`: draws the art from xBRZ-upscaled palette indices at canvas
    /// multiples 2–4 ([`hires`]); off (the default) draws each art pixel as a block.
    pub fn set_upscale(&mut self, on: bool) {
        self.upscale = on;
    }

    pub fn upscale(&self) -> bool {
        self.upscale
    }

    /// Upscaled images built so far ([`hires::Cache::builds`]).
    pub fn upscale_builds(&self) -> usize {
        self.hires.lock().unwrap_or_else(|e| e.into_inner()).builds
    }

    /// Forgets the upscaled images, so they are built again from the current tiles and
    /// tilemaps on their next use (a replaced tilemap is noticed without this).
    pub fn clear_upscaled(&mut self) {
        self.hires
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// The upscaled image of `view` of map `tbl` at canvas multiple `scale`, when
    /// `hud_upscale` is on and the multiple has one; `palette` is the one the map is drawn
    /// with (the reference for the edge decisions on first use).
    fn hires(
        &self,
        tbl: u16,
        scale: usize,
        view: View,
        palette: &Palette,
        repeat: Option<Repeat>,
    ) -> Option<Arc<Hires>> {
        if !self.upscale || !hires::MULTIPLES.contains(&scale) {
            return None;
        }
        let map = self.tilemaps.get(usize::from(tbl))?;
        let key = hires::Key { tbl, scale, view };
        let mut cache = self.hires.lock().unwrap_or_else(|e| e.into_inner());
        cache.get(key, map, palette, || {
            let (mw, mh) = (i32::from(map.width), i32::from(map.height));
            let cols: Vec<i32> = match view {
                View::Whole => (0..mw).collect(),
                View::Wide { first, last } => (first..=last)
                    .map(|tx| match repeat {
                        Some(r) if !(0..mw).contains(&tx) => r.source(tx),
                        _ => tx,
                    })
                    .collect(),
            };
            let rows: Vec<i32> = (0..mh).collect();
            let px = self.render_cells(map, &cols, &rows);
            (cols.len() * TILE_SIZE, rows.len() * TILE_SIZE, px)
        })
    }

    /// Enhancement `wide_menus`: marks tilemap `tbl` as a backdrop that a wide canvas
    /// continues sideways, repeating its pattern ([`Repeat::of`]), wherever a slot shows the
    /// whole map across the 4:3 design. Returns whether the map has a pattern to repeat; one
    /// without keeps the original's centred picture.
    pub fn set_wide_fill(&mut self, tbl: u16) -> bool {
        let Some(r) = self.tilemaps.get(usize::from(tbl)).and_then(Repeat::of) else {
            return false;
        };
        self.put_wide_fill(tbl, WideFill::Repeat(r));
        true
    }

    /// Enhancement `wide_menus`: marks tilemap `tbl` as a picture that a wide canvas
    /// continues sideways with paper synthesised from its plain edges ([`Paper`]): for a
    /// picture with plain texture at its edges but no tile pattern. Returns whether the map
    /// exists.
    pub fn set_wide_paper(&mut self, tbl: u16, paper: Paper) -> bool {
        if self.tilemaps.get(usize::from(tbl)).is_none() {
            return false;
        }
        self.put_wide_fill(tbl, WideFill::Paper(paper));
        true
    }

    /// The paper fill of map `tbl` drawn with `palette`, reaching at least `reach` columns
    /// past each edge: built on first use and kept ([`PaperCache`]) until the palette changes
    /// other than by a fade or a wider canvas needs more; `None` when `spec` does not fit the
    /// map.
    fn paper_fill(
        &self,
        tbl: u16,
        spec: &Paper,
        palette: &Palette,
        reach: usize,
    ) -> Option<Arc<PaperFill>> {
        let mut cache = self.paper.lock().unwrap_or_else(|e| e.into_inner());
        let slot = cache.iter().position(|e| e.tbl == tbl);
        if let Some(e) = slot.map(|i| &mut cache[i]).filter(|e| e.reach >= reach)
            && let Some(fill) = e.get(palette)
        {
            return fill;
        }
        // Rounded up, so a window being resized does not rebuild it every frame.
        let reach = reach
            .max(slot.map_or(0, |i| cache[i].reach))
            .next_multiple_of(64);
        let (w, h, px) = self.art(tbl)?;
        let entry = PaperCache::build(tbl, spec, (w, h, &px), palette, reach);
        let fill = entry.get_built();
        match slot {
            Some(i) => cache[i] = entry,
            None => cache.push(entry),
        }
        fill
    }

    fn put_wide_fill(&mut self, tbl: u16, fill: WideFill) {
        // A changed spec rebuilds the fill.
        self.paper
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|e| e.tbl != tbl);
        match self.wide_fill.iter_mut().find(|(t, _)| *t == tbl) {
            Some(e) => e.1 = fill,
            None => self.wide_fill.push((tbl, fill)),
        }
    }

    fn wide_fill(&self, tbl: u16) -> Option<WideFill> {
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

    /// Installs (or with `None` removes) the font of display-role labels; without one they
    /// use the body label font.
    pub fn set_display_label_font(&mut self, font: Option<LabelFont>) {
        self.display_labels = font;
    }

    pub fn has_label_font(&self) -> bool {
        self.labels.is_some()
    }

    /// The label font of `role`: the display font falls back to the body font.
    /// Installs (or with `None` removes) the font of [`WidgetKind::Legend`]; without one
    /// legends use the body label font.
    pub fn set_legend_font(&mut self, font: Option<LabelFont>) {
        self.legend_font = font;
    }

    fn label_font(&self, role: FontRole) -> Option<&LabelFont> {
        match role {
            FontRole::Display => self.display_labels.as_ref().or(self.labels.as_ref()),
            FontRole::Body => self.labels.as_ref(),
        }
    }

    /// The width of `text` in the label font with capitals `cap` pixels high (any unit: the
    /// result is in the same one); `None` without a label font.
    pub fn label_width(&self, text: &str, cap: f32) -> Option<f32> {
        Some(self.labels.as_ref()?.width(text, cap))
    }

    /// [`SpriteBank::label_width`] in the font of `role`, with the ink of the (one-line) text
    /// relative to its baseline: `(width, top, bottom)` ([`LabelFont::ink`]).
    pub fn label_extent(&self, text: &str, cap: f32, role: FontRole) -> Option<(f32, f32, f32)> {
        let f = self.label_font(role)?;
        let (top, bottom) = f.ink(text, cap);
        Some((f.width(text, cap), top, bottom))
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
    pub fn render_rect(&self, tbl: u16, (c0, r0, w, h): (u8, u8, u8, u8)) -> Vec<u8> {
        let cols: Vec<i32> = (0..i32::from(w)).map(|t| i32::from(c0) + t).collect();
        let rows: Vec<i32> = (0..i32::from(h)).map(|t| i32::from(r0) + t).collect();
        match self.tilemaps.get(usize::from(tbl)) {
            Some(map) => self.render_cells(map, &cols, &rows),
            None => vec![0; cols.len() * rows.len() * TILE_PIXELS],
        }
    }

    /// Palette indices of the tiles of `map` at columns `cols` and rows `rows`, side by side
    /// in that order; cells outside the map, empty cells and missing tiles are 0.
    fn render_cells(&self, map: &Tilemap, cols: &[i32], rows: &[i32]) -> Vec<u8> {
        let pw = cols.len() * TILE_SIZE;
        let mut out = vec![0u8; pw * rows.len() * TILE_SIZE];
        let (mw, mh) = (i32::from(map.width), i32::from(map.height));
        for (ty, &mr) in rows.iter().enumerate() {
            for (tx, &mc) in cols.iter().enumerate() {
                if !(0..mw).contains(&mc) || !(0..mh).contains(&mr) {
                    continue;
                }
                let id = map.tiles[(mr * mw + mc) as usize];
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
    /// Slots cut at the edges of the 4:3 design (original x 0..640, moved with the slot's
    /// anchor) on a wide canvas, where the original's screen edge cut them; rebuilt every tick
    /// by their owner like the widgets ([`SpriteLayer::frame`]).
    pub framed: Vec<usize>,
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
            framed: Vec::new(),
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
        self.framed.clear();
    }

    /// Cuts `slots` at the edges of the 4:3 design on a wide canvas this tick, as the
    /// original's 640-pixel screen cut them (a picture partly off the screen, like the coupling
    /// minigame's partner train). No effect on the 4:3 canvas, whose edges are the design's.
    pub fn frame(&mut self, slots: impl IntoIterator<Item = usize>) {
        self.framed.extend(slots);
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
                    if self.wide > 0 && self.framed.contains(&i) {
                        self.blit_framed(bank, slot, (sx, sy), shift, c, canvas);
                    } else {
                        self.blit(bank, slot, sx, sy, shift, c, canvas);
                    }
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

    /// [`Self::blit`] cut at the 4:3 design's edges ([`SpriteLayer::frame`]): the canvas
    /// columns outside them, on the rows the slot covers, are put back as they were.
    fn blit_framed(
        &self,
        bank: &SpriteBank,
        slot: &Slot,
        (sx, sy): (i32, i32),
        shift: i32,
        c: Canvas,
        canvas: &mut [u8],
    ) {
        let Some(map) = usize::try_from(slot.tilemap)
            .ok()
            .and_then(|id| bank.tilemaps.get(id))
        else {
            return;
        };
        let s = c.s as i32;
        let rows = if slot.h <= 0 {
            i32::from(map.height)
        } else {
            i32::from(slot.h)
        };
        let top = (i32::from(slot.y) + sy) * s;
        let ys = top.clamp(0, c.h as i32)..(top + rows * TILE_SIZE as i32 * s).clamp(0, c.h as i32);
        let (x0, x1) = (
            shift.clamp(0, c.w as i32) as usize,
            (shift + (CANVAS_W * c.s) as i32).clamp(0, c.w as i32) as usize,
        );
        let row = |y: i32| y as usize * c.w * 4;
        let saved: Vec<(Vec<u8>, Vec<u8>)> = ys
            .clone()
            .map(|y| {
                let r = row(y);
                (
                    canvas[r..r + x0 * 4].to_vec(),
                    canvas[r + x1 * 4..r + c.w * 4].to_vec(),
                )
            })
            .collect();
        self.blit(bank, slot, sx, sy, shift, c, canvas);
        for (y, (left, right)) in ys.zip(saved) {
            let r = row(y);
            canvas[r..r + x0 * 4].copy_from_slice(&left);
            canvas[r + x1 * 4..r + c.w * 4].copy_from_slice(&right);
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
        // continuation past both edges of the design (`wide_menus`), by repeat or paper.
        let mut cols: Vec<i32> = (0..w).collect();
        let fill = self.backdrop_fill(bank, slot, mw);
        // The original pixels the canvas shows: [left, right).
        let left = (-shift).div_euclid(s);
        let right = (c.w as i32 - shift + s - 1).div_euclid(s);
        let repeat = match fill {
            Some(WideFill::Repeat(r)) => Some(r),
            _ => None,
        };
        let paper = match fill {
            Some(WideFill::Paper(p)) => {
                let reach = (sx0 - left).max(right - sx0 - mw * ts).max(0) as usize;
                u16::try_from(slot.tilemap)
                    .ok()
                    .and_then(|t| bank.paper_fill(t, &p, palette, reach))
            }
            _ => None,
        };
        // A paper fill covers the picture's outermost columns, so those are not drawn twice.
        if let Some(p) = &paper {
            let (a, b) = p.inner();
            cols.retain(|&tx| (tx + 1) * ts > a && tx * ts < b);
        }
        if repeat.is_some() {
            let first = (left - sx0).div_euclid(ts);
            let last = (right - sx0 - 1).div_euclid(ts);
            cols.extend((first..0).chain(mw..=last));
        }
        // Enhancement `hud_upscale`: the art from the map's upscaled image, if it has one.
        let view = match repeat {
            Some(_) => View::Wide {
                first: cols.iter().copied().min().unwrap_or(0),
                last: cols.iter().copied().max().unwrap_or(0),
            },
            None => View::Whole,
        };
        let hires = u16::try_from(slot.tilemap)
            .ok()
            .and_then(|t| bank.hires(t, c.s, view, palette, repeat));
        let colours: [[u8; 4]; 256] = match hires {
            Some(_) => std::array::from_fn(|i| palette.rgba(i as u8)),
            None => [[0; 4]; 256],
        };
        for ty in 0..h {
            for &tx in &cols {
                let (mc, mr) = match repeat {
                    Some(r) if !(0..w).contains(&tx) => (r.source(tx), r0 + ty),
                    _ => (c0 + tx, r0 + ty),
                };
                if !(0..mw).contains(&mc) || !(0..mh).contains(&mr) {
                    continue;
                }
                let ox = sx0 + tx * ts;
                let oy = sy0 + ty * ts;
                if let Some(hi) = &hires {
                    // Every cell, empty ones too: shaped edges reach into them.
                    let (ic, ir) = match view {
                        View::Whole => (mc, mr),
                        View::Wide { first, .. } => (tx - first, ty),
                    };
                    // Translated text only covers the slot's own columns.
                    let open = text.filter(|_| (0..w).contains(&tx)).map(|parts| {
                        std::array::from_fn(|k| {
                            let bx = (tx * ts) as usize + k % TILE_SIZE;
                            let by = (ty * ts) as usize + k / TILE_SIZE;
                            !parts.iter().any(|p| p.contains(bx * c.s, by * c.s))
                        })
                    });
                    let cell = Cell {
                        image: (ic, ir),
                        at: (ox * s + shift, oy * s),
                        open,
                    };
                    blit_hires(hi, &cell, &colours, c, canvas);
                    continue;
                }
                let id = map.tiles[(mr * mw + mc) as usize];
                let Some(tile) = (id != 0).then(|| bank.tile(id)).flatten() else {
                    continue;
                };
                for py in 0..ts {
                    for px in 0..ts {
                        let index = tile[(py * ts + px) as usize];
                        if index == 0 {
                            continue;
                        }
                        // Translated text only covers the slot's own columns; the wide-fill
                        // columns outside them have negative `tx`, so test that first.
                        let covered = (0..w).contains(&tx)
                            && text.is_some_and(|parts| {
                                let lx = (tx * ts + px) as usize;
                                let (bx, by) = (lx * c.s, (ty * ts + py) as usize * c.s);
                                parts.iter().any(|p| p.contains(bx, by))
                            });
                        if covered {
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
        if let Some(p) = &paper {
            draw_paper(p, (sx0, sy0), shift, c, canvas);
        }
        if let Some(parts) = text {
            draw_parts(parts, palette, sx0 * s + shift, sy0 * s, canvas, c.w, c.h);
        }
    }

    /// Enhancement `wide_menus`: the continuation of a backdrop slot on a wide canvas. Only a
    /// slot showing a whole marked map ([`SpriteBank::set_wide_fill`],
    /// [`SpriteBank::set_wide_paper`]) across the full 640-px design is continued;
    /// everything else keeps its 4:3 place.
    fn backdrop_fill(&self, bank: &SpriteBank, slot: &Slot, mw: i32) -> Option<WideFill> {
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
                scale,
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
                // Canvas pixels per art pixel.
                let z = scale * s;
                if z.is_nan() || z <= 0.0 || sw == 0 || sh == 0 {
                    return;
                }
                // The sprite's size and its top left, centred in the unclipped rectangle on a
                // whole canvas pixel (signed canvas pixels).
                let (dw, dh) = (sw as f32 * z, sh as f32 * z);
                let cx = ((x + sx as f32 + rw / 2.0) * s + shift).round() as i64
                    - (dw / 2.0).floor() as i64;
                let cy =
                    ((y + sy as f32 + rh / 2.0) * s).round() as i64 - (dh / 2.0).floor() as i64;
                let colours: Vec<[u8; 4]> = (0..=255u8).map(|i| pal.rgba(i)).collect();
                let src = Art {
                    px: &art,
                    stride: aw,
                    rect: [ax, ay, sw, sh],
                };
                resample_sprite(&src, &colours, z, (cx, cy), [x0, y0, x1, y1], c, canvas);
            }
            WidgetKind::Legend { parts, style } => {
                let Some(font) = bank.legend_font.as_ref().or(bank.label_font(style.font)) else {
                    return;
                };
                // Glyphs carry their own colours; the words take the paint (white without it).
                let [r, g, b, _] =
                    palette(style.paint.palette).map_or([0xff; 4], |p| p.rgba(style.paint.index));
                // A backdrop paint outside the palettes is near-black.
                let backdrop = style.backdrop.map(|p| {
                    let [r, g, b, _] = palette(p.palette).map_or([0x14; 4], |q| q.rgba(p.index));
                    [r, g, b]
                });
                let cap = style.cap.map_or(f32::MAX, |v| v * s);
                legend::draw_legend(
                    parts,
                    cap,
                    style.align,
                    [r, g, b],
                    backdrop,
                    font,
                    [x0, y0, x1, y1],
                    c,
                    canvas,
                );
            }
            WidgetKind::Text { text, style } => {
                let paint = style.paint;
                let (Some(pal), Some(font)) = (palette(paint.palette), bank.label_font(style.font))
                else {
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

/// Palette indices of a sprite: the `rect` (`[x, y, w, h]`) of a `stride`-wide image.
struct Art<'a> {
    px: &'a [u8],
    stride: usize,
    rect: [usize; 4],
}

/// The art pixels a canvas pixel `i` covers at `z` canvas pixels per art pixel, as
/// `(art pixel, overlap)` pairs: the pixel's footprint `[i / z, (i + 1) / z)` cut to `0..n`.
fn footprint(i: i64, z: f32, n: usize) -> impl Iterator<Item = (usize, f32)> {
    let (a, b) = (i as f32 / z, (i + 1) as f32 / z);
    let first = a.floor().max(0.0) as usize;
    let last = (b.ceil().max(0.0) as usize).min(n);
    (first..last).filter_map(move |k| {
        let w = (b.min(k as f32 + 1.0) - a.max(k as f32)).max(0.0);
        (w > 0.0).then_some((k, w))
    })
}

/// Draws `art` with its top left at canvas pixel `origin`, `z` canvas pixels per art pixel,
/// inside the canvas rectangle `clip` (`[x0, y0, x1, y1]`), colours by palette index
/// (`colours[0]` is unused: index 0 is transparent). Each canvas pixel gets the area average
/// of the art under it, premultiplied by coverage (a box filter of the art enlarged with
/// nearest neighbour): at a whole `z` this is the exact pixel-doubling blit. Partly covered
/// pixels blend over the canvas in sRGB like the label text, or over transparency keep their
/// coverage as alpha (the overlay's alpha test then decides).
fn resample_sprite(
    art: &Art<'_>,
    colours: &[[u8; 4]],
    z: f32,
    (ox, oy): (i64, i64),
    [x0, y0, x1, y1]: [usize; 4],
    c: Canvas,
    canvas: &mut [u8],
) {
    let [ax, ay, sw, sh] = art.rect;
    let (dw, dh) = ((sw as f32 * z).ceil() as i64, (sh as f32 * z).ceil() as i64);
    let area = 1.0 / (z * z);
    let xs = (x0 as i64).max(ox)..(x1 as i64).min(ox + dw);
    for py in (y0 as i64).max(oy)..(y1 as i64).min(oy + dh) {
        let rows: Vec<(usize, f32)> = footprint(py - oy, z, sh).collect();
        for px in xs.clone() {
            let mut sum = [0.0f32; 4];
            for (col, wx) in footprint(px - ox, z, sw) {
                for &(row, wy) in &rows {
                    let index = art.px[(ay + row) * art.stride + ax + col];
                    if index == 0 {
                        continue;
                    }
                    let w = wx * wy;
                    let rgba = colours[usize::from(index)];
                    for (s, v) in sum.iter_mut().zip(rgba) {
                        *s += f32::from(v) * w;
                    }
                }
            }
            if sum[3] <= 0.0 {
                continue;
            }
            // Coverage, and the straight colour of the covered part.
            let a = (sum[3] / 255.0 / area).min(1.0);
            let colour = [0, 1, 2].map(|i| (sum[i] * 255.0 / sum[3]).round() as u8);
            let o = (py as usize * c.w + px as usize) * 4;
            let under: [u8; 4] = canvas[o..o + 4].try_into().expect("4 bytes");
            let out = if a >= 1.0 {
                [colour[0], colour[1], colour[2], 255]
            } else if under[3] == 0 {
                [colour[0], colour[1], colour[2], (a * 255.0).round() as u8]
            } else {
                let mix =
                    |u: u8, v: u8| (f32::from(u) * (1.0 - a) + f32::from(v) * a).round() as u8;
                [
                    mix(under[0], colour[0]),
                    mix(under[1], colour[1]),
                    mix(under[2], colour[2]),
                    under[3].max((a * 255.0).round() as u8),
                ]
            };
            canvas[o..o + 4].copy_from_slice(&out);
        }
    }
}

/// One tile cell of an upscaled image: its tile column and row in the image, the canvas
/// pixel of its top left and, under translated text, which of its art pixels are drawn.
struct Cell {
    image: (i32, i32),
    at: (i32, i32),
    open: Option<[bool; TILE_PIXELS]>,
}

/// Draws one tile cell of an upscaled image ([`hires`]) at canvas multiple `c.s`: index 0
/// transparent, other indices in `colours` (the slot's palette).
fn blit_hires(hi: &Hires, cell: &Cell, colours: &[[u8; 4]; 256], c: Canvas, canvas: &mut [u8]) {
    let t = hires::tile_px(c.s) as i32;
    let s = c.s as i32;
    let (ox, oy) = cell.at;
    let (ic, ir) = cell.image;
    let xs = (-ox).max(0)..(c.w as i32 - ox).min(t);
    let ys = (-oy).max(0)..(c.h as i32 - oy).min(t);
    if xs.is_empty() || ys.is_empty() {
        return;
    }
    let x0 = (ic * t + xs.start) as usize;
    let x1 = (ic * t + xs.end) as usize;
    for yy in ys {
        let row = hi.row(x0, x1, (ir * t + yy) as usize);
        let line = ((oy + yy) as usize * c.w + (ox + xs.start) as usize) * 4;
        let (dst, _) = canvas[line..line + row.len() * 4].as_chunks_mut::<4>();
        match &cell.open {
            None => {
                for (px, &i) in dst.iter_mut().zip(row) {
                    if i != 0 {
                        *px = colours[usize::from(i)];
                    }
                }
            }
            Some(open) => {
                let art_row = (yy / s) as usize * TILE_SIZE;
                for (k, (px, &i)) in dst.iter_mut().zip(row).enumerate() {
                    let art = art_row + ((xs.start + k as i32) / s) as usize;
                    if i != 0 && open[art] {
                        *px = colours[usize::from(i)];
                    }
                }
            }
        }
    }
}

/// Draws a paper fill ([`WideFill::Paper`]) of a picture whose top left is at original pixel
/// `(x0, y0)`: its ramp columns over the picture's edges and the fill out to the canvas edges,
/// each original pixel an `s × s` block like the art. Opaque.
fn draw_paper(p: &PaperFill, (x0, y0): (i32, i32), shift: i32, c: Canvas, canvas: &mut [u8]) {
    let s = c.s as i32;
    let (a, b) = p.inner();
    // The canvas columns of the two sides: left of the picture's column `a`, from `b` on.
    let left = 0..((x0 + a) * s + shift).clamp(0, c.w as i32);
    let right = ((x0 + b) * s + shift).clamp(0, c.w as i32)..c.w as i32;
    for y in 0..c.h as i32 {
        let py = y.div_euclid(s) - y0;
        for x in left.clone().chain(right.clone()) {
            let px = (x - shift).div_euclid(s) - x0;
            if let Some(rgba) = p.at(px, py) {
                let o = (y as usize * c.w + x as usize) * 4;
                canvas[o..o + 4].copy_from_slice(&rgba);
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
            display_labels: None,
            legend_font: None,
            wide_fill: Vec::new(),
            paper: Mutex::default(),
            upscale: false,
            hires: Mutex::default(),
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

    /// Inked rows and columns of the first part of tilemap `t` in `overlay`.
    fn ink_extent(overlay: &TextOverlay, t: u16) -> (usize, usize) {
        let p = &overlay.get(&(t, (0, 0, 1, 1))).unwrap()[0];
        let (w, h) = (usize::from(p.region[2]), usize::from(p.region[3]));
        let rows = (0..h)
            .filter(|y| p.coverage[y * w..][..w].iter().any(|&c| c > 0))
            .count();
        let cols = (0..w)
            .filter(|&x| (0..h).any(|y| p.coverage[y * w + x] > 0))
            .count();
        (rows, cols)
    }

    #[test]
    fn display_parts_use_the_display_font() {
        use crate::text::{
            FontRole, Fonts, Style, TextOverride, TextPart,
            tests::{BoxGlyphs, Stretched},
        };
        let b = bank(3);
        let entry = |font| TextOverride {
            parts: vec![TextPart {
                text: "I".into(),
                style: Style::Plain,
                font,
                ..TextPart::default()
            }],
        };
        let (body, display) = (entry(FontRole::Body), entry(FontRole::Display));
        let narrow = Stretched { w: 0.5, h: 1.0 };
        let two = Fonts {
            body: &BoxGlyphs,
            display: Some(&narrow),
        };
        let build = |o: &TextOverride, fonts| {
            ink_extent(
                &TextOverlay::build_with(&b, [(0, SubRect::WHOLE, o)], fonts, 4),
                0,
            )
        };
        let plain = build(&body, two);
        // A display part is drawn in the display font (narrower here; one letter, so the
        // size is set by the lettering height, not the width) ...
        assert!(build(&display, two).1 < plain.1, "{plain:?}");
        // ... a body part is not affected by it ...
        assert_eq!(build(&body, Fonts::one(&BoxGlyphs)), plain);
        // ... and without a display font, display parts use the body font.
        assert_eq!(build(&display, Fonts::one(&BoxGlyphs)), plain);
    }

    #[test]
    fn shared_groups_span_font_roles() {
        use crate::text::{
            FontRole, Fonts, Style, TextOverride, TextPart,
            tests::{BoxGlyphs, Stretched},
        };
        let mut b = bank(3);
        b.tilemaps.push(b.tilemaps[0].clone());
        let entry = |text: &str, font, shared: Option<&str>| TextOverride {
            parts: vec![TextPart {
                text: text.into(),
                style: Style::Plain,
                font,
                shared_group: shared.map(Into::into),
                ..TextPart::default()
            }],
        };
        // The display font's capitals are 1.5 times as tall per font pixel.
        let tall = Stretched { w: 1.0, h: 1.5 };
        let fonts = Fonts {
            body: &BoxGlyphs,
            display: Some(&tall),
        };
        let rows = |shared| {
            let a = entry("Hi", FontRole::Display, shared);
            let c = entry("Hello there", FontRole::Body, shared);
            let list = [(0, SubRect::WHOLE, &a), (1, SubRect::WHOLE, &c)];
            let overlay = TextOverlay::build_with(&b, list, fonts, 4);
            [ink_extent(&overlay, 0).0, ink_extent(&overlay, 1).0]
        };
        let alone = rows(None);
        assert!(alone[0] > alone[1] + 2, "{alone:?}");
        // One capital height for the group, though the fonts differ.
        let shared = rows(Some("list"));
        assert!(shared[0].abs_diff(shared[1]) <= 1, "{shared:?}");
        assert_eq!(shared[1], alone[1]);
    }

    #[test]
    fn shared_groups_span_entries_and_tilemaps() {
        use crate::text::{Style, SubRect, TextOverlay, TextOverride, TextPart, tests::BoxGlyphs};
        let mut b = bank(3);
        b.tilemaps.push(b.tilemaps[0].clone());
        let entry = |text: &str, shared: Option<&str>| TextOverride {
            parts: vec![TextPart {
                text: text.into(),
                style: Style::Plain,
                shared_group: shared.map(Into::into),
                ..TextPart::default()
            }],
        };
        // Inked rows of the text of each tilemap.
        let rows = |a: &TextOverride, c: &TextOverride| -> Vec<usize> {
            let list = [(0, SubRect::WHOLE, a), (1, SubRect::WHOLE, c)];
            let overlay = TextOverlay::build_scaled(&b, list, &BoxGlyphs, 4);
            (0..2)
                .map(|t| {
                    let p = &overlay.get(&(t, (0, 0, 1, 1))).unwrap()[0];
                    let w = usize::from(p.region[2]);
                    p.coverage
                        .chunks(w)
                        .filter(|r| r.iter().any(|&c| c > 0))
                        .count()
                })
                .collect()
        };
        let alone = rows(&entry("Hi", None), &entry("Hello there", None));
        assert!(alone[0] > alone[1], "{alone:?}");
        let shared = rows(
            &entry("Hi", Some("list")),
            &entry("Hello there", Some("list")),
        );
        assert_eq!(shared, [alone[1], alone[1]]);
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
            scale: 0.5,
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

    /// Fractional sprite scales: an area average of the art under each canvas pixel.
    #[test]
    fn sprite_widgets_resample_at_fractional_scales() {
        // One tile, columns alternating red (2) and green (1).
        let mut tile = [0u8; TILE_PIXELS];
        for (i, v) in tile.iter_mut().enumerate() {
            *v = if i % 2 == 0 { 2 } else { 1 };
        }
        let mut b = bank(1);
        b.tiles[1] = Some(Box::new(tile));
        let sprite = |scale| WidgetKind::Sprite {
            tilemap: 0,
            src: [0, 0, 16, 16],
            scale,
            palette: 0,
        };
        let draw = |canvas_scale: usize, scale: f32| {
            let mut l = SpriteLayer {
                scale: canvas_scale,
                ..SpriteLayer::default()
            };
            l.palettes[0] = Some(palette());
            l.add_widget(0, [100.0, 100.0, 40.0, 40.0], sprite(scale));
            let (w, h) = l.canvas_size();
            let mut c = vec![0; w * h * 4];
            l.compose(&b, &mut c);
            move |x: usize, y: usize| -> [u8; 4] { c[(y * w + x) * 4..][..4].try_into().unwrap() }
        };
        // Canvas multiple 3, half an original pixel per art pixel: 1.5 canvas pixels per art
        // pixel, 24 × 24 centred on (360, 360). Pixel 0 is all red; pixel 1 is a third red and
        // a third green; pixel 2 all green.
        let px = draw(3, 0.5);
        assert_eq!(px(347, 350), [0, 0, 0, 0]);
        assert_eq!(px(348, 350), [255, 0, 0, 255]);
        let mixed = |p: [u8; 4]| {
            (127..=128).contains(&p[0]) && (127..=128).contains(&p[1]) && p[2..] == [0, 255]
        };
        assert!(mixed(px(349, 350)), "{:?}", px(349, 350));
        assert_eq!(px(350, 350), [0, 255, 0, 255]);
        assert_eq!(px(371, 350), [0, 255, 0, 255]);
        assert_eq!(px(372, 350), [0, 0, 0, 0]);
        // Multiple 1: two art pixels per canvas pixel, red and green averaged.
        let px = draw(1, 0.5);
        assert!(mixed(px(116, 116)), "{:?}", px(116, 116));
        assert_eq!(px(115, 116), [0, 0, 0, 0]);
        assert_eq!(px(124, 116), [0, 0, 0, 0]);
        // Multiple 4: a whole 2 canvas pixels per art pixel, the plain doubling.
        let px = draw(4, 0.5);
        assert_eq!(px(464, 470), [255, 0, 0, 255]);
        assert_eq!(px(465, 470), [255, 0, 0, 255]);
        assert_eq!(px(466, 470), [0, 255, 0, 255]);
    }

    #[test]
    fn display_text_widgets_use_the_display_label_font() {
        use crate::text::tests::{BoxGlyphs, Stretched};
        let green = Paint {
            palette: 0,
            index: 1,
        };
        // Inked canvas columns of "Hello" in role `font`.
        let width = |b: &SpriteBank, font| {
            let mut l = SpriteLayer::default();
            l.palettes[0] = Some(palette());
            let style = TextStyle {
                font,
                ..TextStyle::plain(green, Some(5.0))
            };
            l.add_styled_text(0, [100.0, 100.0, 200.0, 20.0], "Hello", style);
            let mut c = vec![0; CANVAS_W * CANVAS_H * 4];
            l.compose(b, &mut c);
            (0..CANVAS_W)
                .filter(|&x| (0..CANVAS_H).any(|y| pixel(&c, x, y)[3] != 0))
                .count()
        };
        let mut b = bank(1);
        b.set_label_font(Some(LabelFont::new(Box::new(BoxGlyphs))));
        let body = width(&b, FontRole::Body);
        assert!(body > 0);
        assert_eq!(width(&b, FontRole::Display), body);
        b.set_display_label_font(Some(LabelFont::new(Box::new(Stretched { w: 0.5, h: 1.0 }))));
        assert!(width(&b, FontRole::Display) < body);
        assert_eq!(width(&b, FontRole::Body), body);
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
    fn legend_widgets_use_the_legend_font_and_their_own_colours() {
        use crate::text::tests::BoxGlyphs;
        let mut b = bank(1);
        let mut l = SpriteLayer::default();
        let glyph = legend::Glyph {
            shape: legend::GlyphShape::Round,
            label: String::new(),
            fill: [10, 200, 30],
            ink: [255; 3],
        };
        let parts = vec![legend::LegendPart::Glyph(glyph)];
        let mut style = TextStyle::plain(
            Paint {
                palette: 9,
                index: 0,
            },
            Some(8.0),
        );
        style.align = Align::Left;
        l.add_widget(
            0,
            [100.0, 100.0, 60.0, 20.0],
            WidgetKind::Legend {
                parts: parts.clone(),
                style,
            },
        );
        let mut c = vec![0; CANVAS_W * CANVAS_H * 4];
        // No font at all: nothing.
        l.compose(&b, &mut c);
        assert!(c.iter().all(|&v| v == 0));
        // The legend font, with no palette in slot 9: the glyph keeps its own colour.
        b.set_legend_font(Some(LabelFont::new(Box::new(BoxGlyphs))));
        l.compose(&b, &mut c);
        // A 14-pixel disc (8 × 1.75) from x = 100, centred on y = 110.
        assert_eq!(pixel(&c, 107, 110), [10, 200, 30, 255]);
        assert_eq!(pixel(&c, 130, 110)[3], 0);
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
    fn paper_continues_a_picture_without_seams() {
        // A 640 × 480 picture of grey noise (indices are grey levels) lightening downwards,
        // with a "logo" block in the middle.
        let (mw, mh) = (40usize, 30usize);
        let mut b = bank(1);
        b.tiles = vec![None];
        for ty in 0..mh {
            for tx in 0..mw {
                let tile: [u8; TILE_PIXELS] = std::array::from_fn(|i| {
                    let (x, y) = (
                        tx * TILE_SIZE + i % TILE_SIZE,
                        ty * TILE_SIZE + i / TILE_SIZE,
                    );
                    if (10..30).contains(&tx) && (5..10).contains(&ty) {
                        return 250;
                    }
                    let hash =
                        (x as u32).wrapping_mul(0x9e37_79b9) ^ (y as u32).wrapping_mul(0x85eb_ca6b);
                    (60 + y / 4 + (hash.wrapping_mul(0xc2b2_ae35) >> 28) as usize) as u8
                });
                b.tiles.push(Some(Box::new(tile)));
            }
        }
        b.tilemaps[0] = Tilemap {
            width: mw as u8,
            height: mh as u8,
            tiles: (1..=(mw * mh) as u16).collect(),
        };
        let paper = Paper {
            source_rows: (256, 480),
            source_cols: 128,
            ramp: 24,
            edge_skip: 1,
            edge_cols: 12,
        };
        let plain = |b: &SpriteBank, l: &SpriteLayer| {
            let (w, h) = l.canvas_size();
            let mut c = vec![0; w * h * 4];
            l.compose(b, &mut c);
            c
        };
        let mut l = SpriteLayer::default();
        l.palettes[0] = Some(Palette(std::array::from_fn(|v| {
            let v = v as u32;
            v | v << 8 | v << 16
        })));
        l.slots[0] = Slot::new(0, 0, 0, 0, 3, true);
        // At 4:3 nothing changes.
        let before = plain(&b, &l);
        assert!(b.set_wide_paper(0, paper));
        assert!(!b.set_wide_paper(7, paper), "no such map");
        assert_eq!(plain(&b, &l), before);
        // Wide: the inner picture is untouched, everything else is opaque paper.
        l.fit_window(1280, 720);
        let (w, h) = l.canvas_size();
        let c = plain(&b, &l);
        let px = |x: usize, y: usize| c[(y * w + x) * 4];
        let x0 = l.wide / 2;
        for y in 0..h {
            for x in 24..616 {
                assert_eq!(px(x0 + x, y), before[(y * CANVAS_W + x) * 4]);
            }
        }
        assert!(c.chunks(4).all(|p| p[3] == 255), "filled to the edges");
        // No logo leaks into the fill, and the light follows the picture's gradient: the row
        // means of the far fill track those of the picture's edge.
        for x in (0..x0).chain(x0 + 640..w) {
            assert!((0..h).all(|y| px(x, y) < 230), "logo grey in column {x}");
        }
        let row_mean = |xs: &mut dyn Iterator<Item = usize>, y: usize| {
            let v: Vec<f32> = xs.map(|x| f32::from(px(x, y))).collect();
            v.iter().sum::<f32>() / v.len() as f32
        };
        for y in (16..h - 16).step_by(16) {
            let edge = row_mean(&mut (x0 + 1..x0 + 13), y);
            let fill = row_mean(&mut (0..x0), y);
            assert!(
                (edge - fill).abs() < 4.0,
                "row {y}: edge {edge}, fill {fill}"
            );
        }
        // Across the joins the steps between neighbouring columns are no larger than inside
        // the picture.
        let step = |x: usize| {
            (0..h)
                .map(|y| f32::from(px(x, y).abs_diff(px(x + 1, y))))
                .sum::<f32>()
                / h as f32
        };
        let inside = (x0 + 200..x0 + 300).map(step).fold(0.0, f32::max);
        for x in (x0 - 40..x0 + 30).chain(x0 + 610..x0 + 680) {
            assert!(
                step(x) <= inside * 1.3,
                "step {} at {x} (inside {inside})",
                step(x)
            );
        }
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
    fn framed_slots_stop_at_the_designs_edges() {
        let b = bank(1);
        let mut l = SpriteLayer::default();
        l.palettes[0] = Some(palette());
        // 16:9 at 1×: 213 canvas pixels wider, the design at x 106..746.
        assert!(l.fit_window(1920, 1080));
        let shift = l.anchor_shift(Anchor::Centre) as usize;
        // A 16-px tile from design x 632 (over the right edge) and one from x −8 (left edge),
        // the right one over another slot that must stay whole.
        l.slots[0] = Slot::new(0, 632, 10, 0, 0, true);
        l.slots[1] = Slot::new(0, -8, 10, 0, 0, true);
        l.slots[2] = Slot::new(0, 640, 20, 0, 3, true);
        let (w, h) = l.canvas_size();
        let draw = |l: &SpriteLayer| {
            let mut c = vec![0; w * h * 4];
            l.compose(&b, &mut c);
            c
        };
        let px = |c: &[u8], x: usize, y: usize| -> [u8; 4] {
            c[(y * w + x) * 4..][..4].try_into().unwrap()
        };
        let green = [0, 255, 0, 255];
        let before = draw(&l);
        assert_eq!(px(&before, shift + 645, 12), green);
        assert_eq!(px(&before, shift - 5, 12), green);
        l.frame([0, 1]);
        let c = draw(&l);
        assert_eq!(px(&c, shift + 639, 12), green);
        assert_eq!(px(&c, shift + 640, 12), [0; 4]);
        assert_eq!(px(&c, shift, 12), green);
        assert_eq!(px(&c, shift - 1, 12), [0; 4]);
        // The slot underneath, outside the design, is untouched.
        assert_eq!(px(&c, shift + 645, 22), green);
        // Cleared with the transient elements.
        l.reset_transient();
        assert!(l.framed.is_empty());
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

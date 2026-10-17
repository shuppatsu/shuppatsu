//! Opt-in text overrides for sprite slots (docs/localisation.md).
//!
//! Every on-screen string of the original is tile art (`tbl.rom` maps, docs/re/hud.md §4). A
//! translation replaces the art of one sprite — a tilemap, or a sub-rectangle of a string sheet —
//! with text rasterised into the same pixel rectangle, so the original layout is kept. Parts of
//! the rectangle can be replaced on their own (`TextPart::region`), leaving the rest of the art
//! (gauges, lamps, digits) in place.
//!
//! Colours come from the slot's palette at draw time: each part stores palette *indices* picked
//! from the original art (its dominant colour, and the second one for outlines or box text), so
//! the palette swaps the game uses for colour variants keep working.
//!
//! Nothing here is used unless the game installs a [`TextOverlay`] on the [`SpriteBank`]; the
//! arcade default draws the original art.

use crate::sprites::SpriteBank;
use ddg_formats::cg::{Palette, TILE_SIZE};
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum TextError {
    #[error("font: {0}")]
    Font(&'static str),
    #[error("reading font {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("no font given")]
    NoFont,
}

/// A source sub-rectangle in 16-px tiles, as in a slot or a 6-byte descriptor: `w`/`h` ≤ 0 mean
/// "to the edge of the map", negative `col`/`row` count as 0 (hud.md §1.1, menus.md §1.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct SubRect {
    pub col: i8,
    pub row: i8,
    pub w: i8,
    pub h: i8,
}

impl SubRect {
    /// The whole map.
    pub const WHOLE: Self = Self {
        col: 0,
        row: 0,
        w: 0,
        h: 0,
    };

    /// A canonical form that compares equal for every spelling of the same request without
    /// knowing the map size (0 = to the edge).
    pub fn canonical(self) -> Self {
        Self {
            col: self.col.max(0),
            row: self.row.max(0),
            w: self.w.max(0),
            h: self.h.max(0),
        }
    }

    /// `(col, row, w, h)` clipped to a `map_w × map_h` map; `None` when empty.
    pub fn resolve(self, map_w: u8, map_h: u8) -> Option<(u8, u8, u8, u8)> {
        let c = self.canonical();
        let (col, row) = (c.col as u8, c.row as u8);
        if col >= map_w || row >= map_h {
            return None;
        }
        let w = if c.w == 0 {
            map_w - col
        } else {
            (c.w as u8).min(map_w - col)
        };
        let h = if c.h == 0 {
            map_h - row
        } else {
            (c.h as u8).min(map_h - row)
        };
        Some((col, row, w, h))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Align {
    Left,
    #[default]
    Center,
    Right,
}

/// How a part is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Style {
    /// `Boxed` when more than [`BOXED_EDGE`] of the region's edge is opaque (the art fills
    /// the region: a plate, a box, a caption band), else `Plain` (lettering on transparency).
    #[default]
    Auto,
    /// The old art is removed; text in the art's dominant colour, outlined in its second.
    Plain,
    /// The backdrop (the colours along the region's edge) is kept and the old lettering is
    /// painted over with it; the text takes the lettering's colour.
    Boxed,
}

/// What a `Boxed` part leaves behind its text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Backdrop {
    /// The backdrop as in the art (a plate's colour, a caption band's dither).
    #[default]
    Art,
    /// Solid in the backdrop's opaque colour: a dither's transparent half is filled too (in
    /// every 2×2 cell with an opaque pixel), so the text never sits on the dither.
    Solid,
}

/// Which of the language's fonts a text is drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum FontRole {
    /// Everything not marked otherwise: HUD labels, hints, captions, plates, legends.
    #[default]
    Body,
    /// Display lettering: screen headers, menu titles, banners, time-message headlines. Drawn
    /// in the body font when no display font is configured.
    Display,
}

impl FontRole {
    /// `"body"` or `"display"`, as in the catalogue.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "body" => Some(Self::Body),
            "display" => Some(Self::Display),
            _ => None,
        }
    }
}

/// The fonts of one language by role; a missing display font falls back to the body font.
#[derive(Clone, Copy)]
pub struct Fonts<'a> {
    pub body: &'a dyn GlyphSource,
    pub display: Option<&'a dyn GlyphSource>,
}

impl<'a> Fonts<'a> {
    /// One font for every role.
    pub fn one(body: &'a dyn GlyphSource) -> Self {
        Self {
            body,
            display: None,
        }
    }

    /// The font of `role`.
    pub fn get(&self, role: FontRole) -> &'a dyn GlyphSource {
        match role {
            FontRole::Body => self.body,
            FontRole::Display => self.display.unwrap_or(self.body),
        }
    }
}

/// Capital height per font pixel of `glyphs` (the ink height of `H`): turns a fitted font
/// size into a size that compares across fonts, for size groups with parts in several roles.
fn cap_per_px(glyphs: &dyn GlyphSource) -> f32 {
    let h = glyphs.glyph('H', 100.0).height as f32 / 100.0;
    if h > 0.0 { h } else { 1.0 }
}

/// Opaque share of the region's edge above which `Style::Auto` picks `Boxed`.
pub const BOXED_EDGE: f32 = 0.35;

/// One piece of replacement text.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct TextPart {
    /// `[x, y, w, h]` in pixels relative to the sprite's source rectangle; `None` = all of it.
    pub region: Option<[u16; 4]>,
    /// `\n` separates lines.
    pub text: String,
    pub align: Align,
    pub style: Style,
    /// Rotated 90° clockwise (reads top to bottom), for the vertical station plates.
    pub vertical: bool,
    /// Plain style only: draw a 1-px outline in the art's second colour.
    pub outline: bool,
    /// Boxed style only: the backdrop behind the text.
    pub backdrop: Backdrop,
    /// Lines this many font sizes apart instead of the font's line height (two lines in a
    /// tight caption band).
    pub line_height: Option<f32>,
    /// Boxed style only: take the lettering colour from this rectangle of the sprite (`[x, y,
    /// w, h]`) instead of the part's own region, for a region whose lettering is mostly
    /// antialiasing (small half-width glyphs) and would otherwise pick a grey.
    pub ink: Option<[u16; 4]>,
    /// Parts of one override with the same group are drawn at one size: the smallest any of
    /// them fits at (the words of one callout cell).
    pub group: Option<String>,
    /// Parts with the same shared group in any override (other entries and tilemaps too) are
    /// drawn at one size: the smallest any of them fits at (the items of one menu list,
    /// which are separate entries of a string sheet or separate tilemaps).
    pub shared_group: Option<String>,
    /// The font role (`font = "display"` in the catalogue).
    pub font: FontRole,
}

/// The replacement for one sprite.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct TextOverride {
    pub parts: Vec<TextPart>,
}

/// One rasterised glyph: coverage rows, top-left at `(xmin, -(ymin + height))` from the pen on
/// the baseline (fontdue's convention).
#[derive(Clone, Debug, Default)]
pub struct Glyph {
    pub advance: f32,
    pub xmin: i32,
    pub ymin: i32,
    pub width: usize,
    pub height: usize,
    pub coverage: Vec<u8>,
}

/// A scalable glyph source (a font, or a stub in tests).
pub trait GlyphSource {
    /// `(ascent, descent)` at `px`, descent negative.
    fn vertical_metrics(&self, px: f32) -> (f32, f32);
    fn glyph(&self, ch: char, px: f32) -> Glyph;
    /// Advance only, for measuring.
    fn advance(&self, ch: char, px: f32) -> f32 {
        self.glyph(ch, px).advance
    }
}

/// TrueType/OpenType fonts through fontdue: the first is the primary font (metrics, and every
/// glyph it has); later ones fill in characters it lacks. A character no font has is drawn
/// without its macron (ō → o) rather than as a missing-glyph box.
pub struct FontGlyphs(Vec<fontdue::Font>);

impl FontGlyphs {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, TextError> {
        fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default())
            .map(|f| Self(vec![f]))
            .map_err(TextError::Font)
    }

    pub fn load(path: &std::path::Path) -> Result<Self, TextError> {
        let bytes = std::fs::read(path).map_err(|source| TextError::Io {
            path: path.to_owned(),
            source,
        })?;
        Self::from_bytes(&bytes)
    }

    /// Adds `other`'s fonts as fallbacks after this one's.
    pub fn with_fallback(mut self, other: Self) -> Self {
        self.0.extend(other.0);
        self
    }

    /// Loads several font files, the first being the primary.
    pub fn load_all(paths: &[std::path::PathBuf]) -> Result<Self, TextError> {
        let mut fonts = Vec::new();
        for p in paths {
            fonts.extend(Self::load(p)?.0);
        }
        if fonts.is_empty() {
            return Err(TextError::NoFont);
        }
        Ok(Self(fonts))
    }

    /// Whether `ch` is drawn as itself or its macron-less vowel, not as a missing-glyph box.
    /// Whitespace and control characters count as drawn.
    pub fn draws(&self, ch: char) -> bool {
        let has = |c: char| self.0.iter().any(|f| f.lookup_glyph_index(c) != 0);
        ch.is_whitespace() || ch.is_control() || has(ch) || has(strip_macron(ch))
    }

    /// The font and character to draw `ch` with.
    fn pick(&self, ch: char) -> (&fontdue::Font, char) {
        let has = |c: char| self.0.iter().find(|f| f.lookup_glyph_index(c) != 0);
        if let Some(f) = has(ch) {
            return (f, ch);
        }
        let plain = strip_macron(ch);
        if plain != ch
            && let Some(f) = has(plain)
        {
            return (f, plain);
        }
        (&self.0[0], ch)
    }
}

/// The base vowel of a Hepburn macron vowel (ā ē ī ō ū and capitals); other characters as is.
fn strip_macron(ch: char) -> char {
    match ch {
        'ā' => 'a',
        'ē' => 'e',
        'ī' => 'i',
        'ō' => 'o',
        'ū' => 'u',
        'Ā' => 'A',
        'Ē' => 'E',
        'Ī' => 'I',
        'Ō' => 'O',
        'Ū' => 'U',
        c => c,
    }
}

impl GlyphSource for FontGlyphs {
    fn vertical_metrics(&self, px: f32) -> (f32, f32) {
        match self.0[0].horizontal_line_metrics(px) {
            Some(m) => (m.ascent, m.descent),
            None => (px * 0.8, -px * 0.2),
        }
    }

    fn glyph(&self, ch: char, px: f32) -> Glyph {
        let (font, ch) = self.pick(ch);
        let (m, coverage) = font.rasterize(ch, px);
        Glyph {
            advance: m.advance_width,
            xmin: m.xmin,
            ymin: m.ymin,
            width: m.width,
            height: m.height,
            coverage,
        }
    }

    fn advance(&self, ch: char, px: f32) -> f32 {
        let (font, ch) = self.pick(ch);
        font.metrics(ch, px).advance_width
    }
}

/// Inner padding in pixels, room for the outline.
const PAD: usize = 1;
/// Latin capital height as a share of the original (kanji) lettering height: kanji fill the
/// em square, capitals about three quarters of it, so the two read as the same size.
const LATIN_CAP_OF_LETTERING: f32 = 0.75;
/// Japanese text replacing Latin lettering (the menu subtitles): a full-height kanji as tall as
/// the original lettering plus its outline (the measured extent excludes the 1-px outline rows).
const CJK_OF_LETTERING: f32 = 1.25;

/// Whether `text` is mostly Japanese (kana, kanji, full-width forms), which is sized by a kanji
/// instead of a Latin capital.
fn is_cjk(text: &str) -> bool {
    let (cjk, other) = text
        .chars()
        .filter(|c| !c.is_whitespace())
        .fold((0, 0), |(a, b), c| {
            if c as u32 >= 0x3000 {
                (a + 1, b)
            } else {
                (a, b + 1)
            }
        });
    cjk > other
}

/// Largest and smallest font sizes tried, in pixels (the largest allows UI scale 4).
const MAX_PX: f32 = 256.0;
const MIN_PX: f32 = 4.0;

/// Rasterises `text` into a `w × h` coverage mask (row-major, 0..=255), fitted: the largest
/// size at which every line fits the width and all lines fit the height, centred vertically.
/// Nothing is ever written outside the mask.
pub fn rasterise(
    text: &str,
    w: usize,
    h: usize,
    align: Align,
    vertical: bool,
    glyphs: &dyn GlyphSource,
) -> Vec<u8> {
    rasterise_sized(text, w, h, align, vertical, glyphs, None)
}

/// [`rasterise`] at a wanted size: `cap_px` is the height of a capital letter (of a kanji for
/// Japanese text) in pixels (set
/// from the original lettering, so neighbouring labels keep the original's type sizes). The
/// text is made smaller only when it would not fit; `None` fills the box. The ink, not the
/// font's ascent and descent, is centred vertically, so tall glyphs are never cut off.
pub fn rasterise_sized(
    text: &str,
    w: usize,
    h: usize,
    align: Align,
    vertical: bool,
    glyphs: &dyn GlyphSource,
    cap_px: Option<f32>,
) -> Vec<u8> {
    rasterise_spaced(text, w, h, align, vertical, glyphs, cap_px, None)
}

/// [`rasterise_sized`] with the lines `line_em` font sizes apart instead of the font's line
/// height (`None`), for several lines in a tight box.
#[allow(clippy::too_many_arguments)]
pub fn rasterise_spaced(
    text: &str,
    w: usize,
    h: usize,
    align: Align,
    vertical: bool,
    glyphs: &dyn GlyphSource,
    cap_px: Option<f32>,
    line_em: Option<f32>,
) -> Vec<u8> {
    rasterise_fit(text, w, h, align, vertical, glyphs, cap_px, line_em, None).0
}

/// [`rasterise_spaced`] at most `max_px` font pixels; also returns the font size it used, so
/// several texts can be drawn at one size (the smallest of their fitted sizes).
#[allow(clippy::too_many_arguments)]
pub fn rasterise_fit(
    text: &str,
    w: usize,
    h: usize,
    align: Align,
    vertical: bool,
    glyphs: &dyn GlyphSource,
    cap_px: Option<f32>,
    line_em: Option<f32>,
    max_px: Option<f32>,
) -> (Vec<u8>, f32) {
    if vertical {
        // Lay out in the rotated frame, then turn it clockwise.
        let (src, px) = rasterise_fit(text, h, w, align, false, glyphs, cap_px, line_em, max_px);
        let mut out = vec![0u8; w * h];
        for sy in 0..w {
            for sx in 0..h {
                // Source pixel (sx, sy) of the h × w image goes to (w − 1 − sy, sx).
                out[sx * w + (w - 1 - sy)] = src[sy * h + sx];
            }
        }
        return (out, px);
    }
    let mut out = vec![0u8; w * h];
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() || w <= 2 * PAD || h <= 2 * PAD {
        return (out, 0.0);
    }
    let (aw, ah) = ((w - 2 * PAD) as f32, (h - 2 * PAD) as f32);
    let width_at = |line: &str, px: f32| line.chars().map(|c| glyphs.advance(c, px)).sum::<f32>();
    let line_h = |px: f32| match line_em {
        Some(em) => em * px,
        None => {
            let (a, d) = glyphs.vertical_metrics(px);
            a - d
        }
    };
    // Ink extent (top, bottom) relative to the first baseline, lines `line_h` apart.
    let ink = |px: f32| {
        let lh = line_h(px);
        let (mut top, mut bottom) = (f32::MAX, f32::MIN);
        for (i, line) in lines.iter().enumerate() {
            let base = lh * i as f32;
            for ch in line.chars() {
                let g = glyphs.glyph(ch, px);
                if g.height == 0 {
                    continue;
                }
                top = top.min(base - (g.ymin + g.height as i32) as f32);
                bottom = bottom.max(base - g.ymin as f32);
            }
        }
        if top > bottom {
            (0.0, 0.0)
        } else {
            (top, bottom)
        }
    };
    // Metrics scale linearly, so solve at a reference size, then step down until it fits
    // (hinting and rounding are not linear).
    let reference = 32.0;
    let widest = lines
        .iter()
        .map(|l| width_at(l, reference))
        .fold(0.0, f32::max);
    let by_w = if widest > 0.0 {
        aw * reference / widest
    } else {
        MAX_PX
    };
    // Height limit: Latin text by the font's line height, which keeps the approved sizes;
    // Japanese text by its ink, because CJK fonts' tall ascent and descent would cap kanji well
    // below what fits.
    let by_h = if is_cjk(text) {
        let (ink_top, ink_bottom) = ink(reference);
        ah * reference / (ink_bottom - ink_top).max(1.0)
    } else {
        ah * reference / (line_h(reference) * lines.len() as f32).max(1.0)
    };
    let mut px = by_w.min(by_h);
    if let Some(cap) = cap_px {
        // The reference glyph: a Latin capital, or a full-height kanji for Japanese text.
        let reference_char = if is_cjk(text) { '国' } else { 'H' };
        let h_ref = glyphs.glyph(reference_char, 100.0).height as f32;
        if h_ref > 0.0 {
            px = px.min(cap * 100.0 / h_ref);
        }
    }
    if let Some(m) = max_px {
        px = px.min(m);
    }
    px = px.clamp(MIN_PX, MAX_PX);
    let fits = |px: f32| {
        let (t, b) = ink(px);
        lines.iter().all(|l| width_at(l, px) <= aw) && b - t <= ah
    };
    while px > MIN_PX && !fits(px) {
        px = (px - 0.5).max(MIN_PX);
    }
    let lh = line_h(px);
    let (top, bottom) = ink(px);
    let shift = PAD as f32 + (ah - (bottom - top)) / 2.0 - top;
    for (i, line) in lines.iter().enumerate() {
        let lw = width_at(line, px);
        let mut pen = PAD as f32
            + match align {
                Align::Left => 0.0,
                Align::Center => (aw - lw) / 2.0,
                Align::Right => aw - lw,
            };
        let baseline = (shift + lh * i as f32).round() as i32;
        for ch in line.chars() {
            let g = glyphs.glyph(ch, px);
            let gx = pen.round() as i32 + g.xmin;
            let gy = baseline - g.ymin - g.height as i32;
            for row in 0..g.height {
                let y = gy + row as i32;
                if !(0..h as i32).contains(&y) {
                    continue;
                }
                for col in 0..g.width {
                    let x = gx + col as i32;
                    if !(0..w as i32).contains(&x) {
                        continue;
                    }
                    let o = y as usize * w + x as usize;
                    out[o] = out[o].max(g.coverage[row * g.width + col]);
                }
            }
            pen += g.advance;
        }
    }
    (out, px)
}

/// A part ready to draw: palette indices per pixel, so any palette can colour it.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderedPart {
    /// `[x, y, w, h]` relative to the sprite's source rectangle, in canvas pixels (original
    /// pixels × the overlay's scale).
    pub region: [u16; 4],
    /// Under-colour per pixel: 0 = keep what is below, else a palette index (box or outline).
    pub base: Vec<u8>,
    /// Text coverage per pixel, blended over `base` in colour `fg`.
    pub coverage: Vec<u8>,
    pub fg: u8,
}

impl RenderedPart {
    pub fn contains(&self, x: usize, y: usize) -> bool {
        let [rx, ry, rw, rh] = self.region.map(usize::from);
        (rx..rx + rw).contains(&x) && (ry..ry + rh).contains(&y)
    }
}

/// Key of a rendered override: tilemap id and its resolved source rectangle in tiles.
pub type SpriteKey = (u16, (u8, u8, u8, u8));

/// All rendered overrides for one bank, looked up per slot when composing.
#[derive(Clone, Debug, Default)]
pub struct TextOverlay {
    parts: HashMap<SpriteKey, Vec<RenderedPart>>,
    /// Canvas pixels per original pixel the parts were rendered at (see `SpriteLayer::scale`).
    scale: usize,
}

/// The size of a size group: its smallest capital height, kept as the font size it was fitted
/// at and that font's capital height per pixel, so a part in the same font gets exactly that
/// font size back and a part in another font the same capital height.
#[derive(Clone, Copy, Debug)]
struct GroupMin {
    px: f32,
    k: f32,
}

impl GroupMin {
    fn cap(self) -> f32 {
        self.px * self.k
    }

    /// The font size of this capital height in a font with `k` capital pixels per pixel.
    fn px_for(self, k: f32) -> f32 {
        if k == self.k { self.px } else { self.cap() / k }
    }

    /// Lowers group `g`'s size to a part fitted at `px` in a font with `k`.
    fn keep<'a>(mins: &mut HashMap<&'a str, GroupMin>, g: &'a str, px: f32, k: f32) {
        let new = Self { px, k };
        mins.entry(g)
            .and_modify(|m| {
                if new.cap() < m.cap() {
                    *m = new;
                }
            })
            .or_insert(new);
    }
}

impl TextOverlay {
    /// Renders every override against the bank's art. Overrides whose tilemap or rectangle
    /// does not exist are skipped (the art is then drawn as usual).
    pub fn build<'a>(
        bank: &SpriteBank,
        overrides: impl IntoIterator<Item = (u16, SubRect, &'a TextOverride)>,
        glyphs: &dyn GlyphSource,
    ) -> Self {
        Self::build_scaled(bank, overrides, glyphs, 1)
    }

    /// [`TextOverlay::build`] for a canvas with `scale` pixels per original pixel: the text is
    /// rasterised at that resolution inside the same rectangles (enhancement `ui_scale`).
    pub fn build_scaled<'a>(
        bank: &SpriteBank,
        overrides: impl IntoIterator<Item = (u16, SubRect, &'a TextOverride)>,
        glyphs: &dyn GlyphSource,
        scale: usize,
    ) -> Self {
        Self::build_with(bank, overrides, Fonts::one(glyphs), scale)
    }

    /// [`TextOverlay::build_scaled`] with a font per role ([`TextPart::font`]). Size groups
    /// span roles: their parts share one capital height, whatever their fonts.
    pub fn build_with<'a>(
        bank: &SpriteBank,
        overrides: impl IntoIterator<Item = (u16, SubRect, &'a TextOverride)>,
        fonts: Fonts<'_>,
        scale: usize,
    ) -> Self {
        let scale = scale.max(1);
        // Every part fitted on its own first.
        struct Entry<'a> {
            tbl: u16,
            rect: (u8, u8, u8, u8),
            o: &'a TextOverride,
            art: Vec<u8>,
            size: (usize, usize),
            fitted: Vec<Option<(RenderedPart, f32)>>,
        }
        let entries: Vec<Entry<'a>> = overrides
            .into_iter()
            .filter_map(|(tbl, rect, o)| {
                let map = bank.tilemaps.get(usize::from(tbl))?;
                let r = rect.resolve(map.width, map.height)?;
                let art = bank.render_rect(tbl, r);
                let (pw, ph) = (usize::from(r.2) * TILE_SIZE, usize::from(r.3) * TILE_SIZE);
                let fitted = o
                    .parts
                    .iter()
                    .map(|p| render_part_at(p, &art, pw, ph, fonts.get(p.font), scale, None))
                    .collect();
                Some(Entry {
                    tbl,
                    rect: r,
                    o,
                    art,
                    size: (pw, ph),
                    fitted,
                })
            })
            .collect();
        let k_of = |p: &TextPart| cap_per_px(fonts.get(p.font));
        // The smallest fitted size of each shared group (across entries and tilemaps).
        let mut shared_min: HashMap<&str, GroupMin> = HashMap::new();
        for e in &entries {
            for (p, f) in e.o.parts.iter().zip(&e.fitted) {
                if let (Some(g), Some((_, px))) = (p.shared_group.as_deref(), f) {
                    GroupMin::keep(&mut shared_min, g, *px, k_of(p));
                }
            }
        }
        let mut parts = HashMap::new();
        for e in entries {
            // ... and of each group inside the entry.
            let mut smallest: HashMap<&str, GroupMin> = HashMap::new();
            for (p, f) in e.o.parts.iter().zip(&e.fitted) {
                if let (Some(g), Some((_, px))) = (p.group.as_deref(), f) {
                    GroupMin::keep(&mut smallest, g, *px, k_of(p));
                }
            }
            let (pw, ph) = e.size;
            let rendered = e
                .o
                .parts
                .iter()
                .zip(e.fitted)
                .filter_map(|(p, f)| {
                    let (part, px) = f?;
                    let k = k_of(p);
                    let group = p.group.as_deref().map(|g| smallest[g]);
                    let sheet = p.shared_group.as_deref().map(|g| shared_min[g]);
                    let min = group
                        .into_iter()
                        .chain(sheet)
                        .map(|m| m.px_for(k))
                        .reduce(f32::min);
                    match min {
                        Some(min) if min < px => {
                            let g = fonts.get(p.font);
                            render_part_at(p, &e.art, pw, ph, g, scale, Some(min)).map(|(r, _)| r)
                        }
                        _ => Some(part),
                    }
                })
                .collect::<Vec<_>>();
            if !rendered.is_empty() {
                parts.insert((e.tbl, e.rect), rendered);
            }
        }
        Self { parts, scale }
    }

    pub fn scale(&self) -> usize {
        self.scale
    }

    pub fn get(&self, key: &SpriteKey) -> Option<&[RenderedPart]> {
        self.parts.get(key).map(Vec::as_slice)
    }

    pub fn len(&self) -> usize {
        self.parts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }
}

#[cfg(test)]
fn render_part(
    part: &TextPart,
    art: &[u8],
    art_w: usize,
    art_h: usize,
    glyphs: &dyn GlyphSource,
    scale: usize,
) -> Option<RenderedPart> {
    render_part_at(part, art, art_w, art_h, glyphs, scale, None).map(|(r, _)| r)
}

/// How the renderer reads the art under one part's region: the region clipped to the sprite,
/// its style, its backdrop pattern and the original lettering's ink box. Shared by the
/// renderer and the catalogue audit ([`crate::text_audit`]), so both agree on what is backdrop
/// and what is lettering.
#[derive(Clone, Debug)]
pub struct RegionScan {
    /// The region clipped to the sprite, `[x, y, w, h]` in art pixels.
    pub rect: [usize; 4],
    /// Painted as a box (the backdrop kept, the lettering painted over) rather than plain.
    pub boxed: bool,
    /// The backdrop per 2×2 parity class of region pixels (`(x & 1) | (y & 1) << 1`): the
    /// commonest value along the region's edge in that class.
    pub pattern: [u8; 4],
    /// The original lettering's ink box `[x0, x1, y0, y1]` (exclusive ends) in region pixels;
    /// `None` when the region has no lettering.
    pub ink: Option<[usize; 4]>,
    /// The region's opaque palette indices, commonest first.
    pub ranked: Vec<u8>,
}

impl RegionScan {
    /// Scans `part`'s region of a `art_w × art_h` sprite; `None` when the region is empty after
    /// clipping to the sprite.
    pub fn of(part: &TextPart, art: &[u8], art_w: usize, art_h: usize) -> Option<Self> {
        let [x, y, w, h] = part.region.unwrap_or([0, 0, art_w as u16, art_h as u16]);
        let (x, y) = (usize::from(x), usize::from(y));
        if x >= art_w || y >= art_h {
            return None;
        }
        let w = usize::from(w).min(art_w - x);
        let h = usize::from(h).min(art_h - y);
        if w == 0 || h == 0 {
            return None;
        }
        let region_art = |px: usize, py: usize| art[(y + py) * art_w + x + px];
        let mut counts = [0u32; 256];
        // Edge values per 2×2 parity class (x & 1, y & 1), for the backdrop pattern.
        let mut class = [[0u32; 256]; 4];
        let mut edge_n = 0u32;
        let mut edge_opaque = 0u32;
        for py in 0..h {
            for px in 0..w {
                let v = region_art(px, py);
                counts[usize::from(v)] += 1;
                if py == 0 || px == 0 || py == h - 1 || px == w - 1 {
                    class[parity(px, py)][usize::from(v)] += 1;
                    edge_n += 1;
                    edge_opaque += u32::from(v != 0);
                }
            }
        }
        let mut ranked: Vec<u8> = (1..=255u8)
            .filter(|&i| counts[usize::from(i)] > 0)
            .collect();
        ranked.sort_by_key(|&i| std::cmp::Reverse(counts[usize::from(i)]));
        let boxed = match part.style {
            Style::Boxed => true,
            Style::Plain => false,
            Style::Auto => edge_opaque as f32 > BOXED_EDGE * edge_n as f32,
        };
        // The backdrop is the commonest value along the region's edge per 2×2 parity class
        // (a plate's face is one colour; a caption band's dither alternates colours or
        // transparency).
        let pattern: [u8; 4] = std::array::from_fn(|c| {
            (0..=255u8)
                .max_by_key(|&i| class[c][usize::from(i)])
                .unwrap_or(0)
        });
        let mut scan = Self {
            rect: [x, y, w, h],
            boxed,
            pattern,
            ink: None,
            ranked,
        };
        // Its ink box `[x0, x1) × [y0, y1)` in region pixels.
        let (mut x0, mut x1, mut y0, mut y1) = (w, 0, h, 0);
        for py in 0..h {
            for px in 0..w {
                if scan.lettering(px, py, region_art(px, py)) {
                    x0 = x0.min(px);
                    x1 = x1.max(px + 1);
                    y0 = y0.min(py);
                    y1 = y1.max(py + 1);
                }
            }
        }
        scan.ink = (x0 < x1).then_some([x0, x1, y0, y1]);
        Some(scan)
    }

    /// Whether value `v` at region pixel `(px, py)` is original lettering: opaque and, in a
    /// box, not the backdrop. The coordinates may lie outside the region (the parity classes
    /// continue), for looking at the art around it.
    pub fn lettering(&self, px: usize, py: usize, v: u8) -> bool {
        v != 0 && (!self.boxed || !self.backdrop(px, py, v))
    }

    /// Whether value `v` at region pixel `(px, py)` matches the backdrop pattern.
    pub fn backdrop(&self, px: usize, py: usize, v: u8) -> bool {
        v == self.pattern[parity(px, py)]
    }

    /// [`RegionScan::lettering`] at sprite pixel `(ax, ay)`, inside the region or not.
    pub fn lettering_at(&self, ax: usize, ay: usize, v: u8) -> bool {
        // (a − x) and (a + x) have the same parity, so no signed arithmetic is needed.
        self.lettering(ax + self.rect[0], ay + self.rect[1], v)
    }

    /// [`RegionScan::backdrop`] at sprite pixel `(ax, ay)`.
    pub fn backdrop_at(&self, ax: usize, ay: usize, v: u8) -> bool {
        self.backdrop(ax + self.rect[0], ay + self.rect[1], v)
    }
}

/// The clear pixels of a `w × h` sprite that the art encloses: those whose 4-connected run of
/// clear pixels does not reach the sprite's edge (the clear half of dithered lettering, a hole
/// in a glyph). The rest is the void around the art.
pub fn enclosed_clear(art: &[u8], w: usize, h: usize) -> Vec<bool> {
    // Flood the void from the clear pixels on the edge; what stays unreached is enclosed.
    let mut void = vec![false; w * h];
    let mut stack: Vec<usize> = (0..w * h)
        .filter(|&i| {
            let (x, y) = (i % w, i / w);
            art[i] == 0 && (x == 0 || y == 0 || x + 1 == w || y + 1 == h)
        })
        .collect();
    for &i in &stack {
        void[i] = true;
    }
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        let near = [
            (x > 0).then(|| i - 1),
            (x + 1 < w).then(|| i + 1),
            (y > 0).then(|| i - w),
            (y + 1 < h).then(|| i + w),
        ];
        for j in near.into_iter().flatten() {
            if art[j] == 0 && !void[j] {
                void[j] = true;
                stack.push(j);
            }
        }
    }
    (0..w * h).map(|i| art[i] == 0 && !void[i]).collect()
}

/// The 2×2 parity class of a pixel.
fn parity(px: usize, py: usize) -> usize {
    (px & 1) | (py & 1) << 1
}

/// One part, its text at most `max_px` font pixels; also returns the font size used.
fn render_part_at(
    part: &TextPart,
    art: &[u8],
    art_w: usize,
    art_h: usize,
    glyphs: &dyn GlyphSource,
    scale: usize,
    max_px: Option<f32>,
) -> Option<(RenderedPart, f32)> {
    let scan = RegionScan::of(part, art, art_w, art_h)?;
    let [x, y, w, h] = scan.rect;
    let region_art = |px: usize, py: usize| art[(y + py) * art_w + x + px];
    let ranked = &scan.ranked;
    let dominant = ranked.first().copied().unwrap_or(0);
    let second = ranked.get(1).copied().unwrap_or(dominant);
    let (boxed, pattern) = (scan.boxed, scan.pattern);
    // The original lettering's extent across the reading direction sets the English size, so
    // labels keep the original's type sizes instead of each filling its own box.
    let [x0, x1, y0, y1] = scan.ink.unwrap_or([w, 0, h, 0]);
    let inked = scan.ink.is_some();
    let (lo, hi) = match (inked, part.vertical) {
        (false, _) => (1, 0),
        (true, false) => (y0, y1 - 1),
        (true, true) => (x0, x1 - 1),
    };
    // The output is `scale` times finer than the art; art lookups use original pixels.
    let s = scale;
    let (sw, sh) = (w * s, h * s);
    let share = if is_cjk(&part.text) {
        CJK_OF_LETTERING
    } else {
        LATIN_CAP_OF_LETTERING
    };
    let cap_px = (lo <= hi).then(|| (hi - lo + 1) as f32 * share * s as f32);
    // The text keeps the original's margins: it is laid out in the lettering's ink box, which
    // may grow along the reading direction up to the smaller of the original's two margins
    // there, but never across it (so it cannot reach a neighbouring HUD element).
    // A solid backdrop is the text's own box: it may use all of it.
    let own_box = boxed && part.backdrop == Backdrop::Solid;
    let (lx0, lx1, ly0, ly1) = if !inked || own_box {
        (0, w, 0, h)
    } else if part.vertical {
        let m = y0.min(h - y1);
        (x0, x1, m, h - m)
    } else {
        let m = x0.min(w - x1);
        (m, w - m, y0, y1)
    };
    // A plain outline is one original pixel wide and the original's ink box includes its
    // outline: keep the fill that far inside (the rasteriser already pads one canvas pixel).
    let inset = if !boxed && part.outline {
        s.saturating_sub(PAD)
    } else {
        0
    };
    let (ox, oy) = (lx0 * s + inset, ly0 * s + inset);
    let lw = ((lx1 - lx0) * s).saturating_sub(2 * inset);
    let lh = ((ly1 - ly0) * s).saturating_sub(2 * inset);
    // One line is fitted to the ink box by its ink, not by the font's line height (which
    // would leave it well short of the original lettering's height).
    let one_line = !part.text.contains('\n');
    let line_em = part.line_height.or(one_line.then_some(1.0));
    let (laid, px) = rasterise_fit(
        &part.text,
        lw,
        lh,
        part.align,
        part.vertical,
        glyphs,
        cap_px,
        line_em,
        max_px,
    );
    let mut coverage = vec![0u8; sw * sh];
    for row in 0..lh {
        let o = (oy + row) * sw + ox;
        coverage[o..o + lw].copy_from_slice(&laid[row * lw..(row + 1) * lw]);
    }
    let mut base = vec![0u8; sw * sh];
    let fg = if boxed {
        // The backdrop's opaque colour: a plate's face, or the dark half of a dither.
        let opaque = (0..4)
            .map(|c| pattern[c])
            .filter(|&v| v != 0)
            .max_by_key(|&v| pattern.iter().filter(|&&p| p == v).count())
            .unwrap_or(0);
        let solid = part.backdrop == Backdrop::Solid && opaque != 0;
        // Every opaque pixel is repainted with the backdrop, which erases the old lettering and
        // keeps the box shape (transparent corners stay transparent). A solid backdrop also
        // fills the transparent half of a dither (any opaque pixel in its 2×2 cell).
        let cell_opaque = |ax: usize, ay: usize| {
            let (cx, cy) = (ax & !1, ay & !1);
            (cy..(cy + 2).min(h)).any(|y| (cx..(cx + 2).min(w)).any(|x| region_art(x, y) != 0))
        };
        for py in 0..sh {
            for px in 0..sw {
                let (ax, ay) = (px / s, py / s);
                if solid && cell_opaque(ax, ay) {
                    base[py * sw + px] = opaque;
                } else if region_art(ax, ay) != 0 {
                    base[py * sw + px] = pattern[parity(ax, ay)];
                }
            }
        }
        // The text takes the original lettering's colour: the commonest non-backdrop one, in
        // the region or in the part's `ink` rectangle.
        let ink_ranked = part.ink.map(|r| ranked_colours(art, art_w, art_h, r));
        ink_ranked
            .as_deref()
            .unwrap_or(ranked)
            .iter()
            .copied()
            .find(|i| !pattern.contains(i))
            .unwrap_or(second)
    } else {
        let (fill, outline) = plain_colours(&region_art, w, h).unwrap_or((dominant, second));
        if part.outline && outline != fill {
            // One original pixel thick at any scale.
            dilate(&coverage, sw, sh, s, 128, |i| base[i] = outline);
        }
        fill
    };
    let rendered = RenderedPart {
        region: [(x * s) as u16, (y * s) as u16, sw as u16, sh as u16],
        base,
        coverage,
        fg,
    };
    Some((rendered, px))
}

/// The opaque palette indices of `rect` (clipped to the `art_w × art_h` art), commonest first.
fn ranked_colours(art: &[u8], art_w: usize, art_h: usize, rect: [u16; 4]) -> Vec<u8> {
    let [x, y, w, h] = rect.map(usize::from);
    let mut counts = [0u32; 256];
    for row in art.chunks(art_w).take(art_h.min(y + h)).skip(y) {
        for &v in row.iter().take(art_w.min(x + w)).skip(x) {
            counts[usize::from(v)] += 1;
        }
    }
    let mut ranked: Vec<u8> = (1..=255u8)
        .filter(|&i| counts[usize::from(i)] > 0)
        .collect();
    ranked.sort_by_key(|&i| std::cmp::Reverse(counts[usize::from(i)]));
    ranked
}

/// The fill and outline colours of plain lettering in a `w × h` region: the fill is the
/// commonest colour of the interior pixels (all four neighbours opaque), the outline the
/// commonest other colour of the pixels next to transparency. Counting all pixels instead gets
/// thin lettering with a thick outline backwards (秒: the outline outnumbers the strokes).
/// `None` without interior pixels (one-pixel lettering).
fn plain_colours(art: &impl Fn(usize, usize) -> u8, w: usize, h: usize) -> Option<(u8, u8)> {
    let opaque = |x: isize, y: isize| {
        (0..w as isize).contains(&x)
            && (0..h as isize).contains(&y)
            && art(x as usize, y as usize) != 0
    };
    let (mut inner, mut rim) = ([0u32; 256], [0u32; 256]);
    for y in 0..h {
        for x in 0..w {
            let v = art(x, y);
            if v == 0 {
                continue;
            }
            let (xi, yi) = (x as isize, y as isize);
            let enclosed = [(-1, 0), (1, 0), (0, -1), (0, 1)]
                .iter()
                .all(|&(dx, dy)| opaque(xi + dx, yi + dy));
            if enclosed {
                inner[usize::from(v)] += 1;
            } else {
                rim[usize::from(v)] += 1;
            }
        }
    }
    let commonest = |c: &[u32; 256], not: u8| {
        (1..=255u8)
            .filter(|&i| i != not && c[usize::from(i)] > 0)
            .max_by_key(|&i| c[usize::from(i)])
    };
    let fill = commonest(&inner, 0)?;
    Some((fill, commonest(&rim, fill).unwrap_or(fill)))
}

/// Calls `f` with the index of every pixel of a `w × h` mask within `r` pixels (a square) of a
/// pixel whose coverage is at least `min`.
fn dilate(coverage: &[u8], w: usize, h: usize, r: usize, min: u8, mut f: impl FnMut(usize)) {
    let r = r as isize;
    let inked = |px: isize, py: isize| {
        (0..w as isize).contains(&px)
            && (0..h as isize).contains(&py)
            && coverage[py as usize * w + px as usize] >= min
    };
    for py in 0..h as isize {
        for px in 0..w as isize {
            if (-r..=r).any(|dy| (-r..=r).any(|dx| inked(px + dx, py + dy))) {
                f(py as usize * w + px as usize);
            }
        }
    }
}

/// Draws the parts of one slot whose source rectangle starts at canvas `(ox, oy)`.
pub(crate) fn draw_parts(
    parts: &[RenderedPart],
    palette: &Palette,
    ox: i32,
    oy: i32,
    canvas: &mut [u8],
    canvas_w: usize,
    canvas_h: usize,
) {
    for p in parts {
        let [rx, ry, rw, rh] = p.region.map(i32::from);
        let fg = palette.rgba(p.fg);
        for py in 0..rh {
            let y = oy + ry + py;
            if !(0..canvas_h as i32).contains(&y) {
                continue;
            }
            for px in 0..rw {
                let x = ox + rx + px;
                if !(0..canvas_w as i32).contains(&x) {
                    continue;
                }
                let i = (py * rw + px) as usize;
                let (b, c) = (p.base[i], p.coverage[i]);
                if b == 0 && c == 0 {
                    continue;
                }
                let o = (y as usize * canvas_w + x as usize) * 4;
                let under: [u8; 4] = if b != 0 {
                    palette.rgba(b)
                } else {
                    canvas[o..o + 4].try_into().expect("4 bytes")
                };
                let px_out = if c == 255 || (under[3] == 0 && c > 0) {
                    // Over transparency the overlay's alpha test decides; keep it crisp.
                    [fg[0], fg[1], fg[2], if c == 255 { 255 } else { c }]
                } else {
                    let t = u16::from(c);
                    let mix =
                        |a: u8, b: u8| ((u16::from(a) * (255 - t) + u16::from(b) * t) / 255) as u8;
                    [
                        mix(under[0], fg[0]),
                        mix(under[1], fg[1]),
                        mix(under[2], fg[2]),
                        255,
                    ]
                };
                canvas[o..o + 4].copy_from_slice(&px_out);
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Every glyph is a solid box `px/2` wide and `px·0.7` tall, deliberately with a negative
    /// bearing so careless clipping would write outside the rectangle.
    pub(crate) struct BoxGlyphs;

    impl GlyphSource for BoxGlyphs {
        fn vertical_metrics(&self, px: f32) -> (f32, f32) {
            (px * 0.8, -px * 0.2)
        }
        fn glyph(&self, ch: char, px: f32) -> Glyph {
            if ch == ' ' {
                return Glyph {
                    advance: px * 0.3,
                    ..Glyph::default()
                };
            }
            let (width, height) = ((px * 0.5) as usize + 2, (px * 0.7) as usize + 2);
            Glyph {
                advance: px * 0.55,
                xmin: -1,
                ymin: -1,
                width,
                height,
                coverage: vec![255; width * height],
            }
        }
    }

    /// [`BoxGlyphs`] with boxes `w` times as wide and `h` times as tall (a second font).
    pub(crate) struct Stretched {
        pub w: f32,
        pub h: f32,
    }

    impl GlyphSource for Stretched {
        fn vertical_metrics(&self, px: f32) -> (f32, f32) {
            BoxGlyphs.vertical_metrics(px)
        }
        fn glyph(&self, ch: char, px: f32) -> Glyph {
            if ch == ' ' {
                return BoxGlyphs.glyph(ch, px * self.w);
            }
            let (width, height) = (
                (px * 0.5 * self.w) as usize + 2,
                (px * 0.7 * self.h) as usize + 2,
            );
            Glyph {
                advance: px * 0.55 * self.w,
                xmin: -1,
                ymin: -1,
                width,
                height,
                coverage: vec![255; width * height],
            }
        }
    }

    #[test]
    fn display_role_falls_back_to_the_body_font() {
        let narrow = Stretched { w: 0.5, h: 1.0 };
        let body = Fonts::one(&BoxGlyphs);
        assert!(std::ptr::addr_eq(body.get(FontRole::Display), &BoxGlyphs));
        let both = Fonts {
            body: &BoxGlyphs,
            display: Some(&narrow),
        };
        assert!(std::ptr::addr_eq(both.get(FontRole::Body), &BoxGlyphs));
        assert!(std::ptr::addr_eq(both.get(FontRole::Display), &narrow));
        assert_eq!(FontRole::parse("display"), Some(FontRole::Display));
        assert_eq!(FontRole::parse("body"), Some(FontRole::Body));
        assert_eq!(FontRole::parse("Display"), None);
    }

    #[test]
    fn sized_text_keeps_its_cap_height_and_centres_the_ink() {
        // Stub capitals are 0.7·px + 2 tall: a 14-px cap gives 15 ink rows, not the box height.
        let (w, h) = (100, 60);
        let m = rasterise_sized("Hi", w, h, Align::Center, false, &BoxGlyphs, Some(14.0));
        let rows: Vec<usize> = (0..h)
            .filter(|y| m[y * w..][..w].iter().any(|&c| c > 0))
            .collect();
        assert_eq!(rows.len(), 15);
        let top = rows[0];
        assert!((21..=23).contains(&top), "ink starts at row {top}");
        // Without a size the text fills the box (up to the width limit).
        let full = rasterise("Hi", w, h, Align::Center, false, &BoxGlyphs);
        let full_rows = (0..h)
            .filter(|y| full[y * w..][..w].iter().any(|&c| c > 0))
            .count();
        assert!(full_rows > 30);
        // A size too big for the box shrinks to fit; nothing is cut off.
        let big = rasterise_sized("Hi", w, 20, Align::Center, false, &BoxGlyphs, Some(40.0));
        assert!(big[..w].iter().all(|&c| c == 0) && big[19 * w..].iter().all(|&c| c == 0));
    }

    #[test]
    fn line_spacing_packs_two_lines_tighter() {
        let (w, h) = (200, 40);
        let rows = |m: &[u8]| {
            (0..h)
                .filter(|y| m[y * w..][..w].iter().any(|&c| c > 0))
                .count()
        };
        let loose = rasterise_sized("Ab\nCd", w, h, Align::Center, false, &BoxGlyphs, None);
        let tight = rasterise_spaced(
            "Ab\nCd",
            w,
            h,
            Align::Center,
            false,
            &BoxGlyphs,
            None,
            Some(0.8),
        );
        // Each line's ink is 0.7 em: closer lines leave room for a bigger size.
        assert!(
            rows(&tight) > rows(&loose),
            "{} vs {}",
            rows(&tight),
            rows(&loose)
        );
    }

    #[test]
    fn solid_backdrop_fills_the_dither() {
        // A 16×8 band dithered 1 / transparent, with lettering 9 in the middle.
        let (w, h) = (16, 8);
        let art: Vec<u8> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                if (5..11).contains(&x) && (2..6).contains(&y) {
                    9
                } else if (x + y) % 2 == 0 {
                    1
                } else {
                    0
                }
            })
            .collect();
        let part = |backdrop| TextPart {
            text: "Go".into(),
            style: Style::Boxed,
            backdrop,
            ..TextPart::default()
        };
        let solid = render_part(&part(Backdrop::Solid), &art, w, h, &BoxGlyphs, 2).unwrap();
        assert_eq!(solid.fg, 9);
        assert!(solid.base.iter().all(|&b| b == 1));
        let dither = render_part(&part(Backdrop::Art), &art, w, h, &BoxGlyphs, 2).unwrap();
        // The art backdrop keeps the dither, over the old lettering too.
        assert_eq!(
            dither.base.iter().filter(|&&b| b == 0).count(),
            4 * w * h / 2
        );
    }

    #[test]
    fn ink_rectangle_sets_the_box_lettering_colour() {
        // A 32×8 black box (1): white lettering 9 on the left; on the right, lettering that is
        // mostly grey 5 (antialiasing) with a little 9.
        let (w, h) = (32, 8);
        let art: Vec<u8> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                match (x, (2..6).contains(&y)) {
                    (2..10, true) => 9,
                    (18..20, true) => 9,
                    (20..30, true) => 5,
                    _ => 1,
                }
            })
            .collect();
        let part = |ink| TextPart {
            region: Some([16, 0, 16, 8]),
            text: "Go".into(),
            style: Style::Boxed,
            ink,
            ..TextPart::default()
        };
        let own = render_part(&part(None), &art, w, h, &BoxGlyphs, 1).unwrap();
        assert_eq!(own.fg, 5);
        let borrowed = render_part(&part(Some([0, 0, 16, 8])), &art, w, h, &BoxGlyphs, 1).unwrap();
        assert_eq!(borrowed.fg, 9);
    }

    #[test]
    fn text_stays_in_the_original_ink_box() {
        // Lettering 7 in x 10..30, y 5..15 of a 40×20 region with nothing else.
        let (w, h) = (40, 20);
        let art: Vec<u8> = (0..w * h)
            .map(|i| u8::from((10..30).contains(&(i % w)) && (5..15).contains(&(i / w))) * 7)
            .collect();
        let part = TextPart {
            text: "A long English label".into(),
            style: Style::Plain,
            ..TextPart::default()
        };
        let p = render_part(&part, &art, w, h, &BoxGlyphs, 2).unwrap();
        let (sw, sh) = (w * 2, h * 2);
        let inked: Vec<(usize, usize)> = (0..sh)
            .flat_map(|y| (0..sw).map(move |x| (x, y)))
            .filter(|&(x, y)| p.coverage[y * sw + x] > 0)
            .collect();
        assert!(!inked.is_empty());
        // Across: the original rows only; along: the region less the smaller margin (10).
        assert!(
            inked
                .iter()
                .all(|&(x, y)| (20..60).contains(&x) && (10..30).contains(&y))
        );
    }

    #[test]
    fn thin_lettering_keeps_its_fill_inside_a_thick_outline() {
        // A 2-px stroke of 5 in a 1-px outline of 6: the outline is the commoner colour.
        let (w, h) = (24, 12);
        let art: Vec<u8> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                if (3..21).contains(&x) && (3..7).contains(&y) {
                    if (4..20).contains(&x) && (4..6).contains(&y) {
                        5
                    } else {
                        6
                    }
                } else {
                    0
                }
            })
            .collect();
        assert!(art.iter().filter(|&&v| v == 6).count() > art.iter().filter(|&&v| v == 5).count());
        let part = TextPart {
            text: "sec".into(),
            style: Style::Plain,
            outline: true,
            ..TextPart::default()
        };
        let p = render_part(&part, &art, w, h, &BoxGlyphs, 1).unwrap();
        assert_eq!(p.fg, 5);
        assert!(p.base.contains(&6));
    }

    #[test]
    fn macrons_fall_back_to_the_plain_vowel() {
        assert_eq!(strip_macron('ō'), 'o');
        assert_eq!(strip_macron('Ū'), 'U');
        assert_eq!(strip_macron('x'), 'x');
    }

    #[test]
    fn subrect_resolution() {
        assert_eq!(SubRect::WHOLE.resolve(8, 4), Some((0, 0, 8, 4)));
        let d = SubRect {
            col: 0,
            row: 36,
            w: -1,
            h: 3,
        };
        assert_eq!(d.resolve(18, 39), Some((0, 36, 18, 3)));
        assert_eq!(d.canonical().w, 0);
        let over = SubRect {
            col: 6,
            row: 0,
            w: 10,
            h: 2,
        };
        assert_eq!(over.resolve(12, 10), Some((6, 0, 6, 2)));
        assert_eq!(over.resolve(6, 10), None);
    }

    #[test]
    fn fits_and_stays_inside() {
        for (text, w, h) in [
            ("Next stop", 96, 32),
            ("A much longer line than fits\nand a second", 64, 16),
            ("W", 3, 3),
            ("Ōmagari", 32, 96),
        ] {
            for vertical in [false, true] {
                let m = rasterise(text, w, h, Align::Center, vertical, &BoxGlyphs);
                assert_eq!(m.len(), w * h);
                if w > 2 * PAD + 1 && h > 2 * PAD + 1 && !vertical {
                    assert!(m.iter().any(|&c| c > 0), "{text} drew nothing");
                }
            }
        }
        // The fitted text keeps the padding on a generous rectangle.
        let (w, h) = (200, 40);
        let m = rasterise("Hello", w, h, Align::Left, false, &BoxGlyphs);
        for y in 0..h {
            for x in 0..w {
                if m[y * w + x] > 0 {
                    assert!(y >= 1 && y < h - 1, "row {y}");
                }
            }
        }
    }

    #[test]
    fn real_font_if_given() {
        // DDG_FONT names a TTF/OTF file; no font ships with the repo.
        let Some(path) = std::env::var_os("DDG_FONT") else {
            eprintln!("skipped: DDG_FONT not set");
            return;
        };
        let font = FontGlyphs::load(std::path::Path::new(&path)).unwrap();
        let (w, h) = (128, 32);
        let m = rasterise("Stop point", w, h, Align::Center, false, &font);
        assert_eq!(m.len(), w * h);
        assert!(m.iter().filter(|&&c| c > 127).count() > 50);
    }
}

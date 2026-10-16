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
    if vertical {
        // Lay out in the rotated frame, then turn it clockwise.
        let src = rasterise_spaced(text, h, w, align, false, glyphs, cap_px, line_em);
        let mut out = vec![0u8; w * h];
        for sy in 0..w {
            for sx in 0..h {
                // Source pixel (sx, sy) of the h × w image goes to (w − 1 − sy, sx).
                out[sx * w + (w - 1 - sy)] = src[sy * h + sx];
            }
        }
        return out;
    }
    let mut out = vec![0u8; w * h];
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() || w <= 2 * PAD || h <= 2 * PAD {
        return out;
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
    out
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
        let scale = scale.max(1);
        let mut parts = HashMap::new();
        for (tbl, rect, o) in overrides {
            let Some(map) = bank.tilemaps.get(usize::from(tbl)) else {
                continue;
            };
            let Some(r) = rect.resolve(map.width, map.height) else {
                continue;
            };
            let art = bank.render_rect(tbl, r);
            let (pw, ph) = (usize::from(r.2) * TILE_SIZE, usize::from(r.3) * TILE_SIZE);
            let rendered = o
                .parts
                .iter()
                .filter_map(|p| render_part(p, &art, pw, ph, glyphs, scale))
                .collect::<Vec<_>>();
            if !rendered.is_empty() {
                parts.insert((tbl, r), rendered);
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

fn render_part(
    part: &TextPart,
    art: &[u8],
    art_w: usize,
    art_h: usize,
    glyphs: &dyn GlyphSource,
    scale: usize,
) -> Option<RenderedPart> {
    let [x, y, w, h] = part.region.unwrap_or([0, 0, art_w as u16, art_h as u16]);
    // Clip the region to the sprite.
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
    let parity = |px: usize, py: usize| (px & 1) | (py & 1) << 1;
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
    let dominant = ranked.first().copied().unwrap_or(0);
    let second = ranked.get(1).copied().unwrap_or(dominant);
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
    // The original lettering's extent across the reading direction sets the English size, so
    // labels keep the original's type sizes instead of each filling its own box.
    let lettering = |px: usize, py: usize| {
        let v = region_art(px, py);
        v != 0 && (!boxed || v != pattern[parity(px, py)])
    };
    let (mut lo, mut hi) = (usize::MAX, 0);
    for py in 0..h {
        for px in 0..w {
            if lettering(px, py) {
                let k = if part.vertical { px } else { py };
                lo = lo.min(k);
                hi = hi.max(k);
            }
        }
    }
    // The output is `scale` times finer than the art; art lookups use original pixels.
    let s = scale;
    let (sw, sh) = (w * s, h * s);
    let share = if is_cjk(&part.text) {
        CJK_OF_LETTERING
    } else {
        LATIN_CAP_OF_LETTERING
    };
    let cap_px = (lo <= hi).then(|| (hi - lo + 1) as f32 * share * s as f32);
    let coverage = rasterise_spaced(
        &part.text,
        sw,
        sh,
        part.align,
        part.vertical,
        glyphs,
        cap_px,
        part.line_height,
    );
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
        // The text takes the original lettering's colour: the commonest non-backdrop one.
        ranked
            .iter()
            .copied()
            .find(|i| !pattern.contains(i))
            .unwrap_or(second)
    } else {
        if part.outline && second != dominant {
            // One original pixel thick at any scale.
            dilate(&coverage, sw, sh, s, 128, |i| base[i] = second);
        }
        dominant
    };
    Some(RenderedPart {
        region: [(x * s) as u16, (y * s) as u16, sw as u16, sh as u16],
        base,
        coverage,
        fg,
    })
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

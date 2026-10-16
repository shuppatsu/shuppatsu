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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Align {
    Left,
    #[default]
    Center,
    Right,
}

/// How a part is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Style {
    /// `Boxed` when more than [`BOXED_COVERAGE`] of the part's art is opaque, else `Plain`.
    #[default]
    Auto,
    /// Text in the art's dominant colour, outlined in its second colour.
    Plain,
    /// The art's opaque pixels are flattened to its dominant colour (keeping the box shape) and
    /// the text is drawn in its second colour.
    Boxed,
}

/// Opaque share of a part above which `Style::Auto` treats the art as a box or plate.
pub const BOXED_COVERAGE: f32 = 0.55;

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

/// A TrueType/OpenType font through fontdue.
pub struct FontGlyphs(fontdue::Font);

impl FontGlyphs {
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, TextError> {
        fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default())
            .map(Self)
            .map_err(TextError::Font)
    }

    pub fn load(path: &std::path::Path) -> Result<Self, TextError> {
        let bytes = std::fs::read(path).map_err(|source| TextError::Io {
            path: path.to_owned(),
            source,
        })?;
        Self::from_bytes(&bytes)
    }
}

impl GlyphSource for FontGlyphs {
    fn vertical_metrics(&self, px: f32) -> (f32, f32) {
        match self.0.horizontal_line_metrics(px) {
            Some(m) => (m.ascent, m.descent),
            None => (px * 0.8, -px * 0.2),
        }
    }

    fn glyph(&self, ch: char, px: f32) -> Glyph {
        let (m, coverage) = self.0.rasterize(ch, px);
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
        self.0.metrics(ch, px).advance_width
    }
}

/// Inner padding in pixels, room for the outline.
const PAD: usize = 1;
/// Largest and smallest font sizes tried, in pixels.
const MAX_PX: f32 = 64.0;
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
    if vertical {
        // Lay out in the rotated frame, then turn it clockwise.
        let src = rasterise(text, h, w, align, false, glyphs);
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
    let line_h = |px: f32| {
        let (a, d) = glyphs.vertical_metrics(px);
        a - d
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
    let by_h = ah * reference / (line_h(reference) * lines.len() as f32).max(1.0);
    let mut px = by_w.min(by_h).clamp(MIN_PX, MAX_PX);
    while px > MIN_PX
        && (lines.iter().any(|l| width_at(l, px) > aw) || line_h(px) * lines.len() as f32 > ah)
    {
        px = (px - 0.5).max(MIN_PX);
    }
    let (ascent, _) = glyphs.vertical_metrics(px);
    let lh = line_h(px);
    let top = PAD as f32 + (ah - lh * lines.len() as f32) / 2.0;
    for (i, line) in lines.iter().enumerate() {
        let lw = width_at(line, px);
        let mut pen = PAD as f32
            + match align {
                Align::Left => 0.0,
                Align::Center => (aw - lw) / 2.0,
                Align::Right => aw - lw,
            };
        let baseline = (top + lh * i as f32 + ascent).round() as i32;
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
    /// `[x, y, w, h]` relative to the sprite's source rectangle, in pixels.
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
}

impl TextOverlay {
    /// Renders every override against the bank's art. Overrides whose tilemap or rectangle
    /// does not exist are skipped (the art is then drawn as usual).
    pub fn build<'a>(
        bank: &SpriteBank,
        overrides: impl IntoIterator<Item = (u16, SubRect, &'a TextOverride)>,
        glyphs: &dyn GlyphSource,
    ) -> Self {
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
                .filter_map(|p| render_part(p, &art, pw, ph, glyphs))
                .collect::<Vec<_>>();
            if !rendered.is_empty() {
                parts.insert((tbl, r), rendered);
            }
        }
        Self { parts }
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
    let mut counts = [0u32; 256];
    for py in 0..h {
        for px in 0..w {
            counts[usize::from(region_art(px, py))] += 1;
        }
    }
    let opaque: u32 = counts[1..].iter().sum();
    let mut ranked: Vec<u8> = (1..=255u8)
        .filter(|&i| counts[usize::from(i)] > 0)
        .collect();
    ranked.sort_by_key(|&i| std::cmp::Reverse(counts[usize::from(i)]));
    let dominant = ranked.first().copied().unwrap_or(0);
    let second = ranked.get(1).copied().unwrap_or(dominant);
    let boxed = match part.style {
        Style::Boxed => true,
        Style::Plain => false,
        Style::Auto => opaque as f32 > BOXED_COVERAGE * (w * h) as f32,
    };
    let coverage = rasterise(&part.text, w, h, part.align, part.vertical, glyphs);
    let mut base = vec![0u8; w * h];
    let fg = if boxed {
        for py in 0..h {
            for px in 0..w {
                if region_art(px, py) != 0 {
                    base[py * w + px] = dominant;
                }
            }
        }
        second
    } else {
        if part.outline && second != dominant {
            let solid = |px: isize, py: isize| {
                (0..w as isize).contains(&px)
                    && (0..h as isize).contains(&py)
                    && coverage[py as usize * w + px as usize] >= 128
            };
            for py in 0..h as isize {
                for px in 0..w as isize {
                    let near = (-1..=1).any(|dy| (-1..=1).any(|dx| solid(px + dx, py + dy)));
                    if near {
                        base[py as usize * w + px as usize] = second;
                    }
                }
            }
        }
        dominant
    };
    Some(RenderedPart {
        region: [x as u16, y as u16, w as u16, h as u16],
        base,
        coverage,
        fg,
    })
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

//! Effects of the original lettering: a drop shadow or an outline (docs/localisation.md,
//! "Shadows and outlines").
//!
//! Much of the original's lettering is not flat: 時間切れ is yellow over a black copy two
//! pixels left and one up, the options' labels cast a dark copy three pixels down and right,
//! many menu words have a one-pixel rim. [`analyse`] finds such an effect in the lettering
//! pixels of a part's region, so the English drawn over it can repeat it in the same palette
//! index (palette swaps keep working) and at the same offset or width.
//!
//! Terms (palette indices; 0 = not lettering: transparency, the backdrop of a box, or outside
//! the region):
//! - *fill*: the colour of the strokes;
//! - *shadow*: a copy of the strokes (or of the outlined strokes) shifted by `(dx, dy)` and
//!   drawn under them, in its own colour: every shadow pixel lies `(dx, dy)` from the strokes,
//!   and few lie `(−dx, −dy)` from them (which tells it from an outline);
//! - *outline*: a rim around the fill on every side, one or more pixels wide: no fill pixel
//!   touches non-lettering.

use crate::text::{RegionScan, TextPart};

/// A drop shadow: the strokes drawn again in `colour`, `(dx, dy)` original pixels away, or
/// (`extrude`) swept there, as a solid extrusion (時間切れ: every offset from (0, 0) to
/// (−2, −1)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shadow {
    pub dx: i8,
    pub dy: i8,
    pub colour: u8,
    pub extrude: bool,
}

/// An outline `width` original pixels wide in `colour` (a square neighbourhood).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outline {
    pub colour: u8,
    pub width: u8,
}

/// What [`analyse`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Effects {
    /// The strokes' colour.
    pub fill: u8,
    /// The commonest other lettering colour (a shadow given by hand takes it).
    pub second: Option<u8>,
    pub outline: Option<Outline>,
    /// Of the outlined strokes when there is an outline too.
    pub shadow: Option<Shadow>,
}

impl Effects {
    /// Whether there is any effect.
    pub fn any(&self) -> bool {
        self.outline.is_some() || self.shadow.is_some()
    }
}

/// Largest shadow offset looked for, per axis (original pixels).
const MAX_OFFSET: i32 = 4;
/// Smallest offset found by itself (per axis, the larger of the two): a one-pixel offset is as
/// likely a bevel, a gradient or a split-flap gap as a shadow (`shadow = [dx, dy]` gives one).
const MIN_OFFSET: i32 = 1;
/// A colour takes part when it has at least this share of the lettering pixels ...
const MIN_SHARE: f32 = 0.04;
/// ... and at least this many pixels.
const MIN_PIXELS: usize = 8;
/// The fill has at least this share of the lettering pixels.
const FILL_SHARE: f32 = 0.2;
/// Shadow: the share of the shadow colour's pixels that lie `(dx, dy)` from the strokes.
const SHADOW_HIT: f32 = 0.8;
/// Shadow: the share of the pixels the shadow adds around the strokes that the art has in the
/// shadow colour.
const SHADOW_COVER: f32 = 0.8;
/// Shadow: at most this share of the shadow lies `(−dx, −dy)` from the strokes too (an outline
/// lies on both sides).
const SHADOW_BACK: f32 = 0.5;
/// Shadow: at least this many pixels per stroke pixel (a rule or a gap line through the
/// lettering is not its shadow).
const SHADOW_MASS: f32 = 0.25;
/// Shadow: its extent along each axis at least this share of the strokes'.
const SHADOW_SPAN: f32 = 0.6;
/// Shadow: offsets scoring within this of the best one count as equally good.
const SHADOW_TIE: f32 = 0.05;
/// Outline: the share of the fill's pixels with no 4-neighbour outside the lettering.
const OUTLINE_ENCLOSED: f32 = 0.9;
/// Outline: the share of the rim colour's pixels within the outline's width of the fill.
const OUTLINE_NEAR: f32 = 0.85;
/// Outline: the share of the ring around the fill in the rim colour.
const OUTLINE_RING: f32 = 0.8;

/// A `w × h` grid of lettering colours (0 = none) with reads outside it as 0.
struct Grid<'a> {
    px: &'a [u8],
    w: usize,
    h: usize,
}

impl Grid<'_> {
    fn at(&self, x: i32, y: i32) -> u8 {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            0
        } else {
            self.px[y as usize * self.w + x as usize]
        }
    }

    fn inside(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && x < self.w as i32 && y < self.h as i32
    }

    fn coords(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        (0..self.h as i32).flat_map(move |y| (0..self.w as i32).map(move |x| (x, y)))
    }
}

/// Finds a shadow or outline in the `w × h` lettering `px` (palette indices, 0 = not
/// lettering). `None` without lettering; [`Effects::any`] is false when it is flat.
pub fn analyse(px: &[u8], w: usize, h: usize) -> Option<Effects> {
    assert_eq!(px.len(), w * h, "lettering grid size");
    let g = Grid { px, w, h };
    let mut counts = [0usize; 256];
    for &v in px {
        counts[usize::from(v)] += 1;
    }
    let total: usize = counts[1..].iter().sum();
    if total == 0 {
        return None;
    }
    let mut colours: Vec<u8> = (1..=255u8)
        .filter(|&c| {
            let n = counts[usize::from(c)];
            n >= MIN_PIXELS && n as f32 >= MIN_SHARE * total as f32
        })
        .collect();
    colours.sort_by_key(|&c| std::cmp::Reverse(counts[usize::from(c)]));
    colours.truncate(4);
    // The commonest lettering colour other than `fill`.
    let second = |fill: u8| {
        (1..=255u8)
            .filter(|&c| c != fill && counts[usize::from(c)] > 0)
            .max_by_key(|&c| counts[usize::from(c)])
    };
    let fill0 = colours.first().copied().unwrap_or_else(|| {
        (1..=255u8)
            .max_by_key(|&c| counts[usize::from(c)])
            .unwrap_or(1)
    });
    let flat = Effects {
        fill: fill0,
        second: second(fill0),
        outline: None,
        shadow: None,
    };
    if colours.len() < 2 {
        return Some(flat);
    }
    // A plain drop shadow: one colour is a shifted copy of another.
    let mut best: Option<(f32, Effects)> = None;
    // Strokes are a good share of the lettering (not a gap line or a rule through it).
    let strokes_colour = |c: &&u8| counts[usize::from(**c)] as f32 >= FILL_SHARE * total as f32;
    for &fill in colours.iter().filter(strokes_colour) {
        let body = |x: i32, y: i32| g.at(x, y) == fill;
        for &colour in colours.iter().filter(|&&c| c != fill) {
            if let Some((score, s)) = find_shadow(&g, &body, counts[usize::from(fill)], colour)
                && best.is_none_or(|(b, _)| score > b)
            {
                let e = Effects {
                    fill,
                    second: Some(colour),
                    outline: None,
                    shadow: Some(s),
                };
                best = Some((score, e));
            }
        }
    }
    if let Some((_, e)) = best {
        return Some(e);
    }
    // An outline, perhaps with a shadow of the outlined strokes.
    for &fill in colours.iter().filter(strokes_colour) {
        let Some(outline) = find_outline(&g, fill, &colours) else {
            continue;
        };
        let r = i32::from(outline.width);
        let near_fill =
            |x: i32, y: i32| (-r..=r).any(|dy| (-r..=r).any(|dx| g.at(x + dx, y + dy) == fill));
        let body = |x: i32, y: i32| {
            let v = g.at(x, y);
            v == fill || (v == outline.colour && near_fill(x, y))
        };
        let n_body = g.coords().filter(|&(x, y)| body(x, y)).count();
        let shadow = colours
            .iter()
            .filter(|&&c| c != fill)
            .filter_map(|&c| find_shadow(&g, &body, n_body, c))
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, s)| s);
        return Some(Effects {
            fill,
            second: second(fill),
            outline: Some(outline),
            shadow,
        });
    }
    Some(flat)
}

/// Which drop shadow a part's English gets (`TextPart::shadow`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ShadowMode {
    /// The one found in the original lettering, if any.
    #[default]
    Auto,
    /// None, whatever the art has (`shadow = false`).
    Off,
    /// At this offset in original pixels (`shadow = [dx, dy]`; swept, `shadow = [dx, dy,
    /// "extrude"]`), in the colour of the shadow
    /// found in the art, else in its second lettering colour.
    At { dx: i8, dy: i8, extrude: bool },
}

/// The effects `part`'s English repeats: found in the original lettering of its region (or
/// of `part.shadow_ink`), with the part's overrides applied (`shadow`, `outline = false`).
/// `None` when there is no lettering.
pub fn for_part(
    part: &TextPart,
    scan: &RegionScan,
    art: &[u8],
    art_w: usize,
    art_h: usize,
) -> Option<Effects> {
    let found = of_region(scan, art, art_w, art_h, part.shadow_ink)?;
    let shadow = match part.shadow {
        ShadowMode::Auto => found.shadow,
        ShadowMode::Off => None,
        ShadowMode::At { dx, dy, extrude } => Some(Shadow {
            dx,
            dy,
            extrude,
            colour: found
                .shadow
                .map(|s| s.colour)
                .or(found.second)
                .unwrap_or(found.fill),
        }),
    };
    Some(Effects {
        outline: found.outline.filter(|_| part.outline),
        shadow,
        ..found
    })
}

/// [`analyse`] of the lettering of a part's region (`scan`, over the `art_w × art_h` sprite
/// `art`), or of the lettering in `rect` (`[x, y, w, h]` sprite pixels, clipped) when given:
/// what is lettering follows the scan (a box's backdrop pattern is not).
pub fn of_region(
    scan: &RegionScan,
    art: &[u8],
    art_w: usize,
    art_h: usize,
    rect: Option<[u16; 4]>,
) -> Option<Effects> {
    let [x, y, w, h] = match rect {
        Some(r) => r.map(usize::from),
        None => scan.rect,
    };
    let (x, y) = (x.min(art_w), y.min(art_h));
    let (w, h) = (w.min(art_w - x), h.min(art_h - y));
    let px: Vec<u8> = (0..w * h)
        .map(|i| {
            let (ax, ay) = (x + i % w, y + i / w);
            let v = art[ay * art_w + ax];
            if scan.lettering_at(ax, ay, v) { v } else { 0 }
        })
        .collect();
    analysed(px, w, h)
}

/// [`analyse`], remembered for the process: it depends only on the pixels, and every change of
/// the canvas multiple rasterises all translated text again (the analysis was most of it).
fn analysed(px: Vec<u8>, w: usize, h: usize) -> Option<Effects> {
    type Memo = std::collections::HashMap<(usize, usize, Vec<u8>), Option<Effects>>;
    static MEMO: std::sync::OnceLock<std::sync::Mutex<Memo>> = std::sync::OnceLock::new();
    let memo = MEMO.get_or_init(Default::default);
    let key = (w, h, px);
    if let Some(e) = memo.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return *e;
    }
    let e = analyse(&key.2, w, h);
    memo.lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, e);
    e
}

/// The best shadow in `colour` of the strokes `body` (`n_body` pixels): its score and offset.
/// Only pixels of `colour` outside `body` count as shadow.
fn find_shadow(
    g: &Grid<'_>,
    body_at: &impl Fn(i32, i32) -> bool,
    n_body: usize,
    colour: u8,
) -> Option<(f32, Shadow)> {
    // The strokes as a bitmap (the callers' tests are costly and asked many times per pixel;
    // outside the grid there are none), and its summed-area table for the swept boxes.
    let (w, h) = (g.w, g.h);
    let map: Vec<bool> = g.coords().map(|(x, y)| body_at(x, y)).collect();
    let body = |x: i32, y: i32| g.inside(x, y) && map[y as usize * w + x as usize];
    let mut sat = vec![0u32; (w + 1) * (h + 1)];
    for y in 0..h {
        for x in 0..w {
            sat[(y + 1) * (w + 1) + x + 1] =
                u32::from(map[y * w + x]) + sat[y * (w + 1) + x + 1] + sat[(y + 1) * (w + 1) + x]
                    - sat[y * (w + 1) + x];
        }
    }
    // Stroke pixels in the box between two corners (inclusive, in any order), cut to the grid.
    let in_box = |(ax, ay): (i32, i32), (bx, by): (i32, i32)| {
        let x0 = ax.min(bx).max(0);
        let x1 = (ax.max(bx) + 1).min(w as i32);
        let y0 = ay.min(by).max(0);
        let y1 = (ay.max(by) + 1).min(h as i32);
        if x0 >= x1 || y0 >= y1 {
            return 0;
        }
        let s = |x: i32, y: i32| sat[y as usize * (w + 1) + x as usize];
        s(x1, y1) + s(x0, y0) - s(x0, y1) - s(x1, y0)
    };
    let shade: Vec<(i32, i32)> = g
        .coords()
        .filter(|&(x, y)| g.at(x, y) == colour && !body(x, y))
        .collect();
    if shade.len() < MIN_PIXELS || (shade.len() as f32) < SHADOW_MASS * n_body as f32 {
        return None;
    }
    let strokes: Vec<(i32, i32)> = g.coords().filter(|&(x, y)| body(x, y)).collect();
    // A shadow spans about as much as the strokes (a split-flap gap or a rule through the
    // lettering is one row).
    let extent = |px: &[(i32, i32)]| {
        let span = |f: fn(&(i32, i32)) -> i32| {
            px.iter().map(f).max().unwrap_or(0) - px.iter().map(f).min().unwrap_or(0) + 1
        };
        (span(|p| p.0), span(|p| p.1))
    };
    let ((sw, sh), (bw, bh)) = (extent(&shade), extent(&strokes));
    if (sw as f32) < SHADOW_SPAN * bw as f32 || (sh as f32) < SHADOW_SPAN * bh as f32 {
        return None;
    }
    let n = shade.len() as f32;
    let share =
        |f: &dyn Fn(i32, i32) -> bool| shade.iter().filter(|&&(x, y)| f(x, y)).count() as f32 / n;
    let mut found: Vec<(f32, Shadow)> = Vec::new();
    for dy in -MAX_OFFSET..=MAX_OFFSET {
        for dx in -MAX_OFFSET..=MAX_OFFSET {
            if dx.abs().max(dy.abs()) < MIN_OFFSET {
                continue;
            }
            // A copy at (dx, dy), else the strokes swept there (every offset in the box).
            let steps = sweep(dx, dy);
            // Strokes at any offset of the box but (0, 0): the box count less the pixel itself.
            let swept = |x: i32, y: i32, sign: i32| {
                in_box((x, y), (x - sign * dx, y - sign * dy)) > u32::from(body(x, y))
            };
            let (extrude, hit, back) = {
                let hit = share(&|x, y| body(x - dx, y - dy));
                if hit >= SHADOW_HIT {
                    (false, hit, share(&|x, y| body(x + dx, y + dy)))
                } else {
                    (
                        true,
                        share(&|x, y| swept(x, y, 1)),
                        share(&|x, y| swept(x, y, -1)),
                    )
                }
            };
            if hit < SHADOW_HIT || back > SHADOW_BACK * hit {
                continue;
            }
            // The shadow drawn from the strokes must be what the art has there: of the pixels
            // it adds (inside the grid: the region's edge may cut the art's shadow), the share
            // in `colour`.
            let mut predicted = vec![false; g.w * g.h];
            for &(x, y) in &strokes {
                let mut mark = |tx: i32, ty: i32| {
                    let (px, py) = (x + tx, y + ty);
                    if g.inside(px, py) && !body(px, py) {
                        predicted[py as usize * g.w + px as usize] = true;
                    }
                };
                if extrude {
                    steps.iter().for_each(|&(tx, ty)| mark(tx, ty));
                } else {
                    mark(dx, dy);
                }
            }
            let (mut drawn, mut right) = (0usize, 0usize);
            for (i, _) in predicted.iter().enumerate().filter(|(_, p)| **p) {
                drawn += 1;
                right += usize::from(g.px[i] == colour);
            }
            let cover = right as f32 / drawn.max(1) as f32;
            if cover < SHADOW_COVER {
                continue;
            }
            let shadow = Shadow {
                dx: dx as i8,
                dy: dy as i8,
                colour,
                extrude,
            };
            found.push((hit + cover - back, shadow));
        }
    }
    // The smallest offset that scores about as well as the best (a larger sweep always
    // explains at least as much); a copy before a sweep.
    let best = found.iter().map(|f| f.0).fold(f32::MIN, f32::max);
    found
        .into_iter()
        .filter(|f| f.0 >= best - SHADOW_TIE)
        .min_by_key(|(_, s)| {
            let (x, y) = (i32::from(s.dx), i32::from(s.dy));
            (s.extrude, x.abs().max(y.abs()), x.abs() + y.abs())
        })
        .map(|(_, s)| (best, s))
}

/// The offsets a swept shadow covers: every `(tx, ty)` between `(0, 0)` (exclusive) and
/// `(dx, dy)` (inclusive), a box.
pub fn sweep(dx: i32, dy: i32) -> Vec<(i32, i32)> {
    let span = |d: i32| if d >= 0 { 0..=d } else { d..=0 };
    span(dy)
        .flat_map(|ty| span(dx).map(move |tx| (tx, ty)))
        .filter(|&t| t != (0, 0))
        .collect()
}

/// An outline around the strokes of `fill`, in one of `colours`.
fn find_outline(g: &Grid<'_>, fill: u8, colours: &[u8]) -> Option<Outline> {
    let fill_px: Vec<(i32, i32)> = g.coords().filter(|&(x, y)| g.at(x, y) == fill).collect();
    if fill_px.is_empty() {
        return None;
    }
    let enclosed = fill_px
        .iter()
        .filter(|&&(x, y)| {
            [(-1, 0), (1, 0), (0, -1), (0, 1)]
                .iter()
                .all(|&(dx, dy)| g.at(x + dx, y + dy) != 0)
        })
        .count() as f32
        / fill_px.len() as f32;
    if enclosed < OUTLINE_ENCLOSED {
        return None;
    }
    // The rim: the colour most often next to the fill, outside it.
    let mut next = [0usize; 256];
    for &(x, y) in &fill_px {
        for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            next[usize::from(g.at(x + dx, y + dy))] += 1;
        }
    }
    let colour = colours
        .iter()
        .copied()
        .filter(|&c| c != fill)
        .max_by_key(|&c| next[usize::from(c)])?;
    if next[usize::from(colour)] == 0 {
        return None;
    }
    // The ring around the fill (pixels 4-adjacent to it) is the rim colour nearly all round:
    // antialiasing is a mix of greys there, with the backdrop between them.
    let (mut ring, mut ringed) = (0usize, 0usize);
    for (x, y) in g.coords() {
        let v = g.at(x, y);
        if v == fill {
            continue;
        }
        if [(-1, 0), (1, 0), (0, -1), (0, 1)]
            .iter()
            .any(|&(dx, dy)| g.at(x + dx, y + dy) == fill)
        {
            ring += 1;
            ringed += usize::from(v == colour);
        }
    }
    let ring_share = ringed as f32 / ring.max(1) as f32;
    if ring_share < OUTLINE_RING {
        return None;
    }
    let rim: Vec<(i32, i32)> = g.coords().filter(|&(x, y)| g.at(x, y) == colour).collect();
    let near = |r: i32| {
        rim.iter()
            .filter(|&&(x, y)| (-r..=r).any(|dy| (-r..=r).any(|dx| g.at(x + dx, y + dy) == fill)))
            .count() as f32
            / rim.len() as f32
    };
    (1..=3).find(|&r| near(r) >= OUTLINE_NEAR).map(|r| Outline {
        colour,
        width: r as u8,
    })
}

/// The [`gradient`] `part`'s English repeats: none for a part marked `gradient = false`,
/// vertical or multi-line text (the bands belong to one line of lettering), or lettering with
/// a shadow or outline found (`effects`: its rows alternate with the shadow's).
pub fn gradient_for(
    part: &TextPart,
    scan: &RegionScan,
    art: &[u8],
    art_w: usize,
    art_h: usize,
    effects: Option<Effects>,
) -> Option<Vec<[u8; 2]>> {
    if part.flat || part.vertical || part.text.contains('\n') || effects.is_some_and(|e| e.any()) {
        return None;
    }
    gradient(scan, art, art_w, art_h)
}

/// A vertical gradient in the strokes of a part's lettering (準備, 運転評価, 仕業検査: bands of
/// colour from top to bottom, some of them dithered): the fill of each row from the first row
/// of strokes to the last, as the colours of its even and odd sprite columns, or `None` when
/// the strokes are not a gradient.
///
/// A row's pair is the commonest colour among its interior lettering pixels (all four
/// neighbours lettering, so a rim or outline does not count) in even and in odd columns; it
/// needs [`GRADIENT_ROW`] of them. Rows without a clear pair take the one above; at least
/// [`GRADIENT_CLEAR`] of the rows need one, and the rows together at least [`GRADIENT_BANDS`]
/// colours.
pub fn gradient(scan: &RegionScan, art: &[u8], art_w: usize, art_h: usize) -> Option<Vec<[u8; 2]>> {
    // Lettering on transparency only: in a box, a split-flap gap or a bevel crossing the
    // lettering reads as bands too.
    if scan.boxed {
        return None;
    }
    let [x, y, w, h] = scan.rect;
    let at = |px: i32, py: i32| -> u8 {
        if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 {
            return 0;
        }
        let (ax, ay) = (x + px as usize, y + py as usize);
        if ax >= art_w || ay >= art_h {
            return 0;
        }
        let v = art[ay * art_w + ax];
        if scan.lettering_at(ax, ay, v) { v } else { 0 }
    };
    // An outline's colour: most of the pixels next to transparency but few of the rest. Its
    // rows (thin horizontal strokes are all outline) are not bands of the gradient.
    let (mut rim, mut body) = ([0usize; 256], [0usize; 256]);
    for py in 0..h as i32 {
        for px in 0..w as i32 {
            let v = at(px, py);
            if v == 0 {
                continue;
            }
            let edge = [(-1, 0), (1, 0), (0, -1), (0, 1)]
                .iter()
                .any(|&(dx, dy)| at(px + dx, py + dy) == 0);
            if edge {
                rim[usize::from(v)] += 1;
            } else {
                body[usize::from(v)] += 1;
            }
        }
    }
    let (n_rim, n_body) = (rim.iter().sum::<usize>(), body.iter().sum::<usize>());
    let outline = (1..=255u8)
        .max_by_key(|&c| rim[usize::from(c)])
        .filter(|&c| {
            rim[usize::from(c)] as f32 >= 0.6 * n_rim as f32
                && (body[usize::from(c)] as f32) < 0.2 * n_body as f32
        });
    let mut rows: Vec<Option<[u8; 2]>> = Vec::with_capacity(h);
    let mut any_rows = 0usize;
    for py in 0..h as i32 {
        let mut counts = [[0usize; 256]; 2];
        let mut n = 0usize;
        for px in 0..w as i32 {
            let v = at(px, py);
            let inner = v != 0
                && [(-1, 0), (1, 0), (0, -1), (0, 1)]
                    .iter()
                    .all(|&(dx, dy)| at(px + dx, py + dy) != 0);
            if inner {
                counts[(x + px as usize) & 1][usize::from(v)] += 1;
                n += 1;
            }
        }
        any_rows += usize::from(n > 0);
        let best = |c: &[usize; 256]| (1..=255u8).max_by_key(|&i| c[usize::from(i)]);
        let pair = best(&counts[0]).zip(best(&counts[1])).filter(|&(e, o)| {
            let hit = counts[0][usize::from(e)] + counts[1][usize::from(o)];
            n >= GRADIENT_PIXELS
                && hit as f32 >= GRADIENT_ROW * n as f32
                && outline.is_none_or(|c| e != c && o != c)
        });
        // A row with strokes in only one parity of columns repeats its colour.
        let pair = pair.map(|(e, o)| {
            let e = if counts[0][usize::from(e)] == 0 { o } else { e };
            let o = if counts[1][usize::from(o)] == 0 { e } else { o };
            [e, o]
        });
        rows.push(pair);
    }
    let first = rows.iter().position(Option::is_some)?;
    let last = rows.iter().rposition(Option::is_some)?;
    let clear = rows.iter().filter(|r| r.is_some()).count();
    if (clear as f32) < GRADIENT_CLEAR * any_rows as f32 {
        return None;
    }
    let mut out: Vec<[u8; 2]> = Vec::with_capacity(last + 1 - first);
    for r in &rows[first..=last] {
        let c = r.or(out.last().copied())?;
        out.push(c);
    }
    let mut colours: Vec<u8> = out.iter().flatten().copied().collect();
    colours.sort_unstable();
    colours.dedup();
    (colours.len() >= GRADIENT_BANDS).then_some(out)
}

/// [`gradient`]: the least number of colours, the share of a row's interior pixels its pair
/// needs, the least number of interior pixels in a row, and the share of the rows with
/// interior pixels that need a clear pair.
const GRADIENT_BANDS: usize = 3;
const GRADIENT_ROW: f32 = 0.5;
const GRADIENT_PIXELS: usize = 4;
const GRADIENT_CLEAR: f32 = 0.6;

/// The text colour of each pixel of a `w × h` coverage mask at `scale` canvas pixels per
/// original pixel for the original's `gradient` ([`gradient`]): the gradient stretched over
/// the English's own ink rows, so its bands sit at the same heights of the letters, and its
/// dithers kept in original-pixel columns.
/// `x0` is the sprite column of the mask's left edge (for the dither's phase).
pub fn spread(
    gradient: &[[u8; 2]],
    coverage: &[u8],
    w: usize,
    h: usize,
    scale: usize,
    x0: usize,
) -> Vec<u8> {
    let inked = |r: usize| coverage[r * w..(r + 1) * w].iter().any(|&c| c >= 128);
    let (Some(top), Some(bottom)) = ((0..h).find(|&r| inked(r)), (0..h).rfind(|&r| inked(r)))
    else {
        return Vec::new();
    };
    let last = gradient.len().saturating_sub(1);
    let mut out = Vec::with_capacity(w * h);
    for r in 0..h {
        let t = (r.clamp(top, bottom) - top) as f32 / (bottom - top).max(1) as f32;
        let pair = gradient[((t * last as f32).round() as usize).min(last)];
        out.extend((0..w).map(|c| pair[(x0 + c / scale.max(1)) & 1]));
    }
    out
}

/// Canvas pixels `[left, top, right, bottom]` the text's box gives up for `shadow` at `scale`
/// canvas pixels per original pixel: the original's ink box includes the shadow, so the
/// strokes keep clear of it on the shadow's side.
pub fn shadow_room(shadow: Option<Shadow>, scale: usize) -> [usize; 4] {
    let Some(s) = shadow else {
        return [0; 4];
    };
    let d = |v: i8| usize::from(v.unsigned_abs()) * scale;
    let (dx, dy) = (d(s.dx), d(s.dy));
    [
        if s.dx < 0 { dx } else { 0 },
        if s.dy < 0 { dy } else { 0 },
        if s.dx > 0 { dx } else { 0 },
        if s.dy > 0 { dy } else { 0 },
    ]
}

/// Paints the effects under text `coverage` (a `w × h` mask at `scale` canvas pixels per
/// original pixel) into `base` (palette indices; 0 = nothing): the shadow of the strokes (with
/// their outline), then the outline. Pixels are taken as strokes from half coverage, so the
/// effects stay crisp like the art's.
pub fn paint(
    coverage: &[u8],
    base: &mut [u8],
    w: usize,
    h: usize,
    scale: usize,
    outline: Option<Outline>,
    shadow: Option<Shadow>,
) {
    let inked = |x: i32, y: i32| {
        x >= 0
            && y >= 0
            && (x as usize) < w
            && (y as usize) < h
            && coverage[y as usize * w + x as usize] >= 128
    };
    // The strokes with their outline (a square of `width` original pixels round each).
    let r = outline.map_or(0, |o| i32::from(o.width) * scale as i32);
    let mut body = vec![false; w * h];
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            body[y as usize * w + x as usize] =
                (-r..=r).any(|dy| (-r..=r).any(|dx| inked(x + dx, y + dy)));
        }
    }
    let in_body = |x: i32, y: i32| {
        x >= 0
            && y >= 0
            && (x as usize) < w
            && (y as usize) < h
            && body[y as usize * w + x as usize]
    };
    if let Some(s) = shadow {
        let k = scale as i32;
        let (dx, dy) = (i32::from(s.dx) * k, i32::from(s.dy) * k);
        let steps = if s.extrude {
            sweep(dx, dy)
        } else {
            vec![(dx, dy)]
        };
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                if !in_body(x, y) && steps.iter().any(|&(tx, ty)| in_body(x - tx, y - ty)) {
                    base[y as usize * w + x as usize] = s.colour;
                }
            }
        }
    }
    if let Some(o) = outline {
        for (i, b) in body.iter().enumerate() {
            if *b {
                base[i] = o.colour;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grid from rows of characters: `.` = 0, digits = that index.
    fn grid(rows: &[&str]) -> (Vec<u8>, usize, usize) {
        let w = rows[0].len();
        let px = rows
            .iter()
            .flat_map(|r| r.bytes().map(|b| if b == b'.' { 0 } else { b - b'0' }))
            .collect();
        (px, w, rows.len())
    }

    /// Strokes of 5 with a copy in 1 at `(dx, dy)` (1 under 5), or swept there.
    fn shadowed(dx: i32, dy: i32, extrude: bool) -> (Vec<u8>, usize, usize) {
        let (w, h) = (24usize, 16usize);
        let strokes = |x: i32, y: i32| {
            (4..20).contains(&x) && (5..7).contains(&y)
                || (6..8).contains(&x) && (3..13).contains(&y)
                || (14..16).contains(&x) && (3..13).contains(&y)
        };
        let mut px = vec![0u8; w * h];
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                px[y as usize * w + x as usize] = if strokes(x, y) {
                    5
                } else if strokes(x - dx, y - dy)
                    || extrude
                        && sweep(dx, dy)
                            .iter()
                            .any(|&(tx, ty)| strokes(x - tx, y - ty))
                {
                    1
                } else {
                    0
                };
            }
        }
        (px, w, h)
    }

    #[test]
    fn a_shadow_down_right_is_found() {
        let (px, w, h) = shadowed(2, 2, false);
        let e = analyse(&px, w, h).unwrap();
        assert_eq!(e.fill, 5);
        assert_eq!(
            e.shadow,
            Some(Shadow {
                dx: 2,
                dy: 2,
                colour: 1,
                extrude: false,
            })
        );
        assert_eq!(e.outline, None);
    }

    #[test]
    fn an_extrusion_up_left_is_found() {
        let (px, w, h) = shadowed(-2, -1, true);
        let e = analyse(&px, w, h).unwrap();
        assert_eq!(e.fill, 5);
        let s = e.shadow.unwrap();
        assert_eq!((s.dx, s.dy, s.colour, s.extrude), (-2, -1, 1, true));
    }

    #[test]
    fn an_outline_is_not_a_shadow() {
        let (px, w, h) = grid(&[
            "..........",
            ".1111111..",
            ".1555551..",
            ".1555551..",
            ".1111551..",
            "....1551..",
            "....1551..",
            "....1551..",
            "....1111..",
            "..........",
        ]);
        let e = analyse(&px, w, h).unwrap();
        assert_eq!(e.fill, 5);
        assert_eq!(e.shadow, None);
        assert_eq!(
            e.outline,
            Some(Outline {
                colour: 1,
                width: 1
            })
        );
    }

    #[test]
    fn flat_lettering_has_no_effect() {
        let (px, w, h) = grid(&[
            "..........",
            ".5555555..",
            ".5555555..",
            "....55....",
            "....55....",
            "....55....",
            "..........",
        ]);
        let e = analyse(&px, w, h).unwrap();
        assert_eq!(e.fill, 5);
        assert!(!e.any());
    }

    #[test]
    fn the_shadow_is_painted_at_the_canvas_scale() {
        // One inked canvas pixel at (2, 2); a (1, 1) shadow at scale 2 lands at (4, 4).
        let (w, h) = (8, 8);
        let mut coverage = vec![0u8; w * h];
        coverage[2 * w + 2] = 255;
        let mut base = vec![0u8; w * h];
        let shadow = Shadow {
            dx: 1,
            dy: 1,
            colour: 3,
            extrude: false,
        };
        paint(&coverage, &mut base, w, h, 2, None, Some(shadow));
        let painted: Vec<usize> = (0..w * h).filter(|&i| base[i] != 0).collect();
        assert_eq!(painted, vec![4 * w + 4]);
        // Swept, every offset up to (2, 2) canvas pixels is painted (not the strokes).
        let mut base = vec![0u8; w * h];
        let swept = Shadow {
            extrude: true,
            ..shadow
        };
        paint(&coverage, &mut base, w, h, 2, None, Some(swept));
        assert_eq!(base.iter().filter(|&&b| b == 3).count(), 8);
        assert_eq!(base[2 * w + 2], 0);
    }

    #[test]
    fn an_outline_is_painted_round_the_strokes() {
        let (w, h) = (5, 5);
        let mut coverage = vec![0u8; w * h];
        coverage[2 * w + 2] = 255;
        let mut base = vec![0u8; w * h];
        let outline = Outline {
            colour: 4,
            width: 1,
        };
        paint(&coverage, &mut base, w, h, 1, Some(outline), None);
        assert_eq!(base.iter().filter(|&&b| b == 4).count(), 9);
    }

    #[test]
    fn bands_of_colour_down_the_strokes_are_a_gradient() {
        // A 12×9 block on transparency: rows 1–3 in 4, 4–6 in 5 / 6 dithered, 7–9 in 7.
        let (w, h) = (16usize, 12usize);
        let mut art = vec![0u8; w * h];
        for y in 1..10 {
            for x in 2..14 {
                art[y * w + x] = match y {
                    1..=3 => 4,
                    4..=6 => 5 + (x & 1) as u8,
                    _ => 7,
                };
            }
        }
        let part = TextPart {
            text: "x".into(),
            ..TextPart::default()
        };
        let scan = RegionScan::of(&part, &art, w, h).unwrap();
        let g = gradient(&scan, &art, w, h).unwrap();
        assert_eq!(g.first(), Some(&[4, 4]));
        assert!(g.contains(&[5, 6]));
        assert_eq!(g.last(), Some(&[7, 7]));
        // Spread over taller English ink, the bands keep their order and the dither its
        // original-pixel columns.
        let (cw, ch) = (4usize, 18usize);
        let coverage = vec![255u8; cw * ch];
        let map = spread(&g, &coverage, cw, ch, 2, 0);
        assert_eq!(map[0], 4);
        assert_eq!(&map[9 * cw..10 * cw], &[5, 5, 6, 6]);
        assert_eq!(map[(ch - 1) * cw], 7);
        // A flat block is not.
        let flat: Vec<u8> = art.iter().map(|&v| if v == 0 { 0 } else { 4 }).collect();
        let scan = RegionScan::of(&part, &flat, w, h).unwrap();
        assert_eq!(gradient(&scan, &flat, w, h), None);
    }
}

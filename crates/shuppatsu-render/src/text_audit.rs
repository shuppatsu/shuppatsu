//! Checks a translated part's region against the art it replaces (docs/localisation.md,
//! "Checking the regions").
//!
//! A part's region must cover the original lettering and nothing else: lettering left outside
//! it stays visible next to the English text, a region reaching onto the panel's border paints
//! the border over with the backdrop, and border pixels inside the region count as lettering
//! when the text is sized ([`RegionScan`]), so the English comes out at the size of the panel
//! instead of the size of the original lettering. This module finds those three defects with
//! the renderer's own notion of backdrop and lettering.
//!
//! Terms, all in the art's palette indices (0 = transparent):
//! - *lettering*: what [`RegionScan::lettering`] calls lettering, over the whole sprite (the
//!   backdrop pattern continues outside the region);
//! - *strokes*: 8-connected components of lettering pixels;
//! - *frame*: a stroke of a box part that touches the void (transparency that is not
//!   backdrop) or the sprite's edge — a border, a bevel, a dither edge — or a thin rule as
//!   long as the panel;
//! - *face*: the backdrop pixels connected to the region's own backdrop (the panel the text
//!   sits on). A transparent backdrop pixel (the clear half of a dither) belongs to it only
//!   next to an opaque one, so the face never leaks into the void around a caption band.

use crate::text::{RegionScan, TextPart, enclosed_clear};

/// Lettering wholly outside the region counts as part of its line ([`Finding::Nearby`]) when
/// it is level with the region and at most this many pixels from it (a character's spacing).
pub const NEAR: usize = 4;

/// A straight run of lettering pixels at least this long and 95 % of the panel's extent
/// is a rule or a bevel.
const RUN_MIN: usize = 24;

/// A thin rule: at most this thick ...
const RULE_THICK: usize = 2;
/// ... and at least this many times as long.
const RULE_RATIO: usize = 8;

/// An axis-aligned box `[x0, x1, y0, y1]` (exclusive ends) in sprite pixels.
pub type Bounds = [usize; 4];

/// A pixel `(x, y)` of the sprite.
pub type Px = (u16, u16);

/// One defect of a part's region.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Finding {
    /// Lettering strokes cut by the region's edge: these pixels outside it stay visible.
    Leftover { pixels: Vec<Px>, bounds: Bounds },
    /// Other art (a sign's post, a gauge) mostly outside the region reaching into it: these
    /// pixels inside are erased with the lettering, and are not counted as lettering here.
    Intrudes { pixels: Vec<Px>, bounds: Bounds },
    /// Lettering wholly outside the region, level with it and within [`NEAR`] pixels (and, in a
    /// box, on its panel): probably the next character of the same line.
    Nearby { pixels: Vec<Px>, bounds: Bounds },
    /// Frame pixels (border, bevel, dither edge) inside the region: painted over with the
    /// backdrop. `sides` lists where they are: `l`, `r`, `t`, `b`, or `i` for inside.
    Border {
        pixels: Vec<Px>,
        bounds: Bounds,
        sides: String,
    },
    /// Region pixels off the panel (transparency that is not backdrop) in a box part; they
    /// stay transparent but widen the box the text is centred in.
    OffPanel { pixels: Vec<Px>, bounds: Bounds },
    /// A box part's region reaches past the art's opaque extent (`[left, top, right, bottom]`
    /// overhang).
    BeyondArt { overhang: [usize; 4] },
    /// The backdrop pattern (taken from the region's edge) is not the colour that fills the
    /// region (the edge lies on a frame, so the whole face counts as lettering and is
    /// repainted in the frame's colour), or it is striped (lettering on one edge outvoted the
    /// face in two of the parity classes, so every other row or column of the box is wrong).
    Backdrop { face: u8, pattern: [u8; 4] },
    /// The renderer's ink box (what the English is sized from) differs from the lettering's
    /// own: frame pixels or foreign art inside the region count as lettering. `None` = no
    /// lettering at all.
    Sizing {
        renderer: Option<Bounds>,
        lettering: Option<Bounds>,
    },
}

impl Finding {
    /// A short name of the defect class.
    pub fn class(&self) -> &'static str {
        match self {
            Finding::Leftover { .. } => "leftover",
            Finding::Nearby { .. } => "nearby",
            Finding::Intrudes { .. } => "intrudes",
            Finding::Border { .. } => "border",
            Finding::OffPanel { .. } => "off-panel",
            Finding::BeyondArt { .. } => "beyond-art",
            Finding::Backdrop { .. } => "backdrop",
            Finding::Sizing { .. } => "sizing",
        }
    }

    /// The pixels the finding is about, for drawing (empty for the box-only findings).
    pub fn pixels(&self) -> &[Px] {
        match self {
            Finding::Leftover { pixels, .. }
            | Finding::Nearby { pixels, .. }
            | Finding::Intrudes { pixels, .. }
            | Finding::Border { pixels, .. }
            | Finding::OffPanel { pixels, .. } => pixels,
            _ => &[],
        }
    }
}

/// The audit of one part.
#[derive(Clone, Debug)]
pub struct PartAudit {
    /// The renderer's view of the region.
    pub scan: RegionScan,
    pub findings: Vec<Finding>,
}

/// Audits `part` against the `art_w × art_h` sprite `art` (palette indices); `others` are the
/// regions of the sprite's other parts. `None` when the region is empty after clipping (the
/// renderer skips the part).
pub fn audit_part(
    part: &TextPart,
    others: &[[u16; 4]],
    art: &[u8],
    art_w: usize,
    art_h: usize,
) -> Option<PartAudit> {
    let scan = RegionScan::of(part, art, art_w, art_h)?;
    let [rx, ry, rw, rh] = scan.rect;
    let inside = |x: usize, y: usize| (rx..rx + rw).contains(&x) && (ry..ry + rh).contains(&y);
    // Pixels another part of the same sprite replaces: lettering there is that part's.
    let covered = |x: usize, y: usize| {
        others.iter().any(|o| {
            let [ox, oy, ow, oh] = o.map(usize::from);
            (ox..ox + ow).contains(&x) && (oy..oy + oh).contains(&y)
        })
    };
    let at = |x: usize, y: usize| art[y * art_w + x];
    let n = art_w * art_h;
    let lettering: Vec<bool> = (0..n)
        .map(|i| scan.lettering_at(i % art_w, i / art_w, art[i]))
        .collect();
    let backdrop: Vec<bool> = (0..n)
        .map(|i| scan.boxed && scan.backdrop_at(i % art_w, i / art_w, art[i]))
        .collect();
    let neighbours4 = |x: usize, y: usize| {
        let mut v = Vec::with_capacity(4);
        if x > 0 {
            v.push((x - 1, y));
        }
        if y > 0 {
            v.push((x, y - 1));
        }
        if x + 1 < art_w {
            v.push((x + 1, y));
        }
        if y + 1 < art_h {
            v.push((x, y + 1));
        }
        v
    };
    // The face: opaque backdrop pixels, and transparent ones next to an opaque one.
    let face_px: Vec<bool> = (0..n)
        .map(|i| {
            let (x, y) = (i % art_w, i / art_w);
            backdrop[i]
                && (art[i] != 0
                    || neighbours8(x, y, art_w, art_h)
                        .any(|(a, b)| backdrop[b * art_w + a] && at(a, b) != 0))
        })
        .collect();
    // The void: transparency that is neither face nor enclosed by the art (the clear half of
    // dithered lettering, which the renderer fills with the backdrop).
    let holes = enclosed_clear(art, art_w, art_h);
    let void =
        |x: usize, y: usize| at(x, y) == 0 && !face_px[y * art_w + x] && !holes[y * art_w + x];
    let faces = components(&face_px, art_w, art_h);
    // The faces the region's own backdrop belongs to.
    let mut own_face = vec![false; faces.count];
    for y in ry..ry + rh {
        for x in rx..rx + rw {
            if let Some(c) = faces.label[y * art_w + x] {
                own_face[c] = true;
            }
        }
    }
    let on_own_face = |x: usize, y: usize| faces.label[y * art_w + x].is_some_and(|c| own_face[c]);
    let face_bounds = bounds_of(
        (0..n)
            .filter(|&i| on_own_face(i % art_w, i / art_w))
            .map(|i| (i % art_w, i / art_w)),
    );

    // Frame, found separately inside and outside the region, so lettering that touches a
    // frame line is not taken for frame: outside, a run of lettering pixels touching the void,
    // a rule as long as the panel or a frame round the region; inside, one touching the void,
    // a rule, or a thin piece of an outside frame (the part of a border the region reaches).
    let touches_void = |c: &[(usize, usize)]| {
        c.iter()
            .any(|&(x, y)| neighbours4(x, y).iter().any(|&(a, b)| void(a, b)))
    };
    let rule_long = |b: Bounds| {
        let (bw, bh) = (b[1] - b[0], b[3] - b[2]);
        let rule = bw.min(bh) <= RULE_THICK && bw.max(bh) >= RULE_RATIO * bw.min(bh).max(1);
        // A rule counts as frame when it runs (nearly) the panel's length.
        rule && face_bounds.is_some_and(|f| {
            (bw >= bh && bw * 4 >= (f[1] - f[0]) * 3) || (bh > bw && bh * 4 >= (f[3] - f[2]) * 3)
        })
    };
    let mut frame = vec![false; n];
    if scan.boxed {
        let out_mask: Vec<bool> = (0..n)
            .map(|i| lettering[i] && !inside(i % art_w, i / art_w))
            .collect();
        for c in components(&out_mask, art_w, art_h).list {
            let b = bounds_of(c.iter().copied()).expect("a component has pixels");
            let surrounds = b[0] < rx && b[2] < ry && b[1] > rx + rw && b[3] > ry + rh;
            if surrounds || rule_long(b) || touches_void(&c) {
                for (x, y) in c {
                    frame[y * art_w + x] = true;
                }
            }
        }
        let in_mask: Vec<bool> = (0..n)
            .map(|i| lettering[i] && inside(i % art_w, i / art_w))
            .collect();
        let mut in_frame = Vec::new();
        for c in components(&in_mask, art_w, art_h).list {
            let b = bounds_of(c.iter().copied()).expect("a component has pixels");
            let thin = (b[1] - b[0]).min(b[3] - b[2]) <= RULE_THICK;
            let joins_frame = || {
                c.iter().any(|&(x, y)| {
                    neighbours8(x, y, art_w, art_h)
                        .any(|(a, d)| frame[d * art_w + a] && !inside(a, d))
                })
            };
            if touches_void(&c) || rule_long(b) || (thin && joins_frame()) {
                in_frame.extend(c);
            }
        }
        for (x, y) in in_frame {
            frame[y * art_w + x] = true;
        }
        // Rules and bevels: lettering pixels on a straight run nearly as long as the panel
        // that stops short of the sprite's edge (a glyph's strokes stop short of the panel's
        // length, except in a label that is all lettering, which reaches the sprite's edge).
        let long_run = |len: usize, extent: usize| len >= RUN_MIN && len * 20 >= extent * 19;
        // Runs per line: `line(i)` is the pixel index of position `i` along it.
        let mut runs = |len: usize, extent: usize, line: &dyn Fn(usize) -> usize| {
            let mut i = 0;
            while i < len {
                let start = i;
                while i < len && lettering[line(i)] {
                    i += 1;
                }
                if start > 0 && i < len && long_run(i - start, extent) {
                    for j in start..i {
                        frame[line(j)] = true;
                    }
                }
                i += 1;
            }
        };
        if let Some(f) = face_bounds {
            let (fw, fh) = (f[1] - f[0], f[3] - f[2]);
            for y in 0..art_h {
                runs(art_w, fw, &|x| y * art_w + x);
            }
            for x in 0..art_w {
                runs(art_h, fh, &|y| y * art_w + x);
            }
        }
        // Frame lines crossing the region: a straight run of lettering entering the region
        // from outside frame and leaving it into outside frame on the other side.
        let mut crossing = Vec::new();
        for y in ry..ry + rh {
            if rx > 0 && rx + rw < art_w {
                let (a, b) = (y * art_w + rx - 1, y * art_w + rx + rw);
                if frame[a] && frame[b] && (rx..rx + rw).all(|x| lettering[y * art_w + x]) {
                    crossing.extend((rx..rx + rw).map(|x| y * art_w + x));
                }
            }
        }
        for x in rx..rx + rw {
            if ry > 0 && ry + rh < art_h {
                let (a, b) = ((ry - 1) * art_w + x, (ry + rh) * art_w + x);
                if frame[a] && frame[b] && (ry..ry + rh).all(|y| lettering[y * art_w + x]) {
                    crossing.extend((ry..ry + rh).map(|y| y * art_w + x));
                }
            }
        }
        for i in crossing {
            frame[i] = true;
        }
    }
    let strokes = components(&lettering, art_w, art_h);

    let mut findings = Vec::new();
    let mut leftover = Vec::new();
    let mut nearby = Vec::new();
    let mut border = Vec::new();
    let mut intrudes = Vec::new();
    // Lettering inside the region that is not frame: what the size should come from.
    let mut true_ink: Vec<(usize, usize)> = Vec::new();
    for s in &strokes.list {
        let is_frame = |&(x, y): &(usize, usize)| frame[y * art_w + x];
        border.extend(
            s.iter()
                .copied()
                .filter(|p| inside(p.0, p.1) && is_frame(p)),
        );
        let ink: Vec<(usize, usize)> = s
            .iter()
            .copied()
            .filter(|p| inside(p.0, p.1) && !is_frame(p))
            .collect();
        let rest: Vec<(usize, usize)> = s
            .iter()
            .copied()
            .filter(|p| !inside(p.0, p.1) && !is_frame(p) && !covered(p.0, p.1))
            .collect();
        let any_inside = s.iter().any(|p| inside(p.0, p.1));
        if !ink.is_empty() && !rest.is_empty() {
            if rest.len() > ink.len() {
                // Mostly outside: other art (a sign's post, a gauge) reaching into the region.
                intrudes.extend(ink);
            } else {
                true_ink.extend(ink);
                leftover.extend(rest);
            }
        } else if !ink.is_empty() {
            true_ink.extend(ink);
        } else if !any_inside && !rest.is_empty() {
            let b = bounds_of(rest.iter().copied()).expect("not empty");
            let (bw, bh) = (b[1] - b[0], b[3] - b[2]);
            // Level with the region and close to it: the next character of the same line.
            let gap_x = gap(b[0], b[1], rx, rx + rw);
            let gap_y = gap(b[2], b[3], ry, ry + rh);
            let level = if part.vertical {
                overlap(b[0], b[1], rx, rx + rw) * 2 >= bw
            } else {
                overlap(b[2], b[3], ry, ry + rh) * 2 >= bh
            };
            let close = level && gap_x.max(gap_y) <= NEAR;
            let belongs = close
                && (!scan.boxed || {
                    // In a box, also on the region's panel: mostly surrounded by its face.
                    let mut ring = std::collections::HashSet::new();
                    for &(x, y) in &rest {
                        for (a, c) in neighbours8(x, y, art_w, art_h) {
                            if !lettering[c * art_w + a] {
                                ring.insert((a, c));
                            }
                        }
                    }
                    let face_n = ring.iter().filter(|&&(a, c)| on_own_face(a, c)).count();
                    face_n * 5 >= ring.len() * 3
                });
            if belongs {
                nearby.extend(rest);
            }
        }
    }
    let px = |v: Vec<(usize, usize)>| -> Vec<Px> {
        v.into_iter().map(|(x, y)| (x as u16, y as u16)).collect()
    };
    if let Some(bounds) = bounds_of(intrudes.iter().copied()) {
        findings.push(Finding::Intrudes {
            pixels: px(intrudes),
            bounds,
        });
    }
    if let Some(bounds) = bounds_of(leftover.iter().copied()) {
        findings.push(Finding::Leftover {
            pixels: px(leftover),
            bounds,
        });
    }
    if let Some(bounds) = bounds_of(nearby.iter().copied()) {
        findings.push(Finding::Nearby {
            pixels: px(nearby),
            bounds,
        });
    }
    if let Some(bounds) = bounds_of(border.iter().copied()) {
        let mut sides = String::new();
        // A side is on the frame when a quarter of its pixels are (a dither edge has half).
        let on = |f: &dyn Fn(usize, usize) -> bool, len: usize| {
            border.iter().filter(|&&(x, y)| f(x, y)).count() * 4 >= len
        };
        for (c, hit) in [
            ('l', on(&|x, _| x == rx, rh)),
            ('r', on(&|x, _| x + 1 == rx + rw, rh)),
            ('t', on(&|_, y| y == ry, rw)),
            ('b', on(&|_, y| y + 1 == ry + rh, rw)),
        ] {
            if hit {
                sides.push(c);
            }
        }
        let edge = |x: usize, y: usize| x == rx || y == ry || x + 1 == rx + rw || y + 1 == ry + rh;
        if sides.is_empty() {
            // `i`: inside the region; `e`: a few pixels on its edge (a rounded corner).
            let interior = border.iter().any(|&(x, y)| !edge(x, y));
            sides.push(if interior { 'i' } else { 'e' });
        }
        findings.push(Finding::Border {
            pixels: px(border),
            bounds,
            sides,
        });
    }
    if scan.boxed {
        let off: Vec<(usize, usize)> = (ry..ry + rh)
            .flat_map(|y| (rx..rx + rw).map(move |x| (x, y)))
            .filter(|&(x, y)| void(x, y))
            .collect();
        // Rounded corners leave a few clear pixels in any box; a side more than a third
        // clear is off the panel (a dither edge, the art's end).
        let side = |f: &dyn Fn(usize, usize) -> bool, len: usize| {
            off.iter().filter(|&&(x, y)| f(x, y)).count() * 3 > len
        };
        let off_side = side(&|x, _| x == rx, rh)
            || side(&|x, _| x + 1 == rx + rw, rh)
            || side(&|_, y| y == ry, rw)
            || side(&|_, y| y + 1 == ry + rh, rw);
        if let Some(bounds) = bounds_of(off.iter().copied()).filter(|_| off_side) {
            findings.push(Finding::OffPanel {
                pixels: px(off),
                bounds,
            });
        }
    }
    let opaque = bounds_of(
        (0..n)
            .filter(|&i| art[i] != 0)
            .map(|i| (i % art_w, i / art_w)),
    );
    if let Some([ox0, ox1, oy0, oy1]) = opaque.filter(|_| scan.boxed) {
        let overhang = [
            ox0.saturating_sub(rx),
            oy0.saturating_sub(ry),
            (rx + rw).saturating_sub(ox1),
            (ry + rh).saturating_sub(oy1),
        ];
        if overhang.iter().any(|&o| o > 0) {
            findings.push(Finding::BeyondArt { overhang });
        }
    }
    if scan.boxed && rw > 2 && rh > 2 {
        // The commonest opaque colour of the region's interior, when it fills most of it.
        let mut counts = [0usize; 256];
        for y in ry + 1..ry + rh - 1 {
            for x in rx + 1..rx + rw - 1 {
                counts[usize::from(at(x, y))] += 1;
            }
        }
        let face = (1..=255u8)
            .max_by_key(|&i| counts[usize::from(i)])
            .unwrap_or(0);
        let interior = (rw - 2) * (rh - 2);
        // A backdrop in stripes (two parity classes one value, two another, along rows or
        // columns) is lettering or a frame line on one edge of the region outvoting the face:
        // the box comes out striped. The art's dithers are checkerboards.
        let p = scan.pattern;
        let striped = (p[0] == p[1] && p[2] == p[3] && p[0] != p[2])
            || (p[0] == p[2] && p[1] == p[3] && p[0] != p[1]);
        let misread = face != 0 && counts[usize::from(face)] * 2 > interior && !p.contains(&face);
        // A box over nothing: plain lettering whose strokes fill the region's edge (`auto` then
        // takes it for a box and paints its pixels clear).
        let clear = p == [0; 4];
        if misread || striped || clear {
            findings.push(Finding::Backdrop {
                face,
                pattern: scan.pattern,
            });
        }
    }
    let renderer = scan
        .ink
        .map(|[x0, x1, y0, y1]| [x0 + rx, x1 + rx, y0 + ry, y1 + ry]);
    let lettering_ink = bounds_of(true_ink.iter().copied());
    if renderer != lettering_ink {
        findings.push(Finding::Sizing {
            renderer,
            lettering: lettering_ink,
        });
    }
    Some(PartAudit { scan, findings })
}

/// 8-connected components of a mask.
struct Components {
    label: Vec<Option<usize>>,
    list: Vec<Vec<(usize, usize)>>,
    count: usize,
}

fn components(mask: &[bool], w: usize, h: usize) -> Components {
    let mut label = vec![None; w * h];
    let mut list = Vec::new();
    for start in 0..w * h {
        if !mask[start] || label[start].is_some() {
            continue;
        }
        let id = list.len();
        let mut px = Vec::new();
        let mut stack = vec![(start % w, start / w)];
        label[start] = Some(id);
        while let Some((x, y)) = stack.pop() {
            px.push((x, y));
            for (a, b) in neighbours8(x, y, w, h) {
                let i = b * w + a;
                if mask[i] && label[i].is_none() {
                    label[i] = Some(id);
                    stack.push((a, b));
                }
            }
        }
        list.push(px);
    }
    let count = list.len();
    Components { label, list, count }
}

fn neighbours8(x: usize, y: usize, w: usize, h: usize) -> impl Iterator<Item = (usize, usize)> {
    (-1isize..=1)
        .flat_map(|dy| (-1isize..=1).map(move |dx| (dx, dy)))
        .filter(|&d| d != (0, 0))
        .filter_map(move |(dx, dy)| {
            let (a, b) = (x as isize + dx, y as isize + dy);
            ((0..w as isize).contains(&a) && (0..h as isize).contains(&b))
                .then_some((a as usize, b as usize))
        })
}

/// The bounding box of some pixels; `None` when there are none.
pub fn bounds_of(px: impl IntoIterator<Item = (usize, usize)>) -> Option<Bounds> {
    px.into_iter().fold(None, |b, (x, y)| {
        Some(match b {
            None => [x, x + 1, y, y + 1],
            Some([x0, x1, y0, y1]) => [x0.min(x), x1.max(x + 1), y0.min(y), y1.max(y + 1)],
        })
    })
}

/// The empty space between `[a0, a1)` and `[b0, b1)` (0 when they touch or overlap).
fn gap(a0: usize, a1: usize, b0: usize, b1: usize) -> usize {
    b0.saturating_sub(a1).max(a0.saturating_sub(b1))
}

/// The length shared by `[a0, a1)` and `[b0, b1)`.
fn overlap(a0: usize, a1: usize, b0: usize, b1: usize) -> usize {
    a1.min(b1).saturating_sub(a0.max(b0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 20×12 plate: dotted border (2×2 dither) round a face of 5, lettering 7 at x 4–15,
    /// y 3–8.
    fn plate() -> (Vec<u8>, usize, usize) {
        let (w, h) = (20, 12);
        let mut art = vec![5u8; w * h];
        for y in 0..h {
            for x in 0..w {
                let edge = x == 0 || y == 0 || x == w - 1 || y == h - 1;
                if edge {
                    art[y * w + x] = if (x + y) % 2 == 0 { 9 } else { 0 };
                } else if (4..16).contains(&x) && (3..9).contains(&y) && x % 3 != 0 {
                    art[y * w + x] = 7;
                }
            }
        }
        (art, w, h)
    }

    fn part(region: [u16; 4]) -> TextPart {
        TextPart {
            region: Some(region),
            text: "X".into(),
            ..TextPart::default()
        }
    }

    fn classes(a: &PartAudit) -> Vec<&'static str> {
        a.findings.iter().map(Finding::class).collect()
    }

    #[test]
    fn a_region_on_the_face_over_all_the_lettering_is_clean() {
        let (art, w, h) = plate();
        let a = audit_part(&part([1, 1, 18, 10]), &[], &art, w, h).unwrap();
        assert!(a.scan.boxed);
        assert_eq!(classes(&a), Vec::<&str>::new());
    }

    #[test]
    fn a_region_starting_a_row_low_leaves_the_top_row() {
        let (art, w, h) = plate();
        let a = audit_part(&part([1, 4, 18, 7]), &[], &art, w, h).unwrap();
        let Finding::Leftover { bounds, .. } = &a.findings[0] else {
            panic!("{:?}", a.findings);
        };
        assert_eq!(*bounds, [4, 15, 3, 4]);
    }

    #[test]
    fn a_region_on_the_border_paints_it_and_is_sized_from_it() {
        let (art, w, h) = plate();
        // One pixel onto the dotted border left and right (the distance plates before 43ec636).
        let a = audit_part(&part([0, 1, 20, 10]), &[], &art, w, h).unwrap();
        assert_eq!(classes(&a), ["border", "off-panel", "sizing"].to_vec());
        let Finding::Border { sides, .. } = &a.findings[0] else {
            unreachable!()
        };
        assert_eq!(sides, "lr");
        let Some(Finding::Sizing { lettering, .. }) =
            a.findings.iter().find(|f| f.class() == "sizing")
        else {
            unreachable!()
        };
        assert_eq!(*lettering, Some([4, 15, 3, 9]));
    }

    #[test]
    fn a_region_whose_edge_is_all_border_takes_the_border_for_backdrop() {
        let (art, w, h) = plate();
        let a = audit_part(&part([0, 0, 20, 12]), &[], &art, w, h).unwrap();
        assert_eq!(a.scan.pattern, [9, 0, 0, 9]);
        assert!(classes(&a).contains(&"backdrop"));
    }

    #[test]
    fn plain_lettering_cut_or_just_beside_the_region() {
        // Two strokes on transparency: x 2–5 and x 8–11, y 2–6.
        let (w, h) = (16, 10);
        let mut art = vec![0u8; w * h];
        for y in 2..7 {
            for x in (2..6).chain(8..12) {
                art[y * w + x] = 3;
            }
        }
        let mut p = part([0, 0, 7, 10]);
        p.style = crate::text::Style::Plain;
        let a = audit_part(&p, &[], &art, w, h).unwrap();
        assert_eq!(classes(&a), vec!["nearby"]);
        let a = audit_part(&part([0, 0, 10, 10]), &[], &art, w, h).unwrap();
        assert!(!a.scan.boxed);
        assert_eq!(classes(&a)[0], "leftover");
    }
}

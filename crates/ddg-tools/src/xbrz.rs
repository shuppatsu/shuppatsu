//! An xBRZ-style pixel-art scaler (any whole factor), written for `upscale-textures`.
//!
//! The method follows xBRZ (Zenju): each 2×2 block of source pixels is classified from its
//! 4×4 neighbourhood (which diagonal is the edge, and whether it clearly dominates), then
//! every source pixel is enlarged to an n×n block and each of its four corners is blended
//! towards the neighbouring colour along the detected edge: a rounded corner, a 45° line, a
//! shallow or steep 2:1 line, or both. Instead of hand-tuned per-scale tables, the blend
//! weights here are the exact areas of each output sub-pixel on the far side of that edge
//! (computed once per scale by supersampling), which gives the same weights as xBRZ's tables
//! at 2× and 4× and extends to any factor.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Colours closer than this (YCbCr distance, 0..255 channels) count as equal.
const EQUAL_TOLERANCE: f32 = 30.0;
/// One diagonal "dominates" when it is this much more uniform than the other.
const DOMINANT_RATIO: f32 = 3.6;
/// A 2:1 line is taken when one gradient is this much stronger than the other.
const STEEP_RATIO: f32 = 2.2;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Blend {
    #[default]
    None,
    Normal,
    Dominant,
}

/// The four corners of a pixel, by direction: (−1,−1) top-left … (1,1) bottom-right.
fn corner_index(sx: i32, sy: i32) -> usize {
    usize::from(sy > 0) * 2 + usize::from(sx > 0)
}

fn dist(a: [u8; 3], b: [u8; 3]) -> f32 {
    let r = f32::from(a[0]) - f32::from(b[0]);
    let g = f32::from(a[1]) - f32::from(b[1]);
    let bl = f32::from(a[2]) - f32::from(b[2]);
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * bl;
    let cb = 0.5 / (1.0 - 0.0722) * (bl - y);
    let cr = 0.5 / (1.0 - 0.2126) * (r - y);
    (y * y + cb * cb + cr * cr).sqrt()
}

fn eq(a: [u8; 3], b: [u8; 3]) -> bool {
    dist(a, b) < EQUAL_TOLERANCE
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Corner,
    Diagonal,
    Shallow,
    Steep,
    SteepAndShallow,
}

const SHAPES: [Shape; 5] = [
    Shape::Corner,
    Shape::Diagonal,
    Shape::Shallow,
    Shape::Steep,
    Shape::SteepAndShallow,
];

/// Whether the point (u, v) of a pixel (0..1, y down) is on the far side of the edge for a
/// blend at the bottom-right corner.
fn beyond(shape: Shape, u: f32, v: f32) -> bool {
    let shallow = v > 1.0 - u / 2.0;
    let steep = u > 1.0 - v / 2.0;
    match shape {
        // A rounded corner: outside the pixel's inscribed circle, in the corner's quadrant.
        Shape::Corner => u > 0.5 && v > 0.5 && (u - 0.5).powi(2) + (v - 0.5).powi(2) > 0.25,
        Shape::Diagonal => u + v > 1.5,
        Shape::Shallow => shallow,
        Shape::Steep => steep,
        Shape::SteepAndShallow => shallow || steep,
    }
}

/// Blend weights (0..1) of the n×n sub-pixels for each shape, bottom-right corner, row-major.
fn coverage_tables(n: usize) -> Vec<Vec<f32>> {
    // 64² samples per sub-pixel, nudged off the exact line positions.
    const SS: usize = 64;
    const NUDGE: f32 = 0.5 + 1.0 / 1024.0;
    SHAPES
        .iter()
        .map(|&shape| {
            let mut t = vec![0.0; n * n];
            for (i, w) in t.iter_mut().enumerate() {
                let (sx, sy) = (i % n, i / n);
                let mut hits = 0;
                for k in 0..SS * SS {
                    let u = (sx as f32 + ((k % SS) as f32 + NUDGE) / SS as f32) / n as f32;
                    let v = (sy as f32 + ((k / SS) as f32 + NUDGE) / SS as f32) / n as f32;
                    hits += usize::from(beyond(shape, u, v));
                }
                *w = hits as f32 / (SS * SS) as f32;
            }
            t
        })
        .collect()
}

/// Blend weights per shape, shared between threads.
type Tables = Arc<Vec<Vec<f32>>>;

/// [`coverage_tables`] computed once per factor.
fn cached_tables(n: usize) -> Tables {
    static CACHE: OnceLock<Mutex<HashMap<usize, Tables>>> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache
        .entry(n)
        .or_insert_with(|| Arc::new(coverage_tables(n)))
        .clone()
}

/// (dx, dy) rotated by `r` quarter turns: (1,1) → (−1,1) → (−1,−1) → (1,−1).
fn rot(r: usize, dx: i32, dy: i32) -> (i32, i32) {
    (0..r).fold((dx, dy), |(x, y), _| (-y, x))
}

/// Scales `src` (`w`×`h` RGB, row-major) by `n`. Edges are clamped.
pub fn scale(src: &[[u8; 3]], w: usize, h: usize, n: usize) -> Vec<[u8; 3]> {
    assert_eq!(src.len(), w * h);
    if n <= 1 || w == 0 || h == 0 {
        return src.to_vec();
    }
    scale_by(
        src,
        src,
        (w, h, n),
        |c| c.map(f32::from),
        |p, c, a| {
            for ch in 0..3 {
                p[ch] += (f32::from(c[ch]) - p[ch]) * a;
            }
        },
    )
    .into_iter()
    .map(|v| v.map(|c| c.round().clamp(0.0, 255.0) as u8))
    .collect()
}

/// The same edge shapes without new colours, for palettised art: the decisions are made on
/// `colours` (`w`×`h` RGB), and every output pixel takes the `payload` (e.g. the palette
/// index) of its own source pixel, or of the neighbour along the edge where more than half of
/// it lies beyond the edge. Palette effects keep working on the result.
pub fn scale_indexed<T: Copy>(
    colours: &[[u8; 3]],
    payload: &[T],
    w: usize,
    h: usize,
    n: usize,
) -> Vec<T> {
    assert_eq!(colours.len(), w * h);
    assert_eq!(payload.len(), w * h);
    if n <= 1 || w == 0 || h == 0 {
        return payload.to_vec();
    }
    scale_by(
        colours,
        payload,
        (w, h, n),
        |t| t,
        |p, t, a| {
            if a > 0.5 {
                *p = t;
            }
        },
    )
}

/// The scaler: corners classified on `src`; each pixel becomes an `n`×`n` block started from
/// its payload (`start`) and blended towards a neighbour's payload with weight `a` (`mix`).
fn scale_by<T: Copy, P: Copy>(
    src: &[[u8; 3]],
    payload: &[T],
    (w, h, n): (usize, usize, usize),
    start: impl Fn(T) -> P,
    mix: impl Fn(&mut P, T, f32),
) -> Vec<P> {
    let pay = |x: i32, y: i32| -> T {
        let x = x.clamp(0, w as i32 - 1) as usize;
        let y = y.clamp(0, h as i32 - 1) as usize;
        payload[y * w + x]
    };
    let px = |x: i32, y: i32| -> [u8; 3] {
        let x = x.clamp(0, w as i32 - 1) as usize;
        let y = y.clamp(0, h as i32 - 1) as usize;
        src[y * w + x]
    };

    // 1. Corner classification of every 2×2 block (F G / J K, F at (x, y)).
    let mut blend = vec![[Blend::None; 4]; w * h];
    let mut set = |x: i32, y: i32, c: usize, b: Blend| {
        if (0..w as i32).contains(&x) && (0..h as i32).contains(&y) {
            blend[y as usize * w + x as usize][c] = b;
        }
    };
    for y in -1..h as i32 {
        for x in -1..w as i32 {
            let k = |dx: i32, dy: i32| px(x + dx, y + dy);
            let (b, c) = (k(0, -1), k(1, -1));
            let (e, f, g, hh) = (k(-1, 0), k(0, 0), k(1, 0), k(2, 0));
            let (i, j, kk, l) = (k(-1, 1), k(0, 1), k(1, 1), k(2, 1));
            let (nn, o) = (k(0, 2), k(1, 2));
            if (f == g && j == kk) || (f == j && g == kk) {
                continue;
            }
            let jg = dist(i, f) + dist(f, c) + dist(nn, kk) + dist(kk, hh) + 4.0 * dist(j, g);
            let fk = dist(e, j) + dist(j, o) + dist(b, g) + dist(g, l) + 4.0 * dist(f, kk);
            if jg < fk {
                let t = if DOMINANT_RATIO * jg < fk {
                    Blend::Dominant
                } else {
                    Blend::Normal
                };
                if f != g && f != j {
                    set(x, y, corner_index(1, 1), t);
                }
                if kk != j && kk != g {
                    set(x + 1, y + 1, corner_index(-1, -1), t);
                }
            } else if fk < jg {
                let t = if DOMINANT_RATIO * fk < jg {
                    Blend::Dominant
                } else {
                    Blend::Normal
                };
                if j != f && j != kk {
                    set(x, y + 1, corner_index(1, -1), t);
                }
                if g != f && g != kk {
                    set(x + 1, y, corner_index(-1, 1), t);
                }
            }
        }
    }

    // 2. Each pixel to an n×n block, blending its corners.
    let tables = cached_tables(n);
    let ow = w * n;
    let first = start(payload[0]);
    let mut out = vec![first; ow * h * n];
    let mut block = vec![first; n * n];
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            block.fill(start(pay(x, y)));
            let corners = blend[y as usize * w + x as usize];
            for r in 0..4 {
                let at = |dx: i32, dy: i32| {
                    let (rx, ry) = rot(r, dx, dy);
                    px(x + rx, y + ry)
                };
                let corner = |sx: i32, sy: i32| {
                    let (rx, ry) = rot(r, sx, sy);
                    corners[corner_index(rx, ry)]
                };
                let main = corner(1, 1);
                if main == Blend::None {
                    continue;
                }
                let (e, b, c, d) = (at(0, 0), at(0, -1), at(1, -1), at(-1, 0));
                let (f, g, hh, i) = (at(1, 0), at(-1, 1), at(0, 1), at(1, 1));
                let line = main == Blend::Dominant
                    || !((corner(1, -1) != Blend::None && !eq(e, g))
                        || (corner(-1, 1) != Blend::None && !eq(e, c))
                        || (!eq(e, i) && eq(g, hh) && eq(hh, i) && eq(i, f) && eq(f, c)));
                let col = if dist(e, f) <= dist(e, hh) {
                    let (rx, ry) = rot(r, 1, 0);
                    pay(x + rx, y + ry)
                } else {
                    let (rx, ry) = rot(r, 0, 1);
                    pay(x + rx, y + ry)
                };
                let shape = if line {
                    let (fg, hc) = (dist(f, g), dist(hh, c));
                    let shallow = STEEP_RATIO * fg <= hc && e != g && d != g;
                    let steep = STEEP_RATIO * hc <= fg && e != c && b != c;
                    match (shallow, steep) {
                        (true, true) => Shape::SteepAndShallow,
                        (true, false) => Shape::Shallow,
                        (false, true) => Shape::Steep,
                        (false, false) => Shape::Diagonal,
                    }
                } else {
                    Shape::Corner
                };
                let table = &tables[SHAPES.iter().position(|s| *s == shape).unwrap_or(0)];
                // Sub-pixel (sx, sy) of the base frame sits at the rotated place in the block.
                for (k, &a) in table.iter().enumerate() {
                    if a <= 0.0 {
                        continue;
                    }
                    let (sx, sy) = ((k % n) as i32, (k / n) as i32);
                    let m = n as i32 - 1;
                    let (rx, ry) = rot(r, 2 * sx - m, 2 * sy - m);
                    let (bx, by) = (((rx + m) / 2) as usize, ((ry + m) / 2) as usize);
                    mix(&mut block[by * n + bx], col, a);
                }
            }
            let (x0, y0) = (x as usize * n, y as usize * n);
            for (k, v) in block.iter().enumerate() {
                out[(y0 + k / n) * ow + x0 + k % n] = *v;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_match_xbrz_at_2x() {
        let t = coverage_tables(2);
        let close = |a: f32, b: f32| (a - b).abs() < 0.02;
        // [corner, diagonal, shallow, steep, both]; sub-pixels TL, TR, BL, BR.
        assert!(close(t[0][3], 0.2146) && t[0][..3].iter().all(|&a| a == 0.0));
        assert!(close(t[1][3], 0.5) && t[1][..3].iter().all(|&a| a == 0.0));
        assert!(close(t[2][2], 0.25) && close(t[2][3], 0.75) && t[2][0] == 0.0);
        assert!(close(t[3][1], 0.25) && close(t[3][3], 0.75) && t[3][2] == 0.0);
    }

    #[test]
    fn flat_and_identity() {
        let src = vec![[10, 20, 30]; 9];
        assert_eq!(scale(&src, 3, 3, 4), vec![[10, 20, 30]; 144]);
        let one = vec![[1, 2, 3], [4, 5, 6]];
        assert_eq!(scale(&one, 2, 1, 1), one);
    }

    #[test]
    fn diagonal_edge_is_smoothed() {
        // A hard staircase: white below the diagonal, black above.
        let (w, h) = (8, 8);
        let src: Vec<[u8; 3]> = (0..w * h)
            .map(|i| if i % w <= i / w { [255; 3] } else { [0; 3] })
            .collect();
        let out = scale(&src, w, h, 4);
        // Nearest neighbour would have only black and white; xBRZ blends along the edge.
        assert!(out.iter().any(|p| p[0] > 20 && p[0] < 235));
        // Far from the edge nothing changes.
        assert_eq!(out[(28 * 32) + 1], [255; 3]);
        assert_eq!(out[30], [0; 3]);
    }
}

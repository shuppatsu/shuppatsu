//! An xBRZ-style pixel-art scaler for any whole factor: the colour version for textures
//! (`ddg-tools upscale-textures`) and the index-preserving one for the palettised 2D sprites
//! (enhancement `hud_upscale`, docs/texture-packs.md).
//!
//! The method follows xBRZ (Zenju): each 2×2 block of source pixels is classified from its
//! 4×4 neighbourhood (which diagonal is the edge, and whether it clearly dominates), then
//! every source pixel is enlarged to an n×n block and each of its four corners is blended
//! towards the neighbouring pixel along the detected edge: a rounded corner, a 45° line, a
//! shallow or steep 2:1 line, or both. Instead of hand-tuned per-scale tables, the blend
//! weights are the exact areas of each output sub-pixel on the far side of that edge
//! (computed once per scale by supersampling), which gives the same weights as xBRZ's tables
//! at 2× and 4× and extends to any factor.
//!
//! The decisions are made on *keys* compared by a distance (colours by YCbCr distance, or
//! palette indices through an [`IndexMetric`]); what ends up in each output pixel is the
//! *payload* of a source pixel (indices) or a blend of colours.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Keys closer than this (YCbCr distance, 0..255 channels) count as equal.
pub const EQUAL_TOLERANCE: f32 = 30.0;
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

/// The YCbCr distance of two sRGB colours.
pub fn colour_dist(a: [u8; 3], b: [u8; 3]) -> f32 {
    let r = f32::from(a[0]) - f32::from(b[0]);
    let g = f32::from(a[1]) - f32::from(b[1]);
    let bl = f32::from(a[2]) - f32::from(b[2]);
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * bl;
    let cb = 0.5 / (1.0 - 0.0722) * (bl - y);
    let cr = 0.5 / (1.0 - 0.2126) * (r - y);
    (y * y + cb * cb + cr * cr).sqrt()
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

/// Per shape, the sub-pixels with a non-zero weight: `(sx, sy, weight)` in the base frame.
type Tables = Arc<Vec<Vec<(i32, i32, f32)>>>;

/// [`coverage_tables`] computed once per factor, shared between threads.
fn cached_tables(n: usize) -> Tables {
    static CACHE: OnceLock<Mutex<HashMap<usize, Tables>>> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    cache
        .entry(n)
        .or_insert_with(|| {
            let sparse = coverage_tables(n)
                .into_iter()
                .map(|t| {
                    t.into_iter()
                        .enumerate()
                        .filter(|&(_, a)| a > 0.0)
                        .map(|(k, a)| ((k % n) as i32, (k / n) as i32, a))
                        .collect()
                })
                .collect();
            Arc::new(sparse)
        })
        .clone()
}

/// (dx, dy) rotated by `r` quarter turns: (1,1) → (−1,1) → (−1,−1) → (1,−1).
fn rot(r: usize, dx: i32, dy: i32) -> (i32, i32) {
    (0..r).fold((dx, dy), |(x, y), _| (-y, x))
}

/// Scales `src` (`w`×`h` RGB, row-major) by `n`, blending colours along the edges. Edges of
/// the image are clamped.
pub fn scale(src: &[[u8; 3]], w: usize, h: usize, n: usize) -> Vec<[u8; 3]> {
    assert_eq!(src.len(), w * h);
    if n <= 1 || w == 0 || h == 0 {
        return src.to_vec();
    }
    scale_by(
        src,
        src,
        (w, h, n),
        colour_dist,
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
    scale_by(colours, payload, (w, h, n), colour_dist, |t| t, take_half)
}

/// An output pixel takes the neighbour's payload when more than half of it lies beyond the
/// edge.
fn take_half<T>(p: &mut T, t: T, a: f32) {
    if a > 0.5 {
        *p = t;
    }
}

/// Distances between the 256 palette indices, for [`scale_indices`]: the YCbCr distance of
/// their reference colours, but never below [`EQUAL_TOLERANCE`] for two different indices.
/// So the decisions follow the colours where they differ clearly, and an edge between two
/// indices exists whatever colour a palette effect gives them later (an unlit segment of the
/// speed readout, a palette fading in from black).
pub struct IndexMetric {
    lut: Box<[f32]>,
}

impl IndexMetric {
    /// The metric of `colours`, one per index.
    pub fn new(colours: &[[u8; 3]; 256]) -> Self {
        let mut lut = vec![0.0f32; 256 * 256].into_boxed_slice();
        for i in 0..256 {
            for j in i + 1..256 {
                let d = colour_dist(colours[i], colours[j]).max(EQUAL_TOLERANCE);
                lut[i * 256 + j] = d;
                lut[j * 256 + i] = d;
            }
        }
        Self { lut }
    }

    pub fn dist(&self, a: u8, b: u8) -> f32 {
        self.lut[usize::from(a) * 256 + usize::from(b)]
    }
}

/// Index-preserving xBRZ on palette indices (`w`×`h`, row-major), with the decisions made by
/// `metric`: the result holds only indices of `idx`, so it is drawn with any palette.
pub fn scale_indices(idx: &[u8], w: usize, h: usize, n: usize, metric: &IndexMetric) -> Vec<u8> {
    assert_eq!(idx.len(), w * h);
    if n <= 1 || w == 0 || h == 0 {
        return idx.to_vec();
    }
    scale_by(
        idx,
        idx,
        (w, h, n),
        |a, b| metric.dist(a, b),
        |t| t,
        take_half,
    )
}

/// The scaler: corners classified on `keys` (compared with `dist`); each pixel becomes an
/// `n`×`n` block started from its payload (`start`) and blended towards a neighbour's payload
/// with weight `a` (`mix`).
fn scale_by<K: Copy + PartialEq, T: Copy, P: Copy>(
    keys: &[K],
    payload: &[T],
    (w, h, n): (usize, usize, usize),
    dist: impl Fn(K, K) -> f32,
    start: impl Fn(T) -> P,
    mix: impl Fn(&mut P, T, f32),
) -> Vec<P> {
    let clamp = |x: i32, y: i32| {
        let x = x.clamp(0, w as i32 - 1) as usize;
        let y = y.clamp(0, h as i32 - 1) as usize;
        y * w + x
    };
    let pay = |x: i32, y: i32| payload[clamp(x, y)];
    let px = |x: i32, y: i32| keys[clamp(x, y)];
    let eq = |a: K, b: K| dist(a, b) < EQUAL_TOLERANCE;

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
            let (f, g, j, kk) = (k(0, 0), k(1, 0), k(0, 1), k(1, 1));
            if (f == g && j == kk) || (f == j && g == kk) {
                continue;
            }
            let (b, c) = (k(0, -1), k(1, -1));
            let (e, hh) = (k(-1, 0), k(2, 0));
            let (i, l) = (k(-1, 1), k(2, 1));
            let (nn, o) = (k(0, 2), k(1, 2));
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
    let m = n as i32 - 1;
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let own = start(pay(x, y));
            let corners = blend[y as usize * w + x as usize];
            let (x0, y0) = (x as usize * n, y as usize * n);
            if corners == [Blend::None; 4] {
                for row in out[y0 * ow..].chunks_mut(ow).take(n) {
                    row[x0..x0 + n].fill(own);
                }
                continue;
            }
            block.fill(own);
            for r in 0..4 {
                let corner = |sx: i32, sy: i32| {
                    let (rx, ry) = rot(r, sx, sy);
                    corners[corner_index(rx, ry)]
                };
                let main = corner(1, 1);
                if main == Blend::None {
                    continue;
                }
                let at = |dx: i32, dy: i32| {
                    let (rx, ry) = rot(r, dx, dy);
                    px(x + rx, y + ry)
                };
                let (e, b, c, d) = (at(0, 0), at(0, -1), at(1, -1), at(-1, 0));
                let (f, g, hh, i) = (at(1, 0), at(-1, 1), at(0, 1), at(1, 1));
                let line = main == Blend::Dominant
                    || !((corner(1, -1) != Blend::None && !eq(e, g))
                        || (corner(-1, 1) != Blend::None && !eq(e, c))
                        || (!eq(e, i) && eq(g, hh) && eq(hh, i) && eq(i, f) && eq(f, c)));
                let (rx, ry) = if dist(e, f) <= dist(e, hh) {
                    rot(r, 1, 0)
                } else {
                    rot(r, 0, 1)
                };
                let col = pay(x + rx, y + ry);
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
                for &(sx, sy, a) in table {
                    let (rx, ry) = rot(r, 2 * sx - m, 2 * sy - m);
                    let (bx, by) = (((rx + m) / 2) as usize, ((ry + m) / 2) as usize);
                    mix(&mut block[by * n + bx], col, a);
                }
            }
            for (k, row) in block.chunks(n).enumerate() {
                let o = (y0 + k) * ow + x0;
                out[o..o + n].copy_from_slice(row);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nearest(src: &[u8], w: usize, h: usize, n: usize) -> Vec<u8> {
        (0..w * h * n * n)
            .map(|i| src[(i / (w * n)) / n * w + (i % (w * n)) / n])
            .collect()
    }

    /// A metric where index `i` is grey level `i`.
    fn grey_metric() -> IndexMetric {
        IndexMetric::new(&std::array::from_fn(|i| [i as u8; 3]))
    }

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
        let m = grey_metric();
        assert_eq!(scale_indices(&[7; 9], 3, 3, 3, &m), vec![7; 81]);
        assert_eq!(scale_indices(&[1, 2], 2, 1, 1, &m), vec![1, 2]);
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

    /// A staircase of index `a` below the diagonal and `b` above.
    fn staircase(a: u8, b: u8) -> Vec<u8> {
        (0..64)
            .map(|i| if i % 8 <= i / 8 { a } else { b })
            .collect()
    }

    #[test]
    fn indices_keep_their_values_and_get_sub_pixel_steps() {
        let m = grey_metric();
        for n in 2..=4 {
            let src = staircase(200, 10);
            let out = scale_indices(&src, 8, 8, n, &m);
            assert_eq!(out.len(), 64 * n * n);
            assert!(out.iter().all(|&i| i == 200 || i == 10));
            assert_ne!(out, nearest(&src, 8, 8, n), "×{n}");
            // Far from the edge each pixel keeps its block.
            assert_eq!(out[(7 * n) * 8 * n], 200);
            assert_eq!(out[7 * n], 10);
        }
    }

    #[test]
    fn indexed_matches_the_colour_decisions() {
        // With distinct, clearly different colours the metric is plain colour distance, so
        // the index path and the colour-keyed path agree.
        let colours: [[u8; 3]; 256] = std::array::from_fn(|i| [i as u8, 0, 255 - i as u8]);
        let m = IndexMetric::new(&colours);
        let src = staircase(240, 3);
        let keys: Vec<[u8; 3]> = src.iter().map(|&i| colours[usize::from(i)]).collect();
        for n in 2..=4 {
            assert_eq!(
                scale_indices(&src, 8, 8, n, &m),
                scale_indexed(&keys, &src, 8, 8, n)
            );
        }
    }

    #[test]
    fn different_indices_are_edges_even_in_one_colour() {
        // Two indices with the same reference colour (an unlit segment on its background):
        // still an edge, shaped the same as when they differ.
        let mut same = [[0u8; 3]; 256];
        same[5] = [40; 3];
        same[6] = [40; 3];
        let m = IndexMetric::new(&same);
        assert_eq!(m.dist(5, 6), EQUAL_TOLERANCE);
        assert_eq!(m.dist(5, 5), 0.0);
        let out = scale_indices(&staircase(5, 6), 8, 8, 4, &m);
        assert_ne!(out, nearest(&staircase(5, 6), 8, 8, 4));
        // A uniform colour scale of the reference does not change the shapes.
        let bright = IndexMetric::new(&std::array::from_fn(|i| [(i as u8).saturating_mul(2); 3]));
        let src: Vec<u8> = (0..64).map(|i| [3, 60, 120][(i * 7 + i / 8) % 3]).collect();
        assert_eq!(
            scale_indices(&src, 8, 8, 3, &grey_metric()),
            scale_indices(&src, 8, 8, 3, &bright)
        );
    }
}

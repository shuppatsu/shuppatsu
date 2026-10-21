//! Enhancement `wide_menus`: a seamless sideways continuation of a picture that has plain
//! paper texture at its edges but no tile pattern (the attract title picture, tbl 0x410).
//!
//! Everything is synthesised on the CPU from the picture itself, at art resolution, for the
//! palette the picture is drawn with:
//!
//! 1. **Exemplars.** A plain block at each edge ([`Paper::source_rows`] ×
//!    [`Paper::source_cols`]) has its light (blurred row and column means) removed, leaving
//!    zero-mean detail, which is made tileable by cross-fading its own wrap edges with a copy
//!    offset by half a period (offset-and-blend), first in x, then in y.
//! 2. **Synthesis.** A plain tiling of a block that small repeats visibly on a very wide
//!    window, so the fill is tiled *and blended* (Heitz & Neyret 2018, "High-performance
//!    by-example noise using a histogram-preserving blending operator"): over a triangle
//!    lattice every vertex shows one of the two exemplars at a hashed offset, and each pixel
//!    mixes the three vertices of its triangle by barycentric weight. All mixes preserve
//!    contrast (variance-preserving blend), so the joins do not go flat, and nothing repeats.
//! 3. **Light.** The light of each side is the mean colour of the picture's outermost columns,
//!    row by row, blurred vertically: added to the detail, it continues the picture's
//!    vertical lighting gradient.
//! 4. **Ramp.** The [`Paper::ramp`] outermost columns of the picture fade into the fill with a
//!    smoothstep, so there is no edge; glow or shadow reaching into them fades out with them.
//!
//! The lattice hash and all arithmetic are fixed, so the fill is deterministic.

use shuppatsu_formats::cg::Palette;
use std::sync::Arc;

/// Where a picture's paper fill comes from and how it joins the picture, in art pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Paper {
    /// Rows `source_rows.0 .. source_rows.1` of the [`Paper::source_cols`] outermost columns
    /// at each edge are plain texture: the exemplars.
    pub source_rows: (u16, u16),
    pub source_cols: u16,
    /// Columns at each edge of the picture blended into the fill.
    pub ramp: u16,
    /// The light is measured over [`Paper::edge_cols`] columns after skipping the
    /// `edge_skip` outermost ones at each edge.
    pub edge_skip: u16,
    pub edge_cols: u16,
}

/// Vertical blur of the light and of the exemplars' own light (Gaussian sigma, art pixels):
/// what is slower than this comes from the picture's edge, what is faster from the exemplars.
const LIGHT_SIGMA: f32 = 12.0;
/// Far from the picture the light is this much blurrier (sigma, art pixels): the blotches
/// of the edge columns would otherwise run on as horizontal streaks across a wide window.
const FAR_LIGHT_SIGMA: f32 = 40.0;
/// The distance from the picture (art pixels) over which the light turns from the near to
/// the far one.
const FAR_LIGHT: f32 = 96.0;
/// Edge length of the synthesis lattice's triangles, art pixels: about a third of an
/// exemplar's width, so each vertex shows a patch of texture, not a whole exemplar.
const CELL: f32 = 40.0;
/// Barycentric weights are raised to this power before mixing: patches keep more of their
/// own texture and blend over narrower joins.
const SHARPEN: i32 = 3;

type Rgb = [f32; 3];

/// One side's fill, `reach + ramp` columns by the picture's height (RGBA, row-major): on the
/// left picture columns `−reach .. ramp`, on the right `width − ramp .. width + reach`.
struct Side {
    first: i32,
    cols: usize,
    px: Vec<[u8; 4]>,
}

/// One side of a [`PaperFill`] for the GPU compositor: first column, column count, pixels.
pub(super) type SideRef<'a> = (i32, usize, &'a [[u8; 4]]);

/// A synthesised paper continuation of a `width × height` picture ([`PaperFill::build`]).
pub(super) struct PaperFill {
    width: i32,
    height: i32,
    ramp: i32,
    left: Side,
    right: Side,
}

/// The two exemplars, zero-mean detail tileable with period `w × h`.
struct Exemplars {
    w: usize,
    h: usize,
    blocks: [Vec<Rgb>; 2],
}

/// A palette that is at most this much brighter than the one a fill was built with is drawn
/// with that fill scaled ([`PaperCache::get`]).
const MAX_SCALE: f32 = 2.0;

/// The paper fill of one map, kept across palette changes. The fill is linear in the
/// palette's colours, so a palette that is a uniform scaling of the one it was built with (a
/// fade, which scales the target palette, 0x4367c0) is drawn with the fill scaled; building
/// takes milliseconds, scaling a fraction of that. A scaled fill serves one call: a palette
/// that stays the same gets the exact fill on the next. So the fill of a settled palette is
/// always exact, and the fill during a fade differs from an exact one by a level or two.
#[derive(Clone)]
pub(super) struct PaperCache {
    pub(super) tbl: u16,
    pub(super) reach: usize,
    /// The palette indices the picture uses (index 0 is transparent).
    used: Vec<u8>,
    /// The last fill built, with its palette.
    base: (Palette, Option<Arc<PaperFill>>),
    /// The fill last handed out, with its palette and whether it is exact.
    current: (Palette, Option<Arc<PaperFill>>, bool),
}

impl PaperCache {
    /// Builds the fill of map `tbl` ([`PaperFill::build`]).
    pub(super) fn build(
        tbl: u16,
        spec: &Paper,
        (width, height, px): (usize, usize, &[u8]),
        palette: &Palette,
        reach: usize,
    ) -> Self {
        let mut seen = [false; 256];
        for &i in px {
            seen[usize::from(i)] = true;
        }
        let used = (1..=255u8).filter(|&i| seen[usize::from(i)]).collect();
        let fill = PaperFill::build(spec, (width, height), px, palette, reach).map(Arc::new);
        Self {
            tbl,
            reach,
            used,
            base: (palette.clone(), fill.clone()),
            current: (palette.clone(), fill, true),
        }
    }

    /// The fill just built.
    pub(super) fn get_built(&self) -> Option<Arc<PaperFill>> {
        self.base.1.clone()
    }

    /// The fill for `palette` if this cache can serve it (exact, or the last build scaled);
    /// `None` when it has to be built again.
    pub(super) fn get(&mut self, palette: &Palette) -> Option<Option<Arc<PaperFill>>> {
        if self.current.0 == *palette {
            // The same palette twice in a row gets the exact fill.
            return self.current.2.then(|| self.current.1.clone());
        }
        if self.base.0 == *palette {
            self.current = (palette.clone(), self.base.1.clone(), true);
            return Some(self.base.1.clone());
        }
        let c = scale_of(palette, &self.base.0, &self.used)?;
        let fill = self.base.1.as_ref().map(|f| Arc::new(f.scaled(c)));
        self.current = (palette.clone(), fill.clone(), false);
        Some(fill)
    }
}

/// The factor `c` with `p ≈ c · r` on the `used` entries (least squares, each channel within
/// `c + 1` of it for the rounding of both), if there is one in `0 ..= MAX_SCALE`.
fn scale_of(p: &Palette, r: &Palette, used: &[u8]) -> Option<f32> {
    let channels = |pal: &Palette, i: u8| {
        let [b, r, g, _] = pal.0[usize::from(i)].to_le_bytes();
        [r, g, b].map(f32::from)
    };
    let pairs = || {
        used.iter()
            .flat_map(move |&i| channels(p, i).into_iter().zip(channels(r, i)))
    };
    let (pr, rr) = pairs().fold((0.0, 0.0), |(pr, rr), (p, r)| (pr + p * r, rr + r * r));
    let c = if rr > 0.0 { pr / rr } else { 0.0 };
    ((0.0..=MAX_SCALE).contains(&c) && pairs().all(|(p, r)| (p - c * r).abs() <= c + 1.0))
        .then_some(c)
}

impl PaperFill {
    /// The fill of the `width × height` picture `px` (palette indices) drawn with `palette`,
    /// `reach` columns past each edge; `None` when `spec`'s regions do not fit the picture.
    pub(super) fn build(
        spec: &Paper,
        (width, height): (usize, usize),
        px: &[u8],
        palette: &Palette,
        reach: usize,
    ) -> Option<Self> {
        let (r0, r1) = (
            usize::from(spec.source_rows.0),
            usize::from(spec.source_rows.1),
        );
        let sw = usize::from(spec.source_cols);
        let ramp = usize::from(spec.ramp);
        let (skip, ec) = (usize::from(spec.edge_skip), usize::from(spec.edge_cols));
        let fits = r0 + 2 <= r1
            && r1 <= height
            && sw >= 2
            && 2 * sw <= width
            && 2 * ramp <= width
            && ec > 0
            && skip + ec <= width / 2
            && px.len() == width * height;
        if !fits {
            return None;
        }
        // Only the edges are read: colours looked up per read, not for the whole picture.
        let rgb: [Rgb; 256] = std::array::from_fn(|i| {
            let [r, g, b, _] = palette.rgba(i as u8);
            [f32::from(r), f32::from(g), f32::from(b)]
        });
        let at = |x: usize, y: usize| rgb[usize::from(px[y * width + x])];
        let ph = r1 - r0;
        let block = |x0: usize| -> Vec<Rgb> {
            let b: Vec<Rgb> = (r0..r1)
                .flat_map(|y| (x0..x0 + sw).map(move |x| (x, y)))
                .map(|(x, y)| at(x, y))
                .collect();
            tileable(&b, sw, ph)
        };
        // The light near the edge and, much blurrier, far from it.
        let light = |cols: std::ops::Range<usize>| -> [Vec<Rgb>; 2] {
            let n = cols.len() as f32;
            let rows: Vec<Rgb> = (0..height)
                .map(|y| {
                    let mut s = [0.0; 3];
                    for x in cols.clone() {
                        add(&mut s, at(x, y), 1.0);
                    }
                    s.map(|v| v / n)
                })
                .collect();
            [blur(&rows, LIGHT_SIGMA), blur(&rows, FAR_LIGHT_SIGMA)]
        };
        // The two exemplar blocks and the two sides' light are independent: built at once.
        let (blocks, (light_l, light_r)) = std::thread::scope(|s| {
            let left = s.spawn(|| block(0));
            let l_light = s.spawn(|| light(skip..skip + ec));
            let r_light = s.spawn(|| light(width - skip - ec..width - skip));
            let right = block(width - sw);
            let lost = "paper worker";
            (
                [left.join().expect(lost), right],
                (l_light.join().expect(lost), r_light.join().expect(lost)),
            )
        });
        let ex = Exemplars {
            w: sw,
            h: ph,
            blocks,
        };
        let (w, r, reach_i) = (width as i32, ramp as i32, reach as i32);
        let (left, right) = std::thread::scope(|s| {
            let left = s.spawn(|| Side::build(&ex, &light_l, -reach_i..r, (r - 1, -1, r), &at));
            let right = Side::build(&ex, &light_r, w - r..w + reach_i, (w - r, 1, r), &at);
            (left.join().expect("paper worker"), right)
        });
        Some(Self {
            width: w,
            height: height as i32,
            ramp: r,
            left,
            right,
        })
    }

    /// The colour at picture pixel `(x, y)` where the fill is drawn (in the picture's ramp
    /// columns and up to the `reach` it was built with past its edges); `None` elsewhere.
    pub(super) fn at(&self, x: i32, y: i32) -> Option<[u8; 4]> {
        if !(0..self.height).contains(&y) {
            return None;
        }
        let side = if x < self.ramp {
            &self.left
        } else if x >= self.width - self.ramp {
            &self.right
        } else {
            return None;
        };
        let c = usize::try_from(x - side.first).ok()?;
        (c < side.cols).then(|| side.px[y as usize * side.cols + c])
    }

    /// This fill for a palette scaled by `c`.
    fn scaled(&self, c: f32) -> Self {
        // Every channel value scaled once (each fade step rescales the whole fill).
        let lut: [u8; 256] = std::array::from_fn(|v| (v as f32 * c).round().min(255.0) as u8);
        let v = |v: u8| lut[usize::from(v)];
        let side = |s: &Side| Side {
            first: s.first,
            cols: s.cols,
            px: s
                .px
                .iter()
                .map(|&[r, g, b, a]| [v(r), v(g), v(b), a])
                .collect(),
        };
        Self {
            width: self.width,
            height: self.height,
            ramp: self.ramp,
            left: side(&self.left),
            right: side(&self.right),
        }
    }

    /// The picture columns `[a, b)` the fill leaves to the picture: it covers everything left
    /// of `a` and from `b` on.
    pub(super) fn inner(&self) -> (i32, i32) {
        (self.ramp, self.width - self.ramp)
    }

    /// For the GPU compositor: the picture's width and height, the ramp, and each side's first
    /// column, column count and pixels (row-major), left then right.
    pub(super) fn sides(&self) -> (i32, i32, i32, [SideRef<'_>; 2]) {
        fn side(s: &Side) -> SideRef<'_> {
            (s.first, s.cols, &s.px)
        }
        (
            self.width,
            self.height,
            self.ramp,
            [side(&self.left), side(&self.right)],
        )
    }
}

/// Where a side's pixels take their detail and light from: the same for every palette, so
/// kept ([`SideShape::get`]) and only the colours are computed per build.
struct SideShape {
    /// Per column: the picture column, the share of the far light, and in the ramp the
    /// picture's share (`t`, 0 at the inside of the ramp, 1 at the edge).
    columns: Vec<(i32, f32, Option<f32>)>,
    /// Per pixel (row-major) the lattice taps ([`Taps`]).
    taps: Vec<Taps>,
}

/// The lattice taps of a pixel: per vertex of its triangle the exemplar (top bit) and the
/// index in it, and the vertex's weight.
type Taps = [(u32, f32); 3];

/// What a [`SideShape`] depends on: the exemplar size, the rows, the columns, and the ramp
/// (start, direction, length).
type ShapeKey = ((usize, usize), usize, (i32, i32), (i32, i32, i32));

/// Rows per band of a side built on its own thread.
const FILL_BAND: usize = 120;

/// Shapes kept (one per side and canvas width in use).
const SHAPES: usize = 8;

/// The reaches (art columns, as [`PaperCache`] rounds them) of 16:10 and 16:9 canvases,
/// whose shapes [`warm_shapes`] builds ahead.
const LIKELY_REACHES: [i32; 2] = [64, 128];

/// Builds on a thread the side shapes of a `width × height` picture's fill for the likely
/// reaches, so the first fill (the first frame of a fade) does not wait for them.
pub(super) fn warm_shapes(spec: &Paper, (width, height): (usize, usize)) {
    let ex = (
        usize::from(spec.source_cols),
        usize::from(spec.source_rows.1.saturating_sub(spec.source_rows.0)),
    );
    let (w, r) = (width as i32, i32::from(spec.ramp));
    if ex.0 == 0 || ex.1 == 0 || height == 0 {
        return;
    }
    std::thread::spawn(move || {
        for reach in LIKELY_REACHES {
            SideShape::get(ex, height, -reach..r, (r - 1, -1, r));
            SideShape::get(ex, height, w - r..w + reach, (w - r, 1, r));
        }
    });
}

impl SideShape {
    fn get(
        (w, h): (usize, usize),
        rows: usize,
        xs: std::ops::Range<i32>,
        (ramp_start, dir, ramp): (i32, i32, i32),
    ) -> Arc<Self> {
        static KEPT: std::sync::Mutex<Vec<(ShapeKey, Arc<SideShape>)>> =
            std::sync::Mutex::new(Vec::new());
        let key = ((w, h), rows, (xs.start, xs.end), (ramp_start, dir, ramp));
        let lock = || KEPT.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, s)) = lock().iter().find(|(k, _)| *k == key) {
            return s.clone();
        }
        // Built unlocked: the two sides build theirs at once.
        let columns = xs
            .clone()
            .map(|x| {
                // Columns from the inside of the ramp: 0 .. ramp in it, then the fill.
                let k = (x - ramp_start) * dir;
                let d = (k - ramp + 1).max(0) as f32;
                let a = smoothstep(d / FAR_LIGHT);
                let t = (k < ramp).then(|| smoothstep((k as f32 + 0.5) / ramp as f32));
                (x, a, t)
            })
            .collect();
        let xw: Vec<usize> = xs
            .clone()
            .map(|x| x.rem_euclid(w as i32) as usize)
            .collect();
        let taps = crate::par::par_range(rows, 64, |y| {
            xs.clone()
                .zip(&xw)
                .map(|(x, &xw)| taps((w, h), (x, y as i32), (xw, y % h)))
                .collect::<Vec<_>>()
        })
        .concat();
        let shape = Arc::new(Self { columns, taps });
        let mut kept = lock();
        // Built meanwhile by another thread (the warm-up, the other side).
        if let Some((_, s)) = kept.iter().find(|(k, _)| *k == key) {
            return s.clone();
        }
        if kept.len() >= SHAPES {
            kept.remove(0);
        }
        kept.push((key, shape.clone()));
        shape
    }
}

impl Side {
    /// One side's fill over picture columns `xs` under `light` (near and far, see
    /// [`FAR_LIGHT`]). The ramp `(start, dir, len)` starts at picture column `start` and runs
    /// `len` columns in direction `dir` (towards the edge); the picture is read with `at`.
    fn build(
        ex: &Exemplars,
        [near, far]: &[Vec<Rgb>; 2],
        xs: std::ops::Range<i32>,
        ramp: (i32, i32, i32),
        at: &(impl Fn(usize, usize) -> Rgb + Sync),
    ) -> Self {
        let (first, cols) = (xs.start, xs.len());
        let shape = SideShape::get((ex.w, ex.h), near.len(), xs, ramp);
        // The palette's part is a few multiply-adds per pixel: rows in a few bands at once.
        let bands = crate::par::par_range(near.len().div_ceil(FILL_BAND), 1, |b| {
            let rows = b * FILL_BAND..((b + 1) * FILL_BAND).min(near.len());
            let mut px = Vec::with_capacity(rows.len() * cols);
            for y in rows {
                let (n, f) = (near[y], far[y]);
                let taps = &shape.taps[y * cols..(y + 1) * cols];
                for (&(x, a, t), taps) in shape.columns.iter().zip(taps) {
                    let l: Rgb = std::array::from_fn(|i| n[i] + (f[i] - n[i]) * a);
                    let mut fill = l;
                    add(&mut fill, ex.synth(taps), 1.0);
                    let c = match t {
                        Some(t) => blend_around(l, at(x as usize, y), fill, t),
                        None => fill,
                    };
                    px.push(quantise(c));
                }
            }
            px
        });
        let px = bands.concat();
        Self { first, cols, px }
    }
}

/// The lattice taps of picture pixel `(x, y)` over `w × h` exemplars: the three vertices of
/// its triangle each show an exemplar at a hashed offset, weighted by sharpened barycentric
/// weight. `xw` and `yh` are `x` and `y` wrapped into the exemplar (`x mod w`, `y mod h`).
fn taps((w, h): (usize, usize), (x, y): (i32, i32), (xw, yh): (usize, usize)) -> Taps {
    // The triangle lattice: skewed coordinates in which the triangles are half squares.
    let (u, v) = (x as f32 / CELL, y as f32 / CELL);
    let (su, sv) = (u - v * 0.577_350_3, v * 1.154_700_5);
    let (bu, bv) = (su.floor() as i32, sv.floor() as i32);
    let (fu, fv) = (su - bu as f32, sv - bv as f32);
    let fz = 1.0 - fu - fv;
    let (verts, wt) = if fz > 0.0 {
        ([(bu, bv), (bu, bv + 1), (bu + 1, bv)], [fz, fv, fu])
    } else {
        (
            [(bu + 1, bv + 1), (bu + 1, bv), (bu, bv + 1)],
            [-fz, 1.0 - fv, 1.0 - fu],
        )
    };
    let wt = wt.map(|w| w.max(0.0).powi(SHARPEN));
    let norm = (wt[0] * wt[0] + wt[1] * wt[1] + wt[2] * wt[2])
        .sqrt()
        .max(f32::EPSILON);
    std::array::from_fn(|k| {
        let (i, j) = verts[k];
        let h32 = hash(i, j);
        // Offsets by multiply-shift: `(h >> 1) · w / 2³¹` and `(h >> 12) · h / 2²⁰`.
        let ox = ((u64::from(h32 >> 1) * w as u64) >> 31) as usize;
        let oy = ((u64::from(h32 >> 12) * h as u64) >> 20) as usize;
        let (sx, sy) = (wrap(xw + ox, w), wrap(yh + oy, h));
        let index = u32::try_from(sy * w + sx).expect("exemplar index");
        ((h32 & 1) << 31 | index, wt[k] / norm)
    })
}

impl Exemplars {
    /// The detail at a pixel with lattice taps `taps` ([`taps`]).
    fn synth(&self, taps: &Taps) -> Rgb {
        let mut out = [0.0; 3];
        for &(t, wn) in taps {
            let block = &self.blocks[(t >> 31) as usize];
            add(&mut out, block[(t & 0x7fff_ffff) as usize], wn);
        }
        out
    }
}

/// `i mod n` for `i < 2n`.
fn wrap(i: usize, n: usize) -> usize {
    if i >= n { i - n } else { i }
}

/// A fixed integer hash of a lattice vertex.
fn hash(i: i32, j: i32) -> u32 {
    let mut h = (i as u32).wrapping_mul(0x8da6_b343) ^ (j as u32).wrapping_mul(0xd816_3841);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297a_2d39);
    h ^ (h >> 15)
}

/// `p` and `q` mixed `1 − t : t` around the common mean `m`, preserving their contrast: the
/// deviations are mixed and rescaled by `1 / √((1 − t)² + t²)`, so the mix of two unrelated
/// textures is as strong as either.
fn blend_around(m: Rgb, p: Rgb, q: Rgb, t: f32) -> Rgb {
    let (a, b) = (1.0 - t, t);
    let norm = (a * a + b * b).sqrt().max(f32::EPSILON);
    std::array::from_fn(|i| m[i] + (a * (p[i] - m[i]) + b * (q[i] - m[i])) / norm)
}

/// The zero-mean detail of a `w × h` block made tileable with period `w × h`.
fn tileable(block: &[Rgb], w: usize, h: usize) -> Vec<Rgb> {
    let detail = detail(block, w, h);
    let zero = [0.0; 3];
    // The cross-fade band at each wrap edge: a quarter of the period.
    let weight = |i: usize, n: usize| {
        let d = (i.min(n - 1 - i) as f32 + 0.5) / (n as f32 / 4.0);
        smoothstep(d.min(1.0))
    };
    // x: away from its edges the block itself; at them the block offset by half a period,
    // which runs on across the wrap (its own join is in the middle, where the block shows).
    let mut tx = vec![zero; w * h];
    for y in 0..h {
        let a = |x| weight(x, w);
        for x in 0..w {
            let p = detail[y * w + x];
            let q = detail[y * w + (x + w / 2) % w];
            tx[y * w + x] = blend_around(zero, q, p, a(x));
        }
    }
    // y, likewise.
    let mut out = vec![zero; w * h];
    for y in 0..h {
        let a = weight(y, h);
        for x in 0..w {
            let q = tx[(y + h / 2) % h * w + x];
            out[y * w + x] = blend_around(zero, q, tx[y * w + x], a);
        }
    }
    out
}

/// A `w × h` block minus its light: the blurred row means, then the blurred column means of
/// what is left.
fn detail(block: &[Rgb], w: usize, h: usize) -> Vec<Rgb> {
    let mean = |it: &mut dyn Iterator<Item = Rgb>, n: usize| {
        let mut s = [0.0; 3];
        for c in it {
            add(&mut s, c, 1.0);
        }
        s.map(|v| v / n as f32)
    };
    let rows: Vec<Rgb> = (0..h)
        .map(|y| mean(&mut block[y * w..(y + 1) * w].iter().copied(), w))
        .collect();
    let rows = blur(&rows, LIGHT_SIGMA);
    let mut d: Vec<Rgb> = block
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            let mut c = c;
            add(&mut c, rows[i / w], -1.0);
            c
        })
        .collect();
    let cols: Vec<Rgb> = (0..w)
        .map(|x| mean(&mut (0..h).map(|y| d[y * w + x]), h))
        .collect();
    let cols = blur(&cols, LIGHT_SIGMA);
    for (i, c) in d.iter_mut().enumerate() {
        add(c, cols[i % w], -1.0);
    }
    d
}

/// A Gaussian blur of `v` (edges clamped).
fn blur(v: &[Rgb], sigma: f32) -> Vec<Rgb> {
    let r = (sigma * 3.0).ceil() as i64;
    let kernel: Vec<f32> = (-r..=r)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let total: f32 = kernel.iter().sum();
    let n = v.len() as i64;
    (0..n)
        .map(|i| {
            let mut s = [0.0; 3];
            for (k, &w) in (-r..=r).zip(&kernel) {
                add(&mut s, v[(i + k).clamp(0, n - 1) as usize], w / total);
            }
            s
        })
        .collect()
}

fn add(s: &mut Rgb, c: Rgb, w: f32) {
    for (s, c) in s.iter_mut().zip(c) {
        *s += c * w;
    }
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn quantise(c: Rgb) -> [u8; 4] {
    let [r, g, b] = c.map(|v| v.round().clamp(0.0, 255.0) as u8);
    [r, g, b, 255]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exemplars_tile_seamlessly() {
        // A non-periodic block: a diagonal ramp plus a checker.
        let (w, h) = (16, 12);
        let block: Vec<Rgb> = (0..w * h)
            .map(|i| {
                let (x, y) = (i % w, i / w);
                let v = (x * 7 + y * 3) as f32 + if (x + y) % 2 == 0 { 20.0 } else { 0.0 };
                [v, v * 0.5, 255.0 - v]
            })
            .collect();
        let t = tileable(&block, w, h);
        let diff = |a: Rgb, b: Rgb| (0..3).map(|i| (a[i] - b[i]).abs()).fold(0.0, f32::max);
        // Across the x wrap the step equals a step inside the tile (the checker's 20 at most,
        // with the contrast restored), never the block's left/right jump.
        let inside = (0..h)
            .flat_map(|y| (1..w).map(move |x| (x, y)))
            .map(|(x, y)| diff(t[y * w + x], t[y * w + x - 1]))
            .fold(0.0, f32::max);
        for y in 0..h {
            assert!(diff(t[y * w], t[y * w + w - 1]) <= inside + 1e-3);
        }
        // Across the y wrap likewise.
        let inside_y = (1..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| diff(t[y * w + x], t[(y - 1) * w + x]))
            .fold(0.0, f32::max);
        for x in 0..w {
            assert!(diff(t[(h - 1) * w + x], t[x]) <= inside_y + 1e-3);
        }
    }

    #[test]
    fn fades_reuse_the_fill_scaled_until_the_palette_settles() {
        // A 64 × 32 picture of greys 100..=131 and a palette of greys.
        let (w, h) = (64, 32);
        let px: Vec<u8> = (0..w * h)
            .map(|i| 100 + ((i * 7 + i / w) % 32) as u8)
            .collect();
        let grey = |f: f64| {
            Palette(std::array::from_fn(|v| {
                let v = (v as f64 * f) as u32;
                v | v << 8 | v << 16
            }))
        };
        let spec = Paper {
            source_rows: (16, 32),
            source_cols: 16,
            ramp: 4,
            edge_skip: 1,
            edge_cols: 4,
        };
        let full = grey(1.0);
        let mut cache = PaperCache::build(0, &spec, (w, h, &px), &full, 8);
        let exact = cache.get_built().expect("fits");
        // Half brightness (the fade's flooring): the fill scaled, within a level of an exact
        // build.
        let half = grey(0.5);
        let scaled = cache.get(&half).expect("a fade").expect("fits");
        let rebuilt = PaperFill::build(&spec, (w, h), &px, &half, 8).expect("fits");
        for x in -8..w as i32 + 8 {
            for y in 0..h as i32 {
                let (Some(s), Some(r)) = (scaled.at(x, y), rebuilt.at(x, y)) else {
                    continue;
                };
                assert!(
                    (0..3).all(|i| s[i].abs_diff(r[i]) <= 1),
                    "{x},{y}: {s:?} {r:?}"
                );
            }
        }
        // The same palette again is built exactly; back at full brightness the first build.
        assert!(cache.get(&half).is_none());
        assert!(Arc::ptr_eq(&cache.get(&full).unwrap().unwrap(), &exact));
        // Not a scaling: built again.
        let mut odd = full.clone();
        odd.0[110] = 0x00_10_20_30;
        assert!(cache.get(&odd).is_none());
        // Black is a scaling of anything; four times brighter is too far.
        assert_eq!(scale_of(&grey(0.0), &full, &cache.used), Some(0.0));
        assert_eq!(scale_of(&full, &grey(0.25), &cache.used), None);
    }

    #[test]
    fn variance_preserving_blend_keeps_the_ends() {
        let (m, p, q) = ([10.0; 3], [20.0; 3], [0.0; 3]);
        assert_eq!(blend_around(m, p, q, 0.0), p);
        assert_eq!(blend_around(m, p, q, 1.0), q);
        // Halfway, opposite deviations cancel to the mean.
        assert_eq!(blend_around(m, p, q, 0.5), m);
    }
}

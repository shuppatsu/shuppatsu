//! 8×8 inverse DCT for MPEG-1 video (11172-2, 2.4.4.2 and Annex A).
//!
//! Computed as the separable form of the defining formula
//! `f(x, y) = Σu Σv C(u) C(v) / 4 · F(u, v) · cos((2x+1)uπ/16) · cos((2y+1)vπ/16)`
//! in `f32` with a precomputed basis, rounded to the nearest integer and saturated to
//! −256..=255 as Annex A requires. A float evaluation of the definition is the reference that
//! IEEE 1180 measures integer IDCTs against, so it meets that accuracy by construction. Zero
//! rows are skipped, and a block whose only coefficient is the DC term takes a shortcut.

use std::sync::OnceLock;

/// `basis[u * 8 + x] = C(u) / 2 · cos((2x + 1) uπ / 16)` with `C(0) = 1/√2`, else 1.
fn basis() -> &'static [f32; 64] {
    static B: OnceLock<[f32; 64]> = OnceLock::new();
    B.get_or_init(|| {
        let mut b = [0f32; 64];
        for u in 0..8 {
            let c = if u == 0 {
                std::f64::consts::FRAC_1_SQRT_2
            } else {
                1.0
            };
            for x in 0..8 {
                let angle = ((2 * x + 1) * u) as f64 * std::f64::consts::PI / 16.0;
                b[u * 8 + x] = (c / 2.0 * angle.cos()) as f32;
            }
        }
        b
    })
}

/// Transforms `coeffs` (natural order) into 64 spatial values in −256..=255.
pub(crate) fn idct(coeffs: &[i32; 64], out: &mut [i32; 64]) {
    let nonzero_ac = coeffs[1..].iter().any(|&c| c != 0);
    if !nonzero_ac {
        // F(0,0) alone: every sample is F(0,0) / 8.
        let v = (coeffs[0] as f32 / 8.0).round() as i32;
        out.fill(v.clamp(-256, 255));
        return;
    }
    let b = basis();
    // Rows: tmp[y][x] = Σu basis[u][x] · F(u, y), skipping empty rows.
    let mut tmp = [0f32; 64];
    for y in 0..8 {
        let row = &coeffs[y * 8..y * 8 + 8];
        if row.iter().all(|&c| c == 0) {
            continue;
        }
        let t = &mut tmp[y * 8..y * 8 + 8];
        for (u, &c) in row.iter().enumerate() {
            if c == 0 {
                continue;
            }
            let c = c as f32;
            let bu = &b[u * 8..u * 8 + 8];
            for x in 0..8 {
                t[x] += c * bu[x];
            }
        }
    }
    // Columns: f[y][x] = Σv basis[v][y] · tmp[v][x].
    let mut acc = [0f32; 64];
    for v in 0..8 {
        let tv = &tmp[v * 8..v * 8 + 8];
        if tv.iter().all(|&c| c == 0.0) {
            continue;
        }
        let bv = &b[v * 8..v * 8 + 8];
        for y in 0..8 {
            let w = bv[y];
            let a = &mut acc[y * 8..y * 8 + 8];
            for x in 0..8 {
                a[x] += w * tv[x];
            }
        }
    }
    for (o, a) in out.iter_mut().zip(acc.iter()) {
        *o = (a.round() as i32).clamp(-256, 255);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The defining double sum in f64, for comparison.
    fn reference(c: &[i32; 64]) -> [i32; 64] {
        let mut out = [0; 64];
        for y in 0..8 {
            for x in 0..8 {
                let mut s = 0f64;
                for v in 0..8 {
                    for u in 0..8 {
                        let cu = if u == 0 { 0.5f64.sqrt() } else { 1.0 };
                        let cv = if v == 0 { 0.5f64.sqrt() } else { 1.0 };
                        s += cu * cv / 4.0
                            * c[v * 8 + u] as f64
                            * (((2 * x + 1) * u) as f64 * std::f64::consts::PI / 16.0).cos()
                            * (((2 * y + 1) * v) as f64 * std::f64::consts::PI / 16.0).cos();
                    }
                }
                out[y * 8 + x] = (s.round() as i32).clamp(-256, 255);
            }
        }
        out
    }

    /// IEEE 1180-style check: random blocks in the spatial range −L..=H, forward-transformed in
    /// f64 and rounded to integer coefficients, then inverted with ours and with the reference.
    #[test]
    fn matches_the_reference_on_random_blocks() {
        let mut seed = 1u64;
        let mut rand = |lo: i64, hi: i64| {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            lo + ((seed >> 33) as i64 % (hi - lo + 1))
        };
        let (mut worst, mut mismatches, mut total) = (0, 0usize, 0usize);
        for &(lo, hi) in &[(-256, 255), (-5, 5), (-300, 300)] {
            for _ in 0..2000 {
                let mut px = [0f64; 64];
                for p in &mut px {
                    *p = rand(lo, hi) as f64;
                }
                let mut coeffs = [0i32; 64];
                for v in 0..8 {
                    for u in 0..8 {
                        let cu = if u == 0 { 0.5f64.sqrt() } else { 1.0 };
                        let cv = if v == 0 { 0.5f64.sqrt() } else { 1.0 };
                        let mut s = 0.0;
                        for y in 0..8 {
                            for x in 0..8 {
                                s += px[y * 8 + x]
                                    * (((2 * x + 1) * u) as f64 * std::f64::consts::PI / 16.0)
                                        .cos()
                                    * (((2 * y + 1) * v) as f64 * std::f64::consts::PI / 16.0)
                                        .cos();
                            }
                        }
                        coeffs[v * 8 + u] = ((cu * cv / 4.0 * s).round() as i32).clamp(-2048, 2047);
                    }
                }
                let mut ours = [0; 64];
                idct(&coeffs, &mut ours);
                let want = reference(&coeffs);
                for (a, b) in ours.iter().zip(want.iter()) {
                    let d = (a - b).abs();
                    worst = worst.max(d);
                    mismatches += usize::from(d != 0);
                    total += 1;
                }
            }
        }
        assert!(worst <= 1, "peak error {worst}");
        // IEEE 1180 allows an overall mean square error of 0.02 per sample.
        assert!(
            (mismatches as f64) / (total as f64) < 0.001,
            "{mismatches}/{total}"
        );
    }

    #[test]
    fn dc_only_and_saturation() {
        let mut c = [0; 64];
        let mut out = [0; 64];
        c[0] = 1024;
        idct(&c, &mut out);
        assert!(out.iter().all(|&v| v == 128));
        c[0] = 2047 * 8;
        idct(&c, &mut out);
        assert!(out.iter().all(|&v| v == 255));
        c[0] = -2048;
        c[1] = 2047;
        idct(&c, &mut out);
        assert!(out.iter().all(|&v| (-256..=255).contains(&v)));
    }
}

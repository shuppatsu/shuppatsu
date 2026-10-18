//! Band-limited resampling kernel for [`crate::Interpolation::Sinc`] (enhancement
//! `audio_resampler = "sinc"`, docs/settings.md).
//!
//! A Kaiser-windowed sinc with [`TAPS`] taps, tabulated at [`PHASES`] fractional positions
//! (rows are blended linearly in between) for [`LEVELS`] cutoffs. Level 0 serves every pitch
//! at or below the output rate (pure upsampling: the cutoff sits just under the source's
//! Nyquist frequency, so the images above it are removed); level `k` serves source steps up to
//! `2^(k/4)` output samples, its cutoff scaled by `2^(−k/4)` so content above the *output's*
//! Nyquist frequency is removed before it can alias. Steps beyond the last level use it
//! (some aliasing above 4×, which the game never reaches: effects play at or below their own
//! rate, at most 44.1 kHz).
//!
//! Every row is normalised to a sum of 1, so DC passes at unity gain at every phase. The
//! tables (≈ 300 KB) are built once, on first use, outside the audio callback's hot path.

use std::sync::OnceLock;

/// Taps per output sample; the window covers source positions `cursor − 15 ..= cursor + 16`.
pub const TAPS: usize = 32;
/// Taps at and before the current source sample: window index `HISTORY − 1` is the cursor.
pub const HISTORY: usize = TAPS / 2;
/// Fractional positions tabulated between two source samples.
const PHASES: usize = 256;
/// Cutoff levels: 2^(−k/4) for k = 0..LEVELS, down to 1/4.
pub const LEVELS: usize = 9;
/// The cutoff (−6 dB point) as a fraction of the band kept: 0.9 of the source's Nyquist when
/// upsampling. With the window below the transition band is about ±0.16 of that Nyquist, so
/// the stopband starts near 1.06 and the passband is flat to about 0.74 (16 kHz for 44.1 kHz
/// music, 3.7 kHz for the 10 kHz voices).
const CUTOFF: f64 = 0.90;
/// Kaiser β: about 80 dB of stopband attenuation.
const BETA: f64 = 8.0;

/// The tabulated kernel: `LEVELS × (PHASES + 1) × TAPS` coefficients.
pub struct Kernel {
    coeffs: Box<[f32]>,
    /// The largest step each level serves.
    max_step: [f64; LEVELS],
}

/// The shared kernel, built on first use.
pub fn kernel() -> &'static Kernel {
    static K: OnceLock<Kernel> = OnceLock::new();
    K.get_or_init(Kernel::build)
}

/// Modified Bessel function of the first kind, order 0 (power series; converges fast for the
/// arguments a Kaiser window uses).
fn bessel_i0(x: f64) -> f64 {
    let q = x * x / 4.0;
    let (mut sum, mut term) = (1.0, 1.0);
    for k in 1..64 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

fn sinc(x: f64) -> f64 {
    if x.abs() < 1e-12 {
        1.0
    } else {
        let px = std::f64::consts::PI * x;
        px.sin() / px
    }
}

impl Kernel {
    fn build() -> Self {
        let mut coeffs = vec![0f32; LEVELS * (PHASES + 1) * TAPS].into_boxed_slice();
        let half = HISTORY as f64;
        let i0_beta = bessel_i0(BETA);
        let mut max_step = [0.0; LEVELS];
        for (level, max) in max_step.iter_mut().enumerate() {
            let scale = 2f64.powf(-(level as f64) / 4.0);
            *max = 1.0 / scale;
            let fc = CUTOFF * scale;
            for phase in 0..=PHASES {
                let t = phase as f64 / PHASES as f64;
                let row = &mut coeffs[Self::offset(level, phase)..][..TAPS];
                let mut taps = [0f64; TAPS];
                for (k, c) in taps.iter_mut().enumerate() {
                    // Distance from the output position (cursor + t) to tap k's source sample.
                    let x = k as f64 - (HISTORY - 1) as f64 - t;
                    let r = x / half;
                    let w = if r.abs() >= 1.0 {
                        0.0
                    } else {
                        bessel_i0(BETA * (1.0 - r * r).sqrt()) / i0_beta
                    };
                    *c = fc * sinc(fc * x) * w;
                }
                let sum: f64 = taps.iter().sum();
                for (dst, c) in row.iter_mut().zip(taps) {
                    *dst = (c / sum) as f32;
                }
            }
        }
        // The first step past 1 must pick level 1, not level 0.
        max_step[0] = 1.0;
        Self { coeffs, max_step }
    }

    fn offset(level: usize, phase: usize) -> usize {
        (level * (PHASES + 1) + phase) * TAPS
    }

    /// The level for a source step (source samples per output sample).
    pub fn level_for(&self, step: f64) -> usize {
        self.max_step
            .iter()
            .position(|&m| step <= m * (1.0 + 1e-9))
            .unwrap_or(LEVELS - 1)
    }

    /// The coefficients at fractional position `frac` (0 ≤ frac < 1) of `level`, blended
    /// between the two nearest tabulated phases.
    #[inline]
    pub fn coefficients(&self, level: usize, frac: f64) -> [f32; TAPS] {
        let p = (frac.clamp(0.0, 1.0) * PHASES as f64).min(PHASES as f64);
        let i = (p as usize).min(PHASES - 1);
        let u = (p - i as f64) as f32;
        let a = &self.coeffs[Self::offset(level, i)..][..TAPS];
        let b = &self.coeffs[Self::offset(level, i + 1)..][..TAPS];
        let mut out = [0f32; TAPS];
        for ((o, &x), &y) in out.iter_mut().zip(a).zip(b) {
            *o = x + (y - x) * u;
        }
        out
    }
}

/// The source frames around the cursor, oldest first, kept as a ring with a mirrored second
/// half so the window is always one contiguous slice (no allocation; part of a voice).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    buf: [[f32; 2]; 2 * TAPS],
    head: usize,
}

impl Window {
    pub const ZERO: Self = Self {
        buf: [[0.0; 2]; 2 * TAPS],
        head: 0,
    };

    /// Appends the newest frame, dropping the oldest.
    #[inline]
    pub fn push(&mut self, f: [f32; 2]) {
        self.buf[self.head] = f;
        self.buf[self.head + TAPS] = f;
        self.head = (self.head + 1) % TAPS;
    }

    /// The window, oldest first: index `HISTORY − 1` is the cursor.
    #[inline]
    pub fn frames(&self) -> &[[f32; 2]; TAPS] {
        self.buf[self.head..self.head + TAPS]
            .try_into()
            .expect("TAPS frames")
    }

    /// Replaces the whole window (oldest first).
    pub fn set(&mut self, frames: &[[f32; 2]; TAPS]) {
        self.buf[..TAPS].copy_from_slice(frames);
        self.buf[TAPS..].copy_from_slice(frames);
        self.head = 0;
    }

    /// Left channel only (mono sources store the same value in both).
    #[inline]
    pub fn dot_mono(&self, c: &[f32; TAPS]) -> f32 {
        self.frames().iter().zip(c).map(|(f, k)| f[0] * k).sum()
    }

    #[inline]
    pub fn dot_stereo(&self, c: &[f32; TAPS]) -> (f32, f32) {
        self.frames()
            .iter()
            .zip(c)
            .fold((0.0, 0.0), |(l, r), (f, k)| (l + f[0] * k, r + f[1] * k))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Magnitude response of the continuous interpolation kernel of `level` (all its phases
    /// together, as the resampler sees it) at `f` cycles per source sample.
    fn response(level: usize, f: f64) -> f64 {
        let k = kernel();
        let m = 128;
        let (mut re, mut im) = (0.0, 0.0);
        for i in 0..m {
            let t = i as f64 / m as f64;
            for (n, &h) in k.coefficients(level, t).iter().enumerate() {
                let x = n as f64 - (HISTORY - 1) as f64 - t;
                let w = 2.0 * std::f64::consts::PI * f * x;
                re += h as f64 * w.cos();
                im += h as f64 * w.sin();
            }
        }
        (re * re + im * im).sqrt() / m as f64
    }

    fn db(x: f64) -> f64 {
        20.0 * x.log10()
    }

    #[test]
    fn rows_have_unity_dc_and_are_symmetric() {
        let k = kernel();
        for level in 0..LEVELS {
            for frac in [0.0, 0.25, 0.5, 0.999] {
                let c = k.coefficients(level, frac);
                let sum: f32 = c.iter().sum();
                assert!((sum - 1.0).abs() < 1e-5, "level {level} frac {frac}: {sum}");
            }
            // Half a sample: the kernel is symmetric about the middle of the window.
            let c = k.coefficients(level, 0.5);
            for i in 0..TAPS / 2 {
                assert!((c[i] - c[TAPS - 1 - i]).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn upsampling_response() {
        // In cycles per source sample; the source's Nyquist frequency is 0.5.
        for f in [0.0, 0.1, 0.2, 0.3, 0.35] {
            let g = db(response(0, f));
            assert!(g.abs() < 0.1, "f {f}: {g:.2} dB");
        }
        let g = db(response(0, 0.45));
        assert!((-7.0..-5.0).contains(&g), "cutoff {g:.1} dB");
        // Images above about 1.07 × Nyquist are gone.
        for f in [0.54, 0.6, 0.75, 0.9, 1.0, 1.5] {
            let g = db(response(0, f));
            assert!(g < -70.0, "f {f}: {g:.1} dB");
        }
    }

    #[test]
    fn levels_cover_steps() {
        let k = kernel();
        assert_eq!(k.level_for(0.2), 0);
        assert_eq!(k.level_for(1.0), 0);
        assert_eq!(k.level_for(1.01), 1);
        assert_eq!(k.level_for(2.0), 4);
        assert_eq!(k.level_for(2.01), 5);
        assert_eq!(k.level_for(4.0), 8);
        assert_eq!(k.level_for(9.0), 8);
        // Level 4 (step 2): flat to 0.15, stopped from just above the output's Nyquist (0.25
        // cycles per source sample).
        assert!(db(response(4, 0.15)).abs() < 0.3);
        for f in [0.31, 0.4, 0.5, 0.75] {
            let g = db(response(4, f));
            assert!(g < -70.0, "f {f}: {g:.1} dB");
        }
    }

    #[test]
    fn window_ring_order() {
        let mut w = Window::ZERO;
        for i in 0..40 {
            w.push([i as f32, -(i as f32)]);
        }
        let f = w.frames();
        assert_eq!(f[0], [8.0, -8.0]);
        assert_eq!(f[TAPS - 1], [39.0, -39.0]);
        let mut d = [0f32; TAPS];
        d[TAPS - 1] = 1.0;
        assert_eq!(w.dot_stereo(&d), (39.0, -39.0));
        assert_eq!(w.dot_mono(&d), 39.0);
    }
}

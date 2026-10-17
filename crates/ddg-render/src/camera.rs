//! Camera and projection, reproducing the original (docs/re/renderer.md §4–§5).
//!
//! The game works in a left-handed, Y-up space and so does this renderer, which keeps model
//! winding and UVs unchanged.

use glam::{Mat4, Vec3};

/// Near/far planes, constant in the original (written once at 0x4255e7 / 0x4255f1).
pub const NEAR: f32 = 0.05;
pub const FAR: f32 = 32.0;

/// Clip-space depth (D3D convention, 0 = near … 1 = far) of eye depth `d` under a far plane.
fn d3d_depth(d: f32, far: f32) -> f32 {
    far * (d - NEAR) / ((far - NEAR) * d)
}

/// Eye depth of D3D clip-space depth `z` under a far plane (the inverse of [`d3d_depth`]).
fn eye_depth(z: f32, far: f32) -> f32 {
    NEAR * far / (far - z * (far - NEAR))
}

/// The eye depth (world units) at which the original's 2D depth word `depth` (z = depth /
/// 32768, renderer.md §8) sits once the far plane is moved to `far`: the same distance in
/// front of the far plane as under the original's 32 (enhancement `draw_distance`). The
/// background layers so keep occluding only the last few units: the sky (0x7ffe) the last
/// 1.2, the panorama (0x7ff0) the last 7.6.
pub fn background_eye_depth(depth: u16, far: f32) -> f32 {
    eye_depth(f32::from(depth) / 32768.0, FAR) + (far - FAR)
}

/// The renderer's (reversed) depth of a 2D depth word with the far plane at `far`; at the
/// original's 32 it is `1 − depth / 32768`.
pub fn background_depth(depth: u16, far: f32) -> f32 {
    if far == FAR {
        return 1.0 - f32::from(depth) / 32768.0;
    }
    1.0 - d3d_depth(background_eye_depth(depth, far), far)
}

/// Default vertical FOV in tenths of a degree (int at 0x47c3e0).
pub const DEFAULT_FOV_TENTHS: u16 = 300;

/// Projection aspect the original uses for its 3D band (int 142 at 0x47c3e4, × 0.01).
pub const ORIGINAL_ASPECT: f32 = 1.42;

/// Horizontal stretch of the original on a 4:3 screen (decision D1: kept).
///
/// The 3D viewport is the band y 52..400 of a 496×400 virtual screen stretched to the mode, so at
/// 640×480 it is 640 × 417.6 px (aspect 1.5326), while the projection uses aspect 1.42 after the
/// 4.12 fixed-point round trip (4096 / 2884 = 1.42025). The world therefore appears
/// 1.5326 / 1.42025 = 1.0791× wider than it is tall.
pub const ORIGINAL_STRETCH: f32 = (640.0 / (348.0 * 480.0 / 400.0)) / (4096.0 / 2884.0);

/// Converts like `Camera_SetProjection` (0x4183b0): `tan(fov/2)` truncated to 4.12 fixed point.
pub fn quantised_tan_half(fov_tenths: u16) -> f32 {
    let t = (f64::from(fov_tenths) * std::f64::consts::PI / 1800.0 * 0.5).tan();
    (t * 4096.0).trunc() as f32 / 4096.0
}

/// How the horizontal field of view adapts to the viewport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Projection {
    pub fov_tenths: u16,
    /// Horizontal stretch applied on top of Hor+ widening; [`ORIGINAL_STRETCH`] reproduces the
    /// original's look, 1.0 gives square pixels.
    pub stretch: f32,
    /// Far plane (world units): [`FAR`], or further with enhancement `draw_distance`.
    pub far: f32,
}

impl Default for Projection {
    fn default() -> Self {
        Self {
            fov_tenths: DEFAULT_FOV_TENTHS,
            stretch: ORIGINAL_STRETCH,
            far: FAR,
        }
    }
}

impl Projection {
    /// Left-handed perspective with depth in 0..1 (wgpu), Hor+: the vertical FOV is fixed and
    /// the horizontal extent follows `viewport_aspect` (width / height in pixels).
    ///
    /// Depth is **reversed** (near → 1, far → 0) for near-uniform float depth precision. X/Y and
    /// the clipping volume match the original; only the depth encoding differs.
    pub fn matrix(&self, viewport_aspect: f32) -> Mat4 {
        let tan_half = quantised_tan_half(self.fov_tenths);
        let aspect = viewport_aspect / self.stretch;
        // The original's PerspectiveLH (0x4261e0) has m00 = 1/(t·a), m11 = 1/t, m23 = 1 and
        // depth m22 = f/(f−n), m32 = −n·f/(f−n). Reversed: m22 = −n/(f−n), m32 = n·f/(f−n).
        let far = self.far;
        let r = NEAR / (far - NEAR);
        Mat4::from_cols_array(&[
            1.0 / (tan_half * aspect),
            0.0,
            0.0,
            0.0,
            0.0,
            1.0 / tan_half,
            0.0,
            0.0,
            0.0,
            0.0,
            -r,
            1.0,
            0.0,
            0.0,
            far * r,
            0.0,
        ])
    }

    /// The terms of the reversed depth `r = k / w − c` of eye depth `w` ([`Projection::matrix`]):
    /// `(k, c)` = (n·f / (f − n), n / (f − n)).
    pub fn depth_terms(&self) -> (f32, f32) {
        let c = NEAR / (self.far - NEAR);
        (self.far * c, c)
    }

    /// Horizontal half-angle tangent for a viewport, for culling/visibility decisions.
    pub fn tan_half_horizontal(&self, viewport_aspect: f32) -> f32 {
        quantised_tan_half(self.fov_tenths) * viewport_aspect / self.stretch
    }
}

/// How the world pass stores depth (renderer.md §6.1). Both store whole steps, so two coplanar
/// surfaces (the ballast of two tracks at a crossover, grass over the edge of the ballast)
/// land on the same step despite rounding and the later one drawn wins, as in the original.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DepthSteps {
    /// The original's 16-bit Z buffer: D3D depth `z = 1 − r` in steps of 1/65535, which grow
    /// with the square of the distance (3 m at 320 m).
    D16,
    /// Enhancement `fine_depth`: eye depth in steps of a constant 1/[`FINE_DEPTH_STEPS`] of an
    /// octave (0.068 %: 22 cm at 320 m, 87 cm at 1280 m).
    Fine,
}

/// Steps per doubling of the eye depth with [`DepthSteps::Fine`].
pub const FINE_DEPTH_STEPS: f32 = 1024.0;

impl DepthSteps {
    /// The bounds (far, near) in reversed depth of the step that holds reversed depth `r`
    /// (`terms` = [`Projection::depth_terms`]).
    pub fn step(self, r: f32, (k, c): (f32, f32)) -> (f32, f32) {
        match self {
            Self::D16 => {
                let z = ((1.0 - r) * 65535.0).round();
                (1.0 - (z + 0.5) / 65535.0, 1.0 - (z - 0.5) / 65535.0)
            }
            Self::Fine => {
                let w = k / (r + c).max(1e-30);
                let s = (w.log2() * FINE_DEPTH_STEPS).floor();
                let far = ((s + 1.0) / FINE_DEPTH_STEPS).exp2();
                let near = (s / FINE_DEPTH_STEPS).exp2();
                (k / far - c, k / near - c)
            }
        }
    }

    /// The depth stored for reversed depth `r`, as the shader's `stepped_depth`: inside r's
    /// step, the nearer the later the instance (`order` = its place among the frame's, 0..=1),
    /// so within a step the later one drawn wins and a nearer step always beats a further one.
    /// [`DepthSteps::Fine`] picks the step with the later instance counted up to
    /// [`FINE_ORDER_BIAS`] nearer, so rounding cannot put it a step behind an earlier
    /// coplanar one either; the original's [`DepthSteps::D16`] has no such bias.
    pub fn depth(self, r: f32, terms: (f32, f32), order: f32) -> f32 {
        let r = match self {
            Self::D16 => r,
            Self::Fine => r + (r + terms.1) * FINE_ORDER_BIAS * order,
        };
        let (far, near) = self.step(r, terms);
        (far + (near - far) * (0.25 + 0.5 * order)).clamp(0.0, 1.0)
    }
}

/// How much nearer (fraction of the eye depth) [`DepthSteps::Fine`] counts the frame's last
/// instance when it picks the step, the first not at all: 0.01 %, well above the rounding
/// between coplanar surfaces and well below a step (at the Keihin crossover of
/// renderer.md §6.1, 1e-5 still left a few one-pixel slivers, 1e-4 none). Mirrored in
/// shader.wgsl.
pub const FINE_ORDER_BIAS: f32 = 1e-4;

/// A look-at camera in game space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub eye: Vec3,
    pub target: Vec3,
}

impl Camera {
    /// The original's view: `Camera_LookAt` (0x418420) builds a right-handed look-at (forward
    /// = −z) and the fixed VIEW transform diag(1, 1, −1, 1) flips z. Net effect: rows `f × up`,
    /// `up`, `f`, i.e. screen x is the mirror of a left-handed look-at. Verified against a
    /// screenshot of the original (Keihin, approaching Higashi-Kanagawa): with a left-handed
    /// look-at the whole scene came out mirrored.
    pub fn view(&self) -> Mat4 {
        let f = (self.target - self.eye).normalize();
        let s = f.cross(Vec3::Y).normalize();
        let u = s.cross(f);
        Mat4::from_cols(
            s.extend(-s.dot(self.eye)),
            u.extend(-u.dot(self.eye)),
            f.extend(-f.dot(self.eye)),
            glam::Vec4::W,
        )
        .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec4;

    #[test]
    fn quantisation_matches_original() {
        // renderer.md §4: 30° gives tan 0.267822 instead of 0.267949.
        assert!((quantised_tan_half(300) - 0.267_822).abs() < 1e-6);
    }

    #[test]
    fn stretch_is_about_eight_percent() {
        assert!((ORIGINAL_STRETCH - 1.0791).abs() < 1e-3);
    }

    #[test]
    fn depth_range_and_hor_plus() {
        let p = Projection::default();
        let m = p.matrix(16.0 / 10.0);
        let near = m * Vec4::new(0.0, 0.0, NEAR, 1.0);
        let far = m * Vec4::new(0.0, 0.0, FAR, 1.0);
        // Reversed depth: near → 1, far → 0.
        assert!((near.z / near.w - 1.0).abs() < 1e-6);
        assert!((far.z / far.w).abs() < 1e-6);
        // Hor+: same vertical scale at any aspect, wider aspect → smaller horizontal scale.
        let m43 = p.matrix(4.0 / 3.0);
        assert_eq!(m.y_axis.y, m43.y_axis.y);
        assert!(m.x_axis.x < m43.x_axis.x);
        // A further far plane maps there to 0 and keeps the near plane at 1.
        let far = Projection { far: 96.0, ..p }.matrix(1.6);
        let f = far * Vec4::new(0.0, 0.0, 96.0, 1.0);
        let n = far * Vec4::new(0.0, 0.0, NEAR, 1.0);
        assert!((f.z / f.w).abs() < 1e-6 && (n.z / n.w - 1.0).abs() < 1e-6);
    }

    /// Reversed depth of eye depth `w`.
    fn reversed(w: f32, (k, c): (f32, f32)) -> f32 {
        k / w - c
    }

    #[test]
    fn depth_steps_merge_coplanar_and_keep_layers() {
        for far in [FAR, 64.0, 128.0] {
            let terms = Projection {
                far,
                ..Projection::default()
            }
            .depth_terms();
            for steps in [DepthSteps::D16, DepthSteps::Fine] {
                let d = |w: f32, order: f32| steps.depth(reversed(w, terms), terms, order);
                let step = |r: f32| steps.step(r, terms);
                // Monotonic: further is never nearer, at any one order.
                for order in [0.0, 0.5, 1.0] {
                    let mut last = f32::INFINITY;
                    for i in 0..20_000 {
                        let w = NEAR + i as f32 * (far - NEAR) / 20_000.0;
                        let x = d(w, order);
                        assert!(x <= last && (0.0..=1.0).contains(&x), "{steps:?} {far} {w}");
                        last = x;
                    }
                }
                // Coplanar surfaces, a rounding error apart, share a step almost everywhere.
                let (mut same, mut n) = (0, 0);
                for i in 0..10_000 {
                    let r = reversed(0.3 + i as f32 * 0.003, terms);
                    same += usize::from(step(r) == step(r * (1.0 + 2e-6)));
                    n += 1;
                }
                assert!(same * 100 >= n * 97, "{steps:?} {far}: {same} of {n}");
                // Inside a step the order tells instances apart (later = nearer) from the
                // ground under the cab's eye (0.26) out, and never leaves the step.
                for i in 0..1000 {
                    let w = 0.26 + i as f32 * (far - 0.26) / 1000.0;
                    let r = reversed(w, terms);
                    let (lo, _) = step(r);
                    if lo <= 0.0 {
                        // The last step, cut in half by the far plane.
                        continue;
                    }
                    assert!(lo < d(w, 0.0) && d(w, 0.0) < d(w, 0.002), "{steps:?} {w}");
                    // (Fine picks the last instance's step as if FINE_ORDER_BIAS nearer.)
                    let biased = match steps {
                        DepthSteps::D16 => r,
                        DepthSteps::Fine => r + (r + terms.1) * FINE_ORDER_BIAS,
                    };
                    assert!(d(w, 1.0) < step(biased).1, "{steps:?} {w}");
                }
            }
            // Fine: two walls 1 % of the distance apart stay apart at any distance, whichever
            // is drawn later (the ground seen from the cab's 0.26 at a glancing angle keeps a
            // layer 1/256 unit above it ~1.5 % of the distance nearer).
            let d = |w: f32, order| DepthSteps::Fine.depth(reversed(w, terms), terms, order);
            for i in 1..200 {
                let w = far * i as f32 / 200.0;
                assert!(d(w * 1.01, 1.0) < d(w, 0.0), "{far} {w}");
            }
        }
    }

    #[test]
    fn fine_depth_lets_the_later_coplanar_instance_win() {
        let terms = Projection {
            far: 64.0,
            ..Projection::default()
        }
        .depth_terms();
        let steps = DepthSteps::Fine;
        for i in 0..20_000 {
            let w = 0.26 + i as f32 * 0.003;
            let r = reversed(w, terms);
            // The later one rounded a little further away: it still shows.
            for (early, late) in [(0.1, 0.3), (0.5, 0.6), (0.0, 1.0)] {
                let later = steps.depth(r * (1.0 - 2e-6), terms, late);
                assert!(later > steps.depth(r, terms, early), "{w} {early} {late}");
            }
        }
    }

    #[test]
    fn d16_steps_are_the_originals() {
        // At 320 m (the original's far plane) one 16-bit step is about 0.3 units.
        let terms = Projection::default().depth_terms();
        let step = |w: f32| DepthSteps::D16.step(reversed(w, terms), terms);
        let mut steps: Vec<_> = (0..100).map(|i| step(31.0 + i as f32 / 100.0)).collect();
        steps.dedup();
        assert!(steps.len() <= 5, "{}", steps.len());
        assert!(step(1.0).0 >= step(1.001).1);
    }

    #[test]
    fn background_keeps_its_distance_from_the_far_plane() {
        for w in [0x7fe0u16, 0x7ff0, 0x7ffe] {
            assert_eq!(background_depth(w, FAR), 1.0 - f32::from(w) / 32768.0);
            let d = background_eye_depth(w, FAR);
            assert!((d3d_depth(d, FAR) - f32::from(w) / 32768.0).abs() < 1e-6);
            for far in [64.0, 96.0, 128.0] {
                let gap = FAR - d;
                assert!((far - background_eye_depth(w, far) - gap).abs() < 1e-3);
            }
        }
        // The panorama sits 7.6 units in front of the far plane, the sky 1.2.
        assert!((FAR - background_eye_depth(0x7ff0, FAR) - 7.6).abs() < 0.1);
        assert!((FAR - background_eye_depth(0x7ffe, FAR) - 1.2).abs() < 0.1);
        // The order of the layers stays.
        assert!(background_depth(0x7ff0, 96.0) > background_depth(0x7ffe, 96.0));
    }
}

#[cfg(test)]
mod view_tests {
    use super::*;

    #[test]
    fn view_matches_original_handedness() {
        // Looking along +z from the origin: forward maps to +z (depth), and world −x appears on
        // the right of the screen (x row = f × up = z × y = −x).
        let v = Camera {
            eye: Vec3::ZERO,
            target: Vec3::Z,
        }
        .view();
        assert!(v.transform_point3(Vec3::Z).abs_diff_eq(Vec3::Z, 1e-6));
        assert!(v.transform_point3(Vec3::NEG_X).abs_diff_eq(Vec3::X, 1e-6));
        assert!(v.transform_point3(Vec3::Y).abs_diff_eq(Vec3::Y, 1e-6));
    }
}

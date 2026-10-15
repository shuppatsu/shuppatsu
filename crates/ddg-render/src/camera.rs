//! Camera and projection, reproducing the original (docs/re/renderer.md §4–§5).
//!
//! The game works in a left-handed, Y-up space and so does this renderer, which keeps model
//! winding and UVs unchanged.

use glam::{Mat4, Vec3};

/// Near/far planes, constant in the original (written once at 0x4255e7 / 0x4255f1).
pub const NEAR: f32 = 0.05;
pub const FAR: f32 = 32.0;

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
}

impl Default for Projection {
    fn default() -> Self {
        Self {
            fov_tenths: DEFAULT_FOV_TENTHS,
            stretch: ORIGINAL_STRETCH,
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
        let r = NEAR / (FAR - NEAR);
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
            FAR * r,
            0.0,
        ])
    }

    /// Horizontal half-angle tangent for a viewport, for culling/visibility decisions.
    pub fn tan_half_horizontal(&self, viewport_aspect: f32) -> f32 {
        quantised_tan_half(self.fov_tenths) * viewport_aspect / self.stretch
    }
}

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

//! Positions along a track polyline, interpolated on the curve.
//!
//! Track points are 1.25 m apart (docs/re/routes.md). Rendering at refresh rates above the
//! 30 Hz simulation interpolates the *distance* along the track and then evaluates the curve, so
//! the camera follows curves instead of cutting chords (CLAUDE.md).

use glam::Vec3;

pub struct TrackPath {
    points: Vec<Vec3>,
}

impl TrackPath {
    pub fn new(points: Vec<Vec3>) -> Self {
        assert!(points.len() >= 2, "a track path needs at least two points");
        Self { points }
    }

    /// Length in point intervals.
    pub fn len_points(&self) -> f32 {
        (self.points.len() - 1) as f32
    }

    /// Position at `t` point intervals from the start (clamped), on a uniform Catmull-Rom spline
    /// through the points.
    pub fn at(&self, t: f32) -> Vec3 {
        let t = t.clamp(0.0, self.len_points());
        let i = (t.floor() as usize).min(self.points.len() - 2);
        let f = t - i as f32;
        let p = |k: isize| {
            let idx = (i as isize + k).clamp(0, self.points.len() as isize - 1) as usize;
            self.points[idx]
        };
        let (p0, p1, p2, p3) = (p(-1), p(0), p(1), p(2));
        let f2 = f * f;
        let f3 = f2 * f;
        0.5 * ((2.0 * p1)
            + (p2 - p0) * f
            + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * f2
            + (3.0 * p1 - p0 - 3.0 * p2 + p3) * f3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_through_points_and_clamps() {
        let path = TrackPath::new(vec![Vec3::ZERO, Vec3::X, Vec3::new(2.0, 0.0, 1.0)]);
        assert!(path.at(1.0).abs_diff_eq(Vec3::X, 1e-6));
        assert!(path.at(-5.0).abs_diff_eq(Vec3::ZERO, 1e-6));
        assert!(path.at(99.0).abs_diff_eq(Vec3::new(2.0, 0.0, 1.0), 1e-6));
        // Straight line stays straight.
        let line = TrackPath::new(vec![Vec3::ZERO, Vec3::X, 2.0 * Vec3::X, 3.0 * Vec3::X]);
        assert!(line.at(1.5).abs_diff_eq(Vec3::new(1.5, 0.0, 0.0), 1e-6));
    }
}

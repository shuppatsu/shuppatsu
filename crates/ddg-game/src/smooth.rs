//! Presentation interpolation between the last two simulation ticks (enhancement, see
//! docs/re/renderer.md §13.9): the window renders at its own rate and places the wiper, the
//! snow scroll and the speedometer needle at the render alpha, like the camera. At alpha 1
//! (headless shots, paused, after a jump) everything is exactly the last tick's state.

use crate::hud;
use ddg_render::sprites::{Fan, SpriteLayer};

/// `a + (b − a)·alpha`, exactly `a` at alpha ≤ 0 and exactly `b` at alpha ≥ 1.
pub fn lerp(a: f32, b: f32, alpha: f32) -> f32 {
    if alpha <= 0.0 {
        a
    } else if alpha >= 1.0 {
        b
    } else {
        a + (b - a) * alpha
    }
}

/// [`lerp`] in f64.
pub fn lerp64(a: f64, b: f64, alpha: f32) -> f64 {
    if alpha <= 0.0 {
        a
    } else if alpha >= 1.0 {
        b
    } else {
        a + (b - a) * f64::from(alpha)
    }
}

/// The needle's colour (opaque black, hud.rs `speedometer_draw`).
const NEEDLE_ARGB: u32 = 0xff00_0000;

/// The analog speedometer needle between ticks. The HUD draws it once per tick as a fan of
/// the rect layer at the tick's speed (hud.rs `needle`); [`Needle::frame`] moves that fan to
/// the interpolated speed and says when the canvas must be composed again (only when a
/// needle pixel changes).
#[derive(Debug, Default, Clone)]
pub struct Needle {
    /// Speeds (km/h) of the tick before and the last tick.
    kmh: [f32; 2],
    /// Index of the needle's fan in the layer after the last HUD update, and the points it
    /// has now (a fan list rebuilt by anything else no longer matches and is left alone).
    fan: Option<(usize, [[i32; 2]; 3])>,
}

impl Needle {
    /// After a tick's HUD update: `before` and `after` are the drive's speed before and after
    /// the tick. Finds the needle the HUD drew for `after`, if any.
    pub fn ticked(&mut self, before: f32, after: f32, layer: &SpriteLayer) {
        self.kmh = [before, after];
        let points = hud::needle(f64::from(after));
        self.fan = layer
            .fans
            .iter()
            .position(|f| is_needle(f, &points))
            .map(|i| (i, points));
    }

    /// Sets the needle's fan for `alpha`; true if it moved (the canvas needs composing).
    pub fn frame(&mut self, alpha: f32, layer: &mut SpriteLayer) -> bool {
        let Some((i, drawn)) = self.fan else {
            return false;
        };
        let Some(fan) = layer.fans.get_mut(i).filter(|f| is_needle(f, &drawn)) else {
            self.fan = None;
            return false;
        };
        let points = Self::points(self.kmh, alpha);
        if points == drawn {
            return false;
        }
        fan.points[..3].copy_from_slice(&points);
        self.fan = Some((i, points));
        true
    }

    /// The needle triangle for the speed interpolated between `kmh[0]` and `kmh[1]`.
    fn points(kmh: [f32; 2], alpha: f32) -> [[i32; 2]; 3] {
        hud::needle(lerp64(f64::from(kmh[0]), f64::from(kmh[1]), alpha))
    }
}

/// Whether `fan` is a needle triangle at `points`.
fn is_needle(fan: &Fan, points: &[[i32; 2]; 3]) -> bool {
    fan.count == 3 && fan.points[..3] == points[..] && fan.argb == NEEDLE_ARGB
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lerp_is_exact_at_the_ticks() {
        let (a, b) = (1.500_000_1f32, 1.526_179_9f32);
        assert_eq!(lerp(a, b, 0.0), a);
        assert_eq!(lerp(a, b, 1.0), b);
        assert!((lerp(a, b, 0.5) - (a + b) / 2.0).abs() < 1e-6);
        assert_eq!(lerp64(8.0, 16.0, 0.5), 12.0);
    }

    #[test]
    fn needle_interpolates_between_the_ticks() {
        let kmh = [60.0, 70.0];
        assert_eq!(Needle::points(kmh, 0.0), hud::needle(60.0));
        assert_eq!(Needle::points(kmh, 0.5), hud::needle(65.0));
        assert_eq!(Needle::points(kmh, 1.0), hud::needle(70.0));
        // The fan the HUD drew for the last tick is found and moved; at alpha 1 it is unchanged.
        let mut layer = SpriteLayer::default();
        layer.add_fan(&hud::needle(70.0), NEEDLE_ARGB);
        let mut n = Needle::default();
        n.ticked(60.0, 70.0, &layer);
        assert!(!n.frame(1.0, &mut layer));
        assert!(n.frame(0.0, &mut layer));
        assert_eq!(layer.fans[0].points[..3], hud::needle(60.0)[..]);
        assert!(n.frame(0.5, &mut layer));
        assert_eq!(layer.fans[0].points[..3], hud::needle(65.0)[..]);
        assert!(n.frame(1.0, &mut layer));
        assert_eq!(layer.fans[0].points[..3], hud::needle(70.0)[..]);
        // A fan list rebuilt by someone else is left alone.
        layer.fans[0].argb = 0xff12_3456;
        assert!(!n.frame(0.0, &mut layer));
    }
}

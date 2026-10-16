//! The black panel over the top of the driving view (docs/re/hud.md §6.5, scene.md §2.1).
//!
//! The drive task's draw callback 0x41e460 first draws `Draw2D_FlatQuad` with the corners at
//! 0x486160, (0,0)–(496,94) virtual, colour word 0 (palette entry 0, black) at depth word 0.
//! It is drawn before the track, scenery and background, and its z = 0 is nearer than anything
//! in the world, so the top 94 of the 400 virtual rows (pixel rows 0..=112 at 640×480) stay black
//! behind the HUD's clocks, remaining time and station plates. The 3D viewport itself starts at
//! virtual y 52, so only rows 52..94 are covered over the world; above them the clear colour is
//! already black.

use super::Play;
use super::ending::Ending;
use crate::scene::{RouteScene, ScreenMap};
use ddg_render::effects::EffectGeometry;

impl Play {
    /// Adds the drive's top mask to the 3D band's effect quads of band aspect `aspect`. It has
    /// to come before the weather overlay: the wiper (also depth 0) is drawn after it in the
    /// original's task list and covers it under the original's LESSEQUAL test, while the drops
    /// and snow (depth 2 and up) fail the Z test inside it.
    ///
    /// The ending animates the same quad (0x486160) as its top letterbox bar
    /// (`ending_masks`), so nothing is added while it runs.
    ///
    /// Widescreen: the quad's left and right edges are moved to the visible Hor+ range, so the
    /// plain black panel fills the full width (filling, not stretching; no art involved).
    pub(super) fn cab_mask(
        scene: &RouteScene,
        ending: Option<&Ending>,
        aspect: f32,
        fx: &mut EffectGeometry,
    ) {
        if ending.is_some() {
            return;
        }
        let quad = &scene.tables.background.mask_quad;
        let map = ScreenMap { aspect };
        let (left, right) = map.x_range();
        let xy = quad.widened([left, right]).map(|[x, y]| map.ndc(x, y));
        // Colour word 0: palette 0 entry 0, black (measured (0,0,0) in the original's capture).
        fx.add_flat_quad(xy, [0.0, 0.0, 0.0, 1.0], 0);
    }
}

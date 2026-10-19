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
use shuppatsu_render::effects::EffectGeometry;

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

impl Play {
    /// Adds the coupling minigame's bottom band (`bottom_band`, `0x84c0f8 = 1`) to the 3D
    /// band's effect quads. The render task 0x425680 draws `Draw2D_FlatQuad` with the corners
    /// at 0x486ec4, (0,268)–(496,400) virtual, colour words 0 (black, 0x47c3e8) at depth 0
    /// before the task lists, so the world under the readout panel is black (pixel rows
    /// 321..480 at 640×480) and only the panel's sprites show there. Like the top mask it comes
    /// before the weather overlay.
    ///
    /// Widescreen: with `wide` (enhancement `wide_coupling_band`) the band spans the visible Hor+
    /// range, so the bottom of the screen is black edge to edge under the centred panel;
    /// without it the band keeps the 4:3 width of the original's screen (virtual x 0..496, the
    /// centred 640-pixel design the panel is drawn in) and the view goes on beside it.
    pub(super) fn bottom_band(
        scene: &RouteScene,
        bottom_band: bool,
        wide: bool,
        aspect: f32,
        fx: &mut EffectGeometry,
    ) {
        if !bottom_band {
            return;
        }
        let map = ScreenMap { aspect };
        let quad = &scene.tables.background.bottom_band_quad;
        let xy = if wide {
            let (left, right) = map.x_range();
            quad.widened([left, right])
        } else {
            quad.xy.map(|p| p.map(f32::from))
        }
        .map(|[x, y]| map.ndc(x, y));
        fx.add_flat_quad(xy, [0.0, 0.0, 0.0, 1.0], 0);
    }
}

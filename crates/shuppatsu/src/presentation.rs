//! The 2D layer's layout and resolution (docs/localisation.md, "UI scaling").
//!
//! Three independent settings (all in [`crate::locale::LocaleConfig`]):
//! * the HUD layout ([`HudLayout`]): the original 4:3 one, or the enhanced one (the driving
//!   HUD's groups at the window's edges, the wider and bilingual station plates);
//! * the canvas multiple (`SpriteLayer::scale`): from the window height
//!   ([`shuppatsu_render::overlay::auto_multiple`]) unless `--ui-scale` fixes it;
//! * the presentation ([`shuppatsu_render::overlay::UiScaling`]): sharp bilinear, whole multiples,
//!   or plain nearest neighbour.

use anyhow::Result;
use shuppatsu_formats::exe::Exe;
use shuppatsu_render::overlay::{UiScaling, wide_canvas_width};
use shuppatsu_render::sprites::{CANVAS_H, CANVAS_W, SpriteBank, SpriteLayer};
use shuppatsu_render::text::TextOverlay;

use crate::locale::LocaleConfig;
use crate::scene::GameData;

/// Enhancement `hud_layout`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HudLayout {
    /// The original layout at any resolution: the 640×480 design, centred.
    #[default]
    Original,
    /// The driving HUD on a canvas of the window's shape with its groups at the edges
    /// (`wide_hud`, docs/re/hud.md §6.4), the wider station plates, and the horizontal
    /// bilingual plates when a translation is drawn (`wide_plates`).
    Enhanced,
}

impl HudLayout {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "original" => Some(Self::Original),
            "enhanced" => Some(Self::Enhanced),
            _ => None,
        }
    }
}

/// The extra canvas width (`SpriteLayer::wide`) of a canvas of multiple `scale` following a
/// `width × height` window: 0 at 4:3 and narrower.
pub fn wide_for(scale: usize, (width, height): (u32, u32), scaling: UiScaling) -> usize {
    let s = scale.max(1);
    wide_canvas_width(width, height, CANVAS_H * s, scaling).saturating_sub(CANVAS_W * s)
}

/// Fits `layer` to a `window`: the canvas multiple of `cfg` for its height and, with `wide`
/// (the enhanced layout, while driving), the window's shape. When the multiple changes the
/// translated text of `bank` is rasterised again at the new one and `cfg.ui_scale` follows.
/// Returns whether the canvas size changed (make a new canvas and compose again).
pub fn fit_layer(
    cfg: &mut LocaleConfig,
    data: &GameData,
    bank: &mut SpriteBank,
    layer: &mut SpriteLayer,
    window: (u32, u32),
    wide: bool,
) -> Result<bool> {
    fit_layer_with(cfg, data, bank, layer, window, wide, false, None)
}

/// [`fit_layer`]; with `keep_pending` a [`TextOverlay::pending`] text overlay stays so (no
/// window yet: the first fit with one builds it); `ahead`: the text built for a canvas
/// multiple ahead ([`text_ahead`]), used if it is this fit's.
#[allow(clippy::too_many_arguments)]
pub fn fit_layer_with(
    cfg: &mut LocaleConfig,
    data: &GameData,
    bank: &mut SpriteBank,
    layer: &mut SpriteLayer,
    window: (u32, u32),
    wide: bool,
    keep_pending: bool,
    ahead: Option<(usize, TextOverlay)>,
) -> Result<bool> {
    let before = layer.canvas_size();
    let s = cfg.multiple_for(window.1);
    if !(keep_pending && bank.text().is_some_and(TextOverlay::is_pending)) {
        rescale_text(cfg, data, bank, s, ahead)?;
    }
    layer.scale = s;
    cfg.ui_scale = s;
    layer.wide = if wide {
        wide_for(s, window, cfg.ui_scaling)
    } else {
        0
    };
    Ok(layer.canvas_size() != before)
}

/// Rasterises the translated text sprites of `bank` again for canvas multiple `scale`, when
/// they were made for another one (rare: only when a resize crosses a multiple), taking
/// `ahead` (built ahead, [`text_ahead`]) when it is for `scale`.
fn rescale_text(
    cfg: &LocaleConfig,
    data: &GameData,
    bank: &mut SpriteBank,
    scale: usize,
    ahead: Option<(usize, TextOverlay)>,
) -> Result<()> {
    if bank.text().is_none_or(|t| t.scale() == scale) {
        return Ok(());
    }
    let t = std::time::Instant::now();
    let exe_bytes = data.exe()?;
    let exe = Exe::parse(&exe_bytes)?;
    let mut c = cfg.clone();
    c.ui_scale = scale;
    match ahead.filter(|(s, _)| *s == scale) {
        Some((_, overlay)) => {
            crate::locale::install_built(&c, &exe, bank, overlay)?;
            eprintln!("ui: text for canvas multiple {scale} built during the GPU set-up");
        }
        None => {
            crate::locale::install(&c, &exe, bank)?;
            eprintln!(
                "ui: text rasterised for canvas multiple {scale} in {:.0?}",
                t.elapsed()
            );
        }
    }
    Ok(())
}

/// The fraction of a `height`-pixel window above the 3D view when the HUD canvas is presented
/// in `rect` and its own band starts at `band_top` of the canvas: `band_top` itself when the
/// canvas spans the height, lower when an integer-scaled canvas leaves bars above and below.
pub fn band_top(rect: [f32; 4], height: u32, band_top: f32) -> f32 {
    ((rect[1] + band_top * rect[3]) / height.max(1) as f32).clamp(0.0, 1.0)
}

/// The translated text of `bank` built for the canvas multiple of a `window` (what its first
/// fit would build), to build beside other start-up work; `None` with nothing to build.
pub fn text_ahead(
    cfg: &LocaleConfig,
    data: &GameData,
    bank: &SpriteBank,
    window: (u32, u32),
) -> Result<Option<(usize, TextOverlay)>> {
    let scale = cfg.multiple_for(window.1);
    let exe_bytes = data.exe()?;
    let exe = Exe::parse(&exe_bytes)?;
    let mut c = cfg.clone();
    c.ui_scale = scale;
    Ok(crate::locale::build_overlay(&c, &exe, bank)?.map(|o| (scale, o)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use shuppatsu_render::overlay::present_rect;

    #[test]
    fn wide_follows_the_layout_and_scaling() {
        // 16:10 at 2× smooth: 1536 × 960.
        assert_eq!(wide_for(2, (1280, 800), UiScaling::Smooth), 256);
        // 4:3 and narrower stay the original canvas.
        assert_eq!(wide_for(2, (1280, 960), UiScaling::Smooth), 0);
        assert_eq!(wide_for(1, (1000, 1000), UiScaling::Smooth), 0);
        // Integer at 1920×1080, multiple 2: 1× fits, so the canvas is the window's width.
        assert_eq!(wide_for(2, (1920, 1080), UiScaling::Integer), 1920 - 1280);
    }

    /// Integer scaling: the edge-anchored HUD fills the width of the whole-multiple area and
    /// the 3D band starts where the canvas's band does.
    #[test]
    fn integer_hud_band_lines_up() {
        let (w, h) = (1920, 1080);
        let s = 2;
        let canvas = (
            CANVAS_W * s + wide_for(s, (w, h), UiScaling::Integer),
            CANVAS_H * s,
        );
        let r = present_rect(w, h, canvas, UiScaling::Integer);
        assert_eq!(r, [0.0, 60.0, 1920.0, 960.0]);
        let top = band_top(r, h, 52.0 / 400.0);
        assert!((top * 1080.0 - (60.0 + 124.8)).abs() < 1e-3);
        // Smooth spans the height: the band is unchanged.
        let r = present_rect(w, h, (2560, 1440), UiScaling::Smooth);
        assert_eq!(band_top(r, h, 0.13), 0.13);
    }
}

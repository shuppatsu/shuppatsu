//! Free text for port-only 2D elements (enhancements): a string fitted into a rectangle of the
//! canvas, coloured from a palette at draw time (see [`crate::sprites::WidgetKind::Text`]).
//!
//! Unlike the overrides in [`crate::text`], which replace the art of an existing sprite, a
//! label has no art behind it: the caller picks the rectangle and the palette colour. The font
//! is installed on the [`crate::sprites::SpriteBank`] with the language; without one, labels
//! draw nothing.

use crate::text::{Align, GlyphSource, rasterise_sized};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Cache key: text, mask size, cap height (bits of the f32) in canvas pixels, alignment and
/// rotation.
type Key = (String, usize, usize, Option<u32>, Align, bool);

/// A font for labels, with the coverage masks rendered so far (labels are redrawn every
/// frame, so each distinct string and size is rasterised once).
pub struct LabelFont {
    glyphs: Box<dyn GlyphSource + Send + Sync>,
    cache: Mutex<HashMap<Key, Arc<[u8]>>>,
}

impl LabelFont {
    pub fn new(glyphs: Box<dyn GlyphSource + Send + Sync>) -> Self {
        Self {
            glyphs,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// `text` in a `w × h` coverage mask (row-major, 0..=255), uniformly scaled and centred:
    /// capitals `cap_px` high, or smaller if that does not fit; `None` = the largest size that
    /// fits ([`rasterise_sized`]); `align` places it along the line; `vertical` turns it 90°
    /// clockwise (reads top to bottom).
    pub fn mask(
        &self,
        text: &str,
        w: usize,
        h: usize,
        cap_px: Option<f32>,
        align: Align,
        vertical: bool,
    ) -> Arc<[u8]> {
        let key = (
            text.to_owned(),
            w,
            h,
            cap_px.map(f32::to_bits),
            align,
            vertical,
        );
        let lock = || self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(m) = lock().get(&key) {
            return m.clone();
        }
        // Rasterised outside the lock: labels are rasterised on several threads at once
        // (`SpriteLayer::label_rasters`). The same key gives the same mask.
        let g = &*self.glyphs;
        let m: Arc<[u8]> = rasterise_sized(text, w, h, align, vertical, g, cap_px).into();
        lock().entry(key).or_insert(m).clone()
    }

    /// The width of the widest line of `text` with capitals `cap` high, in the unit of `cap`
    /// (as [`rasterise_sized`] sizes it, before any fitting).
    pub fn width(&self, text: &str, cap: f32) -> f32 {
        let g = &*self.glyphs;
        let reference = 100.0;
        let h_ref = g.glyph('H', reference).height as f32;
        if h_ref <= 0.0 {
            return 0.0;
        }
        let px = cap * reference / h_ref;
        text.lines()
            .map(|l| l.chars().map(|c| g.advance(c, px)).sum::<f32>())
            .fold(0.0, f32::max)
    }

    /// The ink of the one-line `text` with capitals `cap` high: `(top, bottom)` relative to
    /// the baseline (y down, so `top` is negative), in the unit of `cap`. A caller placing a
    /// line by its baseline needs it, because a text widget centres the ink in its rectangle.
    pub fn ink(&self, text: &str, cap: f32) -> (f32, f32) {
        let g = &*self.glyphs;
        let reference = 100.0;
        let h_ref = g.glyph('H', reference).height as f32;
        if h_ref <= 0.0 {
            return (0.0, 0.0);
        }
        let (mut top, mut bottom) = (f32::MAX, f32::MIN);
        for ch in text.chars() {
            let gl = g.glyph(ch, reference);
            if gl.height == 0 {
                continue;
            }
            top = top.min(-((gl.ymin + gl.height as i32) as f32));
            bottom = bottom.max(-(gl.ymin as f32));
        }
        if top > bottom {
            return (0.0, 0.0);
        }
        let k = cap / h_ref;
        (top * k, bottom * k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::tests::BoxGlyphs;

    #[test]
    fn masks_are_cached_per_size() {
        let f = LabelFont::new(Box::new(BoxGlyphs));
        let a = f.mask("Tokyo", 60, 12, None, Align::Center, false);
        assert_eq!(a.len(), 60 * 12);
        assert!(a.iter().any(|&c| c > 0));
        assert!(Arc::ptr_eq(
            &a,
            &f.mask("Tokyo", 60, 12, None, Align::Center, false)
        ));
        assert!(!Arc::ptr_eq(
            &a,
            &f.mask("Tokyo", 120, 24, None, Align::Center, false)
        ));
    }

    /// Box glyphs 72 rows tall at the reference size, one row below the baseline: the ink
    /// is the H box itself, scaled to `cap`.
    #[test]
    fn ink_is_relative_to_the_baseline() {
        let f = LabelFont::new(Box::new(BoxGlyphs));
        let (t, b) = f.ink("Tokyo", 10.0);
        assert!((t + 710.0 / 72.0).abs() < 1e-4, "{t}");
        assert!((b - 10.0 / 72.0).abs() < 1e-4, "{b}");
        assert!((b - t - 10.0).abs() < 1e-4);
        assert_eq!(f.ink(" ", 10.0), (0.0, 0.0));
    }
}

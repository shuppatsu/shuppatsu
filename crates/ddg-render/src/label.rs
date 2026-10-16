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

/// Cache key: text, mask size, cap height (bits of the f32) in canvas pixels and alignment.
type Key = (String, usize, usize, Option<u32>, Align);

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
    /// fits ([`rasterise_sized`]); `align` places it horizontally.
    pub fn mask(
        &self,
        text: &str,
        w: usize,
        h: usize,
        cap_px: Option<f32>,
        align: Align,
    ) -> Arc<[u8]> {
        let key = (text.to_owned(), w, h, cap_px.map(f32::to_bits), align);
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache
            .entry(key)
            .or_insert_with(|| {
                let g = &*self.glyphs;
                rasterise_sized(text, w, h, align, false, g, cap_px).into()
            })
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::tests::BoxGlyphs;

    #[test]
    fn masks_are_cached_per_size() {
        let f = LabelFont::new(Box::new(BoxGlyphs));
        let a = f.mask("Tokyo", 60, 12, None, Align::Center);
        assert_eq!(a.len(), 60 * 12);
        assert!(a.iter().any(|&c| c > 0));
        assert!(Arc::ptr_eq(
            &a,
            &f.mask("Tokyo", 60, 12, None, Align::Center)
        ));
        assert!(!Arc::ptr_eq(
            &a,
            &f.mask("Tokyo", 120, 24, None, Align::Center)
        ));
    }
}

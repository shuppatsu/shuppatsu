//! The original's sprite-slot calls (docs/re/hud.md §1.1) on a [`SpriteLayer`].
//!
//! The original resets slots whose keep flag is 0 only at screen transitions (0x4316b0 is
//! called from 0x434a00, 0x42a1c0 and 0x4253c0's debug path, not every tick), so a slot keeps
//! its image until something changes it. The port's per-tick [`SpriteLayer::reset_transient`]
//! would wipe such slots, so every call here marks the slot kept.

use shuppatsu_render::sprites::{Slot, SpriteLayer};

/// `0x431790`: off screen, layer 3, no image.
pub fn clear(layer: &mut SpriteLayer, slot: usize) {
    layer.slots[slot] = Slot {
        keep: true,
        ..Slot::CLEAR
    };
}

/// `0x431850`: position, layer and palette; the image stays.
pub fn place(layer: &mut SpriteLayer, slot: usize, x: i16, y: i16, l: u8, palette: u8) {
    let s = &mut layer.slots[slot];
    s.x = x;
    s.y = y;
    s.layer = l;
    s.palette = palette;
    s.keep = true;
}

/// `0x431880`: position only.
pub fn move_to(layer: &mut SpriteLayer, slot: usize, x: i16, y: i16) {
    let s = &mut layer.slots[slot];
    s.x = x;
    s.y = y;
    s.keep = true;
}

/// `0x4318c0`: the image (−1 hides), whole map.
pub fn image(layer: &mut SpriteLayer, slot: usize, tbl: i16) {
    let s = &mut layer.slots[slot];
    s.tilemap = tbl;
    s.col = 0;
    s.row = 0;
    s.w = 0;
    s.h = 0;
    s.keep = true;
}

/// `0x431920`: image and palette.
pub fn image_palette(layer: &mut SpriteLayer, slot: usize, tbl: i16, palette: u8) {
    image(layer, slot, tbl);
    layer.slots[slot].palette = palette;
}

/// `0x4316e0`: everything.
pub fn set(layer: &mut SpriteLayer, slot: usize, tbl: i16, (x, y): (i16, i16), palette: u8, l: u8) {
    layer.slots[slot] = Slot::new(tbl, x, y, palette, l, true);
}

/// `0x436770(4, first, n, colours)`: palette 4 entries `first..first + n` from `colours`.
pub fn palette4(layer: &mut SpriteLayer, first: usize, colours: &[u32]) {
    if let Some(Some(p)) = layer.palettes.get_mut(4) {
        for (i, &c) in colours.iter().enumerate() {
            if let Some(e) = p.0.get_mut(first + i) {
                *e = c;
            }
        }
    }
}

//! Stage-flow screens (docs/re/menus.md §8–§11): pause dialogs, the station result, the continue
//! countdown, the report card, the ranking insert with name entry and the run-end banner, plus [`flow::StageFlow`], which sequences
//! them around a [`shuppatsu_engine::drive::Drive`] run with the original's tick gaps.
//!
//! Every screen is a pure state machine: one `tick` per 33 ms `Task_Tick` with the button edges
//! the original's task would see (polled at the end of the previous tick), sound requests pushed
//! to a list, and a `draw` that writes its sprite slots into the [`SpriteLayer`].

pub mod continue_screen;
pub mod flow;
pub mod pause;
pub mod ranking;
pub mod report;
pub mod result;
pub mod run_end;
pub mod tables;

use shuppatsu_render::sprites::{Slot, SpriteLayer};
use std::collections::BTreeMap;
use tables::{Placement, TextDesc};

/// `hybrid_input` with a gamepad (input.md §10.6): the words beside the Xbox glyphs on the stage
/// flow's screens (the `[ui]` strings of the language).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PadHints {
    pub select: String,
    pub confirm: String,
    pub resume: String,
}

/// Button edges of one tick (the pressed word `0x48c42c`, input.md §6).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MenuInput {
    /// 0x1000: Enter / pad A.
    pub confirm: bool,
    /// 0x2000: Esc / pad B.
    pub cancel: bool,
    /// 0x800000: Esc / pad button 2.
    pub pause: bool,
    /// 0x10 / 0x20: cursor up / down (press, then repeat pulses).
    pub up: bool,
    pub down: bool,
    /// 0x80 / 0x40: cursor left / right pulses (no menu of the original reads them; the port
    /// settings screen changes a value with them).
    pub left: bool,
    pub right: bool,
    /// 0x100: horn (Space).
    pub horn: bool,
    /// 0x4000: the R key (route select: replay playback toggle).
    pub key_r: bool,
    /// 0x100000: the driving A button (Enter in keyboard modes); ends the ranking page.
    pub a: bool,
    /// Held bits 4 / 8 (`0x48c428`): left / right, for the name-entry wheel.
    pub left_held: bool,
    pub right_held: bool,
    /// Analog-lever modes: the power notch, which the dialog cursor follows (menus.md §1.4).
    pub notch: Option<u8>,
    /// Analog-lever modes: the brake notch (`0x829d04`), the name-entry wheel speed.
    pub brake: Option<u8>,
}

impl MenuInput {
    /// The edges and held bits of a button word (set bit = pressed / held) and, in analog-lever
    /// modes, the (power, brake) notches.
    pub fn from_word(word: &crate::input::ButtonWord, levers: Option<(u8, u8)>) -> Self {
        use crate::input::bits;
        let on = |b: u32| word.pressed & b != 0;
        Self {
            confirm: on(bits::MENU_CONFIRM),
            cancel: on(bits::CANCEL),
            pause: on(bits::PAUSE),
            up: on(bits::UP_PULSE),
            down: on(bits::DOWN_PULSE),
            left: on(bits::LEFT_PULSE),
            right: on(bits::RIGHT_PULSE),
            horn: on(bits::HORN),
            a: on(bits::A),
            key_r: on(bits::KEY_R),
            left_held: word.held & bits::LEFT != 0,
            right_held: word.held & bits::RIGHT != 0,
            notch: levers.map(|l| l.0),
            brake: levers.map(|l| l.1),
        }
    }

    pub const NONE: Self = Self {
        confirm: false,
        cancel: false,
        pause: false,
        up: false,
        down: false,
        left: false,
        right: false,
        horn: false,
        a: false,
        key_r: false,
        left_held: false,
        right_held: false,
        notch: None,
        brake: None,
    };
}

/// A sound command from a screen: `Sfx_Play` 0x41fda0 / `Snd_Play` 0x430100 (codes with 0x1000
/// set are jingle sequences, 0x41fdd0) or `Snd_Stop` 0x420650.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sfx {
    Play(u16),
    Stop(u16),
}

/// The sprite slots one screen owns, set through the original's slot calls; drawn every tick.
#[derive(Debug, Clone, Default)]
pub struct SlotSet {
    slots: BTreeMap<usize, Slot>,
    /// Slots owned by the HUD that the screen only re-images (0x4318c0 / 0x431920 on the
    /// remaining-time digits): tilemap and, if given, palette.
    patches: BTreeMap<usize, (i16, Option<u8>)>,
}

impl SlotSet {
    fn slot(&mut self, slot: usize) -> &mut Slot {
        self.slots.entry(slot).or_insert(Slot::CLEAR)
    }

    /// `Sprite_Place` 0x431850: position, layer and palette; the image is kept.
    pub fn place(&mut self, p: &Placement) {
        let Ok(slot) = usize::try_from(p.slot) else {
            return;
        };
        self.place_at(slot, p.x, p.y, p.layer as u8, p.palette as u8);
    }

    pub fn place_at(&mut self, slot: usize, x: i16, y: i16, layer: u8, palette: u8) {
        let s = self.slot(slot);
        s.x = x;
        s.y = y;
        s.layer = layer;
        s.palette = palette;
    }

    /// 0x4318c0: the whole of tilemap `tbl` (−1 hides).
    pub fn image(&mut self, slot: usize, tbl: i16) {
        let s = self.slot(slot);
        s.tilemap = tbl;
        (s.col, s.row, s.w, s.h) = (0, 0, 0, 0);
    }

    /// 0x431920: image and palette.
    pub fn image_pal(&mut self, slot: usize, tbl: i16, palette: u8) {
        self.image(slot, tbl);
        self.slot(slot).palette = palette;
    }

    /// 0x4316e0: everything at once.
    pub fn full(&mut self, slot: usize, tbl: i16, x: i16, y: i16, palette: u8, layer: u8) {
        *self.slot(slot) = Slot::new(tbl, x, y, palette, layer, false);
    }

    /// 0x431730: a label cut out of a string sheet.
    pub fn text(&mut self, slot: usize, d: TextDesc, x: i16, y: i16, palette: u8, layer: u8) {
        *self.slot(slot) = Slot {
            col: d.col,
            row: d.row,
            w: d.w,
            h: d.h,
            ..Slot::new(d.tbl, x, y, palette, layer, false)
        };
    }

    /// Re-images a HUD slot without moving it.
    pub fn patch(&mut self, slot: usize, tbl: i16, palette: Option<u8>) {
        self.patches.insert(slot, (tbl, palette));
    }

    pub fn clear(&mut self) {
        self.slots.clear();
        self.patches.clear();
    }

    /// The slots this set owns (not the HUD patches).
    pub fn indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.slots.keys().copied()
    }

    pub fn get(&self, slot: usize) -> Option<&Slot> {
        self.slots.get(&slot)
    }

    #[cfg(test)]
    pub fn patched(&self, slot: usize) -> Option<(i16, Option<u8>)> {
        self.patches.get(&slot).copied()
    }

    /// Writes the slots into the layer (after the HUD, so they win).
    pub fn draw(&self, layer: &mut SpriteLayer) {
        for (&i, s) in &self.slots {
            if let Some(dst) = layer.slots.get_mut(i) {
                *dst = *s;
            }
        }
        for (&i, &(tbl, pal)) in &self.patches {
            if let Some(dst) = layer.slots.get_mut(i) {
                dst.tilemap = tbl;
                (dst.col, dst.row, dst.w, dst.h) = (0, 0, 0, 0);
                if let Some(p) = pal {
                    dst.palette = p;
                }
            }
        }
    }
}

/// Decimal digits of `v` (0..=999), most significant first.
pub fn digits3(v: i32) -> [usize; 3] {
    let v = v.clamp(0, 999) as usize;
    [v / 100, v / 10 % 10, v % 10]
}

/// The clear colour `Gfx_SetClearColour(0x70, 0x7070)` (0x428990) of the ranking page and the
/// records viewer (menus.md §5, §11.2): with no 3D task left, the whole 640×480 frame is this
/// grey under the 2D layer. Measured 115,113,115 in a 16-bit capture of the original.
pub const RANKING_GREY: [u8; 3] = [0x70, 0x70, 0x70];

/// HUD remaining-time digit slots 0x18–0x1a (hud.md §2), reused by the result and continue
/// screens.
pub const SLOT_REMAINING: usize = 0x18;
/// The HUD's remaining-time masks (flicker frames), hud.md §3.2.
pub const TBL_MASKS: [i16; 2] = [0x40b, 0x40c];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_calls_compose() {
        let mut s = SlotSet::default();
        s.place_at(5, 10, 20, 3, 4);
        s.image(5, 0x100);
        assert_eq!(
            s.get(5).map(|s| (s.x, s.y, s.tilemap)),
            Some((10, 20, 0x100))
        );
        s.patch(0x18, 0x40b, Some(5));
        let mut layer = SpriteLayer::default();
        layer.slots[0x18] = Slot::new(1, 241, 56, 4, 2, true);
        s.draw(&mut layer);
        let d = layer.slots[0x18];
        assert_eq!(
            (d.x, d.y, d.tilemap, d.palette, d.keep),
            (241, 56, 0x40b, 5, true)
        );
        assert_eq!(digits3(507), [5, 0, 7]);
        assert_eq!(digits3(1500), [9, 9, 9]);
    }

    /// The grey goes under the 2D layer in the centred 4:3 rectangle only.
    #[test]
    fn backdrop_fills_under_the_design_rect() {
        let layer = SpriteLayer {
            wide: 100,
            ..SpriteLayer::default()
        };
        let mut canvas = layer.new_canvas();
        let cw = layer.canvas_size().0;
        let px = |x: usize, y: usize| 4 * (y * cw + x);
        // An opaque texel and a half-covered (premultiplied) one inside, one outside.
        canvas[px(60, 10)..px(60, 10) + 4].copy_from_slice(&[200, 0, 0, 255]);
        canvas[px(61, 10)..px(61, 10) + 4].copy_from_slice(&[100, 0, 0, 128]);
        let (cw, ch) = layer.canvas_size();
        layer.put_backdrop(&mut canvas, RANKING_GREY, [0, 0, cw, ch]);
        assert_eq!(canvas[px(49, 0)..px(49, 0) + 4], [0; 4]);
        assert_eq!(canvas[px(50, 0)..px(50, 0) + 4], [0x70, 0x70, 0x70, 255]);
        assert_eq!(
            canvas[px(689, 479)..px(689, 479) + 4],
            [0x70, 0x70, 0x70, 255]
        );
        assert_eq!(canvas[px(690, 479)..px(690, 479) + 4], [0; 4]);
        assert_eq!(canvas[px(60, 10)..px(60, 10) + 4], [200, 0, 0, 255]);
        assert_eq!(canvas[px(61, 10)..px(61, 10) + 4], [155, 55, 55, 255]);
    }
}

//! On-screen button hints of `hybrid_input` (input.md §10.6): which legends a screen shows,
//! and the Xbox-layout glyphs drawn in code (no image assets) for a gamepad.
//!
//! The face buttons are round, in the Xbox colours with their letter; the shoulder, trigger
//! and centre buttons are rounded keys with their name; the d-pad is a cross with the arms in
//! use lit; the left stick a ring with "L". They are sized from the legend text's cap height
//! ([`ddg_render::sprites::legend::GLYPH_PER_CAP`]).

use crate::hybrid::HintDevice;
use ddg_render::sprites::legend::{Glyph, GlyphShape, LegendPart};
use ddg_render::sprites::{Paint, SpriteLayer, TextStyle, WidgetKind};
use ddg_render::text::Align;

/// Which legends the screens show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Hint {
    /// The keyboard art (control modes 0–3 in the original).
    #[default]
    Keys,
    /// The original's pad art (mode 4).
    Pad,
    /// The original's lever art (modes 5/6).
    Levers,
    /// Xbox-layout glyphs drawn in code (`hybrid_input`, the last device used is a gamepad).
    Glyphs,
}

impl Hint {
    /// The original's choice: by control mode.
    pub fn for_mode(mode: u8) -> Self {
        match mode {
            4 => Self::Pad,
            5 | 6 => Self::Levers,
            _ => Self::Keys,
        }
    }

    /// With `hybrid_input` (`device` is `Some`) the hints follow the last device used: the
    /// keyboard art for the keyboard, the glyphs for a gamepad, except in the analog modes
    /// 5/6, whose physical handles keep the original's lever art. Without it (`None`), the
    /// original's art for the control mode.
    pub fn new(mode: u8, device: Option<HintDevice>) -> Self {
        match device {
            None => Self::for_mode(mode),
            Some(HintDevice::Keyboard) => Self::Keys,
            Some(HintDevice::Pad) if matches!(mode, 5 | 6) => Self::Levers,
            Some(HintDevice::Pad) => Self::Glyphs,
        }
    }
}

/// A gamepad button, as its glyph shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Btn {
    A,
    B,
    X,
    Y,
    Lb,
    Rb,
    Lt,
    Rt,
    Menu,
    View,
    /// The d-pad with the arms (up, down, left, right) in use lit.
    DPad([bool; 4]),
    LStick,
}

/// D-pad arms: up/down, left/right.
pub const DPAD_UD: Btn = Btn::DPad([true, true, false, false]);
pub const DPAD_LR: Btn = Btn::DPad([false, false, true, true]);

/// A paint outside the palette table: legend words in white, a legend backdrop near-black
/// (for screens without the menu palette).
pub const UNPALETTED: Paint = Paint {
    palette: u8::MAX,
    index: 0,
};

const WHITE: [u8; 3] = [0xff, 0xff, 0xff];
/// Key bodies: dark grey, like the controller's plastic.
const KEY_GREY: [u8; 3] = [0x3c, 0x3f, 0x44];

/// The glyph of `b`: the Xbox colours (A green, B red, X blue, Y yellow).
pub fn glyph(b: Btn) -> Glyph {
    let round = |label: &str, fill: [u8; 3], ink: [u8; 3]| Glyph {
        shape: GlyphShape::Round,
        label: label.into(),
        fill,
        ink,
    };
    let key = |label: &str| Glyph {
        shape: GlyphShape::Key,
        label: label.into(),
        fill: KEY_GREY,
        ink: WHITE,
    };
    match b {
        Btn::A => round("A", [0x5c, 0xb8, 0x2d], WHITE),
        Btn::B => round("B", [0xe2, 0x3a, 0x31], WHITE),
        Btn::X => round("X", [0x2a, 0x77, 0xe0], WHITE),
        Btn::Y => round("Y", [0xf2, 0xc0, 0x1e], [0x2a, 0x22, 0x08]),
        Btn::Lb => key("LB"),
        Btn::Rb => key("RB"),
        Btn::Lt => key("LT"),
        Btn::Rt => key("RT"),
        Btn::Menu => key("Menu"),
        Btn::View => key("View"),
        Btn::DPad(lit) => Glyph {
            shape: GlyphShape::DPad { lit },
            label: String::new(),
            fill: [0x5a, 0x5e, 0x66],
            ink: WHITE,
        },
        Btn::LStick => Glyph {
            shape: GlyphShape::Stick,
            label: "L".into(),
            fill: [0x5a, 0x5e, 0x66],
            ink: WHITE,
        },
    }
}

/// A legend line under construction: glyphs and words, left to right.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line(pub Vec<LegendPart>);

impl Line {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn btn(mut self, b: Btn) -> Self {
        self.0.push(LegendPart::Glyph(glyph(b)));
        self
    }

    pub fn text(mut self, t: &str) -> Self {
        self.0.push(LegendPart::Text(t.to_owned()));
        self
    }

    /// Space between two entries, in cap heights.
    pub fn gap(mut self, k: f32) -> Self {
        self.0.push(LegendPart::Gap(k));
        self
    }

    /// Adds the line as a widget in `rect` (original pixels): words in `paint`, capitals
    /// `cap` high (glyphs taller), placed along `align`.
    pub fn draw(
        self,
        l: &mut SpriteLayer,
        layer: u8,
        rect: [f32; 4],
        paint: Paint,
        cap: f32,
        align: Align,
    ) {
        self.draw_styled(l, layer, rect, paint, cap, align, None);
    }

    /// [`Line::draw`] on a rounded near-black box as wide as the line (for a line over the
    /// 3D view).
    pub fn draw_boxed(
        self,
        l: &mut SpriteLayer,
        layer: u8,
        rect: [f32; 4],
        paint: Paint,
        cap: f32,
        align: Align,
    ) {
        self.draw_styled(l, layer, rect, paint, cap, align, Some(UNPALETTED));
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_styled(
        self,
        l: &mut SpriteLayer,
        layer: u8,
        rect: [f32; 4],
        paint: Paint,
        cap: f32,
        align: Align,
        backdrop: Option<Paint>,
    ) {
        let mut style = TextStyle::plain(paint, Some(cap));
        style.align = align;
        style.backdrop = backdrop;
        l.add_widget(
            layer,
            rect,
            WidgetKind::Legend {
                parts: self.0,
                style,
            },
        );
    }
}

/// One glyph alone, centred in the square of side `size` around `(cx, cy)`.
pub fn draw_glyph(l: &mut SpriteLayer, layer: u8, b: Btn, (cx, cy): (f32, f32), size: f32) {
    let cap = size / ddg_render::sprites::legend::GLYPH_PER_CAP;
    let w = size * 2.0;
    Line::new().btn(b).draw(
        l,
        layer,
        [cx - w / 2.0, cy - size / 2.0, w, size],
        UNPALETTED,
        cap,
        Align::Center,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_follow_the_device() {
        // Hybrid off: the original's art by control mode.
        assert_eq!(Hint::new(0, None), Hint::Keys);
        assert_eq!(Hint::new(4, None), Hint::Pad);
        assert_eq!(Hint::new(6, None), Hint::Levers);
        // Hybrid on: the device decides; the analog modes keep their lever art for a pad.
        assert_eq!(Hint::new(4, Some(HintDevice::Keyboard)), Hint::Keys);
        assert_eq!(Hint::new(0, Some(HintDevice::Pad)), Hint::Glyphs);
        assert_eq!(Hint::new(4, Some(HintDevice::Pad)), Hint::Glyphs);
        assert_eq!(Hint::new(5, Some(HintDevice::Pad)), Hint::Levers);
    }

    #[test]
    fn xbox_colours() {
        assert_eq!(glyph(Btn::A).label, "A");
        assert!(glyph(Btn::A).fill[1] > glyph(Btn::A).fill[0]);
        assert!(glyph(Btn::B).fill[0] > glyph(Btn::B).fill[1]);
        assert!(glyph(Btn::X).fill[2] > glyph(Btn::X).fill[0]);
        assert_eq!(glyph(Btn::Rt).shape, GlyphShape::Key);
    }
}

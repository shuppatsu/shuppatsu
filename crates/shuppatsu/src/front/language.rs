//! The first-run language screen (port only; docs/localisation.md, "Settings").
//!
//! Shown before attract when no language comes from a flag, the environment or
//! `config.toml`: "Language / 言語" over two choices, English and 日本語, picked with the
//! title menu's cursor frame, keys and sounds (menus.md §1.4, §3). The choice is written to
//! `config.toml` by the app and only sets the language.

use super::{PAL_MENU, ScrollList, snd};
use crate::screens::{MenuInput, Sfx};
use shuppatsu_formats::cg::Palette;
use shuppatsu_render::sprites::{Paint, SpriteLayer, TextStyle};

/// Layer of the cursor frame and the text.
const LAYER: u8 = 3;
/// The title menu's geometry (0x42b530): first item y, cursor frame y, item x offset from
/// the frame; rows [`super::ROW`] apart.
const ITEM_Y: i32 = 0x8e;
const FRAME_Y: i32 = 0x86;
const ITEM_DX: f32 = 20.0;
/// Everything this much lower than on the title menu, to centre the two rows.
const DOWN: i32 = 30;
/// The heading's box and capital height, original pixels.
const HEADING: [f32; 4] = [0.0, 84.0, 640.0, 40.0];
const HEADING_CAP: f32 = 22.0;
const ITEM_CAP: f32 = 26.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    English,
    Japanese,
}

impl Language {
    /// In cursor order.
    pub const ALL: [Self; 2] = [Self::English, Self::Japanese];

    /// The catalogue code ([`crate::locale::LocaleConfig::lang`]).
    pub fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Japanese => crate::locale::IDENTITY_LANG,
        }
    }

    /// The choice's label, in its own language.
    pub fn label(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Japanese => "日本語",
        }
    }
}

/// Colours and the frame's place, read from the user's data by the app.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    /// The centred cursor frame's x ([`super::cursor_x`]) and size.
    pub cursor_x: i16,
    pub frame: (i16, i16),
    pub bright: Paint,
    pub dim: Paint,
}

impl Look {
    /// White and grey from the menu palette (nearest colours; index 0 is transparent).
    pub fn new(cursor_x: i16, frame: (i16, i16), menu: Option<&Palette>) -> Self {
        let paint = |rgb| Paint {
            palette: PAL_MENU,
            index: menu.map_or(1, |p| nearest(p, rgb)),
        };
        Self {
            cursor_x,
            frame,
            bright: paint([255, 255, 255]),
            dim: paint([128, 128, 128]),
        }
    }
}

/// The palette index (1..=255) closest to `rgb`.
pub(super) fn nearest(p: &Palette, rgb: [u8; 3]) -> u8 {
    (1..=255u8)
        .min_by_key(|&i| {
            let c = p.rgba(i);
            (0..3)
                .map(|k| (i32::from(c[k]) - i32::from(rgb[k])).pow(2))
                .sum::<i32>()
        })
        .unwrap_or(1)
}

#[derive(Debug, Clone, Default)]
pub struct LanguageScreen {
    list: ScrollList,
    /// The decide blink countdown, as the title menu's.
    blink: i32,
}

impl LanguageScreen {
    /// The cursor starts on English.
    pub fn new() -> Self {
        Self::default()
    }

    fn last() -> i32 {
        Language::ALL.len() as i32 - 1
    }

    pub fn cursor(&self) -> Language {
        Language::ALL[self.list.cursor.clamp(0, Self::last()) as usize]
    }

    /// One tick: cursor up/down (or the power lever), confirm blinks the frame like the title
    /// menu and returns the choice when the blink ends. Cancel does nothing: there is nothing
    /// to go back to.
    pub fn tick(
        &mut self,
        input: &MenuInput,
        analog: bool,
        sfx: &mut Vec<Sfx>,
    ) -> Option<Language> {
        if self.blink != 0 {
            self.blink -= 1;
            if self.blink == 0 {
                sfx.push(Sfx::Stop(snd::DECIDE));
                return Some(self.cursor());
            }
        } else if self.list.distance() != 0 {
            self.list.approach(analog);
        } else if input.confirm {
            sfx.push(Sfx::Play(snd::DECIDE));
            self.blink = super::title::BLINK_TICKS;
        } else if self.list.step(input, Self::last()) {
            sfx.push(Sfx::Play(snd::MOVE));
        }
        None
    }

    /// The heading, the two labels (the one under the cursor bright) and the cursor frame.
    pub fn draw(&self, look: &Look, l: &mut SpriteLayer) {
        let mut heading = TextStyle::plain(look.bright, Some(HEADING_CAP));
        heading.font = shuppatsu_render::text::FontRole::Display;
        l.add_styled_text(LAYER, HEADING, "Language / 言語", heading);
        let (fw, fh) = look.frame;
        let x = f32::from(look.cursor_x) + ITEM_DX;
        let w = (f32::from(fw) - 2.0 * ITEM_DX).max(1.0);
        // The frame starts 8 pixels above the item (0x86 vs 0x8e).
        let h = (f32::from(fh) - 16.0).max(ITEM_CAP);
        for (i, lang) in Language::ALL.into_iter().enumerate() {
            let y = (ITEM_Y + DOWN + super::ROW * i as i32 + self.list.scroll) as f32;
            let paint = if self.list.cursor == i as i32 {
                look.bright
            } else {
                look.dim
            };
            l.add_text(LAYER, [x, y, w, h], lang.label(), paint, Some(ITEM_CAP));
        }
        // The frame alone (no more-above/below arrows for two items), hidden while it
        // blinks like the title menu's (`blink % 6 > 3`).
        if self.blink % 6 > 3 {
            super::hide(l, super::slot::CURSOR);
        } else {
            let y = (FRAME_Y + DOWN + self.list.frame) as i16;
            let (x, tbl) = (look.cursor_x, super::tbl::CURSOR);
            super::put(l, super::slot::CURSOR, tbl, x, y, PAL_MENU, LAYER);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(f: impl FnOnce(&mut MenuInput)) -> MenuInput {
        let mut i = MenuInput::NONE;
        f(&mut i);
        i
    }

    /// Ticks until the cursor frame has settled.
    fn settle(s: &mut LanguageScreen) {
        for _ in 0..20 {
            s.tick(&MenuInput::NONE, false, &mut Vec::new());
        }
    }

    #[test]
    fn cursor_moves_without_wrapping() {
        let mut s = LanguageScreen::new();
        assert_eq!(s.cursor(), Language::English);
        let mut sfx = Vec::new();
        s.tick(&press(|i| i.up = true), false, &mut sfx);
        assert_eq!((s.cursor(), sfx.len()), (Language::English, 0), "no wrap");
        s.tick(&press(|i| i.down = true), false, &mut sfx);
        assert_eq!(s.cursor(), Language::Japanese);
        assert_eq!(sfx, [Sfx::Play(snd::MOVE)]);
        settle(&mut s);
        let mut sfx = Vec::new();
        s.tick(&press(|i| i.down = true), false, &mut sfx);
        assert_eq!((s.cursor(), sfx.len()), (Language::Japanese, 0));
        s.tick(&press(|i| i.up = true), false, &mut sfx);
        assert_eq!(s.cursor(), Language::English);
    }

    #[test]
    fn confirm_blinks_then_chooses() {
        let mut s = LanguageScreen::new();
        s.tick(&press(|i| i.down = true), false, &mut Vec::new());
        // Confirm waits for the frame to arrive.
        s.tick(&press(|i| i.confirm = true), false, &mut Vec::new());
        assert_eq!(s.blink, 0);
        settle(&mut s);
        let mut sfx = Vec::new();
        assert_eq!(s.tick(&press(|i| i.confirm = true), false, &mut sfx), None);
        assert_eq!(sfx, [Sfx::Play(snd::DECIDE)]);
        for _ in 1..super::super::title::BLINK_TICKS {
            // Input is ignored while blinking.
            let up = press(|i| i.up = true);
            assert_eq!(s.tick(&up, false, &mut Vec::new()), None);
        }
        let mut sfx = Vec::new();
        assert_eq!(
            s.tick(&MenuInput::NONE, false, &mut sfx),
            Some(Language::Japanese)
        );
        assert_eq!(sfx, [Sfx::Stop(snd::DECIDE)]);
    }

    #[test]
    fn cancel_does_nothing_and_codes() {
        let mut s = LanguageScreen::new();
        let mut sfx = Vec::new();
        assert_eq!(s.tick(&press(|i| i.cancel = true), false, &mut sfx), None);
        assert!(sfx.is_empty());
        assert_eq!(Language::English.code(), "en");
        assert_eq!(Language::Japanese.code(), "ja");
        assert_eq!(Language::Japanese.label(), "日本語");
    }

    #[test]
    fn draws_heading_labels_and_frame() {
        let s = LanguageScreen::new();
        let look = Look::new(100, (440, 96), None);
        let mut l = SpriteLayer::default();
        s.draw(&look, &mut l);
        let texts: Vec<_> = l
            .widgets
            .iter()
            .filter_map(|w| match &w.kind {
                shuppatsu_render::sprites::WidgetKind::Text { text, style } => {
                    Some((text.as_str(), style.paint))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            texts,
            [
                ("Language / 言語", look.bright),
                ("English", look.bright),
                ("日本語", look.dim),
            ]
        );
        assert_eq!(
            l.slots[super::super::slot::CURSOR].tilemap,
            super::super::tbl::CURSOR
        );
    }
}

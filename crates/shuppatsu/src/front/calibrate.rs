//! 専用コントローラー設定: the controller calibration screen (`ControllerAdjust_Run` 0x434a00,
//! input.md §5, menus.md §3), title item 4 with the Densha de GO! controller (control mode 6).
//!
//! Six mascon prompts (tbl 0x4b1..0x4b6, slot 0x90) sample the Y axis, then ten brake prompts
//! (tbl 0x4a7..0x4b0, slot 0x91) sample X, each on a confirm edge; cancel aborts without saving.
//! After each sample slot 0x92 shows the next instruction (tbl 0x52d.. ). The sampling itself
//! is [`crate::gamepad::Calibrator`].

use super::tables::FrontTables;
use super::{PAL_MENU, snd};
use crate::gamepad::{CalStep, Calibration, Calibrator, JoyState};
use crate::input::ButtonState;
use crate::screens::{MenuInput, Sfx};
use shuppatsu_render::sprites::SpriteLayer;

const SLOT_MASCON: usize = 0x90;
const SLOT_BRAKE: usize = 0x91;
const SLOT_TEXT: usize = 0x92;
/// First prompts and their positions (0x434a00 immediates, x0/y0-relative).
const TBL_MASCON: i16 = 0x4b1;
const TBL_BRAKE: i16 = 0x4a7;
const TBL_FIRST_TEXT: i16 = 0x52c;
const MASCON_POS: (i16, i16) = (0xb0, 0xb0);
const BRAKE_POS: (i16, i16) = (0x138, 0xb0);
const TEXT_POS: (i16, i16) = (0x50, 0x160);
/// The instruction after sample n is tbl 0x52d + n (15 entries; after the last brake sample
/// the original reads past its table, but clears the screen in the same tick).
const TBL_AFTER_SAMPLE: i16 = 0x52d;
const AFTER_SAMPLE_COUNT: usize = 15;
const LAYER: u8 = 3;

/// `0x434d20(page)`: the bar, the page title and the background.
fn header(l: &mut SpriteLayer, t: &FrontTables, page: usize) {
    let x0 = t.origin.0;
    super::put(l, 0x8f, super::tbl::ROUTE_BAR, x0, 0, PAL_MENU, 1);
    super::text(
        l,
        0x87,
        t.calibration_titles[page],
        x0 + 0x60,
        0x20,
        PAL_MENU,
        1,
    );
    super::put(l, 0xb7, super::tbl::BACKGROUND, 0, 0, PAL_MENU, LAYER);
}

#[derive(Debug, Clone, Default)]
pub struct CalibrationScreen {
    cal: Calibrator,
    samples: usize,
}

impl CalibrationScreen {
    pub fn new(t: &FrontTables, l: &mut SpriteLayer) -> Self {
        super::clear_all(l);
        let (x0, y0) = t.origin;
        let at = |l: &mut SpriteLayer, slot, tbl, (x, y): (i16, i16)| {
            super::put(l, slot, tbl, x0 + x, y0 + y, PAL_MENU, LAYER);
        };
        at(l, SLOT_BRAKE, TBL_BRAKE, BRAKE_POS);
        at(l, SLOT_MASCON, TBL_MASCON, MASCON_POS);
        at(l, SLOT_TEXT, TBL_FIRST_TEXT, TEXT_POS);
        header(l, t, 0);
        Self::default()
    }

    /// One tick with the raw axes. Returns the new calibration (to be written to
    /// `joystick.dat`) or `None` for an abort, when the screen ends (cleared).
    pub fn tick(
        &mut self,
        input: &MenuInput,
        joy: JoyState,
        t: &FrontTables,
        l: &mut SpriteLayer,
        sfx: &mut Vec<Sfx>,
    ) -> Option<Option<Calibration>> {
        let buttons = ButtonState {
            menu_confirm_pressed: input.confirm,
            cancel_pressed: input.cancel,
            ..ButtonState::default()
        };
        let (step, sampled) = self.cal.tick(&joy, &buttons);
        if sampled {
            sfx.push(Sfx::Play(snd::DECIDE_SOFT));
            if self.samples < AFTER_SAMPLE_COUNT {
                let (x0, y0) = t.origin;
                super::put(
                    l,
                    SLOT_TEXT,
                    TBL_AFTER_SAMPLE + self.samples as i16,
                    x0 + TEXT_POS.0,
                    y0 + TEXT_POS.1,
                    PAL_MENU,
                    LAYER,
                );
            }
            self.samples += 1;
        }
        match step {
            CalStep::Mascon(n) => super::image(l, SLOT_MASCON, TBL_MASCON + n as i16),
            CalStep::Brake(n) => {
                if n == 0 && sampled {
                    header(l, t, 1);
                }
                super::image(l, SLOT_BRAKE, TBL_BRAKE + n as i16);
            }
            CalStep::Done(cal) => {
                super::clear_all(l);
                return Some(Some(cal));
            }
            CalStep::Aborted => {
                sfx.push(Sfx::Play(snd::CANCEL));
                super::clear_all(l);
                return Some(None);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::front::tests_support::tables;

    #[test]
    fn sixteen_samples_then_done() {
        let t = tables();
        let mut l = SpriteLayer::default();
        let mut s = CalibrationScreen::new(&t, &mut l);
        let ok = MenuInput {
            confirm: true,
            ..MenuInput::NONE
        };
        let mut sfx = vec![];
        let mut out = None;
        for i in 0..16 {
            let joy = JoyState {
                x: i * 10,
                y: -i,
                ..JoyState::default()
            };
            out = s.tick(&ok, joy, &t, &mut l, &mut sfx);
            if i < 15 {
                assert!(out.is_none());
            }
        }
        let cal = out.flatten().expect("calibrated");
        assert_eq!(cal.mascon, [0, -1, -2, -3, -4, -5]);
        assert_eq!(cal.brake[9], 150);
        assert_eq!(cal.brake[10], 150);
        assert_eq!(sfx.len(), 16);
        let mut s = CalibrationScreen::new(&t, &mut l);
        let no = MenuInput {
            cancel: true,
            ..MenuInput::NONE
        };
        assert_eq!(
            s.tick(&no, JoyState::default(), &t, &mut l, &mut sfx),
            Some(None)
        );
    }
}

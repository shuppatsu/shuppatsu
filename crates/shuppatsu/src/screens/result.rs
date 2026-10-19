//! Station result (`StationResult_Create` 0x413d90, task 0x413f30, destroy 0x4148b0;
//! menus.md §9).
//!
//! Phase A counts down from 96: at 94 the layout is placed (with the monitor frame and the
//! 運転評価 title), at 90 sound 0x23. Phase B runs a counter from 0 with a row cursor: the stop
//! row at 0, the re-power row at 10, the emergency-brake row at 20 (which also bumps the counter
//! to 30), the total at 40, and meanwhile the HUD's remaining-time digits flicker and settle
//! from 40 + 5k. The task ends at counter 120. Confirm or cancel jumps the counter to 80, or to
//! 120 once past 80. A jump to 80 before the EB row has run is pulled back to 30 by that row's
//! bump, exactly as in the original.

use super::tables::{Placement, ScreenTables};
use super::{MenuInput, SLOT_REMAINING, Sfx, SlotSet, TBL_MASKS, digits3};
use shuppatsu_render::sprites::Slot;

const COUNTDOWN: i32 = 96;
const COUNTDOWN_LAYOUT: i32 = 94;
const COUNTDOWN_SOUND: i32 = 90;
const END: i32 = 120;
const SKIP_TO: i32 = 80;

// Slots (0x413f30).
const SLOT_TITLE: usize = 0x4b;
const SLOT_STOP: usize = 0x4c;
const SLOTS_STOP_DIGITS: [usize; 3] = [0x3f, 0x40, 0x41];
const SLOT_REPOWER: usize = 0x4d;
const SLOT_REPOWER_WORD: usize = 0x4e;
const SLOT_REPOWER_SECONDS: usize = 0x4f;
const SLOTS_REPOWER_DIGITS: [usize; 2] = [0x42, 0x43];
const SLOT_EB: usize = 0x53;
const SLOT_EB_WORD: usize = 0x54;
const SLOT_EB_SECONDS: usize = 0x55;
const SLOTS_EB_DIGITS: [usize; 2] = [0x46, 0x47];
const SLOT_TOTAL: usize = 0x56;
const SLOTS_TOTAL_DIGITS: [usize; 3] = [0x48, 0x49, 0x4a];
const SLOT_MONITOR: usize = 0x57;

// Tilemaps (0x413f30).
const TBL_TITLE: i16 = 0x1a0;
/// Monitor frame for resolution mode 1 (0x25f / 0x260 are modes 2 / 3).
const TBL_MONITOR: i16 = 0x25e;
/// Monitor frame position in mode 1 (0x414110: x 16 + offset x, y 192 − offset y; the eeprom
/// screen offset 0x856358/59 is 0 here).
const MONITOR_POS: (i16, i16) = (0x10, 0xc0);
const TBL_STOP_OK: i16 = 0x1c9;
const TBL_STOP_EXCELLENT: i16 = 0x1cc;
const TBL_STOP_FAR: i16 = 0x1a6;
const TBL_REPOWER: i16 = 0x1c0;
const TBL_PENALTY_WORD: i16 = 0x1c4;
const TBL_SECONDS: i16 = 0x1bf;
const TBL_NONE: i16 = 0x1c3;
const TBL_EB: i16 = 0x1ca;
const TBL_TOTAL: i16 = 0x1a9;

/// "合格" only below 50 cm (0x413f30).
const STOP_OK_MAX_CM: i32 = 50;
/// The stop-row digit palette turns from 5 to 4 when the train stopped more than 1 cm short
/// (`0.01 < 0x85708c`, double at 0x4676d0).
const SHORT_EPS_M: f32 = 0.01;
const EB_PENALTY_S: i32 = 10;
const TOTAL_MAX: i32 = 999;
const LESSON_TOTAL_MAX: i32 = 20;

// Sounds.
const SND_PERFECT: u16 = 0x195;
const SND_RESULT: u16 = 0x2d;
const SND_OPEN: u16 = 0x23;
const SND_COUNT: u16 = 0x18;
const VOICE_A: u16 = 0x193;
const VOICE_B: u16 = 0x192;
/// Stages whose EXCELLENT voice is 0x193 (switch in 0x413f30; the others use 0x192).
const VOICE_A_STAGES: [usize; 13] = [2, 3, 4, 8, 9, 10, 12, 13, 14, 15, 16, 17, 18];

const PAL_4: u8 = 4;
const PAL_5: u8 = 5;

/// What the result screen reads when it is created (0x413d90), from the rules and tables.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ResultInput {
    pub stage: usize,
    /// Rounded stop error, m (`0x85722c`, negative = overrun) and the stop tolerance (`0x8570d0`).
    pub dist_round_m: i32,
    pub tolerance_m: i32,
    /// Stop error, m (`0x85708c`).
    pub dist_m: f32,
    /// Re-powered in the approach (`0x8571f6 ≥ 1`) and the penalty (0x45a8b0).
    pub repowered: bool,
    pub repower_s: i32,
    /// Emergency brake used in the approach (`0x856da8 > 0`).
    pub emergency_brake: bool,
    /// Lesson stage (`0x4656c4[stage] == 1`): no EB row, total capped at 20.
    pub lesson: bool,
    /// Clean-stop mark of this station (`0x856fe1[station]`).
    pub clean_mark: bool,
    /// Overrun factor 1–3 (`0x856fb8`).
    pub overrun_factor: i32,
    /// Exact-stop bonus (0x45a9f0).
    pub exact_bonus_s: i32,
}

impl ResultInput {
    /// |stop error| in whole cm (`__ftol(|0x85708c| × 100)`).
    fn cm(&self) -> i32 {
        (f64::from(self.dist_m.abs()) * 100.0) as i32
    }

    fn perfect(&self) -> bool {
        self.cm() == 0 && self.clean_mark
    }

    /// Overrun metres beyond the tolerance, capped at 999 (0x413d90).
    fn overrun_m(&self) -> i32 {
        if self.dist_round_m < -self.tolerance_m {
            let v = -self.dist_round_m;
            if self.dist_round_m != -999 && v > 998 {
                999
            } else {
                v
            }
        } else {
            0
        }
    }
}

/// The parts of [`ScreenTables`] this screen uses.
#[derive(Debug, Clone)]
pub struct ResultDefs {
    pub layout: Vec<Placement>,
    pub lesson_layout: Vec<Placement>,
    pub result_digits: [i16; 10],
    pub time_digits: [i16; 10],
    pub overrun_banners: [i16; 4],
}

impl ResultDefs {
    pub fn new(t: &ScreenTables) -> Self {
        Self {
            layout: t.result_layout.clone(),
            lesson_layout: t.lesson_result_layout.clone(),
            result_digits: t.result_digits,
            time_digits: t.time_digits,
            overrun_banners: t.overrun_banners,
        }
    }
}

pub struct StationResultScreen {
    defs: ResultDefs,
    input: ResultInput,
    /// Phase A countdown (`task+0x38`).
    countdown: i32,
    /// Phase B counter (`+0x34`) and row cursor (`+0x6c`).
    counter: i32,
    row: u8,
    overrun_s: i32,
    eb_s: i32,
    /// Remaining time after the deduction (`0x856fb0` once the total row ran).
    remaining_after: Option<i32>,
    slots: SlotSet,
    /// The sound started at creation, stopped by the destroy.
    opening_sound: u16,
}

impl StationResultScreen {
    /// `StationResult_Create` 0x413d90: penalties and the opening sound.
    pub fn new(defs: ResultDefs, input: ResultInput, sfx: &mut Vec<Sfx>) -> Self {
        let opening_sound = if input.perfect() {
            SND_PERFECT
        } else {
            SND_RESULT
        };
        sfx.push(Sfx::Play(opening_sound));
        Self {
            defs,
            countdown: COUNTDOWN,
            counter: 0,
            row: 0,
            overrun_s: input.overrun_m(),
            eb_s: if !input.lesson && input.emergency_brake {
                EB_PENALTY_S
            } else {
                0
            },
            input,
            remaining_after: None,
            slots: SlotSet::default(),
            opening_sound,
        }
    }

    fn repower_s(&self) -> i32 {
        if self.input.repowered {
            self.input.repower_s
        } else {
            0
        }
    }

    /// The remaining time the HUD should show from the total row on.
    pub fn remaining_after(&self) -> Option<i32> {
        self.remaining_after
    }

    /// The replay camera (0x419730) has been created: from countdown 94 on.
    pub fn replay_shown(&self) -> bool {
        self.countdown <= COUNTDOWN_LAYOUT
    }

    pub fn slots(&self) -> &SlotSet {
        &self.slots
    }

    fn digit(&self, d: usize) -> i16 {
        self.defs.result_digits[d.min(9)]
    }

    /// Up to three digits with leading zeros left blank, image only (0x4318c0).
    fn number(&mut self, slots: &[usize], v: i32) {
        let d = digits3(v);
        let n = slots.len();
        for (k, &slot) in slots.iter().enumerate() {
            let place = 3 - n + k; // 0 = hundreds
            let shown = match place {
                0 => v > 99,
                1 => v > 9,
                _ => true,
            };
            if shown {
                self.slots.image(slot, self.digit(d[place]));
            }
        }
    }

    /// One tick. `remaining_s` is the rules' remaining time now (`0x856fb0`). Returns false
    /// on the tick the task ends (its destroy has then run).
    pub fn tick(&mut self, input: &MenuInput, remaining_s: i32, sfx: &mut Vec<Sfx>) -> bool {
        self.countdown -= 1;
        match self.countdown {
            COUNTDOWN_LAYOUT => self.place_layout(),
            COUNTDOWN_SOUND => sfx.push(Sfx::Play(SND_OPEN)),
            // 0: the lamp flag (0x434fc0(1)); no lamp is drawn on PC.
            _ => {}
        }
        if self.countdown >= 0 {
            return true;
        }
        self.rows(remaining_s, sfx);
        if self.counter >= END {
            self.destroy(sfx);
            return false;
        }
        self.remaining_digits(remaining_s, sfx);
        self.counter += 1;
        if input.confirm || input.cancel {
            self.counter = if self.counter > 0 && self.counter <= SKIP_TO {
                SKIP_TO
            } else {
                END
            };
        }
        true
    }

    fn place_layout(&mut self) {
        let layout = if self.input.lesson {
            &self.defs.lesson_layout
        } else {
            &self.defs.layout
        };
        for (i, p) in layout.iter().enumerate() {
            // The first row is placed 16 px higher (0x413f30).
            let p = if i == 0 {
                Placement { y: p.y - 16, ..*p }
            } else {
                *p
            };
            self.slots.place(&p);
        }
        self.slots.image(SLOT_MONITOR, TBL_MONITOR);
        let m = self.slots.get(SLOT_MONITOR).copied().unwrap_or(Slot::CLEAR);
        self.slots.place_at(
            SLOT_MONITOR,
            MONITOR_POS.0,
            MONITOR_POS.1,
            m.layer,
            m.palette,
        );
        self.slots.image(SLOT_TITLE, TBL_TITLE);
    }

    /// The row cursor of phase B; several rows can run in one tick after a skip.
    fn rows(&mut self, remaining_s: i32, sfx: &mut Vec<Sfx>) {
        if self.counter == 0 && self.row == 0 {
            sfx.push(Sfx::Play(SND_COUNT));
            self.stop_row(sfx);
            self.row = 1;
        }
        if self.counter > 9 && self.row == 1 {
            self.slots.image(SLOT_REPOWER, TBL_REPOWER);
            self.penalty_row(
                self.input.repowered,
                self.repower_s(),
                SLOT_REPOWER_WORD,
                SLOT_REPOWER_SECONDS,
                SLOTS_REPOWER_DIGITS,
            );
            self.row = 2;
        }
        if self.counter > 19 && self.row == 2 {
            self.counter = 30;
            self.row = 3;
        }
        if self.counter > 29 && self.row == 3 {
            if self.input.lesson {
                self.counter = 40;
            } else {
                self.slots.image(SLOT_EB, TBL_EB);
                self.penalty_row(
                    self.eb_s >= 1,
                    self.eb_s,
                    SLOT_EB_WORD,
                    SLOT_EB_SECONDS,
                    SLOTS_EB_DIGITS,
                );
            }
            self.row = 4;
        }
        if self.counter > 39 && self.row == 4 {
            self.total_row(remaining_s);
            self.row = 5;
        }
    }

    fn stop_row(&mut self, sfx: &mut Vec<Sfx>) {
        let overrun = self.overrun_s;
        if overrun != 0 {
            let factor = self.input.overrun_factor.clamp(0, 3) as usize;
            self.slots
                .image(SLOT_STOP, self.defs.overrun_banners[factor]);
            self.number(&SLOTS_STOP_DIGITS, overrun);
            // 0x4143fa: the deduction is metres × the stage's factor.
            self.overrun_s = overrun * self.input.overrun_factor;
            return;
        }
        let palette = if self.input.dist_m > SHORT_EPS_M {
            PAL_4
        } else {
            PAL_5
        };
        let cm = self.input.cm();
        if cm == 0 && self.input.clean_mark {
            self.slots.image(SLOT_STOP, TBL_STOP_EXCELLENT);
            self.slots
                .image_pal(SLOTS_STOP_DIGITS[2], self.digit(0), palette);
            sfx.push(Sfx::Play(VOICE_A));
            let voice = if VOICE_A_STAGES.contains(&self.input.stage) {
                VOICE_A
            } else {
                VOICE_B
            };
            sfx.push(Sfx::Play(voice));
        } else if cm < STOP_OK_MAX_CM {
            self.slots.image(SLOT_STOP, TBL_STOP_OK);
            let d = digits3(cm);
            if cm > 99 {
                self.slots
                    .image_pal(SLOTS_STOP_DIGITS[0], self.digit(d[0]), palette);
            }
            if cm > 9 {
                self.slots
                    .image_pal(SLOTS_STOP_DIGITS[1], self.digit(d[1]), palette);
            }
            self.slots
                .image_pal(SLOTS_STOP_DIGITS[2], self.digit(d[2]), palette);
        } else {
            self.slots.image(SLOT_STOP, TBL_STOP_FAR);
        }
    }

    /// Re-power / EB row body: "減点 nn 秒", or なし in the first digit slot.
    fn penalty_row(
        &mut self,
        applies: bool,
        seconds: i32,
        word: usize,
        unit: usize,
        digits: [usize; 2],
    ) {
        if !applies {
            self.slots.image_pal(digits[0], TBL_NONE, PAL_4);
            return;
        }
        self.slots.image(word, TBL_PENALTY_WORD);
        self.slots.image(unit, TBL_SECONDS);
        self.number(&digits, seconds);
    }

    fn total_row(&mut self, remaining_s: i32) {
        let mut total = (self.eb_s + self.repower_s() + self.overrun_s).min(TOTAL_MAX);
        if self.input.lesson {
            total = total.min(LESSON_TOTAL_MAX);
        }
        self.slots.image(SLOT_TOTAL, TBL_TOTAL);
        self.number(&SLOTS_TOTAL_DIGITS, total);
        let units = SLOTS_TOTAL_DIGITS[2];
        if total == 0 {
            self.slots.image_pal(units, self.digit(0), PAL_4);
        } else {
            self.slots
                .image_pal(units, self.digit(digits3(total)[2]), PAL_5);
        }
        let mut left = remaining_s - total;
        if self.input.perfect() {
            left += self.input.exact_bonus_s;
        }
        self.remaining_after = Some(left.max(0));
    }

    /// The HUD remaining-time digits: flicker, then settle on the new value.
    fn remaining_digits(&mut self, remaining_s: i32, sfx: &mut Vec<Sfx>) {
        let now = self.remaining_after.unwrap_or(remaining_s);
        let palette = if now > 14 { PAL_5 } else { PAL_4 };
        let c = self.counter;
        let mask = |phase: i32| TBL_MASKS[usize::from(phase.rem_euclid(2) != 0)];
        if c < 40 {
            for k in 0..3 {
                if 5 * k as i32 <= c {
                    self.slots
                        .patch(SLOT_REMAINING + k, mask(c - 10 * k as i32), Some(palette));
                }
            }
            return;
        }
        let digits = digits3(now);
        let mut settled = 0;
        for (k, &d) in digits.iter().enumerate() {
            let threshold = 40 + 5 * k as i32;
            let tbl = if threshold < c {
                settled += 1;
                self.defs.time_digits[d]
            } else {
                mask(c - threshold)
            };
            self.slots.patch(SLOT_REMAINING + k, tbl, Some(palette));
        }
        if settled == 3 {
            sfx.push(Sfx::Stop(SND_COUNT));
        }
    }

    /// `StationResult_Destroy` 0x4148b0.
    fn destroy(&mut self, sfx: &mut Vec<Sfx>) {
        self.slots.clear();
        sfx.push(Sfx::Stop(self.opening_sound));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defs() -> ResultDefs {
        let p = |slot: usize, x: i16, y: i16| Placement {
            slot: slot as i16,
            x,
            y,
            layer: 1,
            palette: 4,
        };
        ResultDefs {
            layout: vec![
                p(SLOT_MONITOR, 0, 0),
                p(SLOT_TITLE, 64, 38),
                p(SLOT_STOP, 80, 268),
            ],
            lesson_layout: vec![p(SLOT_MONITOR, 0, 0)],
            result_digits: std::array::from_fn(|i| 0x1b3 + i as i16),
            time_digits: std::array::from_fn(|i| 0x401 + i as i16),
            overrun_banners: [0x1a2, 0x1a3, 0x1a4, 0x1a5],
        }
    }

    fn base() -> ResultInput {
        ResultInput {
            stage: 1,
            dist_round_m: 0,
            tolerance_m: 2,
            dist_m: 0.25,
            overrun_factor: 1,
            exact_bonus_s: 5,
            repower_s: 3,
            ..Default::default()
        }
    }

    const NONE: MenuInput = MenuInput::NONE;
    const OK: MenuInput = MenuInput {
        confirm: true,
        ..NONE
    };

    /// Ticks until the task ends (counting the ending tick); `skip_at` = ticks to press on.
    fn length(input: ResultInput, skip_at: &[u32]) -> u32 {
        let mut sfx = Vec::new();
        let mut s = StationResultScreen::new(defs(), input, &mut sfx);
        for n in 1..1000 {
            let i = if skip_at.contains(&n) { OK } else { NONE };
            if !s.tick(&i, 50, &mut sfx) {
                return n;
            }
        }
        panic!("never ended");
    }

    #[test]
    fn unskipped_length() {
        // 96 phase-A ticks; phase B: counter 0..=19, then 30..=119 (the EB row bumps the
        // counter from 20 to 30), and the tick that sees 120 ends the task.
        assert_eq!(length(base(), &[]), 96 + 20 + 90 + 1);
    }

    #[test]
    fn no_skip_during_phase_a() {
        assert_eq!(length(base(), &[1, 50, 96]), 207);
    }

    #[test]
    fn skip_after_the_eb_row_jumps_to_80_then_120() {
        // B tick 25: counter 35 → 80 (the press is read after the increment). Next tick shows
        // 80 (total row runs), digits settle; the counter climbs to 120.
        let skip = 96 + 25;
        assert_eq!(length(base(), &[skip]), skip + 41);
        // A second press after 80 ends at once.
        assert_eq!(length(base(), &[skip, skip + 2]), skip + 3);
    }

    #[test]
    fn early_skip_is_pulled_back_by_the_eb_row() {
        // B tick 4 (counter 4, 5 after the increment → 80): the next tick runs the re-power row, then the EB row's bump
        // sets the counter to 30, so the screen still needs 90 more ticks.
        let skip = 96 + 5;
        assert_eq!(length(base(), &[skip]), skip + 1 + 90);
    }

    #[test]
    fn opening_and_counting_sounds() {
        let mut sfx = Vec::new();
        let mut s = StationResultScreen::new(defs(), base(), &mut sfx);
        assert_eq!(sfx, [Sfx::Play(SND_RESULT)]);
        sfx.clear();
        for _ in 0..6 {
            s.tick(&NONE, 50, &mut sfx);
        }
        assert_eq!(sfx, [Sfx::Play(SND_OPEN)]);
        sfx.clear();
        for _ in 0..91 {
            s.tick(&NONE, 50, &mut sfx);
        }
        assert_eq!(sfx, [Sfx::Play(SND_COUNT)]);
        // Settles when the counter passes 50.
        sfx.clear();
        while s.tick(&NONE, 50, &mut sfx) {}
        let first_stop = sfx.iter().position(|s| *s == Sfx::Stop(SND_COUNT));
        assert!(first_stop.is_some());
        assert_eq!(sfx.last(), Some(&Sfx::Stop(SND_RESULT)));
    }

    #[test]
    fn perfect_stop_plays_excellent() {
        let input = ResultInput {
            dist_m: 0.004,
            clean_mark: true,
            stage: 3,
            ..base()
        };
        let mut sfx = Vec::new();
        let mut s = StationResultScreen::new(defs(), input, &mut sfx);
        assert_eq!(sfx, [Sfx::Play(SND_PERFECT)]);
        for _ in 0..97 {
            s.tick(&NONE, 50, &mut sfx);
        }
        assert_eq!(
            s.slots().get(SLOT_STOP).map(|s| s.tilemap),
            Some(TBL_STOP_EXCELLENT)
        );
        assert!(sfx.ends_with(&[Sfx::Play(SND_COUNT), Sfx::Play(VOICE_A), Sfx::Play(VOICE_A)]));
        while s.remaining_after().is_none() {
            s.tick(&NONE, 50, &mut sfx);
        }
        assert_eq!(s.remaining_after(), Some(55));
    }

    #[test]
    fn overrun_uses_the_factor() {
        let input = ResultInput {
            dist_round_m: -7,
            dist_m: -7.2,
            overrun_factor: 3,
            repowered: true,
            emergency_brake: true,
            ..base()
        };
        let mut sfx = Vec::new();
        let mut s = StationResultScreen::new(defs(), input, &mut sfx);
        for _ in 0..97 {
            s.tick(&NONE, 100, &mut sfx);
        }
        assert_eq!(s.slots().get(SLOT_STOP).map(|s| s.tilemap), Some(0x1a5));
        // Metres shown, not seconds: "7" in the units slot only.
        assert!(s.slots().get(SLOTS_STOP_DIGITS[1]).is_none());
        assert_eq!(
            s.slots().get(SLOTS_STOP_DIGITS[2]).map(|s| s.tilemap),
            Some(0x1b3 + 7)
        );
        while s.remaining_after().is_none() {
            s.tick(&NONE, 100, &mut sfx);
        }
        // 7 m × 3 + re-power 3 + EB 10 = 34.
        assert_eq!(s.remaining_after(), Some(66));
        let tens = s.slots().get(SLOTS_TOTAL_DIGITS[1]).map(|s| s.tilemap);
        assert_eq!(tens, Some(0x1b3 + 3));
    }

    #[test]
    fn lesson_stage_skips_the_eb_row_and_caps_the_total() {
        let input = ResultInput {
            dist_round_m: -40,
            dist_m: -40.0,
            lesson: true,
            emergency_brake: true,
            ..base()
        };
        let mut sfx = Vec::new();
        let mut s = StationResultScreen::new(defs(), input, &mut sfx);
        while s.remaining_after().is_none() {
            s.tick(&NONE, 100, &mut sfx);
        }
        assert!(s.slots().get(SLOT_EB).is_none());
        assert_eq!(s.remaining_after(), Some(80));
        // Phase B: counter 0..=19, then the lesson bump to 40 and the total row at once.
        assert_eq!(length(input, &[]), 96 + 20 + 80 + 1);
    }

    #[test]
    fn remaining_digits_flicker_then_settle() {
        let mut sfx = Vec::new();
        let mut s = StationResultScreen::new(defs(), base(), &mut sfx);
        for _ in 0..97 {
            s.tick(&NONE, 123, &mut sfx);
        }
        // Counter 0: only digit 0 flickers.
        assert_eq!(
            s.slots().patched(SLOT_REMAINING),
            Some((0x40b, Some(PAL_5)))
        );
        assert!(s.slots().patched(SLOT_REMAINING + 1).is_none());
        // Run to counter 46 (B ticks 0..=19 then counter 30.. ⇒ tick 36 shows 46).
        for _ in 0..36 {
            s.tick(&NONE, 123, &mut sfx);
        }
        assert_eq!(s.counter, 47);
        assert_eq!(
            s.slots().patched(SLOT_REMAINING),
            Some((0x401 + 1, Some(PAL_5)))
        );
        assert_eq!(
            s.slots().patched(SLOT_REMAINING + 1),
            Some((0x401 + 2, Some(PAL_5)))
        );
        assert!(TBL_MASKS.contains(&s.slots().patched(SLOT_REMAINING + 2).unwrap().0));
    }
}

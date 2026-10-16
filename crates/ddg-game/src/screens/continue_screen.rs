//! Continue countdown (`Continue_Create` 0x415ab0, task 0x415ba0, destroy 0x415e80;
//! menus.md §10).
//!
//! The counter runs from 399 down; the HUD's three remaining-time digit slots all show
//! `counter / 40` (9 → 0) and blink the masks in the last four ticks of each digit. Confirm
//! chooses to continue and zeroes the counter; the horn drops to the next digit. At 0 the digits
//! show 0 and the verdict appears, then 30 more ticks run. Free play always applies on PC
//! (`0x8573e0 = 1`, menus.md §2), so the "press START" branch is the only one built.

use super::tables::ScreenTables;
use super::{MenuInput, SLOT_REMAINING, SlotSet, TBL_MASKS};

const START: i32 = 399;
const TICKS_PER_DIGIT: i32 = 40;
/// The task ends once the counter has dropped below this (0x415e34: `t − 1 > −31` continues).
const END: i32 = -31;

const SLOT_TITLE: usize = 0x37;
const SLOT_PROMPT: usize = 0x38;
const TBL_TITLE: i16 = 0x24a;
const TBL_CHOSEN: i16 = 0x24d;
const TBL_PRESS_START: i16 = 0x3f8;
const TBL_YES: i16 = 0x491;
const TBL_NO: i16 = 0x492;
/// Title offset from the screen origin (0x415ab0: x0 + 0xf0, y0 + 200).
const TITLE_OFFSET: (i16, i16) = (0xf0, 200);
/// The verdict sits 0xc0 right of the prompt (0x415ba0).
const VERDICT_DX: i16 = 0xc0;
const PALETTE: u8 = 4;
const LAYER: u8 = 3;

#[derive(Debug, Clone, Copy)]
pub struct ContinueDefs {
    pub origin: (i16, i16),
    pub prompt_pos: (i16, i16),
    pub chosen_pos: (i16, i16),
    pub time_digits: [i16; 10],
}

impl ContinueDefs {
    pub fn new(t: &ScreenTables) -> Self {
        Self {
            origin: t.origin,
            prompt_pos: t.prompt_pos,
            chosen_pos: t.chosen_pos,
            time_digits: t.time_digits,
        }
    }
}

pub struct ContinueScreen {
    defs: ContinueDefs,
    counter: i32,
    chosen: bool,
    slots: SlotSet,
}

impl ContinueScreen {
    pub fn new(defs: ContinueDefs) -> Self {
        let mut slots = SlotSet::default();
        slots.full(
            SLOT_TITLE,
            TBL_TITLE,
            defs.origin.0 + TITLE_OFFSET.0,
            defs.origin.1 + TITLE_OFFSET.1,
            PALETTE,
            LAYER,
        );
        Self {
            defs,
            counter: START,
            chosen: false,
            slots,
        }
    }

    pub fn slots(&self) -> &SlotSet {
        &self.slots
    }

    fn digits(&mut self, tbl: i16) {
        for k in 0..3 {
            // 0x4318c0: image only; the palette is the HUD's last (4 at no time left).
            self.slots.patch(SLOT_REMAINING + k, tbl, Some(PALETTE));
        }
    }

    /// One tick; `Some(true)` = continue, `Some(false)` = the run is over, on the last tick.
    pub fn tick(&mut self, input: &MenuInput) -> Option<bool> {
        if self.counter >= 0 {
            let r = self.counter % TICKS_PER_DIGIT;
            let tbl = match r {
                0..36 => self.defs.time_digits[(self.counter / TICKS_PER_DIGIT) as usize],
                36..38 => TBL_MASKS[0],
                _ => TBL_MASKS[1],
            };
            self.digits(tbl);
            let (px, py) = self.defs.prompt_pos;
            self.slots
                .full(SLOT_PROMPT, TBL_PRESS_START, px, py, PALETTE, LAYER);
            if input.confirm {
                self.chosen = true;
                self.counter = 0;
                let (cx, cy) = self.defs.chosen_pos;
                self.slots
                    .full(SLOT_TITLE, TBL_CHOSEN, cx, cy, PALETTE, LAYER);
            }
            if input.horn {
                self.counter -= r;
            }
            if self.counter < 1 {
                self.digits(self.defs.time_digits[0]);
                let tbl = if self.chosen { TBL_YES } else { TBL_NO };
                self.slots
                    .full(SLOT_PROMPT, tbl, px + VERDICT_DX, py, PALETTE, LAYER);
            }
        }
        self.counter -= 1;
        if self.counter > END {
            return None;
        }
        // Destroy 0x415e80: the screen's slots go, the HUD readout comes back.
        self.slots.clear();
        Some(self.chosen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defs() -> ContinueDefs {
        ContinueDefs {
            origin: (0, 0),
            prompt_pos: (304, 344),
            chosen_pos: (240, 200),
            time_digits: std::array::from_fn(|i| 0x401 + i as i16),
        }
    }

    const NONE: MenuInput = MenuInput::NONE;

    fn run(press: impl Fn(u32) -> MenuInput) -> (u32, bool) {
        let mut s = ContinueScreen::new(defs());
        for n in 1..1000 {
            if let Some(c) = s.tick(&press(n)) {
                return (n, c);
            }
        }
        panic!("never ended");
    }

    #[test]
    fn full_countdown_declines() {
        // 399 … 1, the zero tick, then 30 more.
        assert_eq!(run(|_| NONE), (399 + 1 + 30, false));
    }

    #[test]
    fn digits_count_down_and_blink() {
        // The counter counts down, so the masks come in the first four ticks of each digit:
        // ticks 1..=4 show 399..=396 (r 39..36) → 0x40c, 0x40c, 0x40b, 0x40b; then 9 until 360.
        let mut s = ContinueScreen::new(defs());
        let mut seen = Vec::new();
        for _ in 0..41 {
            s.tick(&NONE);
            seen.push(s.slots().patched(SLOT_REMAINING + 2).unwrap().0);
        }
        assert_eq!(&seen[..5], &[0x40c, 0x40c, 0x40b, 0x40b, 0x401 + 9]);
        assert_eq!(seen[39], 0x401 + 9);
        assert_eq!(seen[40], 0x40c);
    }

    #[test]
    fn confirm_continues_after_31_ticks() {
        let (n, c) = run(|n| MenuInput {
            confirm: n == 100,
            ..NONE
        });
        assert!(c);
        assert_eq!(n, 100 + 30);
    }

    #[test]
    fn horn_skips_a_digit() {
        // Press on tick 1 (counter 399, r 39): 360, then 359 … one digit gone.
        let (n, c) = run(|n| MenuInput {
            horn: n == 1,
            ..NONE
        });
        assert!(!c);
        assert_eq!(n, 399 + 1 + 30 - 39);
    }

    #[test]
    fn verdict_slots() {
        let mut s = ContinueScreen::new(defs());
        s.tick(&MenuInput {
            confirm: true,
            ..NONE
        });
        let title = s.slots().get(SLOT_TITLE).unwrap();
        assert_eq!((title.tilemap, title.x, title.y), (TBL_CHOSEN, 240, 200));
        let p = s.slots().get(SLOT_PROMPT).unwrap();
        assert_eq!((p.tilemap, p.x, p.y), (TBL_YES, 304 + 192, 344));
        assert_eq!(s.slots().patched(SLOT_REMAINING), Some((0x401, Some(4))));
    }
}

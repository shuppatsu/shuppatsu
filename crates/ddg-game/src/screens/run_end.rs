//! Run-end banner (`RunEnd_Create` 0x415f00, task 0x416270, destroy 0x4162a0; menus.md §11.3):
//! 運転中止 after a game over, 運転終了 with the conductor and a grade message after a clear.
//! Lasts 150 ticks; confirm or cancel ends it at once.

use super::tables::ScreenTables;
use super::{MenuInput, Sfx, SlotSet};

const TICKS: i32 = 150;
const SLOT_BANNER: usize = 0x60;
const SLOT_GIRL: usize = 0x61;
const SLOT_MESSAGE: usize = 0x5e;
const TBL_GAME_OVER: i16 = 0x250;
const TBL_CLEARED: i16 = 0x251;
/// Banner offset from the screen origin (0x415f00: x0 + 0xf0, y0 + 0xcc).
const BANNER_OFFSET: (i16, i16) = (0xf0, 0xcc);
const TBL_GIRL_A: i16 = 0x3fc;
const TBL_GIRL_B: i16 = 0x400;
const PAL_GIRL_A: u8 = 0x12;
/// Stages drawn with conductor 0x3fc on palette 0x12 (switch in 0x415f00).
const GIRL_A_STAGES: [usize; 13] = [2, 3, 4, 8, 9, 10, 12, 13, 14, 15, 16, 17, 18];
const TBL_PERFECT: i16 = 0x3fd;
const TBL_EXCELLENT: i16 = 0x3fb;
const PALETTE: u8 = 4;
const LAYER: u8 = 3;

/// Grade thresholds of 0x415f00.
const GRADE_GOOD: i32 = 70;
const GRADE_EXCELLENT: i32 = 90;
const GRADE_PERFECT: i32 = 100;
const VOICE_PERFECT: u16 = 0x193;

/// What the banner needs to know about the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunEndInput {
    pub cleared: bool,
    pub stage: usize,
    /// Stage difficulty class (`0x4656c4[stage]`, `RuleTables::stage_kind`).
    pub stage_kind: i32,
    /// Report-card total 0..=100 (`0x856c88`).
    pub grade: i32,
    pub continues: i32,
}

#[derive(Debug, Clone, Copy)]
pub struct RunEndDefs {
    pub origin: (i16, i16),
    pub message_pos: (i16, i16),
    pub girl_pos: (i16, i16),
}

impl RunEndDefs {
    pub fn new(t: &ScreenTables) -> Self {
        Self {
            origin: t.origin,
            message_pos: t.prompt_pos,
            girl_pos: t.girl_pos,
        }
    }
}

pub struct RunEndBanner {
    counter: i32,
    slots: SlotSet,
}

/// The cleared banner's voice or jingle (0x415f00; jingles have bit 0x1000).
fn cleared_sound(r: &RunEndInput) -> u16 {
    let k = r.stage_kind;
    let by_kind = |easy: u16, mid: u16, hard: u16| {
        if k < 2 {
            easy
        } else if k < 4 {
            mid
        } else {
            hard
        }
    };
    if r.grade < GRADE_GOOD {
        if r.continues == 0 {
            by_kind(0x1004, 0x1004, 0x1002)
        } else {
            by_kind(0xfe, 0xfe, 0xf9)
        }
    } else if r.grade == GRADE_PERFECT {
        VOICE_PERFECT
    } else if r.continues != 0 {
        by_kind(0xf7, 0xf8, 0xfd)
    } else {
        by_kind(0x1000, 0x1001, 0x1003)
    }
}

impl RunEndBanner {
    pub fn new(defs: RunEndDefs, r: RunEndInput, sfx: &mut Vec<Sfx>) -> Self {
        let mut slots = SlotSet::default();
        let (bx, by) = (
            defs.origin.0 + BANNER_OFFSET.0,
            defs.origin.1 + BANNER_OFFSET.1,
        );
        if !r.cleared {
            slots.full(SLOT_BANNER, TBL_GAME_OVER, bx, by, PALETTE, LAYER);
            return Self {
                counter: TICKS,
                slots,
            };
        }
        let (girl, pal) = if GIRL_A_STAGES.contains(&r.stage) {
            (TBL_GIRL_A, PAL_GIRL_A)
        } else {
            (TBL_GIRL_B, PALETTE)
        };
        slots.full(
            SLOT_GIRL,
            girl,
            defs.girl_pos.0,
            defs.girl_pos.1,
            pal,
            LAYER,
        );
        slots.full(SLOT_BANNER, TBL_CLEARED, bx, by, PALETTE, LAYER);
        sfx.push(Sfx::Play(cleared_sound(&r)));
        let message = if r.grade == GRADE_PERFECT {
            Some(TBL_PERFECT)
        } else if r.grade >= GRADE_EXCELLENT {
            Some(TBL_EXCELLENT)
        } else {
            None
        };
        if let Some(m) = message {
            let (mx, my) = defs.message_pos;
            slots.full(SLOT_MESSAGE, m, mx, my, PALETTE, LAYER);
        }
        Self {
            counter: TICKS,
            slots,
        }
    }

    pub fn slots(&self) -> &SlotSet {
        &self.slots
    }

    /// One tick; false on the tick it ends.
    pub fn tick(&mut self, input: &MenuInput) -> bool {
        if input.confirm || input.cancel {
            self.slots.clear();
            return false;
        }
        self.counter -= 1;
        if self.counter < 0 {
            self.slots.clear();
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defs() -> RunEndDefs {
        RunEndDefs {
            origin: (0, 0),
            message_pos: (304, 344),
            girl_pos: (384, 48),
        }
    }

    fn input(grade: i32, continues: i32, stage_kind: i32) -> RunEndInput {
        RunEndInput {
            cleared: true,
            stage: 1,
            stage_kind,
            grade,
            continues,
        }
    }

    #[test]
    fn lasts_151_ticks_or_ends_on_confirm() {
        let mut sfx = Vec::new();
        let mut b = RunEndBanner::new(
            defs(),
            RunEndInput {
                cleared: false,
                ..input(0, 0, 0)
            },
            &mut sfx,
        );
        assert!(sfx.is_empty());
        assert_eq!(
            b.slots().get(SLOT_BANNER).map(|s| (s.tilemap, s.x, s.y)),
            Some((TBL_GAME_OVER, 240, 204))
        );
        let n = (1..).find(|_| !b.tick(&MenuInput::NONE)).unwrap();
        assert_eq!(n, 151);
        let mut b = RunEndBanner::new(defs(), input(50, 0, 0), &mut sfx);
        assert!(!b.tick(&MenuInput {
            cancel: true,
            ..MenuInput::NONE
        }));
    }

    #[test]
    fn grade_sounds_and_messages() {
        let s = |g, c, k| cleared_sound(&input(g, c, k));
        assert_eq!(s(50, 0, 1), 0x1004);
        assert_eq!(s(50, 0, 4), 0x1002);
        assert_eq!(s(50, 2, 4), 0xf9);
        assert_eq!(s(100, 1, 0), 0x193);
        assert_eq!(s(80, 1, 3), 0xf8);
        assert_eq!(s(80, 0, 5), 0x1003);
        let mut sfx = Vec::new();
        let b = RunEndBanner::new(defs(), input(95, 0, 0), &mut sfx);
        assert_eq!(
            b.slots().get(SLOT_MESSAGE).map(|s| s.tilemap),
            Some(TBL_EXCELLENT)
        );
        assert_eq!(
            b.slots().get(SLOT_GIRL).map(|s| (s.tilemap, s.palette)),
            Some((TBL_GIRL_B, 4))
        );
        let b = RunEndBanner::new(defs(), input(89, 0, 0), &mut sfx);
        assert!(b.slots().get(SLOT_MESSAGE).is_none());
    }
}

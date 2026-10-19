//! Report card 運転評価 (`ReportCard_Create` 0x4149d0, task 0x414b70, destroy 0x415050;
//! menus.md §11.1).
//!
//! After a 30-tick delay the counter runs from 0: sound 0x18 at 0, the five grade rows at
//! 25, 50, 75, 100 and 125 (sound 0x16 each: a stamp and a bar of quarter blocks), the total's
//! last two digits spin (`counter % 10`) and settle from 150, with the units spinning until
//! 175, and the task ends at 250. Confirm or cancel jumps the counter to 225.

use super::tables::{Placement, ScreenTables};
use super::{MenuInput, Sfx, SlotSet, digits3};

const DELAY: i32 = 30;
const ROW_TICKS: [i32; 5] = [25, 50, 75, 100, 125];
const SETTLE: i32 = 150;
const SPIN_STEP: i32 = 25;
const SKIP_TO: i32 = 225;
const END: i32 = 250;

/// Stamps by `part / 6`: 0x448 lowest … 0x445 優 (0x414b70's local table).
const STAMPS: [i16; 4] = [0x448, 0x447, 0x446, 0x445];
const TBL_BACKGROUND: i16 = 0x444;
const TBL_BLOCK: i16 = 0x449;
const TBL_HALF_BLOCK: i16 = 0x44a;
const SND_COUNT: u16 = 0x18;
const SND_ROW: u16 = 0x16;

#[derive(Debug, Clone)]
pub struct ReportDefs {
    pub origin: (i16, i16),
    pub layout: Vec<Placement>,
    pub result_digits: [i16; 10],
}

impl ReportDefs {
    pub fn new(t: &ScreenTables) -> Self {
        Self {
            origin: t.origin,
            layout: t.report_layout.clone(),
            result_digits: t.result_digits,
        }
    }

    fn slot(&self, record: usize) -> usize {
        self.layout
            .get(record)
            .and_then(|p| usize::try_from(p.slot).ok())
            .unwrap_or(shuppatsu_render::sprites::SLOT_COUNT)
    }
}

/// Records: five rows of {stamp, 5 bar blocks}, three total digits, the background.
const ROW_STRIDE: usize = 6;
const TOTAL_RECORD: usize = 5 * ROW_STRIDE;
const BACKGROUND_RECORD: usize = TOTAL_RECORD + 3;

pub struct ReportCard {
    defs: ReportDefs,
    parts: [i32; 5],
    total: i32,
    delay: i32,
    counter: i32,
    rows: usize,
    slots: SlotSet,
}

impl ReportCard {
    /// `parts` are the five grade parts (0x40dfd0, `Rules::grades`), each 0..=20.
    pub fn new(defs: ReportDefs, parts: [i32; 5]) -> Self {
        let mut slots = SlotSet::default();
        for p in &defs.layout {
            let p = Placement {
                x: p.x + defs.origin.0,
                y: p.y + defs.origin.1,
                ..*p
            };
            slots.place(&p);
        }
        let bg = defs.slot(BACKGROUND_RECORD);
        slots.image(bg, TBL_BACKGROUND);
        Self {
            defs,
            total: parts.iter().sum(),
            parts,
            delay: DELAY,
            counter: 0,
            rows: 0,
            slots,
        }
    }

    pub fn total(&self) -> i32 {
        self.total
    }

    pub fn slots(&self) -> &SlotSet {
        &self.slots
    }

    fn digit(&self, d: usize) -> i16 {
        self.defs.result_digits[d.min(9)]
    }

    fn row(&mut self, row: usize) {
        let part = self.parts[row].clamp(0, 23);
        let base = row * ROW_STRIDE;
        let stamp = self.defs.slot(base);
        self.slots.image(stamp, STAMPS[(part / 6) as usize]);
        // Bar slots are consecutive from the first bar record's slot (0x414b70).
        let first = self.defs.slot(base + 1);
        let blocks = (part / 4) as usize;
        for k in 0..blocks {
            self.slots.image(first + k, TBL_BLOCK);
        }
        if part % 4 > 1 {
            self.slots.image(first + blocks, TBL_HALF_BLOCK);
        }
    }

    /// One tick; false on the tick the task ends.
    pub fn tick(&mut self, input: &MenuInput, sfx: &mut Vec<Sfx>) -> bool {
        if self.delay > 0 {
            self.delay -= 1;
            return true;
        }
        if input.confirm || input.cancel {
            self.counter = SKIP_TO;
        }
        if self.counter == 0 {
            sfx.push(Sfx::Play(SND_COUNT));
        }
        while self.rows < 5 && self.counter >= ROW_TICKS[self.rows] {
            sfx.push(Sfx::Play(SND_ROW));
            self.row(self.rows);
            self.rows += 1;
        }
        let total_slot = self.defs.slot(TOTAL_RECORD);
        let mut spin_from = 1;
        if self.counter >= SETTLE && self.rows == 5 {
            sfx.push(Sfx::Stop(SND_COUNT));
            spin_from = (self.counter / SPIN_STEP - 5).max(1) + 1;
            let d = digits3(self.total);
            let hundreds = if self.total < 100 {
                -1
            } else {
                self.digit(d[0])
            };
            let tens = if self.total < 10 {
                -1
            } else {
                self.digit(d[1])
            };
            self.slots.image(total_slot, hundreds);
            self.slots.image(self.defs.slot(TOTAL_RECORD + 1), tens);
            self.slots
                .image(self.defs.slot(TOTAL_RECORD + 2), self.digit(d[2]));
        }
        let spin = self.digit((self.counter % 10) as usize);
        for k in spin_from..3 {
            // The spinning digits are addressed as consecutive slots (0x414b70).
            self.slots.image(total_slot + k as usize, spin);
        }
        if self.counter >= END {
            // Destroy 0x415050 clears every non-kept slot.
            self.slots.clear();
            return false;
        }
        self.counter += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defs() -> ReportDefs {
        let mut layout = Vec::new();
        for row in 0..5i16 {
            for k in 0..6i16 {
                layout.push(Placement {
                    slot: if k == 0 { 100 + row } else { 110 + row * 6 + k },
                    x: 360 + 32 * k,
                    y: 152 + 32 * row,
                    layer: 1,
                    palette: 4,
                });
            }
        }
        for k in 0..3 {
            layout.push(Placement {
                slot: 40 + k,
                x: 384 + 32 * k,
                y: 320,
                layer: 1,
                palette: 5,
            });
        }
        layout.push(Placement {
            slot: 43,
            x: 80,
            y: 104,
            layer: 2,
            palette: 4,
        });
        ReportDefs {
            origin: (0, 0),
            layout,
            result_digits: std::array::from_fn(|i| 0x1b3 + i as i16),
        }
    }

    fn length(parts: [i32; 5], press_at: Option<u32>) -> (u32, Vec<Sfx>) {
        let mut r = ReportCard::new(defs(), parts);
        let mut sfx = Vec::new();
        for n in 1..1000 {
            let input = MenuInput {
                confirm: Some(n) == press_at,
                ..MenuInput::NONE
            };
            if !r.tick(&input, &mut sfx) {
                return (n, sfx);
            }
        }
        panic!("never ended");
    }

    #[test]
    fn timing_and_sounds() {
        let (n, sfx) = length([20, 15, 10, 5, 0], None);
        assert_eq!(n, 30 + 251);
        assert_eq!(sfx[0], Sfx::Play(SND_COUNT));
        assert_eq!(sfx.iter().filter(|s| **s == Sfx::Play(SND_ROW)).count(), 5);
        assert_eq!(sfx.last(), Some(&Sfx::Stop(SND_COUNT)));
    }

    #[test]
    fn skip_jumps_to_225() {
        let (n, sfx) = length([20, 20, 20, 20, 20], Some(40));
        // Press on the 10th counting tick: counter 225 … 250.
        assert_eq!(n, 40 + 25);
        assert_eq!(sfx.iter().filter(|s| **s == Sfx::Play(SND_ROW)).count(), 5);
        assert_eq!(sfx[0], Sfx::Play(SND_COUNT));
    }

    #[test]
    fn rows_and_total() {
        let mut r = ReportCard::new(defs(), [20, 19, 13, 6, 1]);
        assert_eq!(r.total(), 59);
        let mut sfx = Vec::new();
        for _ in 0..30 + 126 {
            r.tick(&MenuInput::NONE, &mut sfx);
        }
        fn tbl(r: &ReportCard, slot: usize) -> Option<i16> {
            r.slots().get(slot).map(|s| s.tilemap)
        }
        // Row 0: 優 stamp and five full blocks.
        assert_eq!(tbl(&r, 100), Some(0x445));
        assert_eq!(tbl(&r, 115), Some(TBL_BLOCK));
        // Row 1 (19): four blocks and a half block.
        assert_eq!(tbl(&r, 100 + 1), Some(0x445));
        assert_eq!(tbl(&r, 110 + 6 + 4), Some(TBL_BLOCK));
        assert_eq!(tbl(&r, 110 + 6 + 5), Some(TBL_HALF_BLOCK));
        // Row 4 (1): lowest stamp, no blocks.
        assert_eq!(tbl(&r, 104), Some(0x448));
        assert_eq!(tbl(&r, 110 + 24 + 1), Some(-1));
        // Before 150: hundreds untouched, tens and units spin.
        assert_eq!(tbl(&r, 40), Some(-1));
        for _ in 0..30 {
            r.tick(&MenuInput::NONE, &mut sfx);
        }
        // Counter 155: hundreds hidden (< 100), tens settled, units still spinning (155 % 10).
        assert_eq!(tbl(&r, 41), Some(0x1b3 + 5));
        assert_eq!(tbl(&r, 42), Some(0x1b3 + 5));
        for _ in 0..20 {
            r.tick(&MenuInput::NONE, &mut sfx);
        }
        assert_eq!(tbl(&r, 42), Some(0x1b3 + 9));
    }
}

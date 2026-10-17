//! Ranking insert, name entry and the ranking page (`NameEntry_Create` 0x41e8d0, task
//! 0x41ebe0, page draw 0x41fc60, destroy 0x41f5a0; menus.md §11.2).
//!
//! * Insert (0x41e8d0): the run's ranking value goes into the route's table above the first row
//!   with a lower score, or an equal score and a lower PRO flag; below the tenth row there is no
//!   screen. The new row starts with a blank name.
//! * Name entry (page 0): a 59-symbol wheel scrolled by holding left/right (accelerating) or by
//!   the brake lever, a symbol taken by confirm or by raising the power lever from N. Four
//!   letters or 終 complete the name (filter 0x41e7a0, then the table is packed into the eeprom
//!   mirror, 0x41e670) and the page changes 30 ticks later; ← deletes. After 930 ticks the page
//!   changes anyway, with the name as far as it got, filtered. The original does **not** pack
//!   a timed-out name; the port does (decided port behaviour, menus.md §14).
//! * Ranking page (page 1): cells reveal one per tick with a 16-tick flicker, the new row's rank
//!   label blinks every 4 ticks; it ends after 150 ticks or on the A button or the horn.

use super::tables::{Placement, ScreenTables, TextDesc, placement_list};
use super::{MenuInput, Sfx, SlotSet};
use crate::save::{BLANK, MAX_SCORE, NAME_LEN, ROUTE_COUNT, ROWS, Row, Table};
use anyhow::{Result, ensure};
use ddg_formats::exe::Exe;
use ddg_render::sprites::SpriteLayer;

const WHAT: &str = "ranking tables";

/// Ranking table per route (`0x46bbe8`, i16, −1 = no ranking).
const ROUTE_TABLES: u32 = 0x46bbe8;
/// Page header maps per route (`0x46bc10`, `0x46bc64`; u32).
const HEADERS_A: u32 = 0x46bc10;
const HEADERS_B: u32 = 0x46bc64;
/// Rank labels: name entry (`0x46bcd8`) and ranking page (`0x46bd04`); u32 per rank.
const ENTRY_RANK_LABELS: u32 = 0x46bcd8;
const PAGE_RANK_LABELS: u32 = 0x46bd04;
/// The symbol sprites (`0x46bd30`, u32 × 59).
const SYMBOLS: u32 = 0x46bd30;
/// The two flicker sprites of a revealing cell (`0x46be1c`, `0x46be20`).
const FLICKER: u32 = 0x46be1c;
/// Cell x positions (`0x46be42`, i16 × 8: four name letters, four digits).
const CELL_X: u32 = 0x46be42;
/// Placement lists `{x, y, layer, palette, slot}`: ranking page (`0x46be54`) and name entry
/// (`0x46bec4`); they are 0x70 bytes apart.
const PAGE_LAYOUT: u32 = 0x46be54;
const ENTRY_LAYOUT: u32 = 0x46bec4;
const LAYOUT_MAX: usize = 11;
/// Per-route default names that replace a filtered name (`0x46bf14`, 4 bytes per route).
const DEFAULT_NAMES: u32 = 0x46bf14;
/// Filter lists (0x41e7a0), 4-byte entries until a first byte of 0xff: exact matches
/// (`0x46bf64`), matches ignoring a differing first symbol (`0x46bf88`), matches ignoring a
/// differing last symbol (`0x46bf94`). The lists end where the next begins.
const FILTER_EXACT: (u32, usize) = (0x46bf64, 9);
const FILTER_FIRST_DIFFERS: (u32, usize) = (0x46bf88, 3);
const FILTER_LAST_DIFFERS: (u32, usize) = (0x46bf94, 3);
/// Legend descriptors on slot 0x6d (0x41ebe0): keyboard/pad and analog lever.
const LEGEND_KEYS: u32 = 0x486184;
const LEGEND_LEVER: u32 = 0x48618a;

/// Alphabet size (`0x3a` is the last index, 0x41ebe0 wraps at it).
pub const SYMBOL_COUNT: usize = 59;
const SYM_DELETE: u8 = 57;
const SYM_END: u8 = 58;

/// The name-entry timeout and the delay after a completed name (0x41ebe0).
const TIMEOUT: i32 = 930;
const DONE_DELAY: i32 = 30;
const NOT_DONE: i32 = 99_999;
/// Ranking page: cells reveal until 0x8c, the page ends after 0x96.
const REVEAL_END: i32 = 0x8c;
const PAGE_END: i32 = 0x96;
const FLICKER_TICKS: i32 = 16;
const BLINK_TICKS: i32 = 4;

/// Wheel geometry (0x41ebe0): 32 px per symbol, y 360; the current symbol at (304, 352) when
/// at rest; neighbours 48 px either side, seven each way.
const STEP: i32 = 32;
const WHEEL_Y: i16 = 0x168;
const CENTRE_X: i32 = 0x130;
const CENTRE_REST_Y: i16 = 0x160;
const SIDE: usize = 7;
/// Scale of the scroll offset on the near neighbour (f64 0.5 at 0x457730) and on the centre
/// symbol (f64 −1.5 at 0x458c08).
const NEAR_SCALE: f64 = 0.5;
const CENTRE_SCALE: f64 = -1.5;
/// Entered letters: x 272 + 32k, y 252 (slots 0x55..).
const LETTERS_X: i16 = 0x110;
const LETTERS_Y: i16 = 0xfc;

/// Ranking page: rows at y 92 + 32·rank; the blinking rank label at x 96.
const ROW_Y: i16 = 0x5c;
const ROW_STEP: i16 = 0x20;
const RANK_LABEL_X: i16 = 0x60;

/// Slots.
const SLOT_RANK_LABEL: usize = 0xc;
const SLOT_CENTRE: usize = 0xd;
const SLOT_CELLS: usize = 0xd;
const SLOT_TIMER: usize = 0x18;
const SLOT_TIMER_LABEL: usize = 0x1c;
const SLOT_LEFT: usize = 0x45;
const SLOT_RIGHT: usize = 0x4d;
const SLOT_LETTERS: usize = 0x55;
const SLOT_BADGES: usize = 0x5d;
const SLOT_HEADER_A: usize = 0x67;
const SLOT_HEADER_B: usize = 0x68;
const SLOT_ENTRY_RANK: usize = 0x6b;
const SLOT_LEGEND: usize = 0x6d;
const LEGEND_POS: (i16, i16) = (0x198, 0x3c);

/// Fixed images of 0x41ebe0: name-entry decorations `(slot, tbl, x, y)` and the page's column
/// titles `(slot, tbl)`.
const ENTRY_ART: [(usize, i16, i16, i16); 5] = [
    (0x6c, 0x41d, 0x50, 0x48),
    (0xc, 0x41c, 0x120, 0x144),
    (0xf, 0x30, 0x70, 0x188),
    (0x10, 0x34, 0x50, 0x168),
    (0x11, 0x34, 0x1f0, 0x168),
];
const TBL_TIMER_LABEL: i16 = 0x261;
const PAGE_TITLES: [(usize, i16); 6] = [
    (0x6c, 0x413),
    (0x6d, 0x415),
    (0x6e, 0x416),
    (0x6f, 0x417),
    (0x70, 0x418),
    (0x71, 0x414),
];
const TBL_PRO: i16 = 0x2c8;
/// The remaining-time masks and the trailing sprite of the timer routine 0x417040.
const TBL_TIMER_MASKS: [i16; 2] = [0x40b, 0x40c];
const TBL_TIMER_TAIL: i16 = 0x40e;
const PALETTE: u8 = 4;
const LAYER: u8 = 3;

/// The page's black side bars (0x41fc60): x 0..80 and 560..640, full height.
const BAR_ARGB: u32 = 0xff00_0000;
const BARS: [[i32; 2]; 2] = [[0, 0x50], [0x230, 0x280]];
const SCREEN_H: i32 = 0x1e0;

/// Sounds (0x41ebe0 / 0x41f5a0).
const SND_SCROLL: u16 = 0x15;
const SND_LETTER: u16 = 0x17;
const SND_DELETE: u16 = 0x25;
const SND_DONE: u16 = 0x1d;
const BGM: u16 = 0x19d;

/// The name filter of 0x41e7a0.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NameFilter {
    pub exact: Vec<[u8; NAME_LEN]>,
    /// Matches when the last three symbols are equal and the first differs (the entries use
    /// 0x3c, which is no symbol, so in effect "any first symbol").
    pub first_differs: Vec<[u8; NAME_LEN]>,
    /// Matches when the first three symbols are equal and the last differs.
    pub last_differs: Vec<[u8; NAME_LEN]>,
}

impl NameFilter {
    pub fn matches(&self, n: &[u8; NAME_LEN]) -> bool {
        self.exact.iter().any(|e| e == n)
            || self
                .first_differs
                .iter()
                .any(|e| e[0] != n[0] && e[1..] == n[1..])
            || self
                .last_differs
                .iter()
                .any(|e| e[..3] == n[..3] && e[3] != n[3])
    }
}

/// The exe tables of the ranking screens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankingTables {
    pub route_tables: [i16; ROUTE_COUNT],
    pub headers: [[i16; 2]; ROUTE_COUNT],
    pub entry_rank_labels: [i16; ROWS],
    pub page_rank_labels: [i16; ROWS],
    pub symbols: [i16; SYMBOL_COUNT],
    pub flicker: [i16; 2],
    pub cell_x: [i16; 8],
    pub page_layout: Vec<Placement>,
    pub entry_layout: Vec<Placement>,
    pub default_names: [[u8; NAME_LEN]; ROUTE_COUNT],
    pub filter: NameFilter,
    pub legend_keys: TextDesc,
    pub legend_lever: TextDesc,
}

impl RankingTables {
    pub fn read(exe: &Exe<'_>) -> Result<Self> {
        fn ids<const N: usize>(exe: &Exe<'_>, va: u32) -> Result<[i16; N]> {
            let r = exe.reader_at(va, 4 * N, WHAT)?;
            let mut out = [0i16; N];
            for (i, v) in out.iter_mut().enumerate() {
                *v = i16::try_from(r.i32(4 * i)?)?;
            }
            Ok(out)
        }
        let i16s = |va: u32, out: &mut [i16]| -> Result<()> {
            let r = exe.reader_at(va, 2 * out.len(), WHAT)?;
            for (i, v) in out.iter_mut().enumerate() {
                *v = r.i16(2 * i)?;
            }
            Ok(())
        };
        let list = |(va, max): (u32, usize)| -> Result<Vec<[u8; NAME_LEN]>> {
            let b = exe.bytes_at(va, NAME_LEN * max)?;
            let entries = b.as_chunks::<NAME_LEN>().0;
            let n = entries
                .iter()
                .position(|c| c[0] == 0xff)
                .ok_or_else(|| anyhow::anyhow!("filter list at {va:#x} has no terminator"))?;
            Ok(entries[..n].to_vec())
        };
        let desc = |va: u32| -> Result<TextDesc> {
            let r = exe.reader_at(va, 6, WHAT)?;
            Ok(TextDesc {
                tbl: r.i16(0)?,
                col: r.i8(2)?,
                row: r.i8(3)?,
                w: r.i8(4)?,
                h: r.i8(5)?,
            })
        };
        let mut route_tables = [0i16; ROUTE_COUNT];
        i16s(ROUTE_TABLES, &mut route_tables)?;
        let mut cell_x = [0i16; 8];
        i16s(CELL_X, &mut cell_x)?;
        let a: [i16; ROUTE_COUNT] = ids(exe, HEADERS_A)?;
        let b: [i16; ROUTE_COUNT] = ids(exe, HEADERS_B)?;
        let names = exe.bytes_at(DEFAULT_NAMES, NAME_LEN * ROUTE_COUNT)?;
        let t = Self {
            route_tables,
            headers: std::array::from_fn(|r| [a[r], b[r]]),
            entry_rank_labels: ids(exe, ENTRY_RANK_LABELS)?,
            page_rank_labels: ids(exe, PAGE_RANK_LABELS)?,
            symbols: ids(exe, SYMBOLS)?,
            flicker: ids(exe, FLICKER)?,
            cell_x,
            page_layout: placement_list(exe, PAGE_LAYOUT, LAYOUT_MAX)?,
            entry_layout: placement_list(exe, ENTRY_LAYOUT, LAYOUT_MAX)?,
            default_names: std::array::from_fn(|r| {
                let c = &names[NAME_LEN * r..];
                [c[0], c[1], c[2], c[3]]
            }),
            filter: NameFilter {
                exact: list(FILTER_EXACT)?,
                first_differs: list(FILTER_FIRST_DIFFERS)?,
                last_differs: list(FILTER_LAST_DIFFERS)?,
            },
            legend_keys: desc(LEGEND_KEYS)?,
            legend_lever: desc(LEGEND_LEVER)?,
        };
        ensure!(
            t.route_tables
                .iter()
                .all(|&v| v == -1 || (0..crate::save::TABLE_COUNT as i16).contains(&v)),
            "ranking table index out of range"
        );
        Ok(t)
    }

    /// The route's ranking table, if it has one.
    pub fn table_of(&self, route: usize) -> Option<usize> {
        let t = *self.route_tables.get(route)?;
        usize::try_from(t).ok()
    }
}

#[derive(Debug, Clone)]
pub struct RankingDefs {
    pub t: RankingTables,
    /// The HUD's remaining-time digits (`0x4653d4`), used by the timer routine 0x417040.
    pub time_digits: [i16; 10],
}

impl RankingDefs {
    pub fn new(t: &ScreenTables) -> Self {
        Self {
            t: t.ranking.clone(),
            time_digits: t.time_digits,
        }
    }
}

/// Where a score goes: the first row with a lower score, or an equal one with a lower PRO flag
/// (0x41e8d0); `None` below the tenth row.
pub fn insert_position(rows: &Table, score: u16, pro: bool) -> Option<usize> {
    rows.iter()
        .position(|r| r.score < score || (r.score == score && !r.pro && pro))
}

/// Inserts the new row with a blank name, shifting the rows below down.
pub fn insert(rows: &mut Table, rank: usize, score: u16, pro: bool) {
    rows.copy_within(rank..ROWS - 1, rank + 1);
    rows[rank] = Row {
        name: [BLANK; NAME_LEN],
        score,
        pro,
    };
}

/// The wheel's scroll offset, held-key counter and current symbol (task +0x4c, +0x58, +0x3c).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Wheel {
    pub offset: i32,
    pub hold: i32,
    pub symbol: u8,
}

impl Wheel {
    /// One tick of scrolling. Returns whether a symbol boundary was crossed (sound 0x15).
    ///
    /// Keyboard/pad: holding left counts `hold` down and moves −2, −4, −8, −16 px/tick from
    /// the 1st, 5th, 10th and 15th tick; right the same upwards; with neither the wheel
    /// finishes the current step at 2 px/tick in the direction it was going. Analog lever: the
    /// brake notch picks the speed (0 → −16 … 3 → −2, 4–6 like no key, 7 → +2 … 10 → +16).
    pub fn scroll(&mut self, input: &MenuInput) -> bool {
        let settle = |o: i32| o + 2 * o.signum();
        let held = |hold: i32| match hold.abs() {
            0..5 => 2,
            5..10 => 4,
            10..15 => 8,
            _ => 16,
        } * hold.signum();
        match input.brake {
            None if input.left_held => {
                self.hold = self.hold.min(0) - 1;
                self.offset += held(self.hold);
            }
            None if input.right_held => {
                self.hold = self.hold.max(0) + 1;
                self.offset += held(self.hold);
            }
            None => {
                self.hold = 0;
                self.offset = settle(self.offset);
            }
            Some(b) => match b {
                0 => self.offset -= 16,
                1 => self.offset -= 8,
                2 => self.offset -= 4,
                3 => self.offset -= 2,
                4..=6 => self.offset = settle(self.offset),
                7 => self.offset += 2,
                8 => self.offset += 4,
                9 => self.offset += 8,
                10 => self.offset += 16,
                _ => {}
            },
        }
        let last = SYMBOL_COUNT as u8 - 1;
        if self.offset >= STEP {
            self.offset -= STEP;
            self.symbol = self.symbol.checked_sub(1).unwrap_or(last);
            true
        } else if self.offset < -(STEP - 1) {
            self.offset += STEP;
            self.symbol = if self.symbol >= last {
                0
            } else {
                self.symbol + 1
            };
            true
        } else {
            false
        }
    }
}

/// The run's data for the insert.
#[derive(Debug, Clone, Copy)]
pub struct RankingRequest<'a> {
    pub route: usize,
    /// `Rules::ranking_score` (capped at 9999 here as well).
    pub score: i32,
    pub pro: bool,
    /// Power notch at creation (task +0x54).
    pub power: u8,
    pub rankings: &'a [Table],
    /// Setting `save_timed_out_names`: a timed-out entry is packed like a completed one.
    pub save_timed_out: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Entry,
    Ranking,
}

pub struct NameEntry {
    defs: RankingDefs,
    route: usize,
    table: usize,
    rank: usize,
    rows: Table,
    page: Page,
    counter: i32,
    letters: usize,
    done_at: i32,
    wheel: Wheel,
    prev_power: u8,
    committed: bool,
    /// Setting `save_timed_out_names` ([`RankingRequest::save_timed_out`]).
    save_timed_out: bool,
    slots: SlotSet,
}

impl NameEntry {
    /// 0x41e8d0: `None` when the route has no ranking or the score is not in the top ten.
    pub fn new(defs: RankingDefs, req: &RankingRequest<'_>) -> Option<Self> {
        let table = defs.t.table_of(req.route)?;
        let mut rows = *req.rankings.get(table)?;
        let score = req.score.clamp(0, i32::from(MAX_SCORE)) as u16;
        let rank = insert_position(&rows, score, req.pro)?;
        insert(&mut rows, rank, score, req.pro);
        Some(Self {
            defs,
            route: req.route,
            table,
            rank,
            rows,
            page: Page::Entry,
            counter: 0,
            letters: 0,
            done_at: NOT_DONE,
            wheel: Wheel::default(),
            prev_power: req.power,
            committed: false,
            save_timed_out: req.save_timed_out,
            slots: SlotSet::default(),
        })
    }

    pub fn table(&self) -> usize {
        self.table
    }

    #[cfg(test)]
    pub fn rank(&self) -> usize {
        self.rank
    }

    /// The table as shown (the in-memory copy, `0x84fca0`).
    pub fn rows(&self) -> &Table {
        &self.rows
    }

    /// The table is to be packed into the eeprom mirror (0x41e670): the name was completed,
    /// or (port) the entry timed out.
    pub fn committed(&self) -> bool {
        self.committed
    }

    pub fn slots(&self) -> &SlotSet {
        &self.slots
    }

    /// 0x41e7a0 + 0x41e4c0: a filtered name becomes the route's default name.
    fn filter(&mut self) {
        let name = &mut self.rows[self.rank].name;
        if self.defs.t.filter.matches(name) {
            *name = self.defs.t.default_names[self.route];
        }
    }

    /// The name is complete: filter and pack (0x41e7a0, 0x41e670).
    fn complete(&mut self, sfx: &mut Vec<Sfx>) {
        sfx.push(Sfx::Play(SND_DONE));
        self.done_at = self.counter + DONE_DELAY;
        self.filter();
        self.committed = true;
    }

    /// One task update. Returns false when the task ends (destroy 0x41f5a0 stops the BGM).
    pub fn tick(&mut self, input: &MenuInput, sfx: &mut Vec<Sfx>) -> bool {
        // Analog lever: raising the power notch from N takes a symbol (task +0x54 / +0x56).
        let mut lever_take = false;
        if let Some(power) = input.notch {
            lever_take = self.prev_power == 0 && power > 0;
            self.prev_power = power;
        }
        if self.page == Page::Entry {
            if self.counter == 0 {
                self.create_entry(sfx);
            }
            let legend = if input.notch.is_some() {
                self.defs.t.legend_lever
            } else {
                self.defs.t.legend_keys
            };
            let (x, y) = LEGEND_POS;
            self.slots.text(SLOT_LEGEND, legend, x, y, PALETTE, LAYER);
        } else if self.counter == 0 {
            self.create_page();
        }
        self.counter += 1;
        match self.page {
            Page::Entry => {
                self.entry_tick(input, lever_take, sfx);
                true
            }
            Page::Ranking => {
                self.page_tick();
                let end = self.counter > PAGE_END || input.a || input.horn;
                if end {
                    sfx.push(Sfx::Stop(BGM));
                }
                !end
            }
        }
    }

    fn create_entry(&mut self, sfx: &mut Vec<Sfx>) {
        for p in &self.defs.t.entry_layout {
            self.slots.place(p);
        }
        self.slots.image(SLOT_TIMER_LABEL, TBL_TIMER_LABEL);
        let label = self.defs.t.entry_rank_labels[self.rank];
        self.slots.image(SLOT_ENTRY_RANK, label);
        for (slot, tbl, x, y) in ENTRY_ART {
            self.slots.full(slot, tbl, x, y, PALETTE, LAYER);
        }
        sfx.push(Sfx::Play(BGM));
    }

    fn entry_tick(&mut self, input: &MenuInput, lever_take: bool, sfx: &mut Vec<Sfx>) {
        if self.done_at == NOT_DONE {
            if self.wheel.scroll(input) {
                sfx.push(Sfx::Play(SND_SCROLL));
            }
            if input.confirm || lever_take {
                self.take(sfx);
            }
        }
        self.draw_entry();
        if self.counter == TIMEOUT || self.counter == self.done_at {
            self.page = Page::Ranking;
            self.counter = 0;
            self.filter();
            // Setting `save_timed_out_names` (decided port behaviour, menus.md §14.1): a
            // timed-out name is saved like a completed one. The original skips
            // `Ranking_ToEeprom` here, so the row lives only in the unpacked table and is lost
            // at exit (a completed name was committed by `complete`).
            self.committed |= self.save_timed_out;
        }
    }

    fn take(&mut self, sfx: &mut Vec<Sfx>) {
        let name = &mut self.rows[self.rank].name;
        match self.wheel.symbol {
            SYM_END => {
                name[self.letters..].fill(BLANK);
                self.letters = NAME_LEN;
                self.complete(sfx);
            }
            SYM_DELETE => {
                sfx.push(Sfx::Play(SND_DELETE));
                if self.letters > 0 {
                    self.letters -= 1;
                    name[self.letters] = BLANK;
                }
            }
            s => {
                sfx.push(Sfx::Play(SND_LETTER));
                name[self.letters] = s;
                self.letters += 1;
                if self.letters == NAME_LEN {
                    self.complete(sfx);
                }
            }
        }
    }

    fn symbol(&self, s: u8) -> i16 {
        self.defs.t.symbols[usize::from(s) % SYMBOL_COUNT]
    }

    fn draw_entry(&mut self) {
        let name = self.rows[self.rank].name;
        for (k, &c) in name.iter().enumerate() {
            let s = if k < self.letters { c } else { BLANK };
            let x = LETTERS_X + STEP as i16 * k as i16;
            let tbl = self.symbol(s);
            self.slots
                .full(SLOT_LETTERS + k, tbl, x, LETTERS_Y, PALETTE, LAYER);
        }
        let off = self.wheel.offset;
        let near = |cond: bool| {
            if cond {
                (f64::from(off) * NEAR_SCALE) as i32
            } else {
                0
            }
        };
        let last = SYMBOL_COUNT as u8 - 1;
        // Left side: symbols before the current one; a slot is only redrawn while on screen.
        let mut s = self.wheel.symbol;
        for i in 0..SIDE {
            s = s.checked_sub(1).unwrap_or(last);
            let x = off + 0xc0 - STEP * i as i32;
            if x >= 0x10 {
                let x = x + 0x40 + near(i == 0 && off > 0);
                let tbl = self.symbol(s);
                self.slots
                    .full(SLOT_LEFT + i, tbl, x as i16, WHEEL_Y, PALETTE, LAYER);
            }
        }
        let mut s = self.wheel.symbol;
        for i in 0..SIDE {
            s = if s >= last { 0 } else { s + 1 };
            let x = off + 0x120 + STEP * i as i32;
            if x <= 0x1d0 {
                let x = x + 0x40 + near(i == 0 && off < 0);
                let tbl = self.symbol(s);
                self.slots
                    .full(SLOT_RIGHT + i, tbl, x as i16, WHEEL_Y, PALETTE, LAYER);
            }
        }
        let (x, y) = if off == 0 {
            (CENTRE_X, CENTRE_REST_Y)
        } else {
            (CENTRE_X - (f64::from(off) * CENTRE_SCALE) as i32, WHEEL_Y)
        };
        let tbl = self.symbol(self.wheel.symbol);
        self.slots
            .full(SLOT_CENTRE, tbl, x as i16, y, PALETTE, LAYER);
        self.draw_timer(TIMEOUT - self.counter);
    }

    /// The HUD timer routine 0x417040 on slots 0x18–0x1b: seconds, flickering through the two
    /// masks in the last four ticks of each second.
    fn draw_timer(&mut self, ticks: i32) {
        let t = ticks.max(0);
        let (secs, sub) = (t / 30, t % 30);
        let tbls = if sub < 26 {
            let d = super::digits3(secs);
            d.map(|d| self.defs.time_digits[d])
        } else if sub < 28 {
            [TBL_TIMER_MASKS[0]; 3]
        } else {
            [TBL_TIMER_MASKS[1]; 3]
        };
        for (i, tbl) in tbls.into_iter().enumerate() {
            self.slots.image(SLOT_TIMER + i, tbl);
        }
        self.slots.image(SLOT_TIMER + 3, TBL_TIMER_TAIL);
    }

    fn create_page(&mut self) {
        for p in &self.defs.t.page_layout {
            self.slots.place(p);
        }
        self.slots.image(SLOT_ENTRY_RANK, -1);
        let [a, b] = self.defs.t.headers[self.route];
        self.slots.image(SLOT_HEADER_A, a);
        self.slots.image(SLOT_HEADER_B, b);
        for (slot, tbl) in PAGE_TITLES {
            self.slots.image(slot, tbl);
        }
        let blank = self.symbol(BLANK);
        for row in 0..ROWS {
            let y = ROW_Y + ROW_STEP * row as i16;
            for (c, &x) in self.defs.t.cell_x.iter().enumerate() {
                self.slots
                    .full(SLOT_CELLS + 8 * row + c, blank, x, y, PALETTE, LAYER);
            }
            let x = self.defs.t.cell_x[3] + 0x20;
            self.slots.place_at(SLOT_BADGES + row, x, y, LAYER, PALETTE);
        }
    }

    fn page_tick(&mut self) {
        let y = ROW_Y + ROW_STEP * self.rank as i16;
        if (self.counter / BLINK_TICKS) % 2 == 0 {
            self.slots.image(SLOT_RANK_LABEL, -1);
        } else {
            let label = self.defs.t.page_rank_labels[self.rank];
            self.slots
                .full(SLOT_RANK_LABEL, label, RANK_LABEL_X, y, PALETTE, LAYER);
        }
        if self.counter >= REVEAL_END {
            return;
        }
        for u in 0..ROWS * 8 {
            let t = self.counter - u as i32;
            if t < 0 {
                continue;
            }
            let (row, c) = (u / 8, u % 8);
            let x = self.defs.t.cell_x[c];
            let y = ROW_Y + ROW_STEP * row as i16;
            let tbl = if t < FLICKER_TICKS {
                self.defs.t.flicker[(t % 2) as usize]
            } else {
                let r = &self.rows[row];
                let digits = r.digits();
                if c == 3 {
                    let badge = if r.pro { TBL_PRO } else { -1 };
                    self.slots.image(SLOT_BADGES + row, badge);
                }
                self.symbol(if c < NAME_LEN {
                    r.name[c]
                } else {
                    digits[c - NAME_LEN]
                })
            };
            self.slots.full(SLOT_CELLS + u, tbl, x, y, PALETTE, LAYER);
        }
    }

    /// The slots, plus the page's side bars (draw callback 0x41fc60, page only).
    pub fn draw(&self, layer: &mut SpriteLayer) {
        self.slots.draw(layer);
        if self.page == Page::Ranking {
            for [x0, x1] in BARS {
                layer.add_fan(
                    &[[x0, 0], [x1, 0], [x1, SCREEN_H], [x0, SCREEN_H]],
                    BAR_ARGB,
                );
            }
        }
    }

    #[cfg(test)]
    fn on_page(&self) -> bool {
        self.page == Page::Ranking
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn defs() -> RankingDefs {
        RankingDefs {
            t: tables(),
            time_digits: [0; 10],
        }
    }

    /// Synthetic tables: route 1 → table 3, a filter like the original's.
    pub(crate) fn tables() -> RankingTables {
        let mut route_tables = [-1; ROUTE_COUNT];
        route_tables[1] = 3;
        RankingTables {
            route_tables,
            headers: [[0x424, 0x42d]; ROUTE_COUNT],
            entry_rank_labels: [0x419; ROWS],
            page_rank_labels: [0x2ce; ROWS],
            symbols: std::array::from_fn(|i| 0x271 + i as i16),
            flicker: [0x274, 0x275],
            cell_x: [144, 176, 208, 240, 360, 392, 424, 472],
            page_layout: Vec::new(),
            entry_layout: Vec::new(),
            default_names: [[16, 3, 55, BLANK]; ROUTE_COUNT],
            filter: NameFilter {
                exact: vec![[BLANK; 4], [22, 22, 22, 22]],
                first_differs: vec![[60, 22, 22, 22]],
                last_differs: vec![[22, 22, 22, 60], [30, 16, 35, 36]],
            },
            legend_keys: TextDesc::default(),
            legend_lever: TextDesc::default(),
        }
    }

    fn table(scores: [u16; ROWS], pro: [bool; ROWS]) -> Table {
        std::array::from_fn(|i| Row {
            name: [12, 13, 14, 15],
            score: scores[i],
            pro: pro[i],
        })
    }

    #[test]
    fn insert_order_with_pro_ties() {
        let mut pro = [false; ROWS];
        pro[2] = true;
        let t = table([900, 800, 700, 700, 600, 500, 400, 300, 200, 100], pro);
        // Higher than everything: top.
        assert_eq!(insert_position(&t, 950, false), Some(0));
        // A tie without the PRO flag goes below every equal row.
        assert_eq!(insert_position(&t, 700, false), Some(4));
        // A PRO tie goes above an equal row without it, below an equal PRO row.
        assert_eq!(insert_position(&t, 700, true), Some(3));
        assert_eq!(insert_position(&t, 800, true), Some(1));
        // A tie with the last row does not enter; below it neither.
        assert_eq!(insert_position(&t, 100, false), None);
        assert_eq!(insert_position(&t, 100, true), Some(9));
        assert_eq!(insert_position(&t, 50, true), None);

        let mut rows = t;
        insert(&mut rows, 3, 700, true);
        assert_eq!(
            rows.map(|r| r.score),
            [900, 800, 700, 700, 700, 600, 500, 400, 300, 200]
        );
        assert_eq!(rows[3].name, [BLANK; 4]);
        assert!(rows[3].pro && rows[2].pro && !rows[4].pro);
    }

    #[test]
    fn name_filter() {
        let f = defs().t.filter;
        assert!(f.matches(&[BLANK; 4]));
        assert!(f.matches(&[22, 22, 22, 22]));
        // 0x3c is no symbol: any first symbol matches the rest.
        assert!(f.matches(&[12, 22, 22, 22]));
        assert!(f.matches(&[22, 22, 22, 12]));
        // The last-differs rule rejects every name starting with the three symbols except the
        // listed one itself.
        assert!(f.matches(&[30, 16, 35, BLANK]));
        assert!(!f.matches(&[30, 16, 35, 36]));
        assert!(!f.matches(&[12, 13, 14, 15]));
        assert!(!f.matches(&[22, 22, 12, 22]));
    }

    fn held(left: bool, right: bool) -> MenuInput {
        MenuInput {
            left_held: left,
            right_held: right,
            ..MenuInput::NONE
        }
    }

    #[test]
    fn wheel_accelerates_and_settles() {
        let mut w = Wheel::default();
        let mut offsets = Vec::new();
        let mut crossings = Vec::new();
        for n in 1..=20 {
            if w.scroll(&held(false, true)) {
                crossings.push(n);
            }
            offsets.push(w.offset);
        }
        // +2 ×4, +4 ×5, +8 ×5, then +16, wrapping at 32 (symbol −1 each time).
        assert_eq!(
            offsets,
            [
                2, 4, 6, 8, 12, 16, 20, 24, 28, 4, 12, 20, 28, 4, 20, 4, 20, 4, 20, 4
            ]
        );
        assert_eq!(crossings, [10, 14, 16, 18, 20]);
        assert_eq!(usize::from(w.symbol), SYMBOL_COUNT - 5);
        // Release mid-step: the wheel finishes the step at 2 px/tick in the same direction.
        w.scroll(&held(false, true));
        assert_eq!((w.offset, w.hold), (20, 21));
        let mut n = 0;
        while !w.scroll(&held(false, false)) {
            n += 1;
        }
        assert_eq!((n, w.offset, w.hold), (5, 0, 0));
        // Left from rest: −2, and the counter restarts.
        w.scroll(&held(true, false));
        assert_eq!((w.offset, w.hold), (-2, -1));
        // Left wins when both are held.
        w.scroll(&held(true, true));
        assert_eq!((w.offset, w.hold), (-4, -2));
    }

    #[test]
    fn lever_scroll_speeds() {
        for (brake, step) in [(0, -16), (3, -2), (5, 0), (7, 2), (9, 8)] {
            let mut w = Wheel::default();
            w.scroll(&MenuInput {
                brake: Some(brake),
                notch: Some(0),
                ..MenuInput::NONE
            });
            assert_eq!(w.offset, step, "brake {brake}");
        }
    }

    fn request(rankings: &[Table]) -> RankingRequest<'_> {
        RankingRequest {
            route: 1,
            score: 650,
            pro: false,
            power: 0,
            rankings,
            save_timed_out: true,
        }
    }

    #[test]
    fn enter_a_name_and_show_the_page() {
        let mut rankings = vec![[Row::EMPTY; ROWS]; 13];
        rankings[3] = table(
            [900, 800, 700, 700, 600, 500, 400, 300, 200, 100],
            [false; ROWS],
        );
        assert!(
            NameEntry::new(
                defs(),
                &RankingRequest {
                    route: 0,
                    ..request(&rankings)
                }
            )
            .is_none()
        );
        let mut e = NameEntry::new(defs(), &request(&rankings)).unwrap();
        assert_eq!((e.table(), e.rank()), (3, 4));
        let mut sfx = Vec::new();
        let press = MenuInput {
            confirm: true,
            ..MenuInput::NONE
        };
        // Tick 1 places the screen and starts the BGM; take symbol 0 ("0").
        assert!(e.tick(&press, &mut sfx));
        assert_eq!(sfx, [Sfx::Play(BGM), Sfx::Play(SND_LETTER)]);
        // One tick of right: the wheel completes the step (2 px/tick) to symbol 58 = 終.
        e.tick(&held(false, true), &mut sfx);
        let mut n = 1;
        while e.wheel.offset != 0 {
            assert!(e.tick(&MenuInput::NONE, &mut sfx));
            n += 1;
        }
        assert_eq!((n, e.wheel.symbol), (16, SYM_END));
        sfx.clear();
        e.tick(&press, &mut sfx);
        assert_eq!(sfx, [Sfx::Play(SND_DONE)]);
        assert!(e.committed());
        assert_eq!(e.rows()[4].name, [0, BLANK, BLANK, BLANK]);
        // The page follows 30 ticks later and lasts until its counter passes 150.
        let mut ticks = 0;
        while !e.on_page() {
            e.tick(&MenuInput::NONE, &mut sfx);
            ticks += 1;
        }
        assert_eq!(ticks, 30);
        let mut ticks = 0;
        while e.tick(&MenuInput::NONE, &mut sfx) {
            ticks += 1;
        }
        assert_eq!(ticks, 150);
        assert_eq!(sfx.last(), Some(&Sfx::Stop(BGM)));
    }

    #[test]
    fn lever_takes_on_power_from_n() {
        let rankings = vec![[Row::EMPTY; ROWS]; 13];
        let mut e = NameEntry::new(defs(), &request(&rankings)).unwrap();
        let lever = |power: u8| MenuInput {
            notch: Some(power),
            brake: Some(5),
            ..MenuInput::NONE
        };
        let mut sfx = Vec::new();
        e.tick(&lever(0), &mut sfx);
        e.tick(&lever(2), &mut sfx);
        e.tick(&lever(3), &mut sfx);
        e.tick(&lever(0), &mut sfx);
        e.tick(&lever(1), &mut sfx);
        assert_eq!(e.letters, 2);
        assert_eq!(
            sfx.iter().filter(|s| **s == Sfx::Play(SND_LETTER)).count(),
            2
        );
    }

    #[test]
    fn timeout_filters_and_commits() {
        let rankings = vec![[Row::EMPTY; ROWS]; 13];
        let mut e = NameEntry::new(defs(), &request(&rankings)).unwrap();
        assert_eq!(e.rank(), 0);
        let mut sfx = Vec::new();
        let mut n = 0;
        while !e.on_page() {
            e.tick(&MenuInput::NONE, &mut sfx);
            n += 1;
        }
        assert_eq!(n, TIMEOUT);
        // The original leaves it unpacked; the port saves it (menus.md §14).
        assert!(e.committed());
        // The all-blank name is in the exact list: replaced by the route's default name.
        assert_eq!(e.rows()[0].name, [16, 3, 55, BLANK]);
    }

    /// Setting `save_timed_out_names` off: the original's time-out, shown on the page (filtered)
    /// but never packed into the eeprom mirror.
    #[test]
    fn timeout_without_the_setting_is_not_committed() {
        let rankings = vec![[Row::EMPTY; ROWS]; 13];
        let req = RankingRequest {
            save_timed_out: false,
            ..request(&rankings)
        };
        let mut e = NameEntry::new(defs(), &req).unwrap();
        let mut sfx = Vec::new();
        while !e.on_page() {
            e.tick(&MenuInput::NONE, &mut sfx);
        }
        assert!(!e.committed());
        assert_eq!(e.rows()[0].name, [16, 3, 55, BLANK]);
    }

    #[test]
    fn reads_ranking_tables() {
        let Ok(data) = crate::scene::GameData::from_env() else {
            eprintln!("DDG_DATA_DIR unset; skipping");
            return;
        };
        let bytes = data.exe().unwrap();
        let exe = Exe::parse(&bytes).unwrap();
        let t = RankingTables::read(&exe).unwrap();
        // menus.md §13.1: routes 0, 2, 5 have no table; 8 and 9 share one.
        assert_eq!(t.table_of(0), None);
        assert_eq!(t.table_of(8), t.table_of(9));
        assert_eq!(t.symbols[11], t.symbols[BLANK as usize]);
        assert_eq!(t.cell_x[0], 144);
        assert_eq!(t.page_layout.len(), 10);
        assert_eq!(t.entry_layout.len(), 7);
        assert_eq!(
            (
                t.filter.exact.len(),
                t.filter.first_differs.len(),
                t.filter.last_differs.len()
            ),
            (8, 2, 2)
        );
        assert!(t.filter.matches(&[BLANK; 4]));
    }
}

//! ランキングを見る: the records viewer (`Records_Create` / `_Update` / `_Draw` / `_Destroy`
//! 0x41f5e0 / 0x41f910 / 0x41fc60 / 0x41fd00, menus.md §5).
//!
//! It pages through the routes whose bit is set in `0x8500b0` and that own a ranking table.
//! From the title (interactive, `0x855ffc & 8`) a page fills in and then holds at counter 175
//! until confirm turns the page; cancel leaves. In attract it turns pages every 200 ticks.

use super::snd;
use crate::save::Table;
use crate::screens::ranking::RankingTables;
use crate::screens::{MenuInput, Sfx};
use ddg_render::sprites::SpriteLayer;

/// `0x8500b0`: written once by `Ranking_FromEeprom` 0x41e520 as the immediate 0x3f9da (routes 1,
/// 3, 4, 6–8, 11–17: one route per ranking table); read by this viewer and the attract
/// demo-route picker only.
pub const ROUTE_MASK: u32 = 0x3f9da;
/// Pages are looked for past route 12 only while they keep succeeding (0x41f910).
const LAST_ROUTE_BEFORE_END: usize = 12;
/// A page's counter: cells appear every 2 ticks, the interactive viewer holds at 175, the page
/// turns past 199 (0x41f910).
const HOLD: i32 = 175;
const PAGE_END: i32 = 200;
/// Header slide (+0x40): 33 steps of 8 px.
const SLIDE_STEPS: i32 = 33;
const FLICKER_TICKS: i32 = 16;
const CELLS: usize = 80;

const SLOT_HEADER_A: usize = 0x67;
const SLOT_HEADER_B: usize = 0x68;
const SLOT_COLUMNS: usize = 0x6c;
const SLOT_CELLS: usize = 0xd;
const SLOT_BADGES: usize = 0x5d;
const SLOT_LEGEND: usize = 0x87;
/// Column titles (0x41f5e0 immediates), slots 0x6c..0x71.
const COLUMN_TITLES: [i16; 6] = [0x413, 0x415, 0x416, 0x417, 0x418, 0x414];
/// The PRO badge (0x41f910).
const TBL_PRO: i16 = 0x2c8;
/// Legends (0x41f910): keyboard, pad / levers; at (128, 432).
const TBL_LEGEND_KEYS: i16 = 0x545;
const TBL_LEGEND_PAD: i16 = 0x546;
const LEGEND_POS: (i16, i16) = (0x80, 0x1b0);
const PAL: u8 = 4;
const LAYER: u8 = 3;
/// First cell row y and the row pitch.
const ROW_Y: i16 = 0x5c;
const ROW_DY: i16 = 0x20;
const HEADER_Y: i16 = 0x28;
/// The black side bars of the draw callback 0x41fc60 (x 0..80 and 560..640).
const BAR_ARGB: u32 = 0xff00_0000;
const BARS: [[i32; 2]; 2] = [[0, 0x50], [0x230, 0x280]];
const SCREEN_H: i32 = 0x1e0;
/// The blank name symbol (`0x46bd5c` = symbol 11).
const BLANK_SYMBOL: usize = 11;

/// What the viewer did this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordsStep {
    Running,
    /// The task ended (last page passed, or the tick after a cancel).
    Done,
}

#[derive(Debug, Clone)]
pub struct Records {
    interactive: bool,
    /// Page counter (+0x30).
    counter: i32,
    /// Route of the page (+0x38) and its ranking table (+0x50).
    route: usize,
    table: usize,
    /// Header slide step (+0x40).
    slide: i32,
    /// Cancelled (+0x5c): the task ends next tick.
    leaving: bool,
}

/// The next route at or after `from` with a page, or `None` (the viewer ends) once past
/// route 12 (0x41f5e0 / 0x41f910).
fn next_page(t: &RankingTables, mut route: usize, first: bool) -> Option<(usize, usize)> {
    loop {
        if route < 32
            && ROUTE_MASK & (1 << route) != 0
            && let Some(table) = t.table_of(route)
        {
            return Some((route, table));
        }
        if !first && route > LAST_ROUTE_BEFORE_END {
            return None;
        }
        route += 1;
        if route >= 32 {
            return None;
        }
    }
}

impl Records {
    /// `Records_Create` 0x41f5e0: places the page and the first route's headers.
    pub fn new(t: &RankingTables, interactive: bool, l: &mut SpriteLayer) -> Option<Self> {
        let (route, table) = next_page(t, 0, true)?;
        for p in &t.page_layout {
            super::place(l, p, 0, 0);
        }
        for (i, &tbl) in COLUMN_TITLES.iter().enumerate() {
            super::image(l, SLOT_COLUMNS + i, tbl);
        }
        super::move_to(l, SLOT_HEADER_A, -240, 0);
        super::move_to(l, SLOT_HEADER_B, 512, 0);
        let blank = t.symbols[BLANK_SYMBOL];
        for row in 0..10 {
            let y = ROW_Y + ROW_DY * row as i16;
            for (col, &x) in t.cell_x.iter().enumerate() {
                super::put(l, SLOT_CELLS + 8 * row + col, blank, x, y, PAL, LAYER);
            }
            if let Some(s) = l.slots.get_mut(SLOT_BADGES + row) {
                s.x = t.cell_x[3] + 0x20;
                s.y = y;
                s.layer = LAYER;
                s.palette = PAL;
            }
        }
        let r = Self {
            interactive,
            counter: 0,
            route,
            table,
            slide: 0,
            leaving: false,
        };
        r.header_images(t, l);
        Some(r)
    }

    fn header_images(&self, t: &RankingTables, l: &mut SpriteLayer) {
        let [a, b] = t.headers[self.route];
        super::image(l, SLOT_HEADER_A, a);
        super::image(l, SLOT_HEADER_B, b);
    }

    #[cfg(test)]
    pub fn route(&self) -> usize {
        self.route
    }

    /// One tick of 0x41f910. `pad_legend` is the analog-lever or pad (mode 4) case of the
    /// legend. `rankings` are the unpacked tables (`0x84fca0` / `0x84fc00`).
    pub fn tick(
        &mut self,
        input: &MenuInput,
        pad_legend: bool,
        t: &RankingTables,
        rankings: &[Table],
        l: &mut SpriteLayer,
        sfx: &mut Vec<Sfx>,
    ) -> RecordsStep {
        if self.interactive {
            if self.leaving {
                return RecordsStep::Done;
            }
            if input.confirm {
                sfx.push(Sfx::Play(snd::DECIDE_SOFT));
                if self.counter < HOLD {
                    self.counter = HOLD;
                    self.slide = SLIDE_STEPS - 1;
                } else {
                    self.counter = PAGE_END;
                }
            } else if input.cancel {
                sfx.push(Sfx::Play(snd::CANCEL));
                self.leaving = true;
                return RecordsStep::Running;
            }
            let legend = if pad_legend {
                TBL_LEGEND_PAD
            } else {
                TBL_LEGEND_KEYS
            };
            super::put(
                l,
                SLOT_LEGEND,
                legend,
                LEGEND_POS.0,
                LEGEND_POS.1,
                PAL,
                LAYER,
            );
        }
        if self.slide < SLIDE_STEPS {
            super::move_to(l, SLOT_HEADER_A, (self.slide * 8 - 0xb0) as i16, HEADER_Y);
            super::move_to(l, SLOT_HEADER_B, ((0x48 - self.slide) * 8) as i16, HEADER_Y);
            self.slide += 1;
        }
        super::image(l, SLOT_HEADER_B, t.headers[self.route][1]);
        self.draw_cells(t, rankings, l);
        if !self.interactive || self.counter < HOLD {
            self.counter += 1;
        }
        if self.counter >= PAGE_END {
            self.counter = 0;
            self.slide = 0;
            match next_page(t, self.route + 1, false) {
                Some((route, table)) => {
                    self.route = route;
                    self.table = table;
                }
                None => return RecordsStep::Done,
            }
            super::move_to(l, SLOT_HEADER_A, -0x130, HEADER_Y);
            super::move_to(l, SLOT_HEADER_B, 0x240, HEADER_Y);
            self.header_images(t, l);
        }
        RecordsStep::Running
    }

    fn draw_cells(&self, t: &RankingTables, rankings: &[Table], l: &mut SpriteLayer) {
        let Some(rows) = rankings.get(self.table) else {
            return;
        };
        for u in 0..CELLS {
            let k = self.counter - 2 * u as i32;
            if k < 0 {
                continue;
            }
            let (row, col) = (u / 8, u % 8);
            let x = t.cell_x[col];
            let y = ROW_Y + ROW_DY * row as i16;
            if k < FLICKER_TICKS {
                let tbl = t.flicker[(k % 2) as usize];
                super::put(l, SLOT_CELLS + u, tbl, x, y, PAL, LAYER);
                continue;
            }
            let r = &rows[row];
            let symbol = if col < 4 {
                usize::from(r.name[col])
            } else {
                usize::from(r.digits()[col - 4])
            };
            let tbl = t.symbols.get(symbol).copied().unwrap_or(-1);
            super::put(l, SLOT_CELLS + u, tbl, x, y, PAL, LAYER);
            if col == 3 {
                super::image(l, SLOT_BADGES + row, if r.pro { TBL_PRO } else { -1 });
            }
        }
    }

    /// The draw callback 0x41fc60: black bars either side of the 512-px page (every frame).
    pub fn draw_bars(l: &mut SpriteLayer) {
        for [x0, x1] in BARS {
            l.add_fan(
                &[[x0, 0], [x1, 0], [x1, SCREEN_H], [x0, SCREEN_H]],
                BAR_ARGB,
            );
        }
    }

    /// `Records_Destroy` 0x41fd00: every non-persistent slot cleared.
    pub fn destroy(l: &mut SpriteLayer) {
        super::clear_all(l);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::save::Row;

    fn tables() -> RankingTables {
        crate::front::tests_support::ranking_tables()
    }

    fn press(f: impl FnOnce(&mut MenuInput)) -> MenuInput {
        let mut i = MenuInput::NONE;
        f(&mut i);
        i
    }

    #[test]
    fn pages_follow_the_mask() {
        let t = tables();
        let mut routes = vec![];
        let mut r = next_page(&t, 0, true);
        while let Some((route, _)) = r {
            routes.push(route);
            r = next_page(&t, route + 1, false);
        }
        // One page per ranking table (menus.md §5 listed 9 and 10 instead of 11).
        assert_eq!(routes, [1, 3, 4, 6, 7, 8, 11, 12, 13, 14, 15, 16, 17]);
    }

    #[test]
    fn interactive_page_holds_until_confirm() {
        let t = tables();
        let rankings = vec![[Row::EMPTY; 10]; 13];
        let mut l = SpriteLayer::default();
        let mut sfx = vec![];
        let mut r = Records::new(&t, true, &mut l).unwrap();
        for _ in 0..400 {
            r.tick(&MenuInput::NONE, false, &t, &rankings, &mut l, &mut sfx);
        }
        assert_eq!((r.route(), r.counter), (1, HOLD));
        r.tick(
            &press(|i| i.confirm = true),
            false,
            &t,
            &rankings,
            &mut l,
            &mut sfx,
        );
        assert_eq!(r.route(), 3, "confirm at the hold turns the page");
        r.tick(
            &press(|i| i.confirm = true),
            false,
            &t,
            &rankings,
            &mut l,
            &mut sfx,
        );
        assert_eq!(r.counter, HOLD, "confirm while filling reveals everything");
        let step = r.tick(
            &press(|i| i.cancel = true),
            false,
            &t,
            &rankings,
            &mut l,
            &mut sfx,
        );
        assert_eq!(step, RecordsStep::Running);
        let step = r.tick(&MenuInput::NONE, false, &t, &rankings, &mut l, &mut sfx);
        assert_eq!(step, RecordsStep::Done, "the tick after the cancel");
        assert_eq!(
            sfx,
            [
                Sfx::Play(snd::DECIDE_SOFT),
                Sfx::Play(snd::DECIDE_SOFT),
                Sfx::Play(snd::CANCEL)
            ]
        );
    }

    #[test]
    fn attract_pages_turn_every_200_ticks() {
        let t = tables();
        let rankings = vec![[Row::EMPTY; 10]; 13];
        let mut l = SpriteLayer::default();
        let mut sfx = vec![];
        let mut r = Records::new(&t, false, &mut l).unwrap();
        let mut ticks = 0;
        while r.tick(&MenuInput::NONE, false, &t, &rankings, &mut l, &mut sfx)
            == RecordsStep::Running
        {
            ticks += 1;
        }
        assert_eq!(ticks, 13 * 200 - 1);
    }
}

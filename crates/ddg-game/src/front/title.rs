//! Title menu (`TitleMenu_Create` / `_Update` / `_Draw` 0x42b210 / 0x42b2f0 / 0x42b530,
//! menus.md §3).
//!
//! Items: アーケードモード, ゲーム設定, ランキングを見る, ゲーム終了 and, with the DDG
//! controller (control mode 6) only, 専用コントローラー設定. The sixth label on the sheet,
//! LOVE特急こまち (the Komachi movie), has no descriptor and no cursor position: the cursor
//! maximum is 3 or 4, so the menu cannot reach it (the dispatch in 0x42a1c0 still has a case 5).

use super::tables::FrontTables;
use super::{Env, PAL_MENU, ScrollList, snd};
use crate::screens::{MenuInput, Sfx};
use ddg_render::sprites::SpriteLayer;

/// Header page of the title (0x42a820(0)).
const HEADER_PAGE: usize = 0;
/// Ticks the cursor frame blinks after a decision.
pub const BLINK_TICKS: i32 = 30;
/// Item slots 0x93.. (0x42b530).
const SLOT_ITEMS: usize = 0x93;
/// Item x offset from the centred cursor frame and y of the first item.
const ITEM_DX: i16 = 0x14;
const ITEM_Y: i32 = 0x8e;
const LAYER: u8 = 3;

/// The title menu's result (`0x816834`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleChoice {
    /// 0: アーケードモード → route select.
    Arcade,
    /// 1: ゲーム設定 → options.
    Options,
    /// 2: ランキングを見る → records viewer.
    Ranking,
    /// 3: ゲーム終了 → exit confirmation.
    Exit,
    /// 4: 専用コントローラー設定 → calibration (control mode 6 only).
    Calibrate,
    /// −2: cancel → `Game_MainFlow` shows the attract title again.
    Back,
}

impl TitleChoice {
    fn from_index(i: i32) -> Self {
        match i {
            0 => Self::Arcade,
            1 => Self::Options,
            2 => Self::Ranking,
            3 => Self::Exit,
            _ => Self::Calibrate,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TitleMenu {
    list: ScrollList,
    /// The decide blink countdown (+0x3c).
    blink: i32,
    /// The cursor frame's x.
    x: i16,
}

impl TitleMenu {
    /// `0x42b210(cursor)`: the list settled on `cursor` at once.
    pub fn new(cursor: i32, cursor_x: i16) -> Self {
        Self {
            list: ScrollList::settled(cursor),
            blink: 0,
            x: cursor_x,
        }
    }

    /// The last cursor index: four items, five with the DDG controller (mode 6).
    pub fn last(env: &Env) -> i32 {
        if env.control_mode == 6 { 4 } else { 3 }
    }

    #[cfg(test)]
    pub fn cursor(&self) -> i32 {
        self.list.cursor
    }

    /// One tick of 0x42b2f0. Returns the choice when the task ends.
    pub fn tick(
        &mut self,
        input: &MenuInput,
        env: &Env,
        t: &FrontTables,
        l: &mut SpriteLayer,
        sfx: &mut Vec<Sfx>,
    ) -> Option<TitleChoice> {
        let last = Self::last(env);
        if self.blink != 0 {
            self.blink -= 1;
            if self.blink == 0 {
                sfx.push(Sfx::Stop(snd::DECIDE));
                return Some(TitleChoice::from_index(self.list.cursor));
            }
        } else if self.list.distance() != 0 {
            self.list.approach(env.analog);
        } else if input.confirm {
            sfx.push(Sfx::Play(snd::DECIDE));
            self.blink = BLINK_TICKS;
        } else if input.cancel {
            sfx.push(Sfx::Play(snd::CANCEL));
            return Some(TitleChoice::Back);
        } else if self.list.step(input, last) {
            sfx.push(Sfx::Play(snd::MOVE));
        }
        self.draw(env, t, l);
        None
    }

    /// 0x42b530.
    fn draw(&self, env: &Env, t: &FrontTables, l: &mut SpriteLayer) {
        super::draw_menu_header(l, t, env, HEADER_PAGE);
        let last = Self::last(env);
        for (i, &d) in t.title_items.iter().take(last as usize + 1).enumerate() {
            let y = ITEM_Y + ROW_I * i as i32 + self.list.scroll;
            super::text(
                l,
                SLOT_ITEMS + i,
                d,
                self.x + ITEM_DX,
                y as i16,
                PAL_MENU,
                LAYER,
            );
        }
        super::draw_cursor(l, self.x, &self.list, last, self.blink, LAYER);
    }
}

const ROW_I: i32 = super::ROW;

/// The Komachi movie (`FUN_00429110`, title case 5) is reached only from the attract item
/// 0x404e10, and only when every stage of the route select has a clear mark (0x404cf0 walks
/// the five group lists, menus.md §2). Decision D2 postpones the movie itself.
pub fn komachi_unlocked(t: &FrontTables, clear_mark: impl Fn(usize) -> u8) -> bool {
    t.group_stages.iter().flatten().all(|&s| {
        usize::try_from(t.stages[s].route)
            .map(|r| clear_mark(r) != 0)
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(f: impl FnOnce(&mut MenuInput)) -> MenuInput {
        let mut i = MenuInput::NONE;
        f(&mut i);
        i
    }

    fn run(m: &mut TitleMenu, env: &Env, input: MenuInput) -> (Option<TitleChoice>, Vec<Sfx>) {
        let t = super::super::tests_support::tables();
        let mut l = SpriteLayer::default();
        let mut sfx = Vec::new();
        let r = m.tick(&input, env, &t, &mut l, &mut sfx);
        (r, sfx)
    }

    #[test]
    fn cursor_is_clamped_by_control_mode() {
        let env = Env::default();
        let mut m = TitleMenu::new(0, 0);
        let down = press(|i| i.down = true);
        for _ in 0..40 {
            run(&mut m, &env, down);
        }
        assert_eq!(m.cursor(), 3, "four items without the DDG controller");
        let env6 = Env {
            control_mode: 6,
            ..env
        };
        for _ in 0..40 {
            run(&mut m, &env6, down);
        }
        assert_eq!(m.cursor(), 4, "five with it; never the Komachi item");
    }

    #[test]
    fn decide_blinks_thirty_ticks() {
        let env = Env::default();
        let mut m = TitleMenu::new(2, 0);
        let (r, sfx) = run(&mut m, &env, press(|i| i.confirm = true));
        assert_eq!((r, sfx), (None, vec![Sfx::Play(snd::DECIDE)]));
        for _ in 0..29 {
            // Input is ignored while blinking.
            assert_eq!(run(&mut m, &env, press(|i| i.cancel = true)).0, None);
        }
        let (r, sfx) = run(&mut m, &env, MenuInput::NONE);
        assert_eq!(r, Some(TitleChoice::Ranking));
        assert_eq!(sfx, vec![Sfx::Stop(snd::DECIDE)]);
    }

    #[test]
    fn cancel_and_scroll_gate() {
        let env = Env::default();
        let mut m = TitleMenu::new(0, 0);
        let (r, sfx) = run(&mut m, &env, press(|i| i.cancel = true));
        assert_eq!(r, Some(TitleChoice::Back));
        assert_eq!(sfx, vec![Sfx::Play(snd::CANCEL)]);
        // While the list is still scrolling, input waits.
        let mut m = TitleMenu::new(0, 0);
        run(&mut m, &env, press(|i| i.down = true));
        assert_eq!(m.cursor(), 1);
        let (r, _) = run(&mut m, &env, press(|i| i.confirm = true));
        assert_eq!(r, None);
        assert_eq!(m.blink, 0, "confirm ignored while scrolling");
    }
}

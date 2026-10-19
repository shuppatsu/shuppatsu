//! Title menu (`TitleMenu_Create` / `_Update` / `_Draw` 0x42b210 / 0x42b2f0 / 0x42b530,
//! menus.md §3).
//!
//! Items: アーケードモード, ゲーム設定, ランキングを見る, ゲーム終了 and, with the Densha de GO!
//! controller (control mode 6) only, 専用コントローラー設定. The sixth label on the sheet,
//! LOVE特急こまち (the Komachi movie), has no descriptor and no cursor position: the cursor
//! maximum is 3 or 4, so the menu cannot reach it (the dispatch in 0x42a1c0 still has a case 5:
//! BGM off, the movie, BGM on, the menu again on that item). Setting `movie_in_menu` (port)
//! adds it as the last row with its own label from the sheet ([`FrontTables::movie_item`]); off,
//! the menu is the original's.

use super::tables::FrontTables;
use super::{Env, PAL_MENU, ScrollList, snd};
use crate::screens::tables::TextDesc;
use crate::screens::{MenuInput, Sfx};
use shuppatsu_formats::cg::{TILE_PIXELS, TILE_SIZE, Tilemap};
use shuppatsu_render::sprites::SpriteBank;
use shuppatsu_render::sprites::SpriteLayer;

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
    /// 5: LOVE特急こまち → the Komachi movie (setting `movie_in_menu` only).
    Movie,
    /// −2: cancel → `Game_MainFlow` shows the attract title again.
    Back,
}

impl TitleChoice {
    fn from_item(i: usize) -> Self {
        match i {
            0 => Self::Arcade,
            1 => Self::Options,
            2 => Self::Ranking,
            3 => Self::Exit,
            4 => Self::Calibrate,
            _ => Self::Movie,
        }
    }
}

/// The label index (0x513 sheet row / 6) of the movie item.
const MOVIE_ITEM: usize = 5;
/// Port: the number box of a 512×96 title label, `[x0, y0, x1, y1)` in art pixels: inside
/// the frame (x 0..=2, y 0..=1 and 94..=95) and left of the colour bar (x 56..=87). The
/// digits lie in x 11..=46, y 15..=80; the box's background is the same index in every label
/// (sheet 0x513 dumped, rows 24 and 30).
const NUMBER_BOX: [usize; 4] = [3, 2, 56, 94];

/// Port (setting `movie_in_menu`): the movie label with the "5" of the fifth label for its
/// "6", for the menu without the Densha de GO! controller, where it is the fifth row. Built
/// by [`add_movie_label_five`] in the rows under the sheet.
pub fn movie_label_five(t: &FrontTables) -> TextDesc {
    let m = t.movie_item();
    TextDesc {
        row: m.row + m.h,
        ..m
    }
}

/// Composes [`movie_label_five`] into the bank: the movie label (sheet 0x513 row 30) with its
/// number box taken from the fifth label (row 24), appended as new rows of the sheet, so it
/// is original art in the frame and colours of the movie item. Call it once, before the
/// translations are installed (their `rect` names the new rows). `false` when the sheet is not
/// the expected shape (left as it is).
pub fn add_movie_label_five(bank: &mut SpriteBank, t: &FrontTables) -> bool {
    let (movie, five) = (t.movie_item(), t.title_items[4]);
    let Ok(tbl) = u16::try_from(movie.tbl) else {
        return false;
    };
    let Some(map) = bank.tilemaps.get(usize::from(tbl)).cloned() else {
        return false;
    };
    let (Ok(row), Ok(h), Ok(five_row)) = (
        u8::try_from(movie.row),
        u8::try_from(movie.h),
        u8::try_from(five.row),
    ) else {
        return false;
    };
    // Once, and only if the movie label is the sheet's last (rows 30..36 of 36).
    if u16::from(row) + u16::from(h) != u16::from(map.height)
        || five.h != movie.h
        || usize::from(map.width) * TILE_SIZE < NUMBER_BOX[2]
    {
        return false;
    }
    let cols = NUMBER_BOX[2].div_ceil(TILE_SIZE);
    let pw = cols * TILE_SIZE;
    let mut px = bank.render_rect(tbl, (0, row, cols as u8, h));
    let src = bank.render_rect(tbl, (0, five_row, cols as u8, h));
    for y in NUMBER_BOX[1]..NUMBER_BOX[3].min(usize::from(h) * TILE_SIZE) {
        let r = y * pw;
        px[r + NUMBER_BOX[0]..r + NUMBER_BOX[2]]
            .copy_from_slice(&src[r + NUMBER_BOX[0]..r + NUMBER_BOX[2]]);
    }
    let w = usize::from(map.width);
    let mut tiles = map.tiles.clone();
    for ty in 0..usize::from(h) {
        for tx in 0..w {
            let id = if tx < cols {
                let mut cell = [0u8; TILE_PIXELS];
                for (py, line) in cell.as_chunks_mut::<TILE_SIZE>().0.iter_mut().enumerate() {
                    let s = (ty * TILE_SIZE + py) * pw + tx * TILE_SIZE;
                    line.copy_from_slice(&px[s..s + TILE_SIZE]);
                }
                match bank.add_tile(cell) {
                    Some(id) => id,
                    None => return false,
                }
            } else {
                map.tiles[(usize::from(row) + ty) * w + tx]
            };
            tiles.push(id);
        }
    }
    bank.tilemaps[usize::from(tbl)] = Tilemap {
        height: map.height + h,
        tiles,
        ..map
    };
    true
}

/// The items shown, by label index: 1–4, 5 with the Densha de GO! controller (mode 6), and 6 (the
/// movie) with setting `movie_in_menu`.
fn items(env: &Env) -> impl Iterator<Item = usize> {
    (0..4)
        .chain((env.control_mode == 6).then_some(4))
        .chain(env.movie_in_menu.then_some(MOVIE_ITEM))
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

    /// The last cursor index: four items, five with the Densha de GO! controller (mode 6), one more with
    /// setting `movie_in_menu`.
    pub fn last(env: &Env) -> i32 {
        items(env).count() as i32 - 1
    }

    /// The cursor position of the movie item (the menu comes back on it after the movie).
    pub fn movie_cursor(env: &Env) -> i32 {
        items(env).position(|i| i == MOVIE_ITEM).unwrap_or(0) as i32
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
                let item = items(env).nth(self.list.cursor as usize).unwrap_or(0);
                return Some(TitleChoice::from_item(item));
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
        for (i, item) in items(env).enumerate() {
            let d = match t.title_items.get(item) {
                Some(&d) => d,
                // The fifth row (no Densha de GO! controller) reads 5, not the label's 6.
                None if i == 4 => movie_label_five(t),
                None => t.movie_item(),
            };
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
        assert_eq!(
            m.cursor(),
            3,
            "four items without the Densha de GO! controller"
        );
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

    /// Setting `movie_in_menu`: the movie is the last row (after the calibration item in
    /// mode 6), its label the sheet's sixth; off, the original's rows.
    #[test]
    fn the_movie_row_follows_the_setting() {
        let t = super::super::tests_support::tables();
        let base = Env::default();
        assert_eq!(TitleMenu::last(&base), 3);
        let movie = Env {
            movie_in_menu: true,
            ..base
        };
        assert_eq!(TitleMenu::last(&movie), 4);
        assert_eq!(TitleMenu::movie_cursor(&movie), 4);
        let mode6 = Env {
            control_mode: 6,
            ..movie
        };
        assert_eq!(TitleMenu::last(&mode6), 5);
        assert_eq!(TitleMenu::movie_cursor(&mode6), 5);
        let mut m = TitleMenu::new(TitleMenu::movie_cursor(&movie), 0);
        let mut l = SpriteLayer::default();
        let mut sfx = Vec::new();
        let mut choice = None;
        for i in 0..40 {
            let input = press(|k| k.confirm = i == 0);
            choice = choice.or(m.tick(&input, &movie, &t, &mut l, &mut sfx));
        }
        assert_eq!(choice, Some(TitleChoice::Movie));
        // The row shows the derived label.
        let slot = &l.slots[SLOT_ITEMS + 4];
        let d = movie_label_five(&t);
        assert_eq!((slot.tilemap, slot.row), (d.tbl, d.row));
        assert_eq!(d.row, t.movie_item().row + t.movie_item().h);
        // Mode 6 without the setting: the calibration item last, as the original.
        let orig6 = Env {
            control_mode: 6,
            ..base
        };
        let mut m = TitleMenu::new(4, 0);
        let mut choice = None;
        for i in 0..40 {
            let input = press(|k| k.confirm = i == 0);
            choice = choice.or(m.tick(&input, &orig6, &t, &mut l, &mut sfx));
        }
        assert_eq!(choice, Some(TitleChoice::Calibrate));
        // Mode 6 with the setting: the movie is the sixth row, with the label's own 6.
        let mut m = TitleMenu::new(5, 0);
        run(&mut m, &mode6, MenuInput::NONE);
        let mut l = SpriteLayer::default();
        m.draw(&mode6, &t, &mut l);
        assert_eq!(l.slots[SLOT_ITEMS + 5].row, t.movie_item().row);
    }

    /// The composed label on the real sheet: the movie label's frame, bar and title, the
    /// fifth label's number box. Needs `SHUPPATSU_DATA_DIR`.
    #[test]
    fn the_fifth_row_movie_label_reads_five() {
        let Ok(data) = crate::scene::GameData::from_env() else {
            eprintln!("skipped: SHUPPATSU_DATA_DIR not set");
            return;
        };
        let exe_bytes = data.exe().unwrap();
        let exe = shuppatsu_formats::exe::Exe::parse(&exe_bytes).unwrap();
        let t = FrontTables::read(&exe).unwrap();
        let mut bank = SpriteBank::new(
            &shuppatsu_formats::cg::CgFile::parse(&data.read("cg.rom").unwrap()).unwrap(),
            shuppatsu_formats::cg::parse_tbl(&data.read("tbl.rom").unwrap()).unwrap(),
        );
        assert!(add_movie_label_five(&mut bank, &t));
        assert!(!add_movie_label_five(&mut bank, &t), "only once");
        let (movie, five, new) = (t.movie_item(), t.title_items[4], movie_label_five(&t));
        assert_eq!(new.row, 36);
        let rect = |d: TextDesc| bank.render_rect(0x513, (0, d.row as u8, 32, 6));
        let (a, b, c) = (rect(movie), rect(five), rect(new));
        let [x0, y0, x1, y1] = NUMBER_BOX;
        let mut digit = 0;
        for y in 0..96 {
            for x in 0..512 {
                let i = y * 512 + x;
                if (x0..x1).contains(&x) && (y0..y1).contains(&y) {
                    assert_eq!(c[i], b[i], "({x}, {y}) of the fifth label");
                    digit += usize::from(b[i] != a[i]);
                } else {
                    assert_eq!(c[i], a[i], "({x}, {y}) of the movie label");
                }
            }
        }
        assert!(digit > 500, "the 5 and the 6 differ ({digit} pixels)");
        // Both digits lie inside the box: its edges are the same background in both labels.
        for (x, y) in (x0..x1).flat_map(|x| [(x, y0), (x, y1 - 1)]) {
            assert_eq!(a[y * 512 + x], b[y * 512 + x], "({x}, {y})");
        }
        for (x, y) in (y0..y1).flat_map(|y| [(x0, y), (x1 - 1, y)]) {
            assert_eq!(a[y * 512 + x], b[y * 512 + x], "({x}, {y})");
        }
    }
}

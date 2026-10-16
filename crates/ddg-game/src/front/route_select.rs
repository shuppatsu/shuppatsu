//! 路線選択 / 電車選択: the route select (`RouteSelect_Create` / `_Update` / `_DrawHeader` /
//! `_Destroy` 0x435490 / 0x435510 / 0x436210 / 0x436640, menus.md §6).
//!
//! Two levels in one task: the five line groups, then the trains (select stages) of the chosen
//! group. Every group and stage is selectable (there is no lock); the clear mark of each stage's
//! route is shown as a star. The R key toggles the replay-playback flag `0x816974` at the train
//! level; replays are not implemented, so no row ever has one, and leaving switches the flag
//! off again (0x436640 does that when the chosen stage has no replay).

use super::tables::FrontTables;
use super::{Env, ScrollList, snd};
use crate::screens::tables::TextDesc;
use crate::screens::{MenuInput, Sfx};
use ddg_render::sprites::SpriteLayer;

/// Frame pieces of a group box (0x435510 immediates), in the order of the placements.
const GROUP_FRAME: [i16; 4] = [0x30b, 0x308, 0x30a, 0x309];
/// "Line + series" underline, "〜" and 難易度 (0x435510 immediates).
const TBL_UNDERLINE: i16 = 0x30c;
const TBL_TILDE: i16 = 0x30d;
const TBL_DIFFICULTY: i16 = 0x30f;
const TBL_STAR: i16 = 0x30e;
/// Rank badges by difficulty 0–5 (0x435510's stack table): 初級 for 1, 中級 for 2–3, 上級 for 4,
/// 特級 for 5.
const RANK_BADGES: [i16; 6] = [0x314, 0x314, 0x315, 0x315, 0x316, 0x317];
/// 雫石停車 / 雫石通過 tags of select stages 0x11 / 0x12, drawn over the "〜" slot 6 px left
/// and 4 px up.
const TAG_STOP: (usize, i16) = (0x11, 0x549);
const TAG_PASS: (usize, i16) = (0x12, 0x548);
/// The clear star (tbl 0x54b, sub-row `(3 − mark) · 2`, 2×2 tiles) and the replay mark
/// (tbl 0x50b at column 6, 6×2 tiles).
const TBL_CLEAR_STAR: i16 = 0x54b;
const TBL_REPLAY: i16 = 0x50b;
/// Slots per row (`+0x14` per row).
const ROW_SLOTS: usize = 20;
/// Row tops are `0x86 + 106 i + scroll`.
const ROW_TOP: i32 = 0x86;
const LAYER_CURSOR: u8 = 1;
/// Group-header palette (0x435510 passes 4, not the record's).
const PAL_GROUP_HEADER: u8 = 4;
const GROUP_LEVEL_COUNT: i32 = 5;
/// The train rows' records (`0x463e6c`), by index.
mod rec {
    pub const THUMB: usize = 0;
    pub const CAPTION: usize = 1;
    pub const UNDERLINE: usize = 2;
    pub const FROM: usize = 3;
    pub const TILDE: usize = 4;
    pub const TO: usize = 5;
    pub const TYPE: usize = 6;
    pub const DIFFICULTY: usize = 7;
    pub const STARS: usize = 8;
    pub const BADGE: usize = 13;
    pub const CLEAR: usize = 14;
    pub const REPLAY: usize = 15;
}

/// `0x431dc0`: the palette of a train thumbnail.
pub fn thumb_palette(tbl: i16) -> u8 {
    match tbl {
        0..=8 | 0x67 | 0x68 | 0x184 => 10,
        0x69 => 4,
        _ => 9,
    }
}

/// The chosen stage, as the destroy callback 0x436640 passes it on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chosen {
    pub group: usize,
    /// Select stage 0..=20.
    pub stage: usize,
    /// Route index `0x8570a4`; select stage 20 runs route 10 with `0x48ddf0 = 1`.
    pub route: usize,
    pub variant: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteResult {
    Chosen(Chosen),
    /// Cancel at the line level: `0x8570a4 = −1`, back to the title menu.
    Back,
}

#[derive(Debug, Clone)]
pub struct RouteSelect {
    /// +0x30: 0 line groups, 1 trains.
    level: usize,
    /// +0x34: rows at this level.
    count: i32,
    list: ScrollList,
    /// +0x44: the chosen group.
    group: usize,
    /// +0x4c: decide blink.
    blink: i32,
    cursor_x: i16,
}

impl RouteSelect {
    /// `0x435490`: BGM 0x19e (re)starts; the creation argument (EB + P3 held at the title) is
    /// never read.
    pub fn new(cursor_x: i16, sfx: &mut Vec<Sfx>) -> Self {
        sfx.push(Sfx::Play(snd::BGM_MENU));
        Self {
            level: 0,
            count: GROUP_LEVEL_COUNT,
            list: ScrollList::default(),
            group: 0,
            blink: 0,
            cursor_x,
        }
    }

    #[cfg(test)]
    pub fn level(&self) -> usize {
        self.level
    }

    #[cfg(test)]
    pub fn cursor(&self) -> i32 {
        self.list.cursor
    }

    /// One tick of 0x435510. `replay` is the global 自動運転 flag `0x816974`;
    /// `clear_mark(route)` reads the save's clear marks.
    #[allow(clippy::too_many_arguments)]
    pub fn tick(
        &mut self,
        input: &MenuInput,
        env: &Env,
        t: &FrontTables,
        replay: &mut bool,
        clear_mark: impl Fn(usize) -> u8,
        l: &mut SpriteLayer,
        sfx: &mut Vec<Sfx>,
    ) -> Option<RouteResult> {
        if self.blink != 0 {
            self.blink -= 1;
            if self.blink == 0 {
                super::clear_all(l);
                sfx.push(Sfx::Stop(snd::DECIDE));
                if self.level != 0 {
                    return Some(RouteResult::Chosen(self.destroy(t, replay)));
                }
                self.group = self.list.cursor as usize;
                self.level = 1;
                self.count = t.group_stages[self.group].len() as i32;
                self.list = ScrollList::default();
                return None;
            }
        } else if self.list.distance() != 0 {
            self.list.approach(env.analog);
        } else if input.confirm {
            sfx.push(Sfx::Play(snd::DECIDE));
            self.blink = super::title::BLINK_TICKS;
        } else if input.cancel {
            sfx.push(Sfx::Play(snd::CANCEL));
            super::clear_all(l);
            if self.level == 0 {
                // 0x436640 with both indices −1; the flag word is cleared too.
                return Some(RouteResult::Back);
            }
            self.level = 0;
            self.count = GROUP_LEVEL_COUNT;
            self.list = ScrollList::settled(self.group as i32);
            return None;
        } else if input.key_r {
            if self.level != 0 {
                *replay = !*replay;
            }
        } else if self.list.step(input, self.count - 1) {
            sfx.push(Sfx::Play(snd::MOVE));
        }
        self.draw(env, t, *replay, &clear_mark, l);
        None
    }

    /// `RouteSelect_Destroy` 0x436640 for a chosen stage.
    fn destroy(&self, t: &FrontTables, replay: &mut bool) -> Chosen {
        let stage = t.group_stages[self.group][self.list.cursor as usize];
        let variant = stage == 20;
        let route = if variant {
            10
        } else {
            t.stages[stage].route as usize
        };
        // No stored replay (0x42e450 < 0): playback mode is switched off.
        *replay = false;
        Chosen {
            group: self.group,
            stage,
            route,
            variant,
        }
    }

    fn draw(
        &self,
        env: &Env,
        t: &FrontTables,
        replay: bool,
        clear_mark: &impl Fn(usize) -> u8,
        l: &mut SpriteLayer,
    ) {
        super::draw_route_header(l, t, env, self.level);
        let x0 = self.cursor_x;
        for i in 0..self.count.max(0) as usize {
            let top = (ROW_TOP + super::ROW * i as i32 + self.list.scroll) as i16;
            let off = ROW_SLOTS * i;
            let at = |l: &mut SpriteLayer, r: &crate::screens::tables::Placement, tbl: i16| {
                super::put(
                    l,
                    r.slot as usize + off,
                    tbl,
                    x0 + r.x,
                    top + r.y,
                    r.palette as u8,
                    r.layer as u8,
                );
            };
            if self.level == 0 {
                for (r, &tbl) in t.group_frame.iter().zip(&GROUP_FRAME) {
                    at(l, r, tbl);
                }
                let (header, thumbs) = t.groups[i];
                let h = &t.group_header;
                super::put(
                    l,
                    h.slot as usize + off,
                    header,
                    x0 + h.x,
                    top + h.y,
                    PAL_GROUP_HEADER,
                    h.layer as u8,
                );
                for (r, &tbl) in t.group_thumbs.iter().zip(&thumbs) {
                    if tbl != -1 {
                        super::put(
                            l,
                            r.slot as usize + off,
                            tbl,
                            x0 + r.x,
                            top + r.y,
                            thumb_palette(tbl),
                            r.layer as u8,
                        );
                    }
                }
                continue;
            }
            let s = t.group_stages[self.group][i];
            let st = &t.stages[s];
            let row = &t.train_row;
            let th = &row[rec::THUMB];
            super::put(
                l,
                th.slot as usize + off,
                st.thumb,
                x0 + th.x,
                top + th.y,
                thumb_palette(st.thumb),
                th.layer as u8,
            );
            at(l, &row[rec::CAPTION], st.caption);
            at(l, &row[rec::UNDERLINE], TBL_UNDERLINE);
            at(l, &row[rec::FROM], st.from);
            at(l, &row[rec::TILDE], TBL_TILDE);
            for (stage, tag) in [TAG_STOP, TAG_PASS] {
                if s == stage {
                    let r = &row[rec::TILDE];
                    super::put(
                        l,
                        r.slot as usize + off,
                        tag,
                        x0 + r.x - 6,
                        top + r.y - 4,
                        r.palette as u8,
                        r.layer as u8,
                    );
                }
            }
            at(l, &row[rec::TO], st.to);
            at(l, &row[rec::TYPE], st.train_type);
            at(l, &row[rec::DIFFICULTY], TBL_DIFFICULTY);
            let stars = &row[rec::STARS];
            for k in 0..st.difficulty.clamp(0, 5) as usize {
                super::put(
                    l,
                    stars.slot as usize + off + k,
                    TBL_STAR,
                    x0 + stars.x + 16 * k as i16,
                    top + stars.y,
                    stars.palette as u8,
                    stars.layer as u8,
                );
            }
            at(
                l,
                &row[rec::BADGE],
                RANK_BADGES[st.difficulty.clamp(0, 5) as usize],
            );
            let mark = usize::try_from(st.route).map_or(0, clear_mark);
            if mark != 0 {
                let r = &row[rec::CLEAR];
                let d = TextDesc {
                    tbl: TBL_CLEAR_STAR,
                    col: 0,
                    row: ((3 - i16::from(mark.min(3))) * 2) as i8,
                    w: 2,
                    h: 2,
                };
                super::text(
                    l,
                    r.slot as usize + off,
                    d,
                    x0 + r.x,
                    top + r.y,
                    r.palette as u8,
                    r.layer as u8,
                );
            }
            let r = &row[rec::REPLAY];
            if !replay {
                super::hide(l, r.slot as usize + off);
            } else if replay_exists(s) {
                let d = TextDesc {
                    tbl: TBL_REPLAY,
                    col: 6,
                    row: 0,
                    w: 6,
                    h: 2,
                };
                super::text(
                    l,
                    r.slot as usize + off,
                    d,
                    x0 + r.x,
                    top + r.y,
                    r.palette as u8,
                    r.layer as u8,
                );
            }
        }
        super::draw_cursor(l, x0, &self.list, self.count - 1, self.blink, LAYER_CURSOR);
    }
}

/// `0x42e450(stage)`: the stored replay of a stage. Replays (`REPLAY.DAT`) are not
/// implemented, so there never is one.
fn replay_exists(_stage: usize) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::front::tests_support::tables;

    fn press(f: impl FnOnce(&mut MenuInput)) -> MenuInput {
        let mut i = MenuInput::NONE;
        f(&mut i);
        i
    }

    fn run(
        m: &mut RouteSelect,
        input: MenuInput,
        replay: &mut bool,
    ) -> (Option<RouteResult>, Vec<Sfx>) {
        let t = tables();
        let mut l = SpriteLayer::default();
        let mut sfx = vec![];
        let r = m.tick(&input, &Env::default(), &t, replay, |_| 0, &mut l, &mut sfx);
        (r, sfx)
    }

    fn settle(m: &mut RouteSelect, replay: &mut bool) {
        for _ in 0..20 {
            run(m, MenuInput::NONE, replay);
        }
    }

    #[test]
    fn line_then_train_then_chosen() {
        let mut replay = false;
        let mut sfx = vec![];
        let mut m = RouteSelect::new(0, &mut sfx);
        assert_eq!(sfx, [Sfx::Play(snd::BGM_MENU)]);
        let down = press(|i| i.down = true);
        for _ in 0..10 {
            run(&mut m, down, &mut replay);
            settle(&mut m, &mut replay);
        }
        assert_eq!(m.cursor(), 4, "five groups, clamped");
        let up = press(|i| i.up = true);
        for _ in 0..2 {
            run(&mut m, up, &mut replay);
            settle(&mut m, &mut replay);
        }
        run(&mut m, press(|i| i.confirm = true), &mut replay);
        for _ in 0..29 {
            assert_eq!(run(&mut m, MenuInput::NONE, &mut replay).0, None);
        }
        assert_eq!(m.level(), 0);
        let (r, sfx) = run(&mut m, MenuInput::NONE, &mut replay);
        assert_eq!((r, sfx), (None, vec![Sfx::Stop(snd::DECIDE)]));
        assert_eq!((m.level(), m.cursor()), (1, 0));
        // Group 2 has four trains; R toggles the replay flag only here.
        run(&mut m, press(|i| i.key_r = true), &mut replay);
        assert!(replay);
        for _ in 0..3 {
            run(&mut m, down, &mut replay);
            settle(&mut m, &mut replay);
        }
        assert_eq!(m.cursor(), 3);
        run(&mut m, up, &mut replay);
        settle(&mut m, &mut replay);
        run(&mut m, press(|i| i.confirm = true), &mut replay);
        let mut r = None;
        for _ in 0..30 {
            r = run(&mut m, MenuInput::NONE, &mut replay).0.or(r);
        }
        assert_eq!(
            r,
            Some(RouteResult::Chosen(Chosen {
                group: 2,
                stage: 20,
                route: 10,
                variant: true
            }))
        );
        assert!(!replay, "no replay stored: the flag goes off");
    }

    #[test]
    fn cancel_goes_back_a_level() {
        let mut replay = false;
        let mut m = RouteSelect::new(0, &mut vec![]);
        run(&mut m, press(|i| i.down = true), &mut replay);
        settle(&mut m, &mut replay);
        run(&mut m, press(|i| i.confirm = true), &mut replay);
        for _ in 0..30 {
            run(&mut m, MenuInput::NONE, &mut replay);
        }
        assert_eq!(m.level(), 1);
        let (r, sfx) = run(&mut m, press(|i| i.cancel = true), &mut replay);
        assert_eq!((r, sfx), (None, vec![Sfx::Play(snd::CANCEL)]));
        assert_eq!(
            (m.level(), m.cursor()),
            (0, 1),
            "back on the previous group"
        );
        assert_eq!(m.list.distance(), 0, "settled at once");
        let (r, _) = run(&mut m, press(|i| i.cancel = true), &mut replay);
        assert_eq!(r, Some(RouteResult::Back));
    }
}

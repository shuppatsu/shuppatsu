//! The front end (docs/re/menus.md §2–§7): attract title picture, title menu, options, records
//! viewer, controller calibration, route/train select and the pre-run screens (lever check,
//! departure board). [`crate::app`] sequences them like `Game_MainFlow` 0x419510.
//!
//! Every screen is a state machine with one `tick` per 33 ms `Task_Tick`, fed the button edges
//! the original's task would see (polled at the end of the previous tick). Unlike the stage-flow
//! screens, these write straight into the persistent [`SpriteLayer`] the way the original's
//! sprite calls do: a slot keeps its image until a screen changes or clears it (0x4316b0).

pub mod attract;
pub mod calibrate;
pub mod dialog;
pub mod language;
pub mod options;
pub mod port_settings;
pub mod prerun;
pub mod records;
pub mod route_select;
pub mod tables;
pub mod title;

use crate::glyphs::{self, Btn, Hint};
use crate::screens::tables::{Placement, TextDesc};
use ddg_render::sprites::{Slot, SpriteLayer};
use tables::{FrontTables, Legends};

/// Menu palette (0x18) used by headers, cursors and most labels.
pub const PAL_MENU: u8 = 0x18;
/// Label palette 4.
pub const PAL_LABEL: u8 = 4;

/// Sounds of the menus (menus.md §1.4).
pub mod snd {
    /// Cursor move.
    pub const MOVE: u16 = 0x16;
    /// Decide on the title menu and the route select (stopped after the 30-tick blink).
    pub const DECIDE: u16 = 0xd;
    /// Decide on the options pages, the records viewer and the calibration screen.
    pub const DECIDE_SOFT: u16 = 0x17;
    pub const CANCEL: u16 = 0x13;
    /// Dialog clicks (sent straight to the sound thread, 0x430100).
    pub const DIALOG: u16 = 0x26;
    /// Title / menu BGM (loops; audio-text.md §10.9).
    pub const BGM_MENU: u16 = 0x19e;
    /// START on the attract title picture (0x402700).
    pub const START: u16 = 0x1e;
    /// Departure-board flaps (looped while they spin, 0x413b10).
    pub const FLAPS: u16 = 0x18;
}

/// What the screens know about the machine (the original's input and device globals).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Env {
    /// Control mode 0–6 (`0x433290`, eeprom +0x332).
    pub control_mode: u8,
    /// Mode 5/6 with a joystick and no Shift override (`0x433270`).
    pub analog: bool,
    /// A joystick is attached (`0x8298a4 != 0`).
    pub joystick: bool,
    /// A force-feedback device is attached (`0x816838`).
    pub force_feedback: bool,
    /// DirectSound opened (`0x816ae8`).
    pub sound_device: bool,
    /// Setting `movie_in_menu` (port): the title menu shows its sixth item, the movie.
    pub movie_in_menu: bool,
    /// The button legends: the original's for the control mode, or with `hybrid_input` those of
    /// the last device used ([`Hint::new`]).
    pub hint: Hint,
}

impl Default for Env {
    fn default() -> Self {
        Self {
            control_mode: 0,
            analog: false,
            joystick: false,
            force_feedback: false,
            sound_device: true,
            movie_in_menu: false,
            hint: Hint::Keys,
        }
    }
}

/// `0x4316e0`: tilemap, position, palette and layer at once.
pub fn put(l: &mut SpriteLayer, slot: usize, tbl: i16, x: i16, y: i16, pal: u8, layer: u8) {
    if let Some(s) = l.slots.get_mut(slot) {
        *s = Slot::new(tbl, x, y, pal, layer, false);
    }
}

/// `0x431730`: a label cut out of a string sheet.
pub fn text(l: &mut SpriteLayer, slot: usize, d: TextDesc, x: i16, y: i16, pal: u8, layer: u8) {
    if let Some(s) = l.slots.get_mut(slot) {
        *s = Slot {
            col: d.col,
            row: d.row,
            w: d.w,
            h: d.h,
            ..Slot::new(d.tbl, x, y, pal, layer, false)
        };
    }
}

/// `0x4318c0`: the whole of tilemap `tbl` (−1 hides), position kept.
pub fn image(l: &mut SpriteLayer, slot: usize, tbl: i16) {
    if let Some(s) = l.slots.get_mut(slot) {
        s.tilemap = tbl;
        (s.col, s.row, s.w, s.h) = (0, 0, 0, 0);
    }
}

/// `0x431850`: position, layer and palette; the image is kept.
pub fn place(l: &mut SpriteLayer, p: &Placement, dx: i16, dy: i16) {
    let Some(s) = usize::try_from(p.slot)
        .ok()
        .and_then(|i| l.slots.get_mut(i))
    else {
        return;
    };
    s.x = p.x + dx;
    s.y = p.y + dy;
    s.layer = p.layer as u8;
    s.palette = p.palette as u8;
}

/// `0x431880`: moves a slot.
pub fn move_to(l: &mut SpriteLayer, slot: usize, x: i16, y: i16) {
    if let Some(s) = l.slots.get_mut(slot) {
        (s.x, s.y) = (x, y);
    }
}

/// `0x431790`: clears one slot.
pub fn hide(l: &mut SpriteLayer, slot: usize) {
    if let Some(s) = l.slots.get_mut(slot) {
        *s = Slot::CLEAR;
    }
}

/// `0x4316b0(0)`: clears every slot that is not marked to keep.
pub fn clear_all(l: &mut SpriteLayer) {
    for s in l.slots.iter_mut().filter(|s| !s.keep) {
        *s = Slot::CLEAR;
    }
    l.fans.clear();
    l.widgets.clear();
}

/// Row height of the scrolling lists (title, route select; 0x6a).
pub const ROW: i32 = 0x6a;
/// Scroll window of res mode 1: `((res != 1) + 2) · 0x6a`.
pub const WINDOW: i32 = 2 * ROW;

/// The scrolling list of the title menu (task +0x30/+0x34/+0x38) and the route select
/// (+0x38/+0x3c/+0x40): the cursor index, the cursor frame's offset inside the window and the
/// list's scroll offset (≤ 0).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScrollList {
    pub cursor: i32,
    pub frame: i32,
    pub scroll: i32,
}

impl ScrollList {
    /// The distance still to travel: `cursor · 0x6a − frame + scroll`.
    pub fn distance(&self) -> i32 {
        self.cursor * ROW - self.frame + self.scroll
    }

    /// One tick of the approach: half the remaining distance (rounded up), or all of it with
    /// an analog lever. The frame moves inside the window first, then the list scrolls.
    pub fn approach(&mut self, analog: bool) {
        let d = self.distance();
        if d > 0 {
            let n = if analog { d } else { (d + 1) / 2 };
            for _ in 0..n {
                if self.frame < WINDOW {
                    self.frame += 1;
                } else {
                    self.scroll -= 1;
                }
            }
        } else if d < 0 {
            let n = if analog { -d } else { (1 - d) / 2 };
            for _ in 0..n {
                if self.frame < 1 {
                    self.scroll += 1;
                } else {
                    self.frame -= 1;
                }
            }
        }
    }

    /// A list created with the cursor on `cursor`, settled at once (0x42b210, and the route
    /// select's return to the line list).
    pub fn settled(cursor: i32) -> Self {
        let mut s = Self {
            cursor,
            ..Self::default()
        };
        while s.distance() != 0 {
            s.approach(false);
        }
        s
    }

    /// The cursor step of the title menu and the route select: up/down with the keys (no wrap),
    /// or one step toward the power notch with an analog lever. Returns whether it moved.
    pub fn step(&mut self, input: &crate::screens::MenuInput, last: i32) -> bool {
        let before = self.cursor;
        match input.notch {
            Some(notch) => {
                let notch = i32::from(notch);
                if self.cursor >= 1 && self.cursor > notch {
                    self.cursor -= 1;
                } else if self.cursor < last && self.cursor < notch {
                    self.cursor += 1;
                }
                self.cursor = self.cursor.clamp(0, last.max(0));
            }
            None => {
                if self.cursor >= 1 && input.up {
                    self.cursor -= 1;
                } else if self.cursor < last && input.down {
                    self.cursor += 1;
                }
            }
        }
        self.cursor != before
    }
}

/// Tilemaps the menu code passes as immediates.
pub mod tbl {
    /// Full-screen menu background in res mode 1 (0x42a820, slot 0xb7).
    pub const BACKGROUND: i16 = 0x501;
    /// The "select / decide / back" legend frame (0x42a820, slot 0x8a).
    pub const LEGEND_FRAME: i16 = 0x502;
    /// Cursor frame and the more-below / more-above arrows (0x42b530, 0x435510).
    pub const CURSOR: i16 = 0x504;
    pub const MORE_BELOW: i16 = 0x505;
    pub const MORE_ABOVE: i16 = 0x506;
    /// Route-select header bar (0x436210, 0x434d20).
    pub const ROUTE_BAR: i16 = 0x515;
}

/// Header slots (0x42a820 / 0x436210).
mod slot {
    pub const TITLE: usize = 0x87;
    pub const WORD: usize = 0x88;
    pub const UP_DOWN: usize = 0x89;
    pub const LEGEND_FRAME: usize = 0x8a;
    pub const UPPER_LABEL: usize = 0x8b;
    pub const UPPER: usize = 0x8c;
    pub const LOWER_LABEL: usize = 0x8d;
    pub const LOWER: usize = 0x8e;
    pub const BAR: usize = 0x8f;
    pub const R_LABEL: usize = 0x9c;
    pub const R_KEY: usize = 0x9d;
    pub const BACKGROUND: usize = 0xb7;
    pub const MORE_BELOW: usize = 0x90;
    pub const MORE_ABOVE: usize = 0x91;
    pub const CURSOR: usize = 0x92;
}

/// Enhancement `wide_menus` (menus.md §14): the menu backdrops a wide canvas continues
/// sideways: the patterned background tbl 0x501 and the black header bands (tbl 0x515 and
/// the per-page bars of 0x42a820). Their repeat comes from the tile ids ([`Repeat::of`]);
/// everything else on the menus stays in the centred 4:3 layout. The attract title picture
/// tbl 0x410, which has no pattern but plain paper texture at its edges, continues with paper
/// synthesised from them instead ([`Paper`]); the other attract pictures (photos, posters,
/// the logo) and the departure board have black or photographic surroundings and stay
/// centred.
///
/// [`Repeat::of`]: ddg_render::sprites::Repeat::of
/// [`Paper`]: ddg_render::sprites::Paper
pub fn mark_backdrops(bank: &mut ddg_render::sprites::SpriteBank, t: &FrontTables) {
    let maps = [tbl::BACKGROUND, tbl::ROUTE_BAR]
        .into_iter()
        .chain(t.header_tbl.iter().copied());
    for m in maps.filter_map(|m| u16::try_from(m).ok()) {
        bank.set_wide_fill(m);
    }
    if let Ok(m) = u16::try_from(attract::TBL_TITLE) {
        bank.set_wide_paper(m, attract::TITLE_PAPER);
    }
}

/// The legend set for the hints: keyboard modes, the pad (mode 4) or the levers (5/6); the
/// glyphs replace the keyboard set's pictures ([`legend_glyphs`]).
fn legend_set(l: &Legends<TextDesc>, hint: Hint) -> (TextDesc, TextDesc, TextDesc) {
    match hint {
        Hint::Pad => (l.word_pad, l.upper_pad, l.lower_pad),
        Hint::Levers => (l.word_lever, l.upper_pad, l.lower_pad),
        Hint::Keys | Hint::Glyphs => (l.word_keys, l.upper_keys, l.lower_keys),
    }
}

/// The centre and side of a text slot's picture, in original pixels.
fn picture_box(t: &FrontTables, d: TextDesc, x: i16, y: i16) -> ((f32, f32), f32) {
    let tile = ddg_formats::cg::TILE_SIZE as f32;
    let (w, h) = (f32::from(d.w) * tile, f32::from(t.picture_rows(d)) * tile);
    ((f32::from(x) + w / 2.0, f32::from(y) + h / 2.0), w.min(h))
}

/// `hybrid_input` with a gamepad (input.md §10.6): the key pictures of a menu legend (the
/// cursor keys, Esc, Enter) become the d-pad, B and A glyphs in their places; the labels
/// (選択 / キャンセル / 決定, or their translation) stay. `at` are the three pictures with
/// their slot positions.
fn legend_glyphs(
    l: &mut SpriteLayer,
    t: &FrontTables,
    layer: u8,
    at: [(usize, TextDesc, i16, i16); 3],
) {
    for ((slot, d, x, y), b) in at.into_iter().zip([glyphs::DPAD_UD, Btn::B, Btn::A]) {
        hide(l, slot);
        let (c, side) = picture_box(t, d, x, y);
        let size = if matches!(b, Btn::DPad(_)) {
            side * 1.1
        } else {
            side * 0.85
        };
        glyphs::draw_glyph(l, layer, b, c, size);
    }
}

/// `0x42a820(page)`: the menu header, legends and background (title, options, calibration
/// use layer 3). The Shift override (0x829d08, keyboard legends in joystick modes) is not
/// modelled.
pub fn draw_menu_header(l: &mut SpriteLayer, t: &FrontTables, env: &Env, page: usize) {
    let (x0, _) = t.origin;
    let layer = 3;
    put(
        l,
        slot::BAR,
        t.header_tbl.get(page).copied().unwrap_or(-1),
        x0,
        0,
        PAL_MENU,
        layer,
    );
    if let Some(&d) = t.header_text.get(page) {
        text(l, slot::TITLE, d, x0 + 0x60, 0x20, PAL_MENU, layer);
    }
    let (word, upper, lower) = legend_set(&t.title_legends, env.hint);
    text(l, slot::WORD, word, x0 + 0x180, 0x20, PAL_MENU, layer);
    text(l, slot::UPPER, upper, x0 + 0x250, 0x20, PAL_MENU, layer);
    text(l, slot::LOWER, lower, x0 + 0x250, 0x40, PAL_MENU, layer);
    if env.hint == Hint::Glyphs {
        legend_glyphs(
            l,
            t,
            layer,
            [
                (slot::WORD, word, x0 + 0x180, 0x20),
                (slot::UPPER, upper, x0 + 0x250, 0x20),
                (slot::LOWER, lower, x0 + 0x250, 0x40),
            ],
        );
    }
    put(
        l,
        slot::LEGEND_FRAME,
        tbl::LEGEND_FRAME,
        x0 + 0x1c0,
        0x20,
        PAL_MENU,
        layer,
    );
    let lg = &t.title_legends;
    text(
        l,
        slot::UP_DOWN,
        lg.up_down,
        x0 + 0x180,
        0x50,
        PAL_LABEL,
        layer,
    );
    text(
        l,
        slot::UPPER_LABEL,
        lg.upper_label,
        x0 + 0x1e0,
        0x20,
        PAL_LABEL,
        layer,
    );
    text(
        l,
        slot::LOWER_LABEL,
        lg.lower_label,
        x0 + 0x1e0,
        0x40,
        PAL_LABEL,
        layer,
    );
    // Res mode 1 has one full-screen background picture.
    put(l, slot::BACKGROUND, tbl::BACKGROUND, 0, 0, PAL_MENU, layer);
}

/// `0x436210(level)`: the route-select header (layer 1, no background picture); at the train
/// level the key legends sit 16 px lower and the R-key (replay) legend appears.
pub fn draw_route_header(l: &mut SpriteLayer, t: &FrontTables, env: &Env, level: usize) {
    let (x0, _) = t.origin;
    let layer = 1;
    let dy = if level != 0 { 0x10 } else { 0 };
    put(l, slot::BAR, tbl::ROUTE_BAR, x0, 0, PAL_MENU, layer);
    if let Some(&d) = t.route_titles.get(level.min(1)) {
        text(l, slot::TITLE, d, x0 + 0x60, 0x20, PAL_MENU, layer);
    }
    let (word, upper, lower) = legend_set(&t.route_legends, env.hint);
    text(l, slot::WORD, word, x0 + 0x180, 0x20, PAL_MENU, layer);
    text(
        l,
        slot::UPPER,
        upper,
        x0 + 0x250,
        0x20 + dy,
        PAL_MENU,
        layer,
    );
    text(
        l,
        slot::LOWER,
        lower,
        x0 + 0x250,
        0x40 + dy,
        PAL_MENU,
        layer,
    );
    put(
        l,
        slot::LEGEND_FRAME,
        tbl::LEGEND_FRAME,
        x0 + 0x1c0,
        0x20,
        PAL_MENU,
        layer,
    );
    if env.hint == Hint::Glyphs {
        legend_glyphs(
            l,
            t,
            layer,
            [
                (slot::WORD, word, x0 + 0x180, 0x20),
                (slot::UPPER, upper, x0 + 0x250, 0x20 + dy),
                (slot::LOWER, lower, x0 + 0x250, 0x40 + dy),
            ],
        );
    }
    let lg = &t.route_legends;
    text(
        l,
        slot::UP_DOWN,
        lg.up_down,
        x0 + 0x180,
        0x50,
        PAL_LABEL,
        layer,
    );
    text(
        l,
        slot::UPPER_LABEL,
        lg.upper_label,
        x0 + 0x1e0,
        0x20 + dy,
        PAL_LABEL,
        layer,
    );
    text(
        l,
        slot::LOWER_LABEL,
        lg.lower_label,
        x0 + 0x1e0,
        0x40 + dy,
        PAL_LABEL,
        layer,
    );
    if level != 0 {
        text(l, slot::R_KEY, t.route_r_key, x0 + 0x250, 0x10, PAL_MENU, 0);
        text(
            l,
            slot::R_LABEL,
            t.route_r_label,
            x0 + 0x1e0,
            0x10,
            PAL_LABEL,
            0,
        );
        if env.hint == Hint::Glyphs {
            // R (the automatic run) is Y on the pad.
            hide(l, slot::R_KEY);
            let (c, side) = picture_box(t, t.route_r_key, x0 + 0x250, 0x10);
            glyphs::draw_glyph(l, 0, Btn::Y, c, side * 0.85);
        }
    } else {
        hide(l, slot::R_KEY);
        hide(l, slot::R_LABEL);
    }
}

/// The cursor frame and its arrows (0x42b530 / 0x435510): hidden while blinking
/// (`blink % 6 > 3`), else the frame at the list's frame offset, the more-below arrow when
/// the cursor is above the last item and the more-above arrow when it is below the first.
pub fn draw_cursor(
    l: &mut SpriteLayer,
    x: i16,
    list: &ScrollList,
    last: i32,
    blink: i32,
    layer: u8,
) {
    if blink % 6 > 3 {
        hide(l, slot::CURSOR);
        hide(l, slot::MORE_BELOW);
        hide(l, slot::MORE_ABOVE);
        return;
    }
    let y = list.frame as i16;
    put(l, slot::CURSOR, tbl::CURSOR, x, y + 0x86, PAL_MENU, layer);
    if list.cursor < last {
        put(
            l,
            slot::MORE_BELOW,
            tbl::MORE_BELOW,
            x + 0xf0,
            y + 0xef,
            PAL_MENU,
            layer,
        );
    } else {
        hide(l, slot::MORE_BELOW);
    }
    if list.cursor >= 1 {
        put(
            l,
            slot::MORE_ABOVE,
            tbl::MORE_ABOVE,
            x + 0xf0,
            y + 0x6e,
            PAL_MENU,
            layer,
        );
    } else {
        hide(l, slot::MORE_ABOVE);
    }
}

/// The x of the centred cursor frame: `x0 + (640 − width(tbl 0x504)) / 2`. The original reads
/// the width from its tilemap-size cache (`0x855ae8` = entry 0x504 of the cache at 0x850aa8,
/// 16 bytes per tilemap).
pub fn cursor_x(t: &FrontTables, cursor_width: i16) -> i16 {
    t.origin.0 + (640 - cursor_width) / 2
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::MenuInput;

    #[test]
    fn scroll_list_settles_inside_the_window_then_scrolls() {
        let s = ScrollList::settled(1);
        assert_eq!((s.frame, s.scroll), (ROW, 0));
        let s = ScrollList::settled(4);
        assert_eq!((s.frame, s.scroll), (WINDOW, -2 * ROW));
        assert_eq!(s.distance(), 0);
        // Back up: the frame moves first, then the list.
        let mut s = ScrollList::settled(4);
        s.cursor = 0;
        let mut ticks = 0;
        while s.distance() != 0 {
            s.approach(false);
            ticks += 1;
        }
        assert_eq!((s.frame, s.scroll), (0, 0));
        // Half the distance per tick: 424 px take 9 ticks.
        assert_eq!(ticks, 9);
        let mut s = ScrollList::settled(0);
        s.cursor = 3;
        s.approach(true);
        assert_eq!(s.distance(), 0, "an analog lever jumps at once");
    }

    #[test]
    fn cursor_steps_are_clamped() {
        let mut s = ScrollList::default();
        let up = MenuInput {
            up: true,
            ..MenuInput::NONE
        };
        let down = MenuInput {
            down: true,
            ..MenuInput::NONE
        };
        assert!(!s.step(&up, 3));
        assert!(s.step(&down, 3));
        s.cursor = 3;
        assert!(!s.step(&down, 3));
        // Analog: one step per tick toward the notch, clamped to the list.
        let lever = |n| MenuInput {
            notch: Some(n),
            ..MenuInput::NONE
        };
        s.cursor = 0;
        assert!(s.step(&lever(5), 3));
        assert_eq!(s.cursor, 1);
        s.step(&lever(5), 3);
        s.step(&lever(5), 3);
        assert!(!s.step(&lever(5), 3));
        assert_eq!(s.cursor, 3);
        s.step(&lever(0), 3);
        assert_eq!(s.cursor, 2);
    }
}

/// Synthetic tables for the unit tests (tests must not need the game data).
#[cfg(test)]
pub mod tests_support {
    use super::tables::*;
    use crate::save::Eeprom;
    use crate::screens::ranking::{NameFilter, RankingTables};
    use crate::screens::tables::{Dialog, Placement, TextDesc};

    fn p(x: i16, y: i16, slot: i16) -> Placement {
        Placement {
            x,
            y,
            layer: 3,
            palette: 4,
            slot,
        }
    }

    pub fn tables() -> FrontTables {
        let d = TextDesc {
            tbl: 1,
            col: 0,
            row: 0,
            w: 4,
            h: 2,
        };
        let legends = Legends {
            word_keys: d,
            word_pad: d,
            word_lever: d,
            upper_keys: d,
            upper_pad: d,
            lower_keys: d,
            lower_pad: d,
            up_down: d,
            upper_label: d,
            lower_label: d,
        };
        let stages = [
            [5, 6, 8, 9, 7].as_slice(),
            &[0, 3, 17, 18, 4, 1],
            &[11, 10, 20, 16],
            &[13, 15, 12, 14],
            &[2, 2, 2, 2],
        ];
        FrontTables {
            map_rows: Vec::new(),
            origin: (0, 0),
            header_tbl: [0x516; HEADER_PAGES],
            header_text: [d; HEADER_PAGES],
            title_legends: legends,
            route_legends: legends,
            route_r_label: d,
            route_r_key: d,
            title_items: [d; TITLE_ITEM_COUNT],
            option_selected: vec![d; 38],
            option_normal: vec![d; 38],
            option_lists: [
                vec![0, 22, 23, 24, 25, 26, 27, 28],
                vec![1, 6, 7, 8, 9],
                vec![2, 10, 11, 12, 13],
                vec![3, 14, 15],
                vec![4, 16, 7, 19],
                vec![5, 16, 17, 18],
            ],
            control_pictures: [0x49e; 6],
            groups: [(0x126, [1, 2, 3, -1]); GROUP_COUNT],
            group_stages: stages.map(|s| s.to_vec()),
            stages: std::array::from_fn(|i| SelectStage {
                thumb: 1,
                caption: 2,
                from: 3,
                to: 4,
                train_type: 5,
                difficulty: 1 + (i % 5) as i32,
                route: i as i32,
            }),
            group_frame: (0..4).map(|i| p(32, 8, 9 + i)).collect(),
            group_header: p(40, 28, 13),
            group_thumbs: (0..4).map(|i| p(192 + 80 * i, 16, 21 + i)).collect(),
            train_row: (0..16).map(|i| p(48, 16, 13 + i)).collect(),
            route_titles: [d; 2],
            lever_layout: vec![p(224, 108, 10), p(160, 364, 11)],
            board_layout: (0..5).map(|i| p(128, 256, 54 + i)).collect(),
            board_series: (100..113).collect(),
            board_types: (200..205).collect(),
            board_stations: (300..315).collect(),
            board_routes: std::array::from_fn(|r| [r as i32 % 13, r as i32 % 5, 1, 2]),
            calibration_titles: [d; 2],
            exit_dialog: Dialog {
                frame: 0x542,
                title: 10,
                title_y: 24,
                cursor_x: 96,
                items_y: 72,
                spacing: 48,
                items: vec![12, 14],
            },
        }
    }

    pub fn ranking_tables() -> RankingTables {
        RankingTables {
            route_tables: [
                -1, 3, -1, 10, 11, -1, 0, 4, 5, 5, 7, 7, 9, 8, 2, 12, 1, 6, 6, 6,
            ],
            headers: [[1, 2]; 20],
            entry_rank_labels: [0; 10],
            page_rank_labels: [0; 10],
            symbols: std::array::from_fn(|i| 0x100 + i as i16),
            flicker: [0x274, 0x275],
            cell_x: [144, 176, 208, 240, 360, 392, 424, 472],
            page_layout: vec![p(80, 40, 0x67), p(320, 40, 0x68)],
            entry_layout: vec![],
            default_names: [[11; 4]; 20],
            filter: NameFilter::default(),
            legend_keys: TextDesc::default(),
            legend_lever: TextDesc::default(),
        }
    }

    pub fn eeprom() -> Eeprom {
        Eeprom::from_bytes([0; crate::save::EEPROM_LEN])
    }
}

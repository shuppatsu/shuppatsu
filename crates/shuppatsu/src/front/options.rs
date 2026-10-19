//! ゲーム設定 (menus.md §4): the six-item list (0x42a520 / 0x42a560 / 0x42a730), the
//! control-mode page (0x42abb0 / 0x42ac00 / 0x42ad10) and the value page of the other items
//! (0x42afa0 / 0x42b020 / 0x42b180). `Options_Get` / `Options_Set` (0x42a3f0 / 0x42a460) are
//! [`get`] and [`Settings::set`] over the save's option bytes.

use super::tables::FrontTables;
use super::{Env, PAL_MENU, snd};
use crate::glyphs::{Btn, Hint};
use crate::save::Eeprom;
use crate::screens::{MenuInput, Sfx};
use shuppatsu_render::sprites::SpriteLayer;

/// Header page of the list (0x42a820(8)); value pages use item + 1.
const LIST_PAGE: usize = 8;
const LAYER: u8 = 3;
/// Label and value slots (0x42a730).
const SLOT_LABELS: usize = 0x90;
const SLOT_VALUES: usize = 0x9a;
/// The control page's pictures (0x42ad10).
const SLOT_PICTURE: usize = 0x9a;
const SLOT_PICTURE_2: usize = 0x9b;
/// Every menu task's destroy (0x42a710) clears slots 0x90..0xb6.
const SLOT_CLEAR: std::ops::Range<usize> = 0x90..0xb7;

/// The option items.
pub mod item {
    pub const CONTROL: usize = 0;
    pub const DIFFICULTY: usize = 1;
    pub const VIBRATION: usize = 2;
    pub const SOUND: usize = 3;
    pub const SPEED_METER: usize = 4;
    pub const DISTANCE_METER: usize = 5;
    /// Port only: the port settings screen (`super::super::port_settings`), a seventh row
    /// after the original six. Not an `Options_Get` item.
    pub const PORT: usize = 6;
}

/// The second picture of the control page per mode 0–5 (0x42ad10 immediates): tilemap and
/// offset from (x0, y0). Mode 6 draws 0x4a1 at (0x180, 0xf8) and 0x49d in the main slot.
const CONTROL_SECOND: [(i16, i16, i16); 6] = [
    (0x494, 0x128, 0xd8),
    (0x495, 0x118, 0xd8),
    (0x496, 0x122, 0xd6),
    (0x497, 0x128, 0xd8),
    (0x498, 0x133, 0xd6),
    (0x493, 0x177, 0xbf),
];
const CONTROL_PICTURE_POS: (i16, i16) = (0x118, 0x90);
const MODE6_PICTURES: [(i16, i16, i16); 2] = [(0x49d, 0x138, 0x90), (0x4a1, 0x180, 0xf8)];

/// Clears the menu slots (the destroy callback 0x42a710 of every menu task).
pub fn destroy(l: &mut SpriteLayer) {
    for s in SLOT_CLEAR {
        super::hide(l, s);
    }
}

/// `Options_Get` 0x42a3f0: the value shown for an item.
pub fn get(e: &Eeprom, env: &Env, item: usize) -> u8 {
    let o = e.options();
    match item {
        item::CONTROL => o.control_mode,
        item::DIFFICULTY => e.difficulty(),
        item::VIBRATION if env.force_feedback => o.vibration,
        item::VIBRATION => 0,
        item::SOUND if env.sound_device => o.sound,
        item::SOUND => 1,
        item::SPEED_METER => o.speed_meter,
        item::DISTANCE_METER => o.distance_meter,
        _ => 0,
    }
}

/// What `Options_Set` changed besides the byte, for the owner to act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetEffect {
    None,
    /// The control mode changed: rebuild the input.
    ControlMode(u8),
    /// Sound switched off: stop all, then ignore play messages (the sound thread tests
    /// `0x856356`).
    SoundOff,
    /// Sound switched on: restart the title BGM 0x19e.
    SoundOn,
}

/// `Options_Set` 0x42a460.
pub fn set(e: &mut Eeprom, item: usize, value: u8) -> SetEffect {
    let o = e.options();
    match item {
        item::CONTROL => {
            e.set_option(item::CONTROL, value);
            if o.control_mode != value {
                return SetEffect::ControlMode(value);
            }
        }
        item::DIFFICULTY => e.set_difficulty(value & 3),
        item::SOUND => {
            if o.sound != value {
                if value != 0 {
                    e.set_option(item::SOUND, 1);
                    return SetEffect::SoundOff;
                }
                e.set_option(item::SOUND, 0);
                return SetEffect::SoundOn;
            }
        }
        _ => e.set_option(item, value),
    }
    SetEffect::None
}

/// The list's result (`0x816834`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListResult {
    Open(usize),
    Back,
}

/// The six-item list (0x42a560).
#[derive(Debug, Clone)]
pub struct OptionsList {
    cursor: i32,
}

impl OptionsList {
    pub fn new(cursor: usize) -> Self {
        Self {
            cursor: cursor as i32,
        }
    }

    pub fn cursor(&self) -> usize {
        self.cursor as usize
    }

    pub fn tick(
        &mut self,
        input: &MenuInput,
        env: &Env,
        e: &Eeprom,
        t: &FrontTables,
        l: &mut SpriteLayer,
        sfx: &mut Vec<Sfx>,
    ) -> Option<ListResult> {
        if input.confirm {
            sfx.push(Sfx::Play(snd::DECIDE_SOFT));
            return Some(ListResult::Open(self.cursor()));
        }
        if input.cancel {
            sfx.push(Sfx::Play(snd::CANCEL));
            return Some(ListResult::Back);
        }
        // Port only: with the levers the cursor follows the power notch (P0–P5) and cannot
        // reach the seventh row, so the horn opens the port settings (nothing reads it here
        // in the original).
        if input.notch.is_some() && input.horn {
            sfx.push(Sfx::Play(snd::DECIDE_SOFT));
            return Some(ListResult::Open(item::PORT));
        }
        let (ff, sound) = (env.force_feedback, env.sound_device);
        match input.notch {
            None => {
                if self.cursor >= 1 && input.up {
                    sfx.push(Sfx::Play(snd::MOVE));
                    self.cursor -= 1;
                    if !sound && self.cursor == 3 {
                        self.cursor = 2;
                    }
                    if !ff && self.cursor == 2 {
                        self.cursor = 1;
                    }
                } else if self.cursor < item::PORT as i32 && input.down {
                    sfx.push(Sfx::Play(snd::MOVE));
                    self.cursor += 1;
                    if !ff && self.cursor == 2 {
                        self.cursor = 3;
                    }
                    if !sound && self.cursor == 3 {
                        self.cursor = 4;
                    }
                }
            }
            Some(notch) => {
                // The cursor is the power notch, with the missing items mapped down.
                let mut n = i32::from(notch);
                if !sound && n == 3 {
                    n = 2;
                }
                if !ff && n == 2 {
                    n = 1;
                }
                if n != self.cursor {
                    sfx.push(Sfx::Play(snd::MOVE));
                }
                self.cursor = n.clamp(0, 5);
            }
        }
        self.draw(env, e, t, l);
        None
    }

    /// 0x42a730.
    fn draw(&self, env: &Env, e: &Eeprom, t: &FrontTables, l: &mut SpriteLayer) {
        super::draw_menu_header(l, t, env, LIST_PAGE);
        let (x0, y0) = t.origin;
        for (i, list) in t.option_lists.iter().enumerate() {
            let sheet = if i as i32 == self.cursor {
                &t.option_selected
            } else {
                &t.option_normal
            };
            let y = y0 + 0xa0 + 0x30 * i as i16;
            let label = sheet[list[0] as usize];
            super::text(l, SLOT_LABELS + i, label, x0 + 0x60, y, PAL_MENU, LAYER);
            let v = usize::from(get(e, env, i));
            if let Some(&idx) = list.get(1 + v) {
                let d = sheet[idx as usize];
                super::text(l, SLOT_VALUES + i, d, x0 + 0x150, y, PAL_MENU, LAYER);
            }
        }
    }
}

/// A value page's result: the chosen value, or cancel (−2).
pub type PageResult = Option<u8>;

/// The control-mode page (0x42ac00): modes 0–3, or 0–6 with a joystick; with the levers, a
/// push of the power handle to P3 steps the mode round (mod 7).
#[derive(Debug, Clone)]
pub struct ControlPage {
    cursor: i32,
    /// The notch of the previous tick (+0x34).
    prev_notch: i32,
}

impl ControlPage {
    pub fn new(mode: u8, notch: u8) -> Self {
        Self {
            cursor: i32::from(mode),
            prev_notch: i32::from(notch),
        }
    }

    /// `hybrid_input` with a gamepad on a keyboard mode or mode 4: the page shows the pad's
    /// Xbox-layout mapping ([`draw_pad_panel`]), which works in all of them, instead of the
    /// mode's picture. The analog modes 5/6 keep their handle pictures.
    pub fn shows_pad_panel(&self, env: &Env) -> bool {
        env.hint == Hint::Glyphs && (0..=4).contains(&self.cursor)
    }

    pub fn tick(
        &mut self,
        input: &MenuInput,
        env: &Env,
        t: &FrontTables,
        l: &mut SpriteLayer,
        sfx: &mut Vec<Sfx>,
    ) -> Option<PageResult> {
        if input.confirm {
            sfx.push(Sfx::Play(snd::DECIDE_SOFT));
            return Some(Some(self.cursor as u8));
        }
        if input.cancel {
            sfx.push(Sfx::Play(snd::CANCEL));
            return Some(None);
        }
        let last = if env.joystick { 6 } else { 3 };
        match input.notch {
            None => {
                if self.cursor > 0 && input.up {
                    sfx.push(Sfx::Play(snd::MOVE));
                    self.cursor -= 1;
                } else if self.cursor < last && input.down {
                    sfx.push(Sfx::Play(snd::MOVE));
                    self.cursor += 1;
                }
            }
            Some(notch) => {
                let notch = i32::from(notch);
                if self.prev_notch < 3 && notch > 2 {
                    self.cursor = (self.cursor + 1) % 7;
                    sfx.push(Sfx::Play(snd::MOVE));
                }
                self.prev_notch = notch;
            }
        }
        self.draw(env, t, l);
        None
    }

    /// 0x42ad10: all seven labels, then the controller pictures of the selected mode.
    fn draw(&self, env: &Env, t: &FrontTables, l: &mut SpriteLayer) {
        super::draw_menu_header(l, t, env, item::CONTROL + 1);
        let (x0, y0) = t.origin;
        for (i, &idx) in t.option_values(item::CONTROL).iter().enumerate() {
            let sheet = if i as i32 == self.cursor {
                &t.option_selected
            } else {
                &t.option_normal
            };
            let y = y0 + 0x88 + 0x30 * i as i16;
            super::text(
                l,
                SLOT_LABELS + i,
                sheet[idx as usize],
                x0 + 0x20,
                y,
                PAL_MENU,
                LAYER,
            );
        }
        let put = |l: &mut SpriteLayer, slot, (tbl, x, y): (i16, i16, i16)| {
            super::put(l, slot, tbl, x0 + x, y0 + y, PAL_MENU, LAYER);
        };
        if self.shows_pad_panel(env) {
            // `hybrid_input` with a gamepad: the app draws the Xbox mapping instead.
            super::hide(l, SLOT_PICTURE);
            super::hide(l, SLOT_PICTURE_2);
            return;
        }
        match usize::try_from(self.cursor) {
            Ok(m @ 0..=5) => {
                put(l, SLOT_PICTURE_2, CONTROL_SECOND[m]);
                let (x, y) = CONTROL_PICTURE_POS;
                put(l, SLOT_PICTURE, (t.control_pictures[m], x, y));
            }
            Ok(6) => {
                put(l, SLOT_PICTURE_2, MODE6_PICTURES[1]);
                put(l, SLOT_PICTURE, MODE6_PICTURES[0]);
            }
            _ => {}
        }
    }
}

/// The value page of items 1–5 (0x42b020).
#[derive(Debug, Clone)]
pub struct ValuePage {
    item: usize,
    cursor: i32,
}

impl ValuePage {
    /// `0x42afa0(value, item + 1, list + 1)`.
    pub fn new(item: usize, value: u8) -> Self {
        Self {
            item,
            cursor: i32::from(value),
        }
    }

    pub fn item(&self) -> usize {
        self.item
    }

    pub fn tick(
        &mut self,
        input: &MenuInput,
        env: &Env,
        t: &FrontTables,
        l: &mut SpriteLayer,
        sfx: &mut Vec<Sfx>,
    ) -> Option<PageResult> {
        if input.confirm {
            sfx.push(Sfx::Play(snd::DECIDE_SOFT));
            return Some(Some(self.cursor.max(0) as u8));
        }
        if input.cancel {
            sfx.push(Sfx::Play(snd::CANCEL));
            return Some(None);
        }
        let last = t.option_values(self.item).len() as i32 - 1;
        match input.notch {
            None => {
                if self.cursor >= 1 && input.up {
                    sfx.push(Sfx::Play(snd::MOVE));
                    self.cursor -= 1;
                } else if self.cursor < last && input.down {
                    sfx.push(Sfx::Play(snd::MOVE));
                    self.cursor += 1;
                }
            }
            Some(notch) => {
                // (The analog branch's special case for the list at 0x47c774 never applies:
                // no item uses that list.)
                let notch = i32::from(notch);
                if self.cursor >= 1 && self.cursor > notch {
                    sfx.push(Sfx::Play(snd::MOVE));
                    self.cursor -= 1;
                } else if self.cursor < last && self.cursor < notch {
                    sfx.push(Sfx::Play(snd::MOVE));
                    self.cursor += 1;
                }
                self.cursor = self.cursor.clamp(0, last.max(0));
            }
        }
        self.draw(env, t, l);
        None
    }

    /// 0x42b180.
    fn draw(&self, env: &Env, t: &FrontTables, l: &mut SpriteLayer) {
        super::draw_menu_header(l, t, env, self.item + 1);
        let (x0, y0) = t.origin;
        for (i, &idx) in t.option_values(self.item).iter().enumerate() {
            let sheet = if i as i32 == self.cursor {
                &t.option_selected
            } else {
                &t.option_normal
            };
            let y = y0 + 0xa0 + 0x30 * i as i16;
            super::text(
                l,
                SLOT_LABELS + i,
                sheet[idx as usize],
                x0 + 0x60,
                y,
                PAL_MENU,
                LAYER,
            );
        }
    }
}

/// The controls page with a gamepad (`hybrid_input`, input.md §10.6): the Xbox-layout mapping
/// as glyph rows on a dark panel where the mode's picture was, words from the `[ui]`
/// catalogue in the page's language.
pub fn draw_pad_panel(
    l: &mut SpriteLayer,
    t: &FrontTables,
    look: &super::port_settings::Look,
    ui: &super::port_settings::UiText,
) {
    use crate::glyphs::{DPAD_LR, DPAD_UD, Line};
    use shuppatsu_render::sprites::{TextStyle, WidgetKind};
    use shuppatsu_render::text::Align;
    let (x0, y0) = (f32::from(t.origin.0), f32::from(t.origin.1));
    let (x, y, w, h) = (x0 + 276.0, y0 + 138.0, 348.0, 304.0);
    l.add_widget(LAYER, [x, y, w, h], WidgetKind::Fill(look.panel));
    let mut title = TextStyle::plain(look.gold, Some(13.0));
    title.align = Align::Left;
    l.add_styled_text(
        LAYER,
        [x + 12.0, y + 6.0, w - 24.0, 24.0],
        ui.get("pad_title"),
        title,
    );
    let rows: [(&[Btn], &str); 9] = [
        (&[DPAD_UD, Btn::LStick], "pad_power_dpad"),
        (&[Btn::Rt, Btn::Rb], "pad_power_shoulders"),
        (&[DPAD_LR, Btn::LStick], "pad_brake_dpad"),
        (&[Btn::Lt, Btn::Lb], "pad_brake_shoulders"),
        (&[Btn::A], "pad_a"),
        (&[Btn::X], "pad_x"),
        (&[Btn::B, Btn::View], "pad_b"),
        (&[Btn::Y], "pad_y"),
        (&[Btn::Menu], "pad_menu"),
    ];
    const ROW: f32 = 27.0;
    for (n, (btns, key)) in rows.into_iter().enumerate() {
        let ry = y + 32.0 + ROW * n as f32;
        // The glyphs in a fixed column, so the words line up.
        let mut glyphs = Line::new();
        for &b in btns {
            glyphs = glyphs.btn(b);
        }
        glyphs.draw(
            l,
            LAYER,
            [x + 10.0, ry, 76.0, ROW],
            look.bright,
            12.0,
            Align::Left,
        );
        let mut s = TextStyle::plain(look.bright, Some(12.0));
        s.align = Align::Left;
        l.add_styled_text(LAYER, [x + 90.0, ry, w - 100.0, ROW], ui.get(key), s);
    }
    let mut note = TextStyle::plain(look.dim, Some(10.0));
    note.align = Align::Left;
    let ny = y + 32.0 + ROW * 9.0 + 2.0;
    l.add_styled_text(
        LAYER,
        [x + 12.0, ny, w - 24.0, 22.0],
        ui.get("pad_note"),
        note,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::front::tests_support::{eeprom, tables};

    fn press(f: impl FnOnce(&mut MenuInput)) -> MenuInput {
        let mut i = MenuInput::NONE;
        f(&mut i);
        i
    }

    #[test]
    fn list_skips_missing_devices() {
        let t = tables();
        let e = eeprom();
        let mut l = SpriteLayer::default();
        let mut sfx = Vec::new();
        let env = Env {
            force_feedback: false,
            sound_device: true,
            ..Env::default()
        };
        let mut m = OptionsList::new(1);
        let down = press(|i| i.down = true);
        m.tick(&down, &env, &e, &t, &mut l, &mut sfx);
        assert_eq!(m.cursor(), 3, "vibration skipped without force feedback");
        let quiet = Env {
            sound_device: false,
            ..env
        };
        let mut m = OptionsList::new(1);
        m.tick(&down, &quiet, &e, &t, &mut l, &mut sfx);
        assert_eq!(m.cursor(), 4, "and sound without a device");
        let up = press(|i| i.up = true);
        m.tick(&up, &quiet, &e, &t, &mut l, &mut sfx);
        assert_eq!(m.cursor(), 1);
        for _ in 0..10 {
            m.tick(&down, &quiet, &e, &t, &mut l, &mut sfx);
        }
        assert_eq!(m.cursor(), item::PORT, "clamped at the port's row");
        m.tick(&up, &quiet, &e, &t, &mut l, &mut sfx);
        let r = m.tick(&press(|i| i.confirm = true), &env, &e, &t, &mut l, &mut sfx);
        assert_eq!(r, Some(ListResult::Open(5)));
        assert_eq!(sfx.last(), Some(&Sfx::Play(snd::DECIDE_SOFT)));
    }

    /// Port only: the seventh row opens the port settings; the six original rows keep their
    /// places, and with the levers (cursor = power notch, P0–P5) the horn opens it.
    #[test]
    fn port_settings_row() {
        let t = tables();
        let e = eeprom();
        let mut l = SpriteLayer::default();
        let mut sfx = Vec::new();
        let env = Env::default();
        let mut m = OptionsList::new(item::DISTANCE_METER);
        m.tick(&press(|i| i.down = true), &env, &e, &t, &mut l, &mut sfx);
        assert_eq!(m.cursor(), item::PORT);
        let r = m.tick(&press(|i| i.confirm = true), &env, &e, &t, &mut l, &mut sfx);
        assert_eq!(r, Some(ListResult::Open(item::PORT)));
        // Levers: P5 is the last original row; the horn opens the port settings.
        let mut m = OptionsList::new(0);
        let lever = |n| press(|i| i.notch = Some(n));
        m.tick(&lever(5), &env, &e, &t, &mut l, &mut sfx);
        assert_eq!(m.cursor(), item::DISTANCE_METER);
        let horn = press(|i| {
            i.notch = Some(5);
            i.horn = true;
        });
        let r = m.tick(&horn, &env, &e, &t, &mut l, &mut sfx);
        assert_eq!(r, Some(ListResult::Open(item::PORT)));
        // The keys' horn does nothing here, as in the original.
        let mut m = OptionsList::new(0);
        let r = m.tick(&press(|i| i.horn = true), &env, &e, &t, &mut l, &mut sfx);
        assert_eq!(r, None);
    }

    #[test]
    fn value_pages() {
        let t = tables();
        let mut l = SpriteLayer::default();
        let mut sfx = Vec::new();
        let env = Env::default();
        let mut p = ValuePage::new(item::SPEED_METER, 1);
        let down = press(|i| i.down = true);
        p.tick(&down, &env, &t, &mut l, &mut sfx);
        p.tick(&down, &env, &t, &mut l, &mut sfx);
        assert_eq!(p.cursor, 2, "three values");
        let r = p.tick(&press(|i| i.confirm = true), &env, &t, &mut l, &mut sfx);
        assert_eq!(r, Some(Some(2)));
        let r = p.tick(&press(|i| i.cancel = true), &env, &t, &mut l, &mut sfx);
        assert_eq!(r, Some(None));

        let mut c = ControlPage::new(0, 0);
        for _ in 0..10 {
            c.tick(&down, &env, &t, &mut l, &mut sfx);
        }
        assert_eq!(c.cursor, 3, "keyboard modes only without a joystick");
        let joy = Env {
            joystick: true,
            ..env
        };
        for _ in 0..10 {
            c.tick(&down, &joy, &t, &mut l, &mut sfx);
        }
        assert_eq!(c.cursor, 6);
        // Levers: P3 from below steps the mode round.
        let lever = |n| press(|i| i.notch = Some(n));
        c.tick(&lever(3), &joy, &t, &mut l, &mut sfx);
        assert_eq!(c.cursor, 0);
        c.tick(&lever(4), &joy, &t, &mut l, &mut sfx);
        assert_eq!(c.cursor, 0, "only on the way up through P3");
    }

    #[test]
    fn set_writes_the_eeprom() {
        let mut e = eeprom();
        assert_eq!(
            set(&mut e, item::CONTROL, 2),
            SetEffect::ControlMode(2),
            "a new mode"
        );
        assert_eq!(e.options().control_mode, 2);
        assert_eq!(set(&mut e, item::DIFFICULTY, 3), SetEffect::None);
        assert_eq!(e.difficulty(), 3);
        assert_eq!(set(&mut e, item::SOUND, 1), SetEffect::SoundOff);
        assert_eq!(e.options().sound, 1);
        assert_eq!(set(&mut e, item::SOUND, 1), SetEffect::None);
        assert_eq!(set(&mut e, item::SOUND, 0), SetEffect::SoundOn);
        set(&mut e, item::DISTANCE_METER, 0);
        assert_eq!(e.options().distance_meter, 0);
        let env = Env {
            sound_device: false,
            ..Env::default()
        };
        assert_eq!(get(&e, &env, item::SOUND), 1, "no device reads as off");
        assert_eq!(get(&e, &env, item::VIBRATION), 0);
    }
}

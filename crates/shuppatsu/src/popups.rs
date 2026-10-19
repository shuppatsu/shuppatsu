//! Event-driven HUD elements (docs/re/hud.md §3.9–3.11, §3.14, §3.16): the signal aspect
//! sprite, the gradient / whistle / speed-limit signs, the bonus and penalty time messages,
//! the ATS lamp, the generic timed popups (obstacle warning, all-round clear and its chibi)
//! and the time-up banner. The rules engine raises [`RuleEvent`]s; these become timed sprite
//! slots.

use crate::slots;
use anyhow::Result;
use shuppatsu_engine::rules::{RuleEvent, SignalHud, TimeReason, signal_sprite};
use shuppatsu_formats::exe::Exe;
use shuppatsu_render::sprites::{Slot, SpriteLayer};

const SLOT_SIGNAL: usize = 0x60;
const SIGNAL_POS: (i16, i16) = (528, 113);
const SLOT_GRADE_SIGN: usize = 0x5e;
const SLOT_LIMIT_SIGN: usize = 0x5f;
const SLOT_MESSAGE: usize = 0x28;
/// 減点 word, three digits and 秒 (slots 0x29..=0x2d).
const SLOT_MESSAGE_SIGN: usize = 0x29;
const SLOT_MESSAGE_DIGITS: usize = 0x2a;
const SLOT_MESSAGE_SECONDS: usize = 0x2d;
const SLOT_ATS: usize = 0x32;
const SLOT_POPUP: usize = 0x33;
const SLOT_CHIBI: usize = 0x62;
const SLOT_TIME_UP: usize = 0x39;
const SLOT_TIME_UP_ADVICE: usize = 0x3a;

/// Per-stage `{i32 three_lamp_set, i32 repeater_set}` (0x466c08, stride 8, read by 0x411500).
const SIGNAL_SETS_VA: u32 = 0x466c08;

/// `Sign_Show` kinds (0x411a00): 0 level, 1 up, 2 down grade, 3–4 whistle, 5 limit ends,
/// 6.. limits. Kinds below 5 use slot 0x5e, the others slot 0x5f.
const SIGN_LEVEL: i32 = 0;
const SIGN_UP: i32 = 1;
const SIGN_DOWN: i32 = 2;
const SIGN_WHISTLE: i32 = 4;
const SIGN_LIMIT_END: i32 = 5;
const FIRST_LIMIT_KIND: i32 = 6;
const TBL_GRADIENTS: [i16; 3] = [0x45b, 0x45c, 0x45d];
const TBL_WHISTLE: i16 = 0x46f;
const TBL_LIMIT_END: i16 = 0x46e;
/// A sign task first draws on its 6th update (`+0x38 = 5`, counted down to −1, 0x411d40).
const SIGN_DELAY: i32 = 5;
/// Gradient signs (0x40c4e0) and 制限解除 (0x40acd0 kind 6) show for 60 / 90 ticks.
const GRADIENT_TICKS: i32 = 0x3c;
const LIMIT_END_TICKS: i32 = 0x5a;
/// Limit signs blink with the clock: drawn while `(clock / 15)` is odd (0x411e00).
const BLINK_HALF_PERIOD: i32 = 15;
/// Gradient thresholds on `0x84c3d8` (doubles 10.0 at 0x458b88, −10.0 at 0x459538, 0.0001 at
/// 0x459480) and the steady ticks before the level sign (0x3c), 0x40c4e0. The counter starts
/// at 0x46 each segment (0x40cf40), so a level sign needs an up or down sign first.
const GRADE_UP: f64 = 10.0;
const GRADE_DOWN: f64 = -10.0;
const GRADE_LEVEL: f64 = 0.0001;
const LEVEL_AFTER_TICKS: i32 = 0x3c;
const LEVEL_COUNTER_START: i32 = 0x46;

/// Time messages: `(tilemap, width)` per kind 0..=15; kind 8 has no message (hud.md §3.11).
const MESSAGES: [Option<(i16, i16)>; 16] = [
    Some((0x1be, 384)),
    Some((0x1aa, 160)),
    Some((0x1ab, 160)),
    Some((0x1ac, 160)),
    Some((0x1af, 128)),
    Some((0x1b0, 144)),
    Some((0x1b1, 192)),
    Some((0x1b2, 176)),
    None,
    Some((0x1c6, 256)),
    Some((0x1c7, 176)),
    Some((0x1c8, 144)),
    Some((0x258, 256)),
    Some((0x259, 160)),
    Some((0x1c5, 256)),
    Some((0x1ce, 176)),
];
const TBL_PENALTY_WORD: i16 = 0x1ad;
const TBL_MESSAGE_DIGIT0: i16 = 0x1b3;
const TBL_MESSAGE_SECONDS: i16 = 0x1bd;
const MESSAGE_TICKS: u32 = 60;
/// Number-row x positions (640) by `(bonus ? 3 : 0) + (3 − digits)`: penalty word, three
/// digits, 秒. `OFF` = x 640 (off screen).
const OFF: i16 = 640;
const NUMBER_ROW_X: [[i16; 5]; 6] = [
    [216, 280, 312, 344, 376],
    [232, OFF, 296, 328, 360],
    [248, OFF, OFF, 312, 344],
    [OFF, 248, 280, 312, 344],
    [OFF, OFF, 264, 296, 328],
    [OFF, OFF, OFF, 280, 312],
];

/// ATS lamp task 0x417d10 at `origin + (0xd0, 0x98)`: 0x20e "ATS作動" while tripped, 0x211
/// "ATS確認" for 120 updates after the acknowledgement.
const ATS_POS: (i16, i16) = (0xd0, 0x98);
const TBL_ATS_TRIPPED: i16 = 0x20e;
const TBL_ATS_CONFIRMED: i16 = 0x211;
const ATS_CONFIRMED_TICKS: i32 = 0x78;

/// Generic popups (0x417de0): obstacle warning 非常ブレーキ使え (course event 8, 0x40acd0) and
/// 全区間走破 (final station, 0x40a0a0); `(tbl, origin offset, ticks, delay)`.
const POPUP_OBSTACLE: (i16, (i16, i16), i32, i32) = (0x24b, (0xc0, 0xa8), 0x5a, 10);
const POPUP_ALL_CLEAR: (i16, (i16, i16), i32, i32) = (0x20f, (0xb8, 0xac), 100, 10);
/// 乗務完了 chibi (0x4113e0 → 0x411450): after 5 updates, 0x459 for 50 updates then 0x45a to
/// update 100, at `(origin.x + 0x10, origin.x + 0x139)` (the y adds the x origin, as the
/// original does); the destructor puts the slot back at (0x50, 0x91).
const CHIBI_DELAY: i32 = 5;
const CHIBI_SWITCH: i32 = 0x32;
const CHIBI_END: i32 = 100;
const TBL_CHIBI: [i16; 2] = [0x459, 0x45a];
const CHIBI_POS: (i16, i16) = (0x10, 0x139);
const CHIBI_HOME: (i16, i16) = (0x50, 0x91);

/// Time-up task 0x4158b0 / 0x4159c0: 時間切れ (0x252) slides in from x 640 by 10 per update
/// while its position is above 0xa8, at `origin + (0x40 + pos, 0xcc)`; the advice sprite at
/// update 0x37; the task lasts until its counter passes 0x95.
const TBL_TIME_UP: i16 = 0x252;
const TIME_UP_START: i32 = 0x200;
const TIME_UP_STOP: i32 = 0xa8;
const TIME_UP_STEP: i32 = 10;
const TIME_UP_X: i16 = 0x40;
const TIME_UP_Y: i16 = 0xcc;
/// Placement at creation: (0x280, 0xf4), layer 3.
const TIME_UP_FIRST: (i16, i16) = (0x280, 0xf4);
const TIME_UP_ADVICE_AT: i32 = 0x37;
const TIME_UP_LAST: i32 = 0x95;

#[derive(Clone, Copy, Debug, PartialEq)]
struct SignTask {
    kind: i32,
    tbl: i16,
    delay: i32,
    ticks: i32,
    /// `+0x48`: shown until the odometer passes this (truncated odometer + distance).
    until_m: Option<i32>,
    /// `+0x40`: limit signs blink until the odometer passes this.
    blink_until_m: Option<i32>,
    enabled: bool,
}

impl SignTask {
    /// The end test shared by 0x411d40 and 0x411e00, after drawing.
    fn ends(&mut self, odometer: f32) -> bool {
        let over = match self.until_m {
            None => self.ticks < 0,
            Some(m) => (m as f32) - odometer < 0.0,
        };
        if over || !self.enabled {
            return true;
        }
        self.ticks -= 1;
        false
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Message {
    reason: TimeReason,
    seconds: i32,
    ticks_left: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Popup {
    tbl: i16,
    delay: i32,
    ticks: i32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TimeUp {
    pos: i32,
    count: i32,
    advice: i16,
}

/// The state of the event-driven HUD elements.
pub struct Popups {
    /// Stage's `[three_lamp_set, repeater_set]` (0x466c08).
    signal_sets: [i32; 2],
    /// Slot 0x60: the signal display tasks of `Signal_Show` 0x411500 (hud.md §3.9).
    signal: SignalHud,
    /// Gradient / whistle sign tasks (slot 0x5e), newest last; `0x856d54` is the newest while
    /// `grade_current`.
    grade_signs: Vec<SignTask>,
    grade_current: bool,
    /// Limit sign tasks (slot 0x5f), newest last (`0x856d04`).
    limit_signs: Vec<SignTask>,
    limit_current: bool,
    /// Gradient sign state `0x856faa` (0 level, 1 up, 2 down) and level counter `0x8570a0`.
    grade_state: i32,
    level_ticks: i32,
    message: Option<Message>,
    /// `0x856d08`: −1 off, 0 tripped, 1 acknowledged; and the lamp task's counter.
    ats_lamp: i32,
    ats_ticks: i32,
    popups: Vec<Popup>,
    chibi: Option<(i32, i32)>,
    time_up: Option<TimeUp>,
    origin: (i16, i16),
    advice_pos: (i16, i16),
}

impl Popups {
    pub fn new(
        exe: &Exe<'_>,
        stage: usize,
        origin: (i16, i16),
        advice_pos: (i16, i16),
    ) -> Result<Self> {
        let sets = exe.reader_at(SIGNAL_SETS_VA + 8 * stage as u32, 8, "signal sets")?;
        Ok(Self::with_sets(
            [sets.i32(0)?, sets.i32(4)?],
            origin,
            advice_pos,
        ))
    }

    fn with_sets(signal_sets: [i32; 2], origin: (i16, i16), advice_pos: (i16, i16)) -> Self {
        Self {
            signal_sets,
            signal: SignalHud::default(),
            grade_signs: Vec::new(),
            grade_current: false,
            limit_signs: Vec::new(),
            limit_current: false,
            grade_state: 0,
            level_ticks: LEVEL_COUNTER_START,
            message: None,
            ats_lamp: -1,
            ats_ticks: 0,
            popups: Vec::new(),
            chibi: None,
            time_up: None,
            origin,
            advice_pos,
        }
    }

    /// Segment start: the tasks of the last segment are gone and 0x40cf40 resets the gradient
    /// state.
    pub fn clear(&mut self) {
        self.signal.clear();
        self.grade_signs.clear();
        self.grade_current = false;
        self.limit_signs.clear();
        self.limit_current = false;
        self.grade_state = 0;
        self.level_ticks = LEVEL_COUNTER_START;
        self.message = None;
    }

    /// A time message is up (`0x856d38 ≠ 0`): captions, banners and landmarks hide.
    pub fn message_up(&self) -> bool {
        self.message.is_some()
    }

    /// The time-up popup is running (`0x855ff8 & 0x400`).
    pub fn time_up_running(&self) -> bool {
        self.time_up.is_some()
    }

    /// Applies one rule event, before [`Popups::update`] of the same tick (the original's rule
    /// calls precede its HUD tasks). `start_m` is the odometer the rules saw (before this
    /// tick's physics).
    pub fn event(&mut self, e: &RuleEvent, start_m: f32, layer: &mut SpriteLayer) {
        match *e {
            RuleEvent::SignalShown {
                aspect,
                forced,
                display,
                variant,
                ticks,
                distance_m,
                start_m,
            } => {
                let sprite = signal_sprite(
                    i32::from(aspect),
                    i32::from(display),
                    variant,
                    self.signal_sets,
                );
                if let Some(sprite) = sprite {
                    self.signal.show(sprite, forced, ticks, distance_m, start_m);
                }
            }
            RuleEvent::SignalHidden => self.signal.hide(),
            RuleEvent::SpeedLimit {
                kmh,
                notice_m,
                offset_m,
            } => {
                let kind = kmh / 5 - 4 + FIRST_LIMIT_KIND;
                self.sign_show(kind, 0, notice_m, offset_m, start_m);
            }
            RuleEvent::SpeedLimitEnd => {
                self.sign_show(SIGN_LIMIT_END, LIMIT_END_TICKS, 0, 0, start_m);
            }
            RuleEvent::HornSign { within_m } => {
                self.sign_show(SIGN_WHISTLE, 0, within_m, 0, start_m);
            }
            RuleEvent::Time { reason, seconds } => {
                self.message = Some(Message {
                    reason,
                    seconds,
                    ticks_left: MESSAGE_TICKS,
                });
            }
            RuleEvent::AtsAcknowledged => self.ats_lamp = 1,
            RuleEvent::AtsEmergency => self.ats_lamp = 0,
            RuleEvent::SignalsReset => self.ats_lamp = -1,
            RuleEvent::ObstacleStop => self.popup(layer, POPUP_OBSTACLE),
            _ => {}
        }
    }

    /// The rules cleared the ATS without an acknowledgement (arrival, recovery, loop exit):
    /// `0x856d08 = −1` with `0x857206 = 0`.
    pub fn ats_cleared(&mut self) {
        if self.ats_lamp == 0 {
            self.ats_lamp = -1;
        }
    }

    /// The all-round-clear popup and the 乗務完了 chibi (0x40a0a0 at the final station).
    pub fn all_clear(&mut self, layer: &mut SpriteLayer) {
        self.popup(layer, POPUP_ALL_CLEAR);
        self.chibi = Some((CHIBI_DELAY, 0));
    }

    /// `Popup(slot, tbl, x, y, pal 4, ticks, delay, blink 0)` 0x417de0 on slot 0x33.
    fn popup(
        &mut self,
        layer: &mut SpriteLayer,
        (tbl, (dx, dy), ticks, delay): (i16, (i16, i16), i32, i32),
    ) {
        slots::clear(layer, SLOT_POPUP);
        slots::place(
            layer,
            SLOT_POPUP,
            self.origin.0 + dx,
            self.origin.1 + dy,
            3,
            4,
        );
        self.popups.push(Popup { tbl, delay, ticks });
    }

    /// `Sign_Show(kind, ticks, distance, blink_distance)` 0x411a00.
    fn sign_show(&mut self, kind: i32, ticks: i32, distance_m: i32, blink_m: i32, odometer: f32) {
        let tbl = match kind {
            SIGN_LEVEL | SIGN_UP | SIGN_DOWN => TBL_GRADIENTS[kind as usize],
            3 | SIGN_WHISTLE => TBL_WHISTLE,
            SIGN_LIMIT_END => TBL_LIMIT_END,
            6..=21 => 0x45e + (kind - 6) as i16,
            22..=27 => 0x48a + (kind - 22) as i16,
            _ => return,
        };
        let (list, current) = if kind < SIGN_LIMIT_END {
            (&mut self.grade_signs, &mut self.grade_current)
        } else {
            (&mut self.limit_signs, &mut self.limit_current)
        };
        if *current && let Some(old) = list.last_mut() {
            if old.kind == kind {
                return;
            }
            old.enabled = false;
        }
        let start = odometer as i32;
        let until_m = (distance_m != 0).then_some(start + distance_m);
        let blink_until_m = (distance_m != 0 && blink_m != 0).then_some(start + blink_m);
        list.push(SignTask {
            kind,
            tbl,
            delay: SIGN_DELAY,
            ticks,
            until_m,
            blink_until_m,
            enabled: true,
        });
        *current = true;
    }

    /// The gradient-sign part of 0x40c4e0 (driving-loop body only), with the gradient force
    /// `0x84c3d8` the physics left on the previous tick.
    pub fn gradient_step(&mut self, grade_force: f32, odometer: f32) {
        let g = f64::from(grade_force);
        if g >= GRADE_UP {
            if self.grade_state != SIGN_UP {
                self.sign_show(SIGN_UP, GRADIENT_TICKS, 0, 0, odometer);
                self.grade_state = SIGN_UP;
                self.level_ticks = 0;
            }
        } else if g <= GRADE_DOWN {
            if self.grade_state != SIGN_DOWN {
                self.sign_show(SIGN_DOWN, GRADIENT_TICKS, 0, 0, odometer);
                self.grade_state = SIGN_DOWN;
                self.level_ticks = 0;
            }
        } else if g.abs() <= GRADE_LEVEL {
            self.level_ticks += 1;
            if self.level_ticks == LEVEL_AFTER_TICKS {
                self.sign_show(SIGN_LEVEL, GRADIENT_TICKS, 0, 0, odometer);
                self.grade_state = SIGN_LEVEL;
            }
        }
    }

    /// The time-up popup (0x4158b0): cancels the signal, signs and the time message, and slides
    /// 時間切れ in; `advice` is the sprite of 0x40ddb0.
    pub fn time_up(&mut self, layer: &mut SpriteLayer, advice: i16) {
        self.signal.hide();
        for s in self
            .grade_signs
            .iter_mut()
            .chain(self.limit_signs.iter_mut())
        {
            s.enabled = false;
        }
        slots::clear(layer, SLOT_TIME_UP);
        slots::clear(layer, SLOT_TIME_UP_ADVICE);
        slots::place(layer, SLOT_TIME_UP, TIME_UP_FIRST.0, TIME_UP_FIRST.1, 3, 4);
        slots::image(layer, SLOT_TIME_UP, TBL_TIME_UP);
        self.time_up = Some(TimeUp {
            pos: TIME_UP_START,
            count: 0,
            advice,
        });
    }

    /// Sets the slots for this tick and runs the timers. `horn_held`: the input word of this
    /// tick has the horn bit (0x48c428 & 0x100 clear).
    pub fn update(
        &mut self,
        layer: &mut SpriteLayer,
        odometer_m: f32,
        clock: i32,
        horn_held: bool,
    ) {
        // The signal tasks (0x411940) update after this tick's rule calls.
        self.signal.tick(odometer_m);
        layer.slots[SLOT_SIGNAL] = match self.signal.slot() {
            Some(t) => Slot::new(t as i16, SIGNAL_POS.0, SIGNAL_POS.1, 4, 2, true),
            None => Slot {
                keep: true,
                ..Slot::CLEAR
            },
        };
        self.grade_sign_tasks(layer, odometer_m, horn_held);
        self.limit_sign_tasks(layer, odometer_m, clock);
        self.message_slots(layer);
        self.ats_lamp_task(layer);
        self.popup_tasks(layer);
        self.time_up_task(layer);
    }

    /// 0x411d40 for each gradient / whistle task.
    fn grade_sign_tasks(&mut self, layer: &mut SpriteLayer, odometer: f32, horn_held: bool) {
        let n = self.grade_signs.len();
        let mut index = 0;
        let mut i = 0;
        while i < self.grade_signs.len() {
            let newest = index + 1 == n;
            index += 1;
            let t = &mut self.grade_signs[i];
            if t.delay > -1 {
                t.delay -= 1;
                i += 1;
                continue;
            }
            // The whistle sign ends as soon as the horn is held.
            let ends = if t.tbl != TBL_WHISTLE || !horn_held {
                slots::image_palette(layer, SLOT_GRADE_SIGN, t.tbl, 4);
                t.ends(odometer)
            } else {
                true
            };
            if !ends {
                i += 1;
                continue;
            }
            // Destructor 0x411dd0.
            slots::image(layer, SLOT_GRADE_SIGN, -1);
            self.grade_signs.remove(i);
            if newest {
                self.grade_current = false;
            }
        }
    }

    /// 0x411e00 for each limit task: hidden while a gradient / whistle task exists, blinking
    /// until the enforcement point.
    fn limit_sign_tasks(&mut self, layer: &mut SpriteLayer, odometer: f32, clock: i32) {
        let n = self.limit_signs.len();
        let grade_up = self.grade_current;
        let mut index = 0;
        let mut i = 0;
        while i < self.limit_signs.len() {
            let newest = index + 1 == n;
            index += 1;
            let t = &mut self.limit_signs[i];
            if t.delay > -1 {
                t.delay -= 1;
                i += 1;
                continue;
            }
            if grade_up {
                slots::image(layer, SLOT_LIMIT_SIGN, -1);
            } else {
                match t.blink_until_m {
                    None => slots::image_palette(layer, SLOT_LIMIT_SIGN, t.tbl, 4),
                    Some(m) if (m as f32) - odometer < 0.0 => {
                        // Enforcement reached: no draw this update, steady from the next.
                        t.blink_until_m = None;
                    }
                    Some(_) if (clock / BLINK_HALF_PERIOD) % 2 != 0 => {
                        slots::image_palette(layer, SLOT_LIMIT_SIGN, t.tbl, 4);
                    }
                    Some(_) => slots::image(layer, SLOT_LIMIT_SIGN, -1),
                }
            }
            if !t.ends(odometer) {
                i += 1;
                continue;
            }
            // Destructor 0x411ef0.
            slots::image(layer, SLOT_LIMIT_SIGN, -1);
            self.limit_signs.remove(i);
            if newest {
                self.limit_current = false;
            }
        }
    }

    fn message_slots(&mut self, layer: &mut SpriteLayer) {
        for slot in SLOT_MESSAGE..=SLOT_MESSAGE_SECONDS {
            layer.slots[slot] = Slot {
                keep: true,
                ..Slot::CLEAR
            };
        }
        if self.time_up.is_some() {
            // The message task ends when the phase flag 0x400 is set (0x415810).
            self.message = None;
        }
        let Some(m) = self.message.as_mut() else {
            return;
        };
        m.ticks_left = m.ticks_left.saturating_sub(1);
        let m = *m;
        if m.ticks_left == 0 {
            self.message = None;
        }
        let kind = m.reason as usize;
        if let Some((tbl, width)) = MESSAGES.get(kind).copied().flatten() {
            layer.slots[SLOT_MESSAGE] = Slot::new(tbl, 320 - width / 2, 200, 4, 1, true);
        }
        if m.seconds == 0 {
            return;
        }
        let bonus = m.reason.is_bonus();
        let v = m.seconds.unsigned_abs().min(999);
        let digits: Vec<u32> = v.to_string().bytes().map(|b| u32::from(b - b'0')).collect();
        let row = NUMBER_ROW_X[usize::from(bonus) * 3 + (3 - digits.len())];
        let y = if matches!(kind, 0..=3 | 13) { 264 } else { 232 };
        let palette = if bonus { 4 } else { 5 };
        if !bonus {
            layer.slots[SLOT_MESSAGE_SIGN] =
                Slot::new(TBL_PENALTY_WORD, row[0], y, palette, 1, true);
        }
        // Digits fill the last positions of the three digit columns.
        for (i, d) in digits.iter().enumerate() {
            let col = 3 - digits.len() + i;
            layer.slots[SLOT_MESSAGE_DIGITS + col] = Slot::new(
                TBL_MESSAGE_DIGIT0 + *d as i16,
                row[1 + col],
                y,
                palette,
                1,
                true,
            );
        }
        layer.slots[SLOT_MESSAGE_SECONDS] =
            Slot::new(TBL_MESSAGE_SECONDS, row[4], y, palette, 1, true);
    }

    /// The ATS lamp task 0x417d10.
    fn ats_lamp_task(&mut self, layer: &mut SpriteLayer) {
        let pos = (self.origin.0 + ATS_POS.0, self.origin.1 + ATS_POS.1);
        match self.ats_lamp {
            0 => {
                slots::set(layer, SLOT_ATS, TBL_ATS_TRIPPED, pos, 4, 3);
                self.ats_ticks = 0;
            }
            1 => {
                slots::set(layer, SLOT_ATS, TBL_ATS_CONFIRMED, pos, 4, 3);
                self.ats_ticks += 1;
                if self.ats_ticks >= ATS_CONFIRMED_TICKS {
                    self.ats_lamp = -1;
                }
            }
            _ => {
                slots::image(layer, SLOT_ATS, -1);
                self.ats_ticks = 0;
            }
        }
    }

    /// The generic popup tasks 0x417e80 and the chibi task 0x411450.
    fn popup_tasks(&mut self, layer: &mut SpriteLayer) {
        let mut i = 0;
        while i < self.popups.len() {
            let p = &mut self.popups[i];
            if p.delay > 0 {
                p.delay -= 1;
            } else if p.delay == 0 {
                p.delay = -1;
                slots::image(layer, SLOT_POPUP, p.tbl);
            } else if p.ticks < 0 {
                // Destructor 0x417f00.
                slots::clear(layer, SLOT_POPUP);
                self.popups.remove(i);
                continue;
            } else {
                p.ticks -= 1;
            }
            i += 1;
        }
        if let Some((delay, count)) = self.chibi.as_mut() {
            if *delay > 0 {
                *delay -= 1;
                return;
            }
            let x = self.origin.0 + CHIBI_POS.0;
            let y = self.origin.0 + CHIBI_POS.1;
            slots::move_to(layer, SLOT_CHIBI, x, y);
            let tbl = TBL_CHIBI[usize::from(*count >= CHIBI_SWITCH)];
            slots::image_palette(layer, SLOT_CHIBI, tbl, 4);
            *count += 1;
            if *count > CHIBI_END {
                // Destructor 0x4114d0.
                slots::move_to(layer, SLOT_CHIBI, CHIBI_HOME.0, CHIBI_HOME.1);
                slots::image(layer, SLOT_CHIBI, -1);
                self.chibi = None;
            }
        }
    }

    /// The time-up task 0x4159c0.
    fn time_up_task(&mut self, layer: &mut SpriteLayer) {
        let Some(t) = self.time_up.as_mut() else {
            return;
        };
        if t.pos > TIME_UP_STOP {
            t.pos -= TIME_UP_STEP;
            let x = self.origin.0 + TIME_UP_X + t.pos as i16;
            let y = self.origin.1 + TIME_UP_Y;
            slots::set(layer, SLOT_TIME_UP, TBL_TIME_UP, (x, y), 4, 3);
        }
        if t.count == TIME_UP_ADVICE_AT {
            slots::set(layer, SLOT_TIME_UP_ADVICE, t.advice, self.advice_pos, 4, 3);
        }
        if t.count > TIME_UP_LAST {
            // Destructor 0x415a80.
            slots::clear(layer, SLOT_TIME_UP);
            slots::clear(layer, SLOT_TIME_UP_ADVICE);
            self.time_up = None;
            return;
        }
        t.count += 1;
    }
}

/// The time-up advice sprite of 0x40ddb0 for the cause `0x856d84` (the category of the last
/// statistic added; 2 when statistic 2 exceeds 1). `early`: the early-running accumulator
/// `0x8570c4` is positive.
pub fn time_up_advice(cause: i32, early: bool) -> i16 {
    match cause {
        0 => 0x3f6,
        1 if early => 0x3f6,
        2 => 0x3f9,
        3 | 0xb => 0x3fa,
        4 => 0x3f3,
        6 => 0x3f2,
        7 => 0x3f4,
        9 => 0x3f7,
        _ => 0x3fe,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn popups() -> Popups {
        Popups::with_sets([0, 1], (0, 0), (304, 344))
    }

    fn signal(aspect: i16, forced: bool, ticks: i32, distance_m: i32, start_m: i32) -> RuleEvent {
        RuleEvent::SignalShown {
            aspect,
            forced,
            display: 2,
            variant: 0,
            ticks,
            distance_m,
            start_m,
        }
    }

    /// Runs one HUD update and returns slot 0x60's tilemap.
    fn step(p: &mut Popups, layer: &mut SpriteLayer, odometer_m: f32) -> i16 {
        p.update(layer, odometer_m, 0, false);
        layer.slots[SLOT_SIGNAL].tilemap
    }

    const RED_STARTING: i16 = 0x472;
    const GREEN_STARTING: i16 = 0x47e;

    #[test]
    fn signal_waits_five_ticks_then_counts_down() {
        let mut p = popups();
        let mut layer = SpriteLayer::default();
        p.event(&signal(4, false, 100, 0, 0), 0.0, &mut layer);
        let seen: Vec<i16> = (0..108).map(|_| step(&mut p, &mut layer, 0.0)).collect();
        assert!(seen[..5].iter().all(|&t| t == -1), "5-tick delay");
        // First draw with the counter at 100, then 99 … 0: 101 updates.
        assert!(seen[5..106].iter().all(|&t| t == RED_STARTING));
        assert_eq!(seen[106], -1);
        assert_eq!(seen[107], -1);
    }

    #[test]
    fn signal_with_a_distance_shows_until_the_odometer_passes_it() {
        let mut p = popups();
        let mut layer = SpriteLayer::default();
        // Called at odometer 1000.7 (truncated to 1000) for 10 m.
        p.event(&signal(4, false, 0, 10, 1000), 1000.7, &mut layer);
        for _ in 0..5 {
            assert_eq!(step(&mut p, &mut layer, 1000.7), -1);
        }
        // The tick counter (0) runs out, but the distance decides.
        assert_eq!(step(&mut p, &mut layer, 1005.0), RED_STARTING);
        assert_eq!(step(&mut p, &mut layer, 1010.0), RED_STARTING);
        assert_eq!(step(&mut p, &mut layer, 1010.01), -1);
        assert_eq!(step(&mut p, &mut layer, 1000.0), -1, "the task has ended");
    }

    #[test]
    fn forced_redisplay_keeps_the_old_sprite_through_the_delay() {
        let mut p = popups();
        let mut layer = SpriteLayer::default();
        p.event(&signal(4, false, 100, 0, 0), 0.0, &mut layer);
        for _ in 0..20 {
            step(&mut p, &mut layer, 0.0);
        }
        // Departure: the real aspect, forced, until 10 m travelled.
        p.event(&signal(0, true, 0, 10, 0), 0.0, &mut layer);
        let seen: Vec<i16> = (0..7).map(|_| step(&mut p, &mut layer, 0.0)).collect();
        assert_eq!(seen[..5], [RED_STARTING; 5]);
        assert_eq!(seen[5..], [GREEN_STARTING; 2]);
        assert_eq!(step(&mut p, &mut layer, 10.5), -1);
    }

    #[test]
    fn limit_signs() {
        let mut p = popups();
        for (kmh, tbl) in [(20, 0x45e), (95, 0x46d), (100, 0x48a), (125, 0x48f)] {
            p.limit_signs.clear();
            p.limit_current = false;
            p.event(
                &RuleEvent::SpeedLimit {
                    kmh,
                    notice_m: 10,
                    offset_m: 0,
                },
                0.0,
                &mut SpriteLayer::default(),
            );
            assert_eq!(p.limit_signs[0].tbl, tbl);
        }
    }

    #[test]
    fn penalty_message_layout() {
        let mut p = popups();
        let mut layer = SpriteLayer::default();
        p.event(
            &RuleEvent::Time {
                reason: TimeReason::Overspeed,
                seconds: -12,
            },
            0.0,
            &mut layer,
        );
        p.update(&mut layer, 0.0, 0, false);
        assert_eq!(layer.slots[SLOT_MESSAGE].tilemap, 0x1b1);
        assert_eq!(layer.slots[SLOT_MESSAGE].x, 320 - 96);
        // Two digits: penalty layout row 1 → word 232, digits at 296 and 328, 秒 at 360.
        assert_eq!(layer.slots[SLOT_MESSAGE_SIGN].x, 232);
        assert_eq!(layer.slots[SLOT_MESSAGE_DIGITS + 1].tilemap, 0x1b3 + 1);
        assert_eq!(layer.slots[SLOT_MESSAGE_DIGITS + 1].x, 296);
        assert_eq!(layer.slots[SLOT_MESSAGE_DIGITS + 2].x, 328);
        assert_eq!(layer.slots[SLOT_MESSAGE_SECONDS].x, 360);
        assert_eq!(layer.slots[SLOT_MESSAGE_SIGN].y, 232);
    }

    #[test]
    fn limit_sign_waits_six_updates_blinks_then_stays_until_notice() {
        let mut p = popups();
        let mut layer = SpriteLayer::default();
        p.event(
            &RuleEvent::SpeedLimit {
                kmh: 45,
                notice_m: 300,
                offset_m: 100,
            },
            1000.0,
            &mut layer,
        );
        for _ in 0..6 {
            p.update(&mut layer, 1000.0, 15, false);
            assert_eq!(layer.slots[SLOT_LIMIT_SIGN].tilemap, -1);
        }
        // (clock / 15) odd: drawn; even: hidden.
        p.update(&mut layer, 1000.0, 15, false);
        assert_eq!(layer.slots[SLOT_LIMIT_SIGN].tilemap, 0x463);
        p.update(&mut layer, 1050.0, 30, false);
        assert_eq!(layer.slots[SLOT_LIMIT_SIGN].tilemap, -1);
        // Past the enforcement point: one update without a draw, then steady.
        p.update(&mut layer, 1100.5, 30, false);
        assert_eq!(layer.slots[SLOT_LIMIT_SIGN].tilemap, -1);
        p.update(&mut layer, 1150.0, 30, false);
        assert_eq!(layer.slots[SLOT_LIMIT_SIGN].tilemap, 0x463);
        p.update(&mut layer, 1300.0, 30, false);
        assert_eq!(layer.slots[SLOT_LIMIT_SIGN].tilemap, 0x463);
        p.update(&mut layer, 1300.5, 30, false);
        assert_eq!(layer.slots[SLOT_LIMIT_SIGN].tilemap, -1);
        assert!(p.limit_signs.is_empty());
    }

    #[test]
    fn whistle_sign_ends_when_the_horn_is_held_and_hides_the_limit() {
        let mut p = popups();
        let mut layer = SpriteLayer::default();
        p.event(
            &RuleEvent::SpeedLimit {
                kmh: 45,
                notice_m: 300,
                offset_m: 0,
            },
            0.0,
            &mut layer,
        );
        p.event(&RuleEvent::HornSign { within_m: 200 }, 0.0, &mut layer);
        for _ in 0..7 {
            p.update(&mut layer, 0.0, 0, false);
        }
        assert_eq!(layer.slots[SLOT_GRADE_SIGN].tilemap, TBL_WHISTLE);
        assert_eq!(layer.slots[SLOT_LIMIT_SIGN].tilemap, -1);
        p.update(&mut layer, 0.0, 0, true);
        assert_eq!(layer.slots[SLOT_GRADE_SIGN].tilemap, -1);
        assert!(p.grade_signs.is_empty());
        p.update(&mut layer, 0.0, 0, false);
        assert_eq!(layer.slots[SLOT_LIMIT_SIGN].tilemap, 0x463);
        // The horn released again does not bring the whistle sign back.
        assert_eq!(layer.slots[SLOT_GRADE_SIGN].tilemap, -1);
    }

    #[test]
    fn gradient_signs_follow_the_grade_force() {
        let mut p = popups();
        let mut layer = SpriteLayer::default();
        p.gradient_step(12.0, 0.0);
        assert_eq!(p.grade_signs.last().unwrap().tbl, 0x45c);
        p.gradient_step(15.0, 0.0);
        assert_eq!(p.grade_signs.len(), 1, "no repeat while the state holds");
        // Level: after 60 steady ticks.
        for _ in 0..59 {
            p.gradient_step(0.0, 0.0);
        }
        assert_eq!(p.grade_signs.last().unwrap().tbl, 0x45c);
        p.gradient_step(0.0, 0.0);
        assert_eq!(p.grade_signs.last().unwrap().tbl, 0x45b);
        // 60 ticks after its 6-update delay, then gone.
        let mut shown = 0;
        for _ in 0..80 {
            p.update(&mut layer, 0.0, 0, false);
            shown += i32::from(layer.slots[SLOT_GRADE_SIGN].tilemap == 0x45b);
        }
        assert_eq!(shown, 61);
    }

    #[test]
    fn ats_lamp_tripped_then_confirmed_for_120_updates() {
        let mut p = popups();
        let mut layer = SpriteLayer::default();
        p.event(&RuleEvent::AtsEmergency, 0.0, &mut layer);
        p.update(&mut layer, 0.0, 0, false);
        assert_eq!(layer.slots[SLOT_ATS].tilemap, TBL_ATS_TRIPPED);
        assert_eq!(
            (layer.slots[SLOT_ATS].x, layer.slots[SLOT_ATS].y),
            (208, 152)
        );
        p.event(&RuleEvent::AtsAcknowledged, 0.0, &mut layer);
        let shown = (0..130)
            .filter(|_| {
                p.update(&mut layer, 0.0, 0, false);
                layer.slots[SLOT_ATS].tilemap == TBL_ATS_CONFIRMED
            })
            .count();
        assert_eq!(shown, 120);
    }

    #[test]
    fn obstacle_popup_after_ten_updates_for_ninety_two() {
        let mut p = popups();
        let mut layer = SpriteLayer::default();
        p.event(&RuleEvent::ObstacleStop, 0.0, &mut layer);
        let seen: Vec<bool> = (0..110)
            .map(|_| {
                p.update(&mut layer, 0.0, 0, false);
                layer.slots[SLOT_POPUP].tilemap == 0x24b
            })
            .collect();
        assert!(!seen[9]);
        assert!(seen[10]);
        assert_eq!(seen.iter().filter(|&&s| s).count(), 92);
        assert_eq!(
            (layer.slots[SLOT_POPUP].x, layer.slots[SLOT_POPUP].y),
            (0x800, 0x800)
        );
    }

    #[test]
    fn time_up_slides_in_then_shows_advice() {
        let mut p = popups();
        let mut layer = SpriteLayer::default();
        p.time_up(&mut layer, 0x3f9);
        p.update(&mut layer, 0.0, 0, false);
        assert_eq!(layer.slots[SLOT_TIME_UP].x, 64 + 502);
        for _ in 0..40 {
            p.update(&mut layer, 0.0, 0, false);
        }
        assert_eq!(layer.slots[SLOT_TIME_UP].x, 64 + 162);
        assert_eq!(layer.slots[SLOT_TIME_UP].y, 204);
        for _ in 0..15 {
            p.update(&mut layer, 0.0, 0, false);
        }
        assert_eq!(layer.slots[SLOT_TIME_UP_ADVICE].tilemap, 0x3f9);
        for _ in 0..100 {
            p.update(&mut layer, 0.0, 0, false);
        }
        assert!(!p.time_up_running());
        assert_eq!(layer.slots[SLOT_TIME_UP].tilemap, -1);
    }
}

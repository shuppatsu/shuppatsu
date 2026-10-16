//! Event-driven HUD elements (docs/re/hud.md §3.9–3.11): the signal aspect sprite, the
//! speed-limit / whistle signs and the bonus/penalty time messages. The rules engine raises
//! [`RuleEvent`]s; these become timed sprite slots.

use anyhow::Result;
use ddg_engine::rules::{RuleEvent, TimeReason};
use ddg_formats::exe::Exe;
use ddg_render::sprites::{Slot, SpriteLayer};

const SLOT_SIGNAL: usize = 0x60;
const SIGNAL_POS: (i16, i16) = (528, 113);
const SLOT_GRADE_SIGN: usize = 0x5e;
const SLOT_LIMIT_SIGN: usize = 0x5f;
const SIGN_POS: (i16, i16) = (528, 265);
const SLOT_MESSAGE: usize = 0x28;
/// 減点 word, three digits and 秒 (slots 0x29..=0x2d).
const SLOT_MESSAGE_SIGN: usize = 0x29;
const SLOT_MESSAGE_DIGITS: usize = 0x2a;
const SLOT_MESSAGE_SECONDS: usize = 0x2d;

/// Signal sprites, built from immediates on the stack in `Signal_Show` (0x411500): indexed
/// `aspect + base`, see [`signal_sprite`]. Verified against the code listing.
const SIGNAL_SPRITES: [u16; 65] = [
    0x451, 0x454, 0x454, 0x454, 0x457, // 3-lamp set 0, home
    0x47d, 0x47a, 0x477, 0x474, 0x471, // 4-lamp home
    0x47f, 0x47a, 0x477, 0x474, 0x471, // 3-lamp set 1, home
    0x452, 0x455, 0x455, 0x455, 0x458, // 3-lamp set 0, starting
    0x47e, 0x47b, 0x478, 0x475, 0x472, // 4-lamp starting
    0x47f, 0x47b, 0x478, 0x475, 0x472, // 3-lamp set 1, starting
    0x450, 0x453, 0x453, 0x453, 0x456, // 3-lamp set 0, block
    0x47c, 0x479, 0x476, 0x473, 0x470, // 4-lamp block
    0x47f, 0x479, 0x476, 0x473, 0x470, // 3-lamp set 1, block
    0x485, 0x484, 0x483, 0x482, 0x480, // repeater set 0
    0x485, 0x481, 0x481, 0x481, 0x480, // repeater set 1
    0x486, 0x484, 0x483, 0x482, 0x480, // repeater set 2
    0x486, 0x481, 0x481, 0x481, 0x480, // repeater set 3
];
/// Per-stage `{i32 three_lamp_set, i32 repeater_set}` (0x466c08, stride 8, read by 0x411500).
const SIGNAL_SETS_VA: u32 = 0x466c08;

/// Sign sprites (hud.md §3.10).
const TBL_LIMIT_END: i16 = 0x46e;
const TBL_WHISTLE: i16 = 0x46f;
const LIMIT_END_TICKS: u32 = 90;
/// Limit signs blink with a 30-tick period until the limit is enforced.
const BLINK_HALF_PERIOD: i32 = 15;

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

#[derive(Clone, Copy, Debug, PartialEq)]
struct LimitSign {
    tbl: i16,
    /// Hidden once the odometer passes this, m.
    until_m: f32,
    /// Blinks until the odometer passes this, m.
    blink_until_m: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Message {
    reason: TimeReason,
    seconds: i32,
    ticks_left: u32,
}

/// The state of the event-driven HUD elements.
pub struct Popups {
    three_lamp_set: i32,
    repeater_set: i32,
    signal: Option<i16>,
    limit: Option<LimitSign>,
    limit_end_ticks: u32,
    whistle_until_m: Option<f32>,
    message: Option<Message>,
}

impl Popups {
    pub fn new(exe: &Exe<'_>, stage: usize) -> Result<Self> {
        let sets = exe.reader_at(SIGNAL_SETS_VA + 8 * stage as u32, 8, "signal sets")?;
        Ok(Self {
            three_lamp_set: sets.i32(0)?,
            repeater_set: sets.i32(4)?,
            signal: None,
            limit: None,
            limit_end_ticks: 0,
            whistle_until_m: None,
            message: None,
        })
    }

    /// Clears everything (segment start, arrival).
    pub fn clear(&mut self) {
        self.signal = None;
        self.limit = None;
        self.limit_end_ticks = 0;
        self.whistle_until_m = None;
        self.message = None;
    }

    /// Applies one rule event. `variant` is the signal display variant (`0x8570c2`).
    pub fn event(&mut self, e: &RuleEvent, odometer_m: f32, variant: i16) {
        match *e {
            RuleEvent::SignalShown { aspect, display } => {
                self.signal = self
                    .signal_sprite(aspect, display, variant)
                    .map(|t| t as i16);
            }
            RuleEvent::SpeedLimit {
                kmh,
                notice_m,
                offset_m,
            } => {
                self.limit = limit_sprite(kmh).map(|tbl| LimitSign {
                    tbl,
                    until_m: odometer_m + notice_m as f32,
                    blink_until_m: odometer_m + offset_m as f32,
                });
                self.limit_end_ticks = 0;
            }
            RuleEvent::SpeedLimitEnd => {
                self.limit = None;
                self.limit_end_ticks = LIMIT_END_TICKS;
            }
            RuleEvent::HornSign { within_m } => {
                self.whistle_until_m = Some(odometer_m + within_m as f32);
            }
            RuleEvent::Time { reason, seconds } => {
                self.message = Some(Message {
                    reason,
                    seconds,
                    ticks_left: MESSAGE_TICKS,
                });
            }
            _ => {}
        }
    }

    fn signal_sprite(&self, aspect: i16, display: i16, variant: i16) -> Option<u16> {
        let a = usize::try_from(aspect & 0xff).ok().filter(|&a| a <= 4)?;
        let set = |s: i32| usize::try_from(s).unwrap_or(0) * 5;
        let base = if variant & 1 == 0 {
            match display {
                0 => 0x23,
                1 => 5,
                2 => 0x14,
                3 => set(self.repeater_set) + 0x2d,
                _ => return None,
            }
        } else {
            let s = set(self.three_lamp_set);
            match display {
                0 => s + 0x1e,
                1 => s,
                2 => s + 0xf,
                3 => set(self.repeater_set) + 0x2d,
                _ => return None,
            }
        };
        SIGNAL_SPRITES.get(a + base).copied()
    }

    /// Sets the slots for this tick and runs the timers.
    pub fn update(
        &mut self,
        layer: &mut SpriteLayer,
        odometer_m: f32,
        clock: i32,
        horn_held: bool,
    ) {
        let show = |layer: &mut SpriteLayer, slot: usize, tbl: Option<i16>, pos: (i16, i16)| {
            layer.slots[slot] = match tbl {
                Some(t) => Slot::new(t, pos.0, pos.1, 4, 2, true),
                None => Slot {
                    keep: true,
                    ..Slot::CLEAR
                },
            };
        };
        show(layer, SLOT_SIGNAL, self.signal, SIGNAL_POS);

        if self.whistle_until_m.is_some_and(|m| odometer_m >= m) {
            self.whistle_until_m = None;
        }
        // The whistle sign hides while the horn is held (0x411d40).
        let whistle = self.whistle_until_m.is_some() && !horn_held;
        show(
            layer,
            SLOT_GRADE_SIGN,
            whistle.then_some(TBL_WHISTLE),
            SIGN_POS,
        );

        if self.limit.is_some_and(|l| odometer_m >= l.until_m) {
            self.limit = None;
        }
        let limit = if self.whistle_until_m.is_some() {
            None // the gradient/whistle sign shares the position
        } else if let Some(l) = self.limit {
            let blinking = odometer_m < l.blink_until_m;
            (!blinking || (clock / BLINK_HALF_PERIOD) % 2 == 0).then_some(l.tbl)
        } else if self.limit_end_ticks > 0 {
            self.limit_end_ticks -= 1;
            Some(TBL_LIMIT_END)
        } else {
            None
        };
        show(layer, SLOT_LIMIT_SIGN, limit, SIGN_POS);

        self.message_slots(layer);
    }

    fn message_slots(&mut self, layer: &mut SpriteLayer) {
        for slot in SLOT_MESSAGE..=SLOT_MESSAGE_SECONDS {
            layer.slots[slot] = Slot {
                keep: true,
                ..Slot::CLEAR
            };
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
}

/// Limit sign for `kmh`: kind `kmh/5 − 4 + 6`, sprites 0x45e.. for 20–95 km/h and 0x48a.. for
/// 100–125 km/h (hud.md §3.10).
fn limit_sprite(kmh: i32) -> Option<i16> {
    let kind = kmh / 5 - 4 + 6;
    match kind {
        6..=21 => Some(0x45e + (kind - 6) as i16),
        22..=27 => Some(0x48a + (kind - 22) as i16),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn popups() -> Popups {
        Popups {
            three_lamp_set: 0,
            repeater_set: 1,
            signal: None,
            limit: None,
            limit_end_ticks: 0,
            whistle_until_m: None,
            message: None,
        }
    }

    #[test]
    fn signal_sprites_follow_the_table() {
        let p = popups();
        // 4-lamp block signal: green 0x47c … red 0x470.
        assert_eq!(p.signal_sprite(0, 0, 0), Some(0x47c));
        assert_eq!(p.signal_sprite(4, 0, 0), Some(0x470));
        // Forced redisplay bit is ignored for the lookup.
        assert_eq!(p.signal_sprite(0x100, 2, 0), Some(0x47e));
        // 3-lamp set 0 home, repeater set 1.
        assert_eq!(p.signal_sprite(2, 1, 1), Some(0x454));
        assert_eq!(p.signal_sprite(1, 3, 0), Some(0x481));
        assert_eq!(p.signal_sprite(5, 0, 0), None);
    }

    #[test]
    fn limit_signs() {
        assert_eq!(limit_sprite(20), Some(0x45e));
        assert_eq!(limit_sprite(95), Some(0x46d));
        assert_eq!(limit_sprite(100), Some(0x48a));
        assert_eq!(limit_sprite(125), Some(0x48f));
        assert_eq!(limit_sprite(130), None);
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
            0,
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
    fn limit_sign_blinks_until_enforced_then_stays_until_notice() {
        let mut p = popups();
        let mut layer = SpriteLayer::default();
        p.event(
            &RuleEvent::SpeedLimit {
                kmh: 45,
                notice_m: 300,
                offset_m: 100,
            },
            1000.0,
            0,
        );
        p.update(&mut layer, 1000.0, 15, false);
        assert_eq!(layer.slots[SLOT_LIMIT_SIGN].tilemap, -1); // blink-off half
        p.update(&mut layer, 1050.0, 30, false);
        assert_eq!(layer.slots[SLOT_LIMIT_SIGN].tilemap, 0x463);
        p.update(&mut layer, 1150.0, 15, false);
        assert_eq!(layer.slots[SLOT_LIMIT_SIGN].tilemap, 0x463); // enforced: steady
        p.update(&mut layer, 1300.0, 15, false);
        assert_eq!(layer.slots[SLOT_LIMIT_SIGN].tilemap, -1);
    }
}

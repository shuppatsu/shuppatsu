//! Event-driven HUD elements (docs/re/hud.md §3.9–3.11): the signal aspect sprite, the
//! speed-limit / whistle signs and the bonus/penalty time messages. The rules engine raises
//! [`RuleEvent`]s; these become timed sprite slots.

use anyhow::Result;
use ddg_engine::rules::{RuleEvent, SignalHud, TimeReason, signal_sprite};
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
    /// Stage's `[three_lamp_set, repeater_set]` (0x466c08).
    signal_sets: [i32; 2],
    /// Slot 0x60: the signal display tasks of `Signal_Show` 0x411500 (hud.md §3.9).
    signal: SignalHud,
    limit: Option<LimitSign>,
    limit_end_ticks: u32,
    whistle_until_m: Option<f32>,
    message: Option<Message>,
}

impl Popups {
    pub fn new(exe: &Exe<'_>, stage: usize) -> Result<Self> {
        let sets = exe.reader_at(SIGNAL_SETS_VA + 8 * stage as u32, 8, "signal sets")?;
        Ok(Self {
            signal_sets: [sets.i32(0)?, sets.i32(4)?],
            signal: SignalHud::default(),
            limit: None,
            limit_end_ticks: 0,
            whistle_until_m: None,
            message: None,
        })
    }

    /// Clears everything (segment start, arrival).
    pub fn clear(&mut self) {
        self.signal.clear();
        self.limit = None;
        self.limit_end_ticks = 0;
        self.whistle_until_m = None;
        self.message = None;
    }

    /// Applies one rule event, before [`Popups::update`] of the same tick (the original's rule
    /// calls precede its HUD tasks). `_variant` is unused: [`RuleEvent::SignalShown`] carries
    /// the display variant in effect for the call (the departure task forces 0).
    pub fn event(&mut self, e: &RuleEvent, odometer_m: f32, _variant: i16) {
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
        // The signal tasks (0x411940) update after this tick's rule calls.
        self.signal.tick(odometer_m);
        let signal = self.signal.slot().map(|t| t as i16);
        show(layer, SLOT_SIGNAL, signal, SIGNAL_POS);

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
            signal_sets: [0, 1],
            signal: SignalHud::default(),
            limit: None,
            limit_end_ticks: 0,
            whistle_until_m: None,
            message: None,
        }
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
        p.event(&signal(4, false, 100, 0, 0), 0.0, 0);
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
        p.event(&signal(4, false, 0, 10, 1000), 1000.7, 0);
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
    fn showing_the_same_sprite_again_does_nothing() {
        let mut p = popups();
        let mut layer = SpriteLayer::default();
        p.event(&signal(4, false, 10, 0, 0), 0.0, 0);
        for _ in 0..8 {
            step(&mut p, &mut layer, 0.0);
        }
        // Same sprite: no new delay, no new duration.
        p.event(&signal(4, false, 100, 0, 0), 0.0, 0);
        let seen: Vec<i16> = (0..10).map(|_| step(&mut p, &mut layer, 0.0)).collect();
        assert_eq!(seen[..8], [RED_STARTING; 8]);
        assert_eq!(seen[8..], [-1, -1]);
        // Once it has ended the same sprite shows again (after the delay).
        p.event(&signal(4, false, 100, 0, 0), 0.0, 0);
        let seen: Vec<i16> = (0..6).map(|_| step(&mut p, &mut layer, 0.0)).collect();
        assert_eq!(seen, [-1, -1, -1, -1, -1, RED_STARTING]);
    }

    #[test]
    fn forced_redisplay_keeps_the_old_sprite_through_the_delay() {
        let mut p = popups();
        let mut layer = SpriteLayer::default();
        p.event(&signal(4, false, 100, 0, 0), 0.0, 0);
        for _ in 0..20 {
            step(&mut p, &mut layer, 0.0);
        }
        // Departure: the real aspect, forced, until 10 m travelled.
        p.event(&signal(0, true, 0, 10, 0), 0.0, 0);
        let seen: Vec<i16> = (0..7).map(|_| step(&mut p, &mut layer, 0.0)).collect();
        assert_eq!(
            seen,
            [
                RED_STARTING,
                RED_STARTING,
                RED_STARTING,
                RED_STARTING,
                RED_STARTING,
                GREEN_STARTING,
                GREEN_STARTING
            ]
        );
        assert_eq!(step(&mut p, &mut layer, 10.5), -1);
    }

    #[test]
    fn unforced_replacement_leaves_a_gap() {
        let mut p = popups();
        let mut layer = SpriteLayer::default();
        p.event(&signal(4, false, 100, 0, 0), 0.0, 0);
        for _ in 0..20 {
            step(&mut p, &mut layer, 0.0);
        }
        p.event(&signal(0, false, 100, 0, 0), 0.0, 0);
        let seen: Vec<i16> = (0..6).map(|_| step(&mut p, &mut layer, 0.0)).collect();
        assert_eq!(seen, [-1, -1, -1, -1, -1, GREEN_STARTING]);
    }

    #[test]
    fn hiding_ends_the_signal_at_the_next_update() {
        let mut p = popups();
        let mut layer = SpriteLayer::default();
        p.event(&signal(4, false, 100, 0, 0), 0.0, 0);
        for _ in 0..10 {
            step(&mut p, &mut layer, 0.0);
        }
        p.event(&RuleEvent::SignalHidden, 0.0, 0);
        assert_eq!(step(&mut p, &mut layer, 0.0), -1);
        assert_eq!(step(&mut p, &mut layer, 0.0), -1);
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

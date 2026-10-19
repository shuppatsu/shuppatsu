//! Navigator hints (docs/re/hud.md §3.12): `Hint_Show(id, ticks, distance)` 0x411f20 → task
//! 0x4121d0, portrait in slot 0x34 and speech balloon in slot 0x35; and the handling-error
//! hint of the train task 0x422c20 (slots 0x2e / 0x2f).
//!
//! A new hint keeps the previous one on screen for 5 more updates (its distance is dropped and
//! its tick count set to 5) while the new one waits its own 5 updates, so the handover is
//! seamless. The rules decide when a hint is shown ([`RuleEvent::HintShown`]); they already
//! leave out calls made while the ATS awaits confirmation or brakes.
//!
//! [`RuleEvent::HintShown`]: shuppatsu_engine::rules::RuleEvent::HintShown

use crate::slots;
use shuppatsu_render::sprites::SpriteLayer;

const SLOT_PORTRAIT: usize = 0x34;
const SLOT_BALLOON: usize = 0x35;
/// The handling-error hint's own slots, same positions (0x422c20).
const SLOT_ERROR_PORTRAIT: usize = 0x2e;
const SLOT_ERROR_BALLOON: usize = 0x2f;
/// Updates before the first draw (`+0x38 = 5`, 0x411f20).
const DELAY: i32 = 5;
/// A replaced hint stays this many more updates (`old+0x34 = 5`, 0x411f20).
const HANDOVER: i32 = 5;
/// Balloons: `0x135 + id` (stack table in 0x411f20, ids 0..=0x21). Id 0x0b's balloon 0x140
/// ("the horn is the foot pedal") becomes 0x508 (Space key) or, with a joystick in modes 5–6,
/// 0x509 (B button) (`FUN_00433270`).
const BALLOON_BASE: i16 = 0x135;
const BALLOON_COUNT: i16 = 0x22;
const BALLOON_PEDAL: i16 = 0x140;
const BALLOON_HORN_KEY: i16 = 0x508;
const BALLOON_HORN_BUTTON: i16 = 0x509;
/// Portraits: 0x159 for id 0x12 (dead section), 0x15a for ids 0x1d / 0x1e (pass / stop
/// advice), else 0x157 on stages of rank < 2 (beginner badge) or 0x158.
const PORTRAIT_DEAD_SECTION: i16 = 0x159;
const PORTRAIT_ADVICE: i16 = 0x15a;
const PORTRAIT_BEGINNER: i16 = 0x157;
const PORTRAIT_REGULAR: i16 = 0x158;
/// Handling error (0x422c20): power and brake both applied for 11 driving ticks shows balloon
/// 0x13f with portrait 0x157 until one is released.
const ERROR_AFTER_TICKS: i32 = 0xb;
const ERROR_BALLOON: i16 = 0x13f;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Task {
    delay: i32,
    ticks: i32,
    /// `+0x48`: odometer (truncated) + distance, or none.
    until_m: Option<i32>,
    enabled: bool,
    portrait: i16,
    balloon: i16,
}

/// The hint tasks.
#[derive(Debug, Clone, PartialEq)]
pub struct Hints {
    /// In creation order; the newest is `0x856d44` while it lives.
    tasks: Vec<Task>,
    current: bool,
    /// The horn hint names the B button (joystick modes 5–6) instead of the Space key.
    pub horn_on_button: bool,
    /// Ticks with power and brake both applied (`0x84c428`).
    error_ticks: i32,
}

impl Hints {
    pub fn new(horn_on_button: bool) -> Self {
        Self {
            tasks: Vec::new(),
            current: false,
            horn_on_button,
            error_ticks: 0,
        }
    }

    /// A hint is on screen or waiting to be (`0x856d44 ≠ 0`); hides the high-speed readouts.
    pub fn up(&self) -> bool {
        self.current
    }

    /// `Hint_Show(id, ticks, distance)` with the odometer at the call. `stage_rank` picks the
    /// portrait.
    pub fn show(&mut self, id: i16, ticks: i32, distance_m: i32, start_m: i32, stage_rank: i32) {
        if !(0..BALLOON_COUNT).contains(&id) {
            return;
        }
        if self.current
            && let Some(old) = self.tasks.last_mut()
        {
            old.until_m = None;
            old.ticks = HANDOVER;
        }
        let portrait = match id {
            0x12 => PORTRAIT_DEAD_SECTION,
            0x1d | 0x1e => PORTRAIT_ADVICE,
            _ if stage_rank < 2 => PORTRAIT_BEGINNER,
            _ => PORTRAIT_REGULAR,
        };
        let mut balloon = BALLOON_BASE + id;
        if balloon == BALLOON_PEDAL {
            balloon = if self.horn_on_button {
                BALLOON_HORN_BUTTON
            } else {
                BALLOON_HORN_KEY
            };
        }
        self.tasks.push(Task {
            delay: DELAY,
            ticks,
            until_m: (distance_m != 0).then_some(start_m + distance_m),
            enabled: true,
            portrait,
            balloon,
        });
        self.current = true;
    }

    /// `Hint_Show(-1)`: the current hint ends at its next update.
    pub fn hide(&mut self) {
        if self.current
            && let Some(t) = self.tasks.last_mut()
        {
            t.enabled = false;
        }
    }

    /// Ends every hint at once (segment start: the task list is rebuilt).
    pub fn clear(&mut self, layer: &mut SpriteLayer) {
        if !self.tasks.is_empty() {
            slots::image(layer, SLOT_PORTRAIT, -1);
            slots::image(layer, SLOT_BALLOON, -1);
        }
        self.tasks.clear();
        self.current = false;
    }

    /// The task updates (0x4121d0), in creation order.
    pub fn update(&mut self, layer: &mut SpriteLayer, odometer: f32) {
        let n = self.tasks.len();
        let mut i = 0;
        let mut index = 0;
        while i < self.tasks.len() {
            let newest = index + 1 == n;
            index += 1;
            let t = &mut self.tasks[i];
            if t.delay > 0 {
                t.delay -= 1;
                i += 1;
                continue;
            }
            t.ticks -= 1;
            slots::image(layer, SLOT_PORTRAIT, t.portrait);
            slots::image(layer, SLOT_BALLOON, t.balloon);
            let alive = match t.until_m {
                None => t.ticks >= 0,
                Some(m) => m as f32 - odometer >= 0.0,
            } && t.enabled;
            if alive {
                i += 1;
                continue;
            }
            // Destructor 0x412250: both slots hidden; the pointer cleared if it was this one.
            self.tasks.remove(i);
            slots::image(layer, SLOT_PORTRAIT, -1);
            slots::image(layer, SLOT_BALLOON, -1);
            if newest && self.current {
                self.current = false;
            }
        }
    }

    /// The handling-error hint of the train task (0x422c20), with the notches the task saw on
    /// the previous tick. `driving`: the train task's driving branch ran.
    pub fn handling_error(
        &mut self,
        layer: &mut SpriteLayer,
        driving: bool,
        power: i32,
        brake: i32,
    ) {
        if !driving || power < 1 || brake < 1 {
            self.error_ticks = 0;
            slots::image(layer, SLOT_ERROR_PORTRAIT, -1);
            slots::image(layer, SLOT_ERROR_BALLOON, -1);
            return;
        }
        self.error_ticks += 1;
        if self.error_ticks >= ERROR_AFTER_TICKS {
            slots::image(layer, SLOT_ERROR_PORTRAIT, PORTRAIT_BEGINNER);
            slots::image(layer, SLOT_ERROR_BALLOON, ERROR_BALLOON);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn balloon(layer: &SpriteLayer) -> i16 {
        layer.slots[SLOT_BALLOON].tilemap
    }

    #[test]
    fn hint_waits_five_then_shows_ticks_updates() {
        let mut h = Hints::new(false);
        let mut layer = SpriteLayer::default();
        h.show(3, 4, 0, 0, 2);
        let seen: Vec<i16> = (0..12)
            .map(|_| {
                h.update(&mut layer, 0.0);
                balloon(&layer)
            })
            .collect();
        assert_eq!(seen[..5], [-1; 5]);
        assert_eq!(seen[5..9], [0x138; 4]);
        assert_eq!(seen[9], -1);
        assert!(!h.up());
        assert_eq!(layer.slots[SLOT_PORTRAIT].tilemap, -1);
    }

    #[test]
    fn distance_hint_ends_past_its_target() {
        let mut h = Hints::new(false);
        let mut layer = SpriteLayer::default();
        h.show(0x1e, 0, 120, 1000, 2);
        for _ in 0..6 {
            h.update(&mut layer, 1000.0);
        }
        assert_eq!(layer.slots[SLOT_PORTRAIT].tilemap, PORTRAIT_ADVICE);
        h.update(&mut layer, 1120.0);
        assert_eq!(balloon(&layer), 0x135 + 0x1e);
        h.update(&mut layer, 1120.5);
        assert_eq!(balloon(&layer), -1);
    }

    #[test]
    fn replacement_hands_over_after_five_updates() {
        let mut h = Hints::new(false);
        let mut layer = SpriteLayer::default();
        h.show(1, 100, 0, 0, 2);
        for _ in 0..10 {
            h.update(&mut layer, 0.0);
        }
        h.show(2, 100, 0, 0, 2);
        let seen: Vec<i16> = (0..7)
            .map(|_| {
                h.update(&mut layer, 0.0);
                balloon(&layer)
            })
            .collect();
        // The old one draws with 4..0 left, then ends as the new one first draws.
        assert_eq!(seen[..5], [0x136; 5]);
        assert_eq!(seen[5..], [0x137; 2]);
        assert!(h.up());
    }

    #[test]
    fn hide_ends_the_hint_after_one_more_draw() {
        let mut h = Hints::new(false);
        let mut layer = SpriteLayer::default();
        h.show(1, 100, 0, 0, 2);
        for _ in 0..7 {
            h.update(&mut layer, 0.0);
        }
        h.hide();
        h.update(&mut layer, 0.0);
        assert_eq!(balloon(&layer), -1);
        assert!(!h.up());
    }

    #[test]
    fn horn_hint_names_the_control() {
        let mut layer = SpriteLayer::default();
        for (button, want) in [(false, 0x508), (true, 0x509)] {
            let mut h = Hints::new(button);
            h.show(0xb, 1, 0, 0, 0);
            for _ in 0..6 {
                h.update(&mut layer, 0.0);
            }
            assert_eq!(balloon(&layer), want);
            assert_eq!(layer.slots[SLOT_PORTRAIT].tilemap, PORTRAIT_BEGINNER);
        }
    }

    #[test]
    fn handling_error_after_eleven_ticks() {
        let mut h = Hints::new(false);
        let mut layer = SpriteLayer::default();
        for _ in 0..10 {
            h.handling_error(&mut layer, true, 1, 1);
        }
        assert_eq!(layer.slots[SLOT_ERROR_BALLOON].tilemap, -1);
        h.handling_error(&mut layer, true, 1, 1);
        assert_eq!(layer.slots[SLOT_ERROR_BALLOON].tilemap, ERROR_BALLOON);
        h.handling_error(&mut layer, true, 0, 1);
        assert_eq!(layer.slots[SLOT_ERROR_BALLOON].tilemap, -1);
    }
}

//! Setting `hybrid_input` (docs/re/input.md §10.6): the keyboard and a controller live at the
//! same time, whichever control mode the options select, like a modern PC game.
//!
//! The pieces here are pure and tested on their own; [`crate::gamepad::Controls`] puts them
//! together once per tick:
//!
//! - [`ModernPad`]: the Xbox-layout mapping of a gamepad onto the button word and the four
//!   lever directions (keyboard modes and mode 4).
//! - [`MenuRepeat`]: the menu cursor pulses of the merged word, one counter per direction.
//! - [`LeverArbiter`]: last device used wins between stepped input and a physical handle
//!   (modes 5/6 and the learned combination controllers).
//! - [`HintTracker`]: which device the on-screen hints show, from the first input of the other
//!   device, with deadbands against stick noise.

use crate::gamepad::GenericInputs;
use crate::input::{LeverHolds, bits, repeat_pulse};

/// Stick travel (gilrs units, ±1) that counts as a direction: the same half travel as mode 4's
/// ±500 of ±1000 (0x43425f…).
pub const STICK_STEP: f32 = 0.5;
/// Stick travel below which a held stick direction lets go again: the hysteresis under
/// [`STICK_STEP`], so a stick resting near half travel never chatters into extra steps.
pub const STICK_RELEASE: f32 = 0.35;
/// Analog trigger travel (0 at rest, 1 fully pulled) that presses LT / RT ...
pub const TRIGGER_PRESS: f32 = 0.5;
/// ... and the travel below which it lets go again.
pub const TRIGGER_RELEASE: f32 = 0.35;
/// The left stick's direction: `true` = pulling it toward you (down) is more power, matching the
/// d-pad (↓ = power +, the original's keyboard mode 0) and the arcade one-handle lever pulled
/// back. `false` flips it (the d-pad stays as it is).
pub const STICK_PULL_IS_MORE: bool = true;
/// The right stick's direction: `false` = pushing it away from you (up) applies the brake and
/// pulling it back releases it (the user's preference after testing a real pad). `true` flips it.
pub const BRAKE_PULL_IS_MORE: bool = false;
/// Analog handle travel (axis units of ±1000) that hands the levers back to the handle after a
/// key or button step took them over: 5 % of the range, well above the jitter of a resting
/// pot and well below the 40–250 units between two calibrated notches (0x48c434).
pub const HANDLE_DEADBAND: i32 = 50;
/// Axis travel (gilrs units) that makes the pad the hint device: a quarter of the range from
/// where the axis last stood, so a stick resting off centre or drifting never flips the hints.
pub const HINT_AXIS_DEADBAND: f32 = 0.25;

/// One direction with hysteresis: on at `press`, off again below `release`.
fn latch(on: &mut bool, v: f32, press: f32, release: f32) -> bool {
    *on = if *on { v >= release } else { v >= press };
    *on
}

/// The Xbox-layout mapping of a gamepad onto the button word and the four lever directions
/// (hybrid only; keyboard modes and mode 4). It keeps the stick and trigger latches from tick
/// to tick; [`ModernPad::reset`] on unplug.
///
/// | Pad | Driving | Menus |
/// |---|---|---|
/// | D-pad ↓ ↑ | power + / − | cursor down / up |
/// | D-pad → ← | brake + / − (past B8 = EB) | right / left |
/// | Left stick ↓ ↑ | power + / − | cursor down / up |
/// | Left stick → ← | — | right / left |
/// | Right stick ↑ ↓ | brake + / − (past B8 = EB) | — |
/// | LT / LB | power + / − | — |
/// | RT / RB | brake + / − | — |
/// | A | the A button (Pro-mode panel, ATS confirm) | confirm |
/// | B | — | cancel / back |
/// | View | issue mark (docs/marks.md) | issue mark |
/// | X | horn | — |
/// | Y | R (route select: automatic run) | — |
/// | Menu | pause | pause |
///
/// The stick rows are for [`STICK_PULL_IS_MORE`] = `true` and [`BRAKE_PULL_IS_MORE`] =
/// `false`; the shoulders are the user's choice after testing a real pad (left side power,
/// right side brake). Each lever direction is one held virtual key whichever input holds it,
/// so the d-pad and a stick (or a trigger) pressed together step once and repeat on one
/// counter (`Keyboard::poll_with`).
///
/// The menu cursor pulses are not set here ([`MenuRepeat`] makes them from the merged word),
/// and Y's R bit is a press-frame bit like the keyboard's, made by the caller from the edge.
#[derive(Clone, Debug, Default)]
pub struct ModernPad {
    /// Left stick ↑ ↓ ← →, right stick ↑ ↓, LT, RT.
    on: [bool; 8],
}

impl ModernPad {
    /// Unplugged: every latch lets go.
    pub fn reset(&mut self) {
        self.on = [false; 8];
    }

    /// The held word and lever directions of this tick.
    pub fn map(&mut self, i: &GenericInputs) -> (u32, LeverHolds) {
        let o = &mut self.on;
        let stick = |on: &mut bool, v: f32| latch(on, v, STICK_STEP, STICK_RELEASE);
        let l_up = stick(&mut o[0], i.left_y);
        let l_down = stick(&mut o[1], -i.left_y);
        let l_left = stick(&mut o[2], -i.left_x);
        let l_right = stick(&mut o[3], i.left_x);
        let r_up = stick(&mut o[4], i.right_y);
        let r_down = stick(&mut o[5], -i.right_y);
        let lt = latch(&mut o[6], i.lt, TRIGGER_PRESS, TRIGGER_RELEASE);
        let rt = latch(&mut o[7], i.rt, TRIGGER_PRESS, TRIGGER_RELEASE);
        // The d-pad wins over the left stick on its axis, as in `generic_state`.
        let dpad_v = i.dpad_up || i.dpad_down;
        let (up, down) = if dpad_v {
            (i.dpad_up && !i.dpad_down, i.dpad_down && !i.dpad_up)
        } else {
            (l_up && !l_down, l_down && !l_up)
        };
        let (left, right) = if i.dpad_left || i.dpad_right {
            (i.dpad_left && !i.dpad_right, i.dpad_right && !i.dpad_left)
        } else {
            (l_left && !l_right, l_right && !l_left)
        };
        let mut w = 0;
        for (on, bit) in [
            (up, bits::UP),
            (down, bits::DOWN),
            (left, bits::LEFT),
            (right, bits::RIGHT),
            (i.south, bits::A | bits::MENU_CONFIRM),
            (i.east, bits::CANCEL),
            (i.west, bits::HORN),
            (i.start, bits::PAUSE),
        ] {
            if on {
                w |= bit;
            }
        }
        // Power: the vertical of the d-pad (↓ = power +) or, when the d-pad's vertical is
        // free, the left stick. Brake: the d-pad's horizontal or the right stick; the left
        // stick's ← → are menu directions only.
        let (power_more, power_less) = if dpad_v || STICK_PULL_IS_MORE {
            (down, up)
        } else {
            (up, down)
        };
        let (r_more, r_less) = if BRAKE_PULL_IS_MORE {
            (r_down && !r_up, r_up && !r_down)
        } else {
            (r_up && !r_down, r_down && !r_up)
        };
        let levers = LeverHolds {
            power_up: power_more || lt,
            power_down: power_less || i.lb,
            brake_up: (i.dpad_right && !i.dpad_left) || r_more || rt,
            brake_down: (i.dpad_left && !i.dpad_right) || r_less || i.rb,
        };
        (w, levers)
    }
}

/// The menu cursor of the merged word: each direction is held while either device holds it,
/// with one hold counter, so the pulses (press, then frames 9, 12, 15, …; input.md §4.2) come
/// once however many devices press it.
#[derive(Clone, Debug, Default)]
pub struct MenuRepeat {
    count: [u16; 4],
}

const DIRS: [(u32, u32); 4] = [
    (bits::UP, bits::UP_PULSE),
    (bits::DOWN, bits::DOWN_PULSE),
    (bits::LEFT, bits::LEFT_PULSE),
    (bits::RIGHT, bits::RIGHT_PULSE),
];
const PULSES: u32 = bits::UP_PULSE | bits::DOWN_PULSE | bits::LEFT_PULSE | bits::RIGHT_PULSE;

impl MenuRepeat {
    /// The OR of the devices' held words with the pulses made again from the merged holds.
    pub fn merge(&mut self, words: &[u32]) -> u32 {
        let held = words.iter().fold(0, |a, w| a | w) & !PULSES;
        let mut out = held;
        for (count, (dir, pulse)) in self.count.iter_mut().zip(DIRS) {
            *count = if held & dir != 0 {
                count.saturating_add(1)
            } else {
                0
            };
            if repeat_pulse(*count) {
                out |= pulse;
            }
        }
        out
    }
}

/// Who sets a notch handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// The physical handle: the notch follows it (with mode 6's gates and EB debounce, which
    /// the joystick decoder applies before the arbiter sees its notch).
    Handle,
    /// A key or button step took the handle over; the notch steps from where it was.
    Steps,
}

/// Last device used wins between stepped input (keyboard, pad buttons) and a physical handle
/// (setting `hybrid_input`, modes 5/6). Index 0 is the power handle, 1 the brake handle.
///
/// Each tick: a step that changed a notch gives that handle to the steps and remembers the
/// handle's reading (its raw axis, or the learned notch of a combination controller) as the
/// anchor; while the steps own it, the handle takes it back, snapping the notch to its own
/// position, once its reading is more than the deadband from the anchor.
#[derive(Clone, Debug)]
pub struct LeverArbiter {
    pub owner: [Owner; 2],
    anchor: [Option<i32>; 2],
    /// Mode 5: one axis is both handles, so a step on either hands both over.
    linked: bool,
}

/// One handle's view for a tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HandleTick {
    /// The notch before the steps ran (the shared notch of the last tick).
    pub before: u8,
    /// The notch after the keyboard / pad steps.
    pub stepped: u8,
    /// The notch the handle decodes this tick.
    pub handle: u8,
    /// The handle's raw reading; `None` when it has none (a combination pad between
    /// patterns, or no pad this tick): it then never takes over.
    pub reading: Option<i32>,
}

impl LeverArbiter {
    pub fn new(linked: bool) -> Self {
        Self {
            owner: [Owner::Handle; 2],
            anchor: [None; 2],
            linked,
        }
    }

    /// Station start, or the handle reconnecting: the handle is authoritative again.
    pub fn hand_back(&mut self) {
        self.owner = [Owner::Handle; 2];
    }

    /// The notches for this tick; `deadband` is in the reading's units (0 for learned notches:
    /// any change).
    pub fn tick(&mut self, h: [HandleTick; 2], deadband: i32) -> [u8; 2] {
        let stepped = [0, 1].map(|i| h[i].stepped != h[i].before);
        let take = if self.linked {
            [stepped[0] || stepped[1]; 2]
        } else {
            stepped
        };
        let mut out = [0; 2];
        for i in 0..2 {
            if take[i] {
                if self.owner[i] == Owner::Handle || stepped[i] {
                    // The anchor is where the handle stood when the steps took over.
                    if self.owner[i] == Owner::Handle {
                        self.anchor[i] = h[i].reading;
                    }
                    self.owner[i] = Owner::Steps;
                }
                out[i] = h[i].stepped;
                continue;
            }
            match self.owner[i] {
                Owner::Handle => out[i] = h[i].handle,
                Owner::Steps => {
                    let moved = match (h[i].reading, self.anchor[i]) {
                        (Some(r), Some(a)) => (r - a).abs() > deadband,
                        // No reading when the steps took over: the first one counts as moved.
                        (Some(_), None) => true,
                        (None, _) => false,
                    };
                    if moved {
                        self.owner[i] = Owner::Handle;
                        out[i] = h[i].handle;
                    } else {
                        out[i] = h[i].stepped;
                    }
                }
            }
        }
        if self.linked && self.owner[0] != self.owner[1] {
            // One axis: when it moves, both notches are the handle's again.
            self.owner = [Owner::Handle; 2];
            out = [h[0].handle, h[1].handle];
        }
        out
    }
}

/// The device the on-screen hints show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HintDevice {
    Keyboard,
    Pad,
}

impl HintDevice {
    /// `SHUPPATSU_HINT_DEVICE` / `--hint-device`: `pad` or `keyboard` (headless shots have no pad).
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "pad" | "gamepad" | "controller" => Some(Self::Pad),
            "keyboard" | "keys" => Some(Self::Keyboard),
            _ => None,
        }
    }
}

/// Follows the last device used. A key press switches to the keyboard; a pad button press,
/// or an axis moved more than [`HINT_AXIS_DEADBAND`] from where it last stood, switches to the
/// pad. Axis noise and a drifting stick never switch.
#[derive(Clone, Debug)]
pub struct HintTracker {
    pub device: HintDevice,
    prev_buttons: u32,
    /// Where each axis (left x, left y, then any device axes) stood at the last switch or
    /// press.
    anchor: Vec<f32>,
}

impl HintTracker {
    pub fn new(device: HintDevice) -> Self {
        Self {
            device,
            prev_buttons: 0,
            anchor: Vec::new(),
        }
    }

    /// One tick: `key_pressed` when any key went down; `pad` the pad's buttons (one bit each)
    /// and axes, `None` without a pad (disconnected: back to the keyboard).
    pub fn update(&mut self, key_pressed: bool, pad: Option<(u32, &[f32])>) -> HintDevice {
        let Some((buttons, axes)) = pad else {
            self.prev_buttons = 0;
            self.anchor.clear();
            self.device = HintDevice::Keyboard;
            return self.device;
        };
        let pressed = buttons & !self.prev_buttons != 0;
        self.prev_buttons = buttons;
        if self.anchor.len() != axes.len() {
            // A new pad: its resting axes are the anchors.
            self.anchor = axes.to_vec();
        }
        let moved = axes
            .iter()
            .zip(&self.anchor)
            .any(|(v, a)| (v - a).abs() > HINT_AXIS_DEADBAND);
        if moved {
            self.anchor = axes.to_vec();
        }
        if key_pressed {
            self.device = HintDevice::Keyboard;
        } else if pressed || moved {
            self.device = HintDevice::Pad;
        }
        self.device
    }
}

/// The pad's axes for [`HintTracker::update`]: both sticks and the trigger travels.
pub fn pad_axes(i: &GenericInputs) -> [f32; 6] {
    [i.left_x, i.left_y, i.right_x, i.right_y, i.lt, i.rt]
}

/// The pad's buttons as one bit each, for [`HintTracker::update`].
pub fn pad_buttons(i: &GenericInputs) -> u32 {
    [
        i.south,
        i.east,
        i.west,
        i.north,
        i.lb,
        i.rb,
        i.start,
        i.select,
        i.dpad_up,
        i.dpad_down,
        i.dpad_left,
        i.dpad_right,
    ]
    .iter()
    .enumerate()
    .fold(0, |m, (n, &on)| if on { m | 1 << n } else { m })
}

#[cfg(test)]
pub mod tests {
    use super::*;

    fn pad(f: impl FnOnce(&mut GenericInputs)) -> GenericInputs {
        let mut i = GenericInputs::default();
        f(&mut i);
        i
    }

    /// The stick Y of "pulled toward you" (gilrs +Y is up), per [`STICK_PULL_IS_MORE`].
    pub const PULL: f32 = if STICK_PULL_IS_MORE { -1.0 } else { 1.0 };
    /// The right stick Y that applies the brake, per [`BRAKE_PULL_IS_MORE`].
    pub const BRAKE_PULL: f32 = if BRAKE_PULL_IS_MORE { -1.0 } else { 1.0 };

    /// One tick of a fresh mapping.
    fn modern_pad(i: &GenericInputs) -> (u32, LeverHolds) {
        ModernPad::default().map(i)
    }

    #[test]
    fn xbox_mapping() {
        let (w, l) = modern_pad(&pad(|i| i.south = true));
        assert_eq!(w, bits::A | bits::MENU_CONFIRM);
        assert_eq!(l, LeverHolds::NONE);
        assert_eq!(modern_pad(&pad(|i| i.east = true)).0, bits::CANCEL);
        assert_eq!(modern_pad(&pad(|i| i.west = true)).0, bits::HORN);
        assert_eq!(modern_pad(&pad(|i| i.start = true)).0, bits::PAUSE);
        // Y is the R key, made from the edge by the caller; View is the issue-mark button only.
        assert_eq!(modern_pad(&pad(|i| i.north = true)).0, 0);
        assert_eq!(modern_pad(&pad(|i| i.select = true)).0, 0);
        // D-pad down is power+ and the menu cursor down; → is brake+ and the cursor right.
        let (w, l) = modern_pad(&pad(|i| i.dpad_down = true));
        assert_eq!(w, bits::DOWN);
        assert!(l.power_up && !l.power_down && !l.brake_up);
        let (w, l) = modern_pad(&pad(|i| i.dpad_right = true));
        assert_eq!(w, bits::RIGHT);
        assert!(l.brake_up && !l.power_up && !l.brake_down);
        // Shoulders: LT/LB power, RT/RB brake; not menu directions.
        let (w, l) = modern_pad(&pad(|i| {
            i.lt = 1.0;
            i.rb = true;
        }));
        assert_eq!(w, 0);
        assert!(l.power_up && l.brake_down && !l.power_down && !l.brake_up);
        let (w, l) = modern_pad(&pad(|i| {
            i.rt = 1.0;
            i.lb = true;
        }));
        assert_eq!(w, 0);
        assert!(l.brake_up && l.power_down && !l.power_up && !l.brake_down);
        // The left stick past half travel (gilrs +Y is up): ↑ = power − and cursor up.
        let (w, l) = modern_pad(&pad(|i| i.left_y = 0.6));
        assert_eq!(w, bits::UP);
        assert!(l.power_down && !l.power_up);
        assert_eq!(modern_pad(&pad(|i| i.left_y = 0.49)).0, 0);
        // The d-pad wins on its axis; the stick's ← stays a menu direction.
        let (w, l) = modern_pad(&pad(|i| {
            i.left_y = 0.9;
            i.dpad_down = true;
            i.left_x = -0.7;
        }));
        assert_eq!(w, bits::DOWN | bits::LEFT);
        assert!(l.power_up && !l.power_down && !l.brake_down && !l.brake_up);
    }

    #[test]
    fn twin_sticks_step_power_and_brake() {
        // Left stick pulled (↓ by default): power +, ↑ power −, ← → never the brake (menu
        // directions only).
        let (w, l) = modern_pad(&pad(|i| i.left_y = 0.8 * PULL));
        assert_eq!(w, if PULL < 0.0 { bits::DOWN } else { bits::UP });
        assert!(l.power_up && !l.power_down);
        for x in [-1.0, 1.0] {
            let (w, l) = modern_pad(&pad(|i| i.left_x = x));
            assert_eq!(w, if x < 0.0 { bits::LEFT } else { bits::RIGHT });
            assert_eq!(l, LeverHolds::NONE);
        }
        // Right stick: ↑ brake +, ↓ brake − (by default), and no menu direction at all.
        let (w, l) = modern_pad(&pad(|i| i.right_y = 0.8 * BRAKE_PULL));
        assert_eq!(w, 0);
        assert!(l.brake_up && !l.brake_down && !l.power_up);
        let (w, l) = modern_pad(&pad(|i| i.right_y = -0.8 * BRAKE_PULL));
        assert_eq!(w, 0);
        assert!(l.brake_down && !l.brake_up);
        assert_eq!(modern_pad(&pad(|i| i.right_x = 1.0)).1, LeverHolds::NONE);
        // Both sticks at once: power and brake together.
        let l = modern_pad(&pad(|i| {
            i.left_y = PULL;
            i.right_y = BRAKE_PULL;
        }))
        .1;
        assert!(l.power_up && l.brake_up);
    }

    #[test]
    fn stick_and_trigger_hysteresis() {
        let mut m = ModernPad::default();
        let hold =
            |m: &mut ModernPad, f: fn(&mut GenericInputs, f32), v: f32| m.map(&pad(|i| f(i, v))).1;
        // Right stick toward brake + (↑ by default): on at half travel, held down to the
        // release travel, off below it.
        let ry: fn(&mut GenericInputs, f32) = |i, v| i.right_y = v * BRAKE_PULL;
        let seq: Vec<bool> = [0.3, 0.49, 0.5, 0.4, 0.36, 0.34, 0.45, 0.5]
            .iter()
            .map(|&v| hold(&mut m, ry, v).brake_up)
            .collect();
        assert_eq!(seq, [false, false, true, true, true, false, false, true]);
        // Trigger travel 0..1: pressed at 0.5, released below 0.35.
        let rt: fn(&mut GenericInputs, f32) = |i, v| i.rt = v;
        let mut m = ModernPad::default();
        let seq: Vec<bool> = [0.0, 0.3, 0.49, 0.5, 0.9, 0.4, 0.36, 0.3, 0.45, 1.0]
            .iter()
            .map(|&v| hold(&mut m, rt, v).brake_up)
            .collect();
        assert_eq!(
            seq,
            [
                false, false, false, true, true, true, true, false, false, true
            ]
        );
        // A stick resting off centre (a 0..65535 stick at 34354 reads ~+0.05) holds nothing.
        let mut m = ModernPad::default();
        let l = m
            .map(&pad(|i| {
                i.left_x = 0.048;
                i.left_y = -0.02;
                i.right_x = 0.05;
                i.right_y = 0.017;
            }))
            .1;
        assert_eq!(l, LeverHolds::NONE);
        // Unplugged: the latches let go.
        m.map(&pad(|i| i.lt = 1.0));
        m.reset();
        assert!(!m.map(&pad(|i| i.lt = 0.4)).1.power_up);
    }

    #[test]
    fn menu_pulses_once_across_devices() {
        let mut m = MenuRepeat::default();
        // Keyboard ↑ from tick 1, the pad's ↑ joins on tick 3 and stays after the key is let go.
        let mut pulses = Vec::new();
        for t in 1..=12 {
            let kb = if t <= 5 { bits::UP | bits::UP_PULSE } else { 0 };
            let pad = if t >= 3 { bits::UP } else { 0 };
            if m.merge(&[kb, pad]) & bits::UP_PULSE != 0 {
                pulses.push(t);
            }
        }
        // One press pulse, then the repeat of the one merged hold.
        assert_eq!(pulses, [1, 9, 12]);
        assert_eq!(m.merge(&[0, 0]), 0);
        // Alone, the keyboard's own pulses come out unchanged.
        let mut m = MenuRepeat::default();
        assert_eq!(m.merge(&[bits::DOWN]), bits::DOWN | bits::DOWN_PULSE);
        assert_eq!(m.merge(&[bits::DOWN]), bits::DOWN);
    }

    fn tick(before: u8, stepped: u8, handle: u8, reading: i32) -> HandleTick {
        HandleTick {
            before,
            stepped,
            handle,
            reading: Some(reading),
        }
    }

    #[test]
    fn last_used_wins_with_deadband_and_snap() {
        let mut a = LeverArbiter::new(false);
        // The handle at P2 (y = -310) owns the power notch.
        let idle = tick(0, 0, 0, 0);
        assert_eq!(a.tick([tick(0, 0, 2, -310), idle], 50), [2, 0]);
        // A key step to P3 takes it over.
        assert_eq!(a.tick([tick(2, 3, 2, -310), idle], 50), [3, 0]);
        assert_eq!(a.owner[0], Owner::Steps);
        // Jitter inside the deadband keeps the stepped notch.
        assert_eq!(a.tick([tick(3, 3, 2, -350), idle], 50), [3, 0]);
        assert_eq!(a.tick([tick(3, 3, 2, -270), idle], 50), [3, 0]);
        // More steps move on from the stepped notch; the anchor stays where the handle was.
        assert_eq!(a.tick([tick(3, 4, 2, -300), idle], 50), [4, 0]);
        assert_eq!(a.tick([tick(4, 4, 2, -355), idle], 50), [4, 0]);
        // Past the deadband: the notch snaps to the handle.
        assert_eq!(a.tick([tick(4, 4, 1, -102), idle], 50), [1, 0]);
        assert_eq!(a.owner[0], Owner::Handle);
        // The brake handle was never touched.
        assert_eq!(a.owner[1], Owner::Handle);
    }

    #[test]
    fn handle_keeps_its_debounce_after_the_snap() {
        // The decoder's EB debounce: the handle reports B8 until the sixth poll at EB. After
        // the snap the arbiter passes on whatever the decoder says, so EB still waits.
        let mut a = LeverArbiter::new(false);
        let p = tick(0, 0, 0, 0);
        a.tick([p, tick(9, 3, 9, 0)], 50);
        assert_eq!(a.owner[1], Owner::Steps);
        let mut seen = Vec::new();
        for n in 0..7 {
            let decoded = if n < 6 { 8 } else { 9 };
            seen.push(a.tick([p, tick(3, 3, decoded, 426)], 50)[1]);
        }
        assert_eq!(seen, [8, 8, 8, 8, 8, 8, 9]);
    }

    #[test]
    fn one_axis_hands_over_both_notches() {
        let mut a = LeverArbiter::new(true);
        // Mode 5 at P1: a brake key step takes both handles.
        assert_eq!(a.tick([tick(1, 1, 1, 150), tick(0, 1, 0, 150)], 50), [1, 1]);
        assert_eq!(a.owner, [Owner::Steps; 2]);
        // The lever moves: both come back from it.
        assert_eq!(
            a.tick([tick(1, 1, 0, -200), tick(1, 1, 2, -200)], 50),
            [0, 2]
        );
        assert_eq!(a.owner, [Owner::Handle; 2]);
    }

    #[test]
    fn learned_notches_take_over_on_any_change() {
        let mut a = LeverArbiter::new(false);
        let idle = tick(0, 0, 0, 0);
        a.tick([tick(0, 1, 0, 0), idle], 0);
        // Between two patterns there is no reading: hold.
        let between = HandleTick {
            before: 1,
            stepped: 1,
            handle: 0,
            reading: None,
        };
        assert_eq!(a.tick([between, idle], 0), [1, 0]);
        assert_eq!(a.tick([tick(1, 1, 0, 0), idle], 0), [1, 0]);
        assert_eq!(a.tick([tick(1, 1, 2, 2), idle], 0), [2, 0]);
    }

    #[test]
    fn hint_device_switches_on_first_input_only() {
        let mut h = HintTracker::new(HintDevice::Keyboard);
        let rest = [0.05f32, -0.08];
        assert_eq!(h.update(false, Some((0, &rest))), HintDevice::Keyboard);
        // Stick noise and drift inside the deadband: no switch.
        for v in [0.1f32, 0.2, 0.25, 0.28] {
            assert_eq!(
                h.update(false, Some((0, &[v, -0.08]))),
                HintDevice::Keyboard
            );
        }
        // A real push: pad.
        assert_eq!(h.update(false, Some((0, &[0.9, -0.08]))), HintDevice::Pad);
        // A key press: keyboard again; holding the stick where it is does not flip it back.
        assert_eq!(
            h.update(true, Some((0, &[0.9, -0.08]))),
            HintDevice::Keyboard
        );
        for _ in 0..5 {
            assert_eq!(
                h.update(false, Some((0, &[0.92, -0.08]))),
                HintDevice::Keyboard
            );
        }
        // A button press (edge, not hold) switches.
        assert_eq!(h.update(false, Some((1, &[0.92, -0.08]))), HintDevice::Pad);
        assert_eq!(
            h.update(true, Some((1, &[0.92, -0.08]))),
            HintDevice::Keyboard
        );
        assert_eq!(
            h.update(false, Some((1, &[0.92, -0.08]))),
            HintDevice::Keyboard
        );
        // Unplugged: keyboard.
        h.device = HintDevice::Pad;
        assert_eq!(h.update(false, None), HintDevice::Keyboard);
        assert_eq!(HintDevice::parse("Pad"), Some(HintDevice::Pad));
        assert_eq!(HintDevice::parse("x"), None);
    }
}

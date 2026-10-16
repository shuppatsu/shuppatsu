//! The bonus game (`BonusGame_*` 0x41b540 / 0x41b810 / 0x41c2e0, docs/re/menus.md §12.1):
//! after the station result of station `0x459e10[stage]` on a run without a continue, the
//! player drives a pair of EF63 locomotives onto a standing 489-series car in a small set of
//! its own. Only the speed at contact counts: below 7.5 km/h the coupling succeeds and gives
//! up to 10 bonus seconds and a prize picture.
//!
//! The locomotives run on their own physics task (0x4240d0 / 0x4241d0: train type 16, no
//! gradient, curve or traction fade, a brake cylinder easing at 1.15 × its rate, no top-speed
//! cap) on the player's notches of the previous tick. The bonus task then moves the pair by
//! `v / 150` units per tick (1 unit = 5 m). Positions are the task's z values; the scene and
//! the sprites are presentation and live in the game crate.

use ddg_formats::Result;
use ddg_formats::exe::Exe;

use crate::physics::{
    TrainSpec, brake_force, running_resistance, speed_resistance, traction_force,
};

/// Bonus stations per stage (i16[20] at 0x459e10; 99 = none).
const STATIONS: u32 = 0x459e10;
/// Camera keys: 5 × `{cx, cz, r_eye, r_target, y_eye, y_target, angle}` f32 at 0x4693c8.
const CAMERA_KEYS: u32 = 0x4693c8;
const KEY_COUNT: usize = 5;
/// The locomotives' train type (0x4240d0: `+0x5c = 0x10`).
pub const TRAIN_TYPE: usize = 0x10;
/// `Vehicle_Set(0x1002)`: the EF63 plate (row 22) and the ATC speedometer mode.
pub const VEHICLE: u32 = 0x1002;

/// Task fields at creation (0x41b540): pair x/z, car z, timer, start delay, needle start.
pub const PAIR_X: f32 = -1.5;
const PAIR_Z0: f32 = 4.4;
const CAR_Z0: f32 = -4.2;
/// The second locomotive is drawn 3.9 units behind the first (0x41c3c0).
pub const PAIR_SPACING: f32 = 3.9;
const TIME_LIMIT: i32 = 0x1da;
const START_DELAY: i32 = 60;
pub const NEEDLE_START: i32 = 248;
/// The intro ends after `+0x38` passes 0xd2; captions at 0x50, 0xb4, 0xd2; the instruction is
/// hidden at 0xf0; the camera starts moving at 120 by 0.02666… per count, up to key 2.
const INTRO_END: i32 = 0xd2;
const CAPTION_AT: [i32; 3] = [0x50, 0xb4, 0xd2];
const HIDE_INSTRUCTION_AT: i32 = 0xf0;
const CAMERA_FROM: i32 = 120;
const CAMERA_RATE: f64 = 0.0266666666;
const CAMERA_LAST: f64 = 2.0;
/// Units per tick from m/s (f64 0.00666666 at 0x469840).
const MPS_TO_UNITS: f64 = 0.00666666;
/// Contact at gap < 0 with `gap = z + 0.4` (f64 −0.4 at 0x458c50 subtracted).
const GAP_OFFSET: f64 = 0.4;
/// Distance readout in metres: `trunc(gap × 5)`.
const UNIT_M: f32 = 5.0;
/// Approach camera: `t = min(gap, 3.6) × 0.2777…`, key 3 → key 2.
const APPROACH_SPAN: f32 = 3.6;
const APPROACH_RATE: f64 = 0.2777777777777778;
/// Shock: `S = trunc(min(v, 5.5555…) × 0.18 × 216 + 248)`, pass if `S ≤ 328`.
const SHOCK_CAP_MPS: f64 = 5.555555555555555;
const SHOCK_CAP_F32: f32 = 5.555_555_3;
const SHOCK_GAIN: f64 = 0.18;
const SHOCK_SCALE: f32 = 216.0;
pub const SHOCK_LIMIT: i32 = 0x148;
/// The needle sweeps 2 px per tick.
const NEEDLE_STEP: i32 = 2;
/// After contact the car rides 3.8 units ahead of the pair (f64 at 0x469810); the slide loses
/// 0.0004 units/tick per tick (0x469800), then the recoil 0.0002 down to −0.01 (0x4697f8,
/// 0x4676d0); the physics gets `18 ×` the slide speed for its sounds (0x459568).
const CAR_OFFSET: f64 = 3.8;
const SLIDE_DECEL: f64 = 0.0004;
const RECOIL_DECEL: f64 = 0.0002;
const RECOIL_MIN: f32 = -0.01;
const SLIDE_SOUND_GAIN: f64 = 18.0;
/// The ending: points at `+0x38 == 0x10e`, the prize at 300, the end at 0x186.
const POINTS_AT: i32 = 0x10e;
const PRIZE_AT: i32 = 300;
const END_AT: i32 = 0x186;
const MAX_POINTS: i32 = 10;
/// Sounds (0x41fda0 / 0x42ff20): start 1, hard hit 0x1b (looped, released on settling),
/// coupler 0x2a, time up / no points 0x11, points jingle 0x199, BGM 0x19a; the EF63 horn 3.
pub mod snd {
    pub const START: u16 = 1;
    pub const HARD_HIT: u16 = 0x1b;
    pub const COUPLER: u16 = 0x2a;
    pub const BUZZER: u16 = 0x11;
    pub const POINTS: u16 = 0x199;
    pub const MUSIC: u16 = 0x19a;
    pub const HORN: u16 = 3;
    pub const BRAKE_AIR: u16 = 0x14;
    pub const BRAKE_RELEASE: u16 = 0x13;
}
/// Physics (0x4241d0): brake easing 1.15 × rate (f64 0x4726e0), cylinder floor −0.4, force to
/// m/s² (0.0324254215304799 × 0.2777777777777778), tick 0.03333333 s.
const BRAKE_EASE: f64 = 1.15;
const CYL_RELEASED: f32 = -0.4;
const FORCE_TO_ACCEL: f64 = 0.032_425_421_530_479_9 * 0.277_777_777_777_777_8;
const TICK_S: f64 = 0.033_333_33;
const KMH: f64 = 3.6;
const CYL_EPSILON: f64 = 1e-5;

/// The exe data of the bonus game.
#[derive(Debug, Clone, PartialEq)]
pub struct BonusTables {
    /// Bonus station per stage, 99 = none.
    pub stations: Vec<i16>,
    pub keys: [[f32; 7]; KEY_COUNT],
}

impl BonusTables {
    pub fn read(exe: &Exe<'_>) -> Result<Self> {
        let n = ddg_formats::stages::STAGE_COUNT;
        let r = exe.reader_at(STATIONS, 2 * n, "bonus stations")?;
        let stations = (0..n).map(|i| r.i16(2 * i)).collect::<Result<_>>()?;
        let r = exe.reader_at(CAMERA_KEYS, 0x1c * KEY_COUNT, "bonus camera")?;
        let mut keys = [[0.0; 7]; KEY_COUNT];
        for (k, key) in keys.iter_mut().enumerate() {
            for (j, v) in key.iter_mut().enumerate() {
                *v = r.f32(0x1c * k + 4 * j)?;
            }
        }
        Ok(Self { stations, keys })
    }

    /// Whether the bonus game follows the result of the stop that made `station` current.
    pub fn at(&self, stage: usize, station: usize) -> bool {
        self.stations
            .get(stage)
            .is_some_and(|&s| usize::try_from(s).is_ok_and(|s| s == station))
    }
}

/// The physics task of the locomotives (0x4240d0 / 0x4241d0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BonusTrain {
    /// Speed, m/s (`0x84c43c`).
    pub speed: f32,
    cylinder: f32,
    prev_power: u8,
    prev_brake: u8,
    /// Ticks the cylinder moved (+0x58), for the brake-air sound.
    moving: u32,
    pub odometer: f32,
    was_moving: bool,
}

impl BonusTrain {
    fn new() -> Self {
        Self {
            speed: 0.0,
            cylinder: CYL_RELEASED,
            prev_power: 0,
            prev_brake: 0,
            moving: 0,
            odometer: 0.0,
            was_moving: false,
        }
    }

    /// One update with this tick's notches (the task applies last tick's).
    fn step(&mut self, spec: &TrainSpec, power: u8, brake: u8, out: &mut Vec<BonusEvent>) {
        // The task posts the release every still tick; only the first one is passed on.
        if self.moving == 1 {
            out.push(BonusEvent::Sound(snd::BRAKE_AIR));
        } else if self.moving == 0 && self.was_moving {
            out.push(BonusEvent::Release(snd::BRAKE_AIR));
        }
        self.was_moving = self.moving > 0;
        if brake < self.prev_brake && brake == 0 {
            out.push(BonusEvent::Sound(snd::BRAKE_RELEASE));
        }
        let v = self.speed;
        let kmh = f64::from(v) * KMH;
        let r = running_resistance(spec, kmh);
        let r2 = speed_resistance(spec, v);
        let mut f = traction_force(spec, i32::from(self.prev_power), kmh as f32, 1.0);
        let rate = f64::from(spec.brake_rate);
        let old = f64::from(self.cylinder);
        let mut c = old;
        if self.prev_brake > 0 {
            f = 0.0;
            c += rate;
            let target = f64::from(self.prev_brake) * 0.1 * f64::from(spec.brake_per_notch);
            if c >= target {
                c -= BRAKE_EASE * rate;
                if c < target {
                    c = target;
                }
            }
        } else {
            c -= BRAKE_EASE * rate;
            if c < f64::from(CYL_RELEASED) {
                c = f64::from(CYL_RELEASED);
            }
        }
        self.cylinder = c as f32;
        self.moving = if (c - old).abs() > CYL_EPSILON {
            self.moving + 1
        } else {
            0
        };
        let fb = brake_force(spec, self.cylinder, v, 0, false, false, 1.0);
        let a = (f - r - r2 - fb) * FORCE_TO_ACCEL;
        let mut nv = (f64::from(v) + a * TICK_S) as f32;
        if nv < 0.0 {
            nv = 0.0;
        }
        self.speed = nv;
        self.odometer = (f64::from(self.odometer) + f64::from(nv) * TICK_S) as f32;
        self.prev_power = power;
        self.prev_brake = brake;
    }
}

/// How the game ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BonusResult {
    /// Coupled with shock value `shock` ≤ 328.
    Coupled,
    /// Too hard (shock above 328): 連結失敗.
    TooHard,
    /// The time ran out before contact.
    TimeUp,
}

/// Something for the presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BonusEvent {
    Sound(u16),
    Release(u16),
    /// `0x42ff20(code)`: music, not gated by the sound option.
    Music(u16),
    Result(BonusResult),
    /// Bonus seconds shown and added (TimeMsg 0x103).
    Points(i32),
    /// The prize picture: 0x25d for 10 points, 0x25c for 1–9.
    Prize(i16),
}

/// What the screen shows (the slots of 0x41b810).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BonusScreen {
    /// The draw callback 0x41c3c0 and the meter are up (after the 60-tick delay).
    pub started: bool,
    /// Slot 0x38: ボーナスゲーム 0x2d8, then 連結成功 0x2dc / 連結失敗 0x2dd.
    pub banner: Option<i16>,
    /// The time-up placement of slot 0x38 (lower) and slot 0x3a = 時間切れ 0x252.
    pub time_up: bool,
    /// Slot 0x39: 0x2d9, 0x2de 準備, 0x2df 開始.
    pub instruction: Option<i16>,
    /// Distance readout, m (`0x416ab0`), `None` hidden.
    pub distance_m: Option<i32>,
    /// Needle x offset (task+0x30).
    pub needle: i32,
    pub prize: Option<i16>,
    /// Timer ticks (task+0x44) for 0x417040.
    pub timer: i32,
}

/// The bonus task.
#[derive(Debug, Clone, PartialEq)]
pub struct BonusGame {
    keys: [[f32; 7]; KEY_COUNT],
    delay: i32,
    /// `+0x38`.
    count: i32,
    pub train: BonusTrain,
    /// Pair z (+0x20) and car z (+0x64), units.
    pub pair_z: f32,
    pub car_z: f32,
    /// Movement this tick, units (+0x7c).
    slide: f32,
    /// Shock value (+0x34), −1 before contact.
    pub shock: i32,
    hard: u8,
    /// +0x40: 1 contacted and settled, 2 time-out.
    end: u8,
    pub points: i32,
    pub screen: BonusScreen,
    /// The camera key in force (interpolated).
    pub camera: [f32; 7],
    done: bool,
}

impl BonusGame {
    pub fn new(t: &BonusTables) -> Self {
        Self {
            keys: t.keys,
            delay: START_DELAY,
            count: 0,
            train: BonusTrain::new(),
            pair_z: PAIR_Z0,
            car_z: CAR_Z0,
            slide: 0.0,
            shock: -1,
            hard: 0,
            end: 0,
            points: 0,
            screen: BonusScreen {
                needle: NEEDLE_START,
                timer: TIME_LIMIT,
                ..BonusScreen::default()
            },
            camera: t.keys[0],
            done: false,
        }
    }

    /// The task ended (flag 0x4000 clear).
    pub fn done(&self) -> bool {
        self.done
    }

    /// Eye and target of the camera key (the tail at 0x41c240).
    pub fn eye_target(&self) -> ([f32; 3], [f32; 3]) {
        let [cx, cz, re, rt, ye, yt, a] = self.camera.map(f64::from);
        let pi = std::f64::consts::PI;
        let eye = [
            ((a + pi).sin() * re + cx) as f32,
            ye as f32,
            ((a + pi).cos() * re + cz) as f32,
        ];
        let target = [
            (a.sin() * rt + cx) as f32,
            yt as f32,
            (a.cos() * rt + cz) as f32,
        ];
        (eye, target)
    }

    fn lerp(&mut self, from: usize, to: usize, t: f64) {
        let (a, b) = (self.keys[from], self.keys[to]);
        for i in 0..7 {
            self.camera[i] = (f64::from(a[i]) + (f64::from(b[i]) - f64::from(a[i])) * t) as f32;
        }
    }

    /// One `Task_Tick`: the physics task (list 2) on the notches, then the bonus task (list 3).
    pub fn tick(
        &mut self,
        spec: &TrainSpec,
        power: u8,
        brake: u8,
        // (pressed, released) edges of the horn button.
        horn: (bool, bool),
    ) -> Vec<BonusEvent> {
        let mut out = Vec::new();
        if self.done {
            return out;
        }
        if horn.1 {
            out.push(BonusEvent::Release(snd::HORN));
        } else if horn.0 {
            out.push(BonusEvent::Sound(snd::HORN));
        }
        self.train.step(spec, power, brake, &mut out);
        self.update(&mut out);
        out
    }

    /// 0x41b810.
    fn update(&mut self, out: &mut Vec<BonusEvent>) {
        let s = &mut self.screen;
        if self.delay > 0 {
            self.delay -= 1;
            if self.delay == 0 {
                out.push(BonusEvent::Music(snd::MUSIC));
                s.started = true;
                s.banner = Some(0x2d8);
            }
            return;
        }
        if self.count <= INTRO_END {
            match self.count {
                c if c == CAPTION_AT[0] => {
                    s.banner = None;
                    s.instruction = Some(0x2d9);
                }
                c if c == CAPTION_AT[1] => s.instruction = Some(0x2de),
                c if c == CAPTION_AT[2] => {
                    s.instruction = Some(0x2df);
                    out.push(BonusEvent::Sound(snd::START));
                }
                _ => {}
            }
            self.train.speed = 0.0;
            self.count += 1;
            if self.count >= CAMERA_FROM {
                let t = (f64::from(self.count - CAMERA_FROM) * CAMERA_RATE).min(CAMERA_LAST);
                let i = (t as usize).min(KEY_COUNT - 2);
                self.lerp(i, i + 1, t - i as f64);
            }
            return;
        }
        if self.count <= HIDE_INSTRUCTION_AT {
            self.count += 1;
            if self.count == HIDE_INSTRUCTION_AT {
                self.screen.instruction = None;
            }
        }
        if self.shock == -1 {
            self.slide = (f64::from(self.train.speed) * MPS_TO_UNITS) as f32;
            self.screen.timer -= 1;
        } else {
            self.screen.timer = (self.screen.timer / 30) * 30 - 1;
        }
        if self.end == 0 && self.slide > 0.0 {
            self.pair_z -= self.slide;
        }
        let gap = (f64::from(self.pair_z) + GAP_OFFSET) as f32;
        if gap >= 0.0 {
            self.screen.distance_m = Some((gap * UNIT_M) as i32);
            let t = f64::from(gap.min(APPROACH_SPAN)) * APPROACH_RATE;
            self.lerp(3, 2, t);
        } else {
            self.screen.distance_m = None;
            if self.shock == -1 {
                let v = self.train.speed;
                let vs = if f64::from(v) < SHOCK_CAP_MPS {
                    v
                } else {
                    SHOCK_CAP_F32
                };
                let s = (f64::from(vs) * SHOCK_GAIN * f64::from(SHOCK_SCALE) + 248.0) as i32;
                self.shock = s;
                let result = if s <= SHOCK_LIMIT {
                    BonusResult::Coupled
                } else {
                    self.hard = 1;
                    out.push(BonusEvent::Sound(snd::HARD_HIT));
                    BonusResult::TooHard
                };
                self.screen.banner = Some(if result == BonusResult::Coupled {
                    0x2dc
                } else {
                    0x2dd
                });
                out.push(BonusEvent::Result(result));
                out.push(BonusEvent::Sound(snd::COUPLER));
            } else if self.screen.needle < self.shock {
                self.screen.needle = (self.screen.needle + NEEDLE_STEP).min(self.shock);
            }
            self.car_z = (f64::from(self.pair_z) - CAR_OFFSET) as f32;
            if self.slide > 0.0 {
                self.train.speed = (f64::from(self.slide) * SLIDE_SOUND_GAIN) as f32;
                self.slide = (f64::from(self.slide) - SLIDE_DECEL) as f32;
            } else {
                if self.hard == 1 {
                    out.push(BonusEvent::Release(snd::HARD_HIT));
                    self.hard = 2;
                }
                self.slide = (f64::from(self.slide) - RECOIL_DECEL) as f32;
                if self.slide <= RECOIL_MIN {
                    self.slide = RECOIL_MIN;
                }
                self.car_z -= self.slide;
                self.train.speed = 0.0;
                self.end = 1;
            }
            let p = f64::from(gap).max(-1.0);
            self.lerp(3, 4, -p);
        }
        if self.screen.timer == 0 && self.shock == -1 {
            self.end = 2;
            self.screen.banner = Some(0x2dd);
            self.screen.time_up = true;
            self.screen.timer = -1;
            out.push(BonusEvent::Result(BonusResult::TimeUp));
            out.push(BonusEvent::Sound(snd::BUZZER));
        }
        if self.end > 0 {
            self.count += 1;
            if self.count == POINTS_AT && self.end == 1 {
                let d = SHOCK_LIMIT - self.shock;
                if d < 0 {
                    self.points = 0;
                    out.push(BonusEvent::Sound(snd::BUZZER));
                } else {
                    self.points = (d / 8 + 1).min(MAX_POINTS);
                    out.push(BonusEvent::Music(snd::POINTS));
                }
                if self.points > 0 {
                    out.push(BonusEvent::Points(self.points));
                }
            }
            if self.count == PRIZE_AT && self.end == 1 && self.points > 0 {
                let prize = if self.points == MAX_POINTS {
                    0x25d
                } else {
                    0x25c
                };
                self.screen.prize = Some(prize);
                out.push(BonusEvent::Prize(prize));
            }
            if self.count >= END_AT {
                self.screen.distance_m = None;
                self.done = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> TrainSpec {
        // Type 16 (0x471e98).
        TrainSpec::from_floats([
            65.0, 18.0, 45.0, 73.0, 103.0, 135.0, 174.0, 100.0, 0.055, 4.0, 18.0, 3.75, 300.6, 5.0,
            0.05,
        ])
    }

    fn tables() -> BonusTables {
        BonusTables {
            stations: vec![99; 20],
            keys: [[0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0]; KEY_COUNT],
        }
    }

    fn run(g: &mut BonusGame, drive: impl Fn(&BonusGame) -> (u8, u8)) -> Vec<BonusEvent> {
        let s = spec();
        let mut all = Vec::new();
        for _ in 0..3000 {
            let (p, b) = drive(g);
            all.extend(g.tick(&s, p, b, (false, false)));
            if g.done() {
                break;
            }
        }
        all
    }

    #[test]
    fn a_gentle_touch_couples_with_points() {
        let mut g = BonusGame::new(&tables());
        let ev = run(&mut g, |g| {
            let kmh = g.train.speed * 3.6;
            let target = if g.pair_z > 0.0 { 7.0 } else { 3.0 };
            if kmh < target - 0.5 {
                (1, 0)
            } else if kmh > target + 0.3 {
                (0, 3)
            } else {
                (0, 0)
            }
        });
        assert!(g.done());
        assert!(
            ev.contains(&BonusEvent::Result(BonusResult::Coupled)),
            "{ev:?}"
        );
        assert!(g.points >= 3, "{}", g.points);
        assert!(ev.iter().any(|e| matches!(e, BonusEvent::Prize(_))));
    }

    #[test]
    fn full_power_fails_and_standing_still_times_out() {
        let mut g = BonusGame::new(&tables());
        let ev = run(&mut g, |_| (5, 0));
        assert!(ev.contains(&BonusEvent::Result(BonusResult::TooHard)));
        assert_eq!(g.points, 0);
        let mut g = BonusGame::new(&tables());
        let ev = run(&mut g, |_| (0, 0));
        assert!(ev.contains(&BonusEvent::Result(BonusResult::TimeUp)));
        assert!(g.done());
    }
}

//! Per-tick train sounds (audio-text.md §6): motor, running noise, rail joints, brake air,
//! flange squeal, horn, and the track-class ambient sounds.
//!
//! The original computes these inside the player-train task (0x422c20, sound part
//! 0x4234eb–0x423eb0) and in the per-tick track lookup 0x407c10. Both only post sound messages,
//! so they live here as deterministic controllers that read plain simulation values (see
//! [`TrainTick`]) and emit commands through a [`SoundSystem`]. Float maths follows the x87 code
//! (53-bit precision, `f32` only where the original stores to a float, truncating `ftol`).

use crate::code::RUNNING_NOISE;
use crate::system::SoundSystem;
use crate::tables::{JointPattern, STAGE_COUNT, SoundTables, StageSounds};

/// Brake-air hiss while the cylinder moves (0x422c20).
const BRAKE_AIR: u16 = 0x14;
/// Brake release "pshh" when the brake goes to 0.
const BRAKE_RELEASE: u16 = 0x13;
/// Released when stopped or not driving; its starter was not found (open question).
const CODE_1C: u16 = 0x1c;
/// Flange squeal on sharp curves.
const FLANGE_SQUEAL: u16 = 0x19;
/// Played on the first moving tick from standstill (trains with the flange flag).
const DEPARTURE: u16 = 0x30;
/// Melody horn of one stage; a BGM-slot sound that is never released and rate-limited.
const MELODY_HORN: u16 = 0x1a3;
/// Horn code that is played but never released.
const HORN_NO_RELEASE: u16 = 8;
/// Rail-joint row used while 0x8570c2 is set.
const JOINT_OVERRIDE_ROW: usize = 20;

/// Horn button edge this tick (0x48c42c / 0x48c430 bit 0x100, or the replay byte).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HornEdge {
    #[default]
    None,
    Pressed,
    Released,
}

/// Simulation values the train sounds read each tick. Field docs give the original storage
/// and the matching `ddg_engine::physics::TrainState` field where there is one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrainTick {
    /// Driving phase (flag 0x200 of 0x855ff8; `TickContext::driving`).
    pub driving: bool,
    /// Horn allowed (flag 0x4000 of 0x855ff8 clear).
    pub horn_enabled: bool,
    pub horn: HornEdge,
    /// Current train type (0x84c3fc; `TrainState::train_type`).
    pub train_type: usize,
    /// Top speed of the current train type in km/h (0x471aec; `TrainSpec::max_speed_kmh`).
    pub top_speed_kmh: f32,
    /// Speed before this tick's physics, m/s (0x857234).
    pub prev_speed: f32,
    /// Speed after this tick's physics, m/s (task+0x3c; `TrainState::speed` after the step).
    pub speed: f32,
    /// Traction fade 0..1 after the step (task+0x38; `TrainState::traction_ramp`).
    pub traction_fade: f32,
    /// Power notch of the previous tick (task+0x60; `TrainState::prev_power` before the step).
    pub prev_power: i32,
    /// Effective brake notch of the previous tick (task+0x64; `prev_brake` before the step).
    pub prev_brake: i32,
    /// Effective brake notch this tick (after the EB/overrun overrides).
    pub brake: i32,
    /// Ticks the brake cylinder has been moving, after the step (task+0x58;
    /// `TrainState::brake_moving_ticks`).
    pub brake_moving_ticks: i32,
    /// Odometer in metres (0x857004; `TrainState::odometer`).
    pub odometer: f32,
    /// Curve sharpness (0x84c408; `TrainState::curve_sharpness`).
    pub curve_sharpness: f32,
    /// Stage tick counter (0x8570b0; `StageClock`'s driving tick count).
    pub stage_tick: i32,
    /// Rail-joint override flag 0x8570c2 (selects joint row 20).
    pub joint_override: bool,
}

impl Default for TrainTick {
    fn default() -> Self {
        Self {
            driving: true,
            horn_enabled: true,
            horn: HornEdge::None,
            train_type: 0,
            top_speed_kmh: 100.0,
            prev_speed: 0.0,
            speed: 0.0,
            traction_fade: 1.0,
            prev_power: 0,
            prev_brake: 0,
            brake: 0,
            brake_moving_ticks: 0,
            odometer: 0.0,
            curve_sharpness: 0.0,
            stage_tick: 0,
            joint_override: false,
        }
    }
}

/// How code 9 (running noise) follows the speed; chosen per train type at task creation
/// (task+0x96, 0x4229c0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoiseMode {
    /// Pitch target from speed / (top × 1.4); most types.
    Standard,
    /// Type 9: pitch held at 127 below 32 km/h, special low-speed braking behaviour.
    Type9,
    /// Types 4 and 5: different motor scale, pitch target from speed / (top × 1.2).
    Type45,
    /// Types 2, 3, 6 and 8: noise pitch fixed at 127.
    Fixed,
}

impl NoiseMode {
    /// The mapping hardcoded in 0x4229c0.
    pub fn for_train_type(train_type: usize) -> Self {
        match train_type {
            9 => Self::Type9,
            2 | 3 | 6 | 8 => Self::Fixed,
            4 | 5 => Self::Type45,
            _ => Self::Standard,
        }
    }
}

/// Clamp as the original does it: `< lo → lo`, else `> hi → hi`.
fn clamp_f(x: f64, lo: f64, hi: f64) -> f64 {
    if x < lo {
        lo
    } else if x > hi {
        hi
    } else {
        x
    }
}

/// x87 `ftol` (truncation) of a finite value into 32 bits; out-of-range gives 0x80000000.
fn ftol(x: f64) -> i32 {
    if x.is_finite() && x > i32::MIN as f64 - 1.0 && x < i32::MAX as f64 + 1.0 {
        x as i32
    } else {
        i32::MIN
    }
}

/// The player train's sounds (task fields 0x6c, 0x80, 0x84, 0x90–0x96).
#[derive(Debug, Clone, PartialEq)]
pub struct TrainSound {
    stage: StageSounds,
    joints: Vec<JointPattern>,
    mode: NoiseMode,
    /// 0x84c430: brake-air sounds (joint-table flag bit 0 of the type at creation).
    brake_air: bool,
    /// 0x84c400: flange squeal and departure sound (flag bit 1).
    flange: bool,
    /// task+0x90: motor pitch parameter.
    motor_pitch: i16,
    /// task+0x92 / +0x94: running-noise volume and pitch parameters.
    noise_volume: i16,
    noise_pitch: i16,
    /// task+0x6c: ticks since the melody horn last played.
    horn_ticks: i32,
    /// task+0x80: rail-joint click state 0..=4.
    joint_state: i32,
    /// task+0x84: flange-squeal cooldown.
    squeal_ticks: i32,
    /// `0x84c3dc`: rail-joint clicks so far (the attract demo's logo animation counts them).
    pub clicks: u32,
}

impl TrainSound {
    /// Creates the train sounds for `stage` and `train_type` (0x4229c0) and posts the motor's
    /// base pitch.
    pub fn new(
        tables: &SoundTables,
        stage: usize,
        train_type: usize,
        snd: &mut SoundSystem,
    ) -> Self {
        let stage = if stage < STAGE_COUNT {
            tables.stages[stage]
        } else {
            StageSounds::default()
        };
        let flags = tables.joints.get(train_type).map_or(0, |j| j.flags);
        let ts = Self {
            stage,
            joints: tables.joints.clone(),
            mode: NoiseMode::for_train_type(train_type),
            brake_air: flags & JointPattern::BRAKE_AIR != 0,
            flange: flags & JointPattern::FLANGE != 0,
            motor_pitch: stage.motor_base_pitch,
            noise_volume: 1,
            noise_pitch: 0x7f,
            horn_ticks: 0,
            joint_state: 0,
            squeal_ticks: 0,
            clicks: 0,
        };
        snd.set_pitch(stage.motor, stage.motor_base_pitch);
        ts
    }

    pub fn mode(&self) -> NoiseMode {
        self.mode
    }

    /// (motor pitch, noise volume, noise pitch) parameters as last computed.
    pub fn params(&self) -> (i16, i16, i16) {
        (self.motor_pitch, self.noise_volume, self.noise_pitch)
    }

    pub fn stage_sounds(&self) -> StageSounds {
        self.stage
    }

    /// 0x84c400, the flange-squeal / departure-sound flag. Latched from the train type at
    /// creation; course events 18 / 19 set and clear it (0x40acd0).
    pub fn flange(&self) -> bool {
        self.flange
    }

    pub fn set_flange(&mut self, on: bool) {
        self.flange = on;
    }

    /// Station-segment start (0x4080d0): starts the running noise (silent, lowest pitch) and
    /// the motor at its base pitch.
    pub fn start_segment(&self, snd: &mut SoundSystem) {
        snd.request(RUNNING_NOISE);
        snd.set_pitch(RUNNING_NOISE, 0x7f);
        snd.set_volume(RUNNING_NOISE, 1);
        snd.request(self.stage.motor);
        snd.set_pitch(self.stage.motor, self.stage.motor_base_pitch);
    }

    /// The train task's reset hook (0x424070): running noise back to full volume, motor to its
    /// base pitch.
    pub fn reset_sounds(&self, snd: &mut SoundSystem) {
        snd.set_volume(RUNNING_NOISE, 0x7f);
        snd.set_pitch(self.stage.motor, self.stage.motor_base_pitch);
    }

    /// One 33 ms tick, in the order of 0x422c20.
    pub fn tick(&mut self, t: &TrainTick, snd: &mut SoundSystem) {
        self.horn(t, snd);

        if self.brake_air {
            match t.brake_moving_ticks {
                1 => snd.request(BRAKE_AIR),
                0 => snd.release(BRAKE_AIR),
                _ => {}
            }
            if t.brake < t.prev_brake && t.brake == 0 {
                snd.request(BRAKE_RELEASE);
            }
        }

        if !t.driving {
            snd.release(CODE_1C);
            self.squeal_ticks = 0;
            self.joint_state = 0;
            return;
        }

        if t.speed <= 0.0 {
            snd.release(CODE_1C);
            self.noise_volume = 1;
            self.noise_pitch = 0x7f;
            snd.set_volume(RUNNING_NOISE, self.noise_volume);
            snd.set_pitch(RUNNING_NOISE, self.noise_pitch);
            snd.set_pitch(self.stage.motor, self.stage.motor_base_pitch);
            return;
        }

        self.motor_and_noise(t);
        // The original posts the volume twice (0x4301b0 and its wrapper 0x42ff30); the second
        // post is a no-op because the voice caches the parameter.
        snd.set_volume(RUNNING_NOISE, self.noise_volume);
        snd.set_pitch(RUNNING_NOISE, self.noise_pitch);
        snd.set_pitch(self.stage.motor, self.motor_pitch);

        self.rail_joints(t, snd);

        if self.flange {
            if t.curve_sharpness as f64 >= 2.1 && self.squeal_ticks >= 0x50 {
                snd.request(FLANGE_SQUEAL);
                self.squeal_ticks = 0;
            } else if self.squeal_ticks <= 0x50 {
                self.squeal_ticks += 1;
            }
            if t.prev_speed == 0.0 {
                snd.request(DEPARTURE);
            }
        }
    }

    fn horn(&mut self, t: &TrainTick, snd: &mut SoundSystem) {
        let horn = self.stage.horn;
        if t.horn_enabled {
            match t.horn {
                HornEdge::Pressed if horn == MELODY_HORN => {
                    if self.horn_ticks > 99 {
                        snd.request(MELODY_HORN);
                        self.horn_ticks = 0;
                    }
                }
                HornEdge::Pressed => snd.request(horn),
                HornEdge::Released if horn != MELODY_HORN && horn != HORN_NO_RELEASE => {
                    snd.release(horn)
                }
                _ => {}
            }
        }
        self.horn_ticks = self.horn_ticks.wrapping_add(1);
    }

    /// Motor pitch and running-noise volume/pitch (0x4235aa–0x423c32).
    fn motor_and_noise(&mut self, t: &TrainTick) {
        let fade = t.traction_fade as f64;
        let v_prev = t.prev_speed as f64;
        let v = t.speed as f64;
        let kmh_wide = v * 3.6;
        // [esp+0x14] is the f32 store of the km/h value; task+0x30 continues from the
        // unrounded product (constants 0x459420, 0x472638, 0x472630).
        let kmh = kmh_wide as f32 as f64;
        let motor_x = (kmh_wide * 34.6597 * 1.149425287) as f32 as f64;
        let top = t.top_speed_kmh as f64;

        // Running-noise volume steps by 1 toward v_prev·32·fade (0x458bf0).
        let vol_target = clamp_f(v_prev * 32.0 * fade, 1.0, 127.0);
        let nv = self.noise_volume as f64;
        if nv < vol_target {
            self.noise_volume += 1;
        } else if nv > vol_target {
            self.noise_volume -= 1;
        }

        let motor_scale = match self.mode {
            NoiseMode::Type45 => 0.0005,      // 0x472660
            _ => 0.000_333_333_333_333_333_3, // 0x472698 (1/3000)
        };
        let p = ftol(motor_x * motor_scale * fade * 127.0) as i16;
        self.motor_pitch = p.clamp(1, 127);

        let pitch_target = |div: f64| 128.0 - kmh / (top * div) * fade * 127.0;
        match self.mode {
            NoiseMode::Standard => {
                let target = clamp_f(pitch_target(1.4), 1.0, 127.0);
                self.noise_pitch_steps(t.stage_tick, v * -0.117_647_058_823_529_41, target, true);
                self.idle_adjust(t);
            }
            NoiseMode::Type9 => {
                let target = if v_prev < 8.888_888_888_888_89 {
                    127.0
                } else {
                    clamp_f(pitch_target(1.4), 1.0, 127.0)
                };
                self.noise_pitch_steps(
                    t.stage_tick,
                    v_prev * -0.117_647_058_823_529_41,
                    target,
                    false,
                );
                if v_prev > v && v_prev < 5.555_555_555_555_555 && t.brake != 0 {
                    // Low-speed braking: jump the noise pitch up and let the volume rise.
                    if self.noise_pitch == 0x7f {
                        self.noise_pitch = 0x20;
                    }
                    if self.noise_pitch == 0x20 {
                        self.noise_pitch = 0x1f;
                    }
                    let t2 = clamp_f(v_prev * 30.0 * fade, 1.0, 127.0);
                    if (self.noise_volume as f64) < t2 {
                        self.noise_volume += 1;
                    }
                } else {
                    self.far_above_step(target);
                    self.idle_adjust(t);
                }
            }
            NoiseMode::Type45 => {
                let target = if v_prev <= 5.0 && v_prev < v {
                    let k = ftol(v_prev * 3.6);
                    if k == 9 || k == 17 {
                        self.noise_pitch = 0x78;
                    }
                    120.0
                } else {
                    pitch_target(1.2)
                };
                let target = if v_prev < v {
                    // Accelerating: capped at 80, no lower clamp.
                    if target > 80.0 { 80.0 } else { target }
                } else {
                    clamp_f(target, 1.0, 127.0)
                };
                self.noise_pitch_steps(t.stage_tick, v * -0.25, target, true);
                if t.prev_power == 0 {
                    self.idle_adjust(t);
                } else if self.noise_volume < 0x70 {
                    self.noise_volume = 0x70;
                }
            }
            NoiseMode::Fixed => self.noise_pitch = 0x7f,
        }
    }

    /// The shared pitch-slew block: a slow decrement every `1 − trunc(rate)` stage ticks while
    /// above the target, an extra decrement when more than 32 above (if `far_step`), and an
    /// increment while below.
    fn noise_pitch_steps(&mut self, stage_tick: i32, rate: f64, target: f64, far_step: bool) {
        let div = 1i32.wrapping_sub(ftol(rate)) as i16 as i32;
        if div != 0 && stage_tick.wrapping_rem(div) == 0 && self.noise_pitch as f64 > target {
            self.noise_pitch -= 1;
        }
        if far_step {
            self.far_above_step(target);
        }
        if (self.noise_pitch as f64) < target {
            self.noise_pitch += 1;
        }
    }

    fn far_above_step(&mut self, target: f64) {
        let np = self.noise_pitch as f64;
        if np > target && (target - np).abs() > 32.0 {
            self.noise_pitch -= 1;
        }
    }

    /// Coasting (no power last tick): noise pitch +3 up to 0x70, volume −5 down to 0x40.
    fn idle_adjust(&mut self, t: &TrainTick) {
        if t.prev_power != 0 {
            return;
        }
        if self.noise_pitch < 0x70 {
            self.noise_pitch += 3;
        }
        if self.noise_volume > 0x40 {
            self.noise_volume -= 5;
        }
    }

    /// Rail-joint clicks (0x423c79–0x423e2b): four phases per pattern period, the second and
    /// fourth skipped at ≥ 22.2 m/s (80 km/h).
    fn rail_joints(&mut self, t: &TrainTick, snd: &mut SoundSystem) {
        let row = if t.joint_override {
            JOINT_OVERRIDE_ROW
        } else {
            t.train_type
        };
        let (Some(row), Some(own)) = (self.joints.get(row).copied(), self.joints.get(t.train_type))
        else {
            return;
        };
        if row.period <= 0 {
            return;
        }
        let phase = ftol(t.odometer as f64 * 10.0).wrapping_rem(row.period);
        if phase <= row.clicks[0] && self.joint_state == 4 {
            self.joint_state = 0;
        }
        if own.flags & JointPattern::JOINTS == 0 {
            return;
        }
        let fast = t.speed as f64 >= 22.2;
        let code = self.stage.joint;
        for (i, &at) in row.clicks.iter().enumerate() {
            if phase >= at && self.joint_state == i as i32 {
                snd.request(code);
                self.joint_state += 1;
                self.clicks = self.clicks.wrapping_add(1);
                if fast && (i == 0 || i == 2) {
                    self.joint_state += 1;
                }
            }
        }
    }
}

/// Start offsets of the three wheel sets crossing a turnout, m (0x4205c0: 0, −2.5f, −14.5f).
const TURNOUT_WHEELS: [f32; 3] = [0.0, -2.5, -14.5];
/// Distance to a wheel set's next click after its 1st, 2nd and 3rd click, m (stack floats in
/// 0x4204d0).
const TURNOUT_SPACING: [f32; 3] = [4.0, 3.0, 3.0];
/// Played when a turnout has been crossed below 40 km/h with the flange flag set (0x4204d0).
const TURNOUT_SQUEAL: u16 = 0x2f;

/// Turnout clatter: task 0x4204d0, created by 0x4205c0 from the train task (0x422c20) on the
/// tick after 0x407c10 flagged a turnout under the point 6 ahead (`0x856d96`). Three wheel sets
/// advance by the speed at creation; each plays the rail-joint code when its distance reaches 0
/// and then waits for the next spacing (at most three clicks per set). The task ends once every
/// wheel set has clicked twice, with a squeal (0x2f) at low speed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TurnoutClatter {
    /// task+0x3c: metres per tick, `speed × (1/30)` (double 0x459550).
    step: f32,
    /// task+0x40..: distance of each wheel set past its next click point, m.
    wheels: [f32; 3],
    /// task+0x4c..: clicks so far per wheel set.
    clicks: [usize; 3],
}

impl TurnoutClatter {
    /// 0x4205c0 with the train speed in m/s.
    pub fn new(speed: f32) -> Self {
        Self {
            step: (f64::from(speed) * (1.0 / 30.0)) as f32,
            wheels: TURNOUT_WHEELS,
            clicks: [0; 3],
        }
    }

    /// One update; `joint` is the stage's rail-joint code (0x856d98), `flange` the flag
    /// 0x84c400. Returns false when the task has ended.
    pub fn tick(&mut self, joint: u16, flange: bool, snd: &mut SoundSystem) -> bool {
        let mut settled = 0;
        for (pos, clicks) in self.wheels.iter_mut().zip(self.clicks.iter_mut()) {
            if *pos >= 0.0 && *clicks < 3 {
                snd.request(joint);
                // The original indexes past its 3-entry table if a wheel set could skip three
                // spacings in one tick; no speed the game reaches does (10 m > 2.8 m per tick).
                while *pos >= 0.0 && *clicks < 3 {
                    *pos -= TURNOUT_SPACING[*clicks];
                    *clicks += 1;
                }
            }
            *pos += self.step;
            if *clicks > 1 {
                settled += 1;
            }
        }
        if settled < 3 {
            return true;
        }
        // fld +0x3c; fmul 108.0 (0x46c638); fcomp 40.0 (0x4594b0): below 40 km/h.
        if flange && f64::from(self.step) * 108.0 < 40.0 {
            snd.request(TURNOUT_SQUEAL);
        }
        false
    }
}

/// Codes of the three track-class ambient sounds (0x407c10). Class 1 is distance-based
/// (approach, pass, recede; probably level crossings), classes 2 and 3 follow speed (probably
/// bridges/tunnels). The interpretations are guesses.
const AMBIENT_POINT: u16 = 0x2c;
const AMBIENT_CLASS2: u16 = 0x2e;
const AMBIENT_CLASS3: u16 = 0x29;

/// Track-class ambient sounds (0x407c10). The game looks up the class of the track points 16
/// behind, 4 ahead and 24 ahead of the train in the stage's `track_sounds` table and passes the
/// three classes every tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AmbientSound {
    /// 0x857200[3]: class at each probe point.
    classes: [i16; 3],
    /// 0x856d94: odometer anchor of the class-1 sound (an `i16`, so it wraps past 32767 m).
    anchor: i16,
    /// 0x856d9c: class-1 state (−1 idle, 0 approaching, 1 at the point, 2 receding).
    state: i16,
}

impl Default for AmbientSound {
    fn default() -> Self {
        Self::new()
    }
}

impl AmbientSound {
    /// State after the segment reset 0x40cf40 (anchor −1, state 0).
    pub fn new() -> Self {
        Self {
            classes: [0; 3],
            anchor: -1,
            state: 0,
        }
    }

    /// Segment reset (0x40cf40). The probe classes are kept.
    pub fn reset(&mut self) {
        self.anchor = -1;
        self.state = 0;
    }

    /// Class of a track point: `track_sounds[flags & 0x1f]` (0x407c10).
    pub fn class_for(track_sounds: &[i8; 32], track_type: u8) -> i16 {
        track_sounds[(track_type & 0x1f) as usize] as i16
    }

    pub fn classes(&self) -> [i16; 3] {
        self.classes
    }

    /// One tick. `classes` = [16 behind, 4 ahead, 24 ahead]; `speed` is 0x857234 (m/s, the
    /// speed before this tick's physics); `odometer` in metres.
    pub fn tick(&mut self, classes: [i16; 3], speed: f32, odometer: f32, snd: &mut SoundSystem) {
        let mut entered = [0i16; 3];
        let mut left = [0i16; 3];
        for i in 0..3 {
            if classes[i] != self.classes[i] {
                entered[i] = classes[i];
                left[i] = self.classes[i];
            }
        }
        self.classes = classes;
        let odo = odometer as f64;
        let speed_volume = || (ftol(speed as f64 * 3.6 + 64.0) as i16).min(0x7f);

        for (class, code) in [(2, AMBIENT_CLASS2), (3, AMBIENT_CLASS3)] {
            if entered[1] == class {
                snd.request(code);
            }
            if left[1] == class {
                snd.release(code);
                if class == 2 {
                    snd.set_volume(code, 0x7f);
                }
            }
            if classes[1] == class {
                snd.set_volume(code, speed_volume());
            }
        }

        if entered[2] == 1 {
            self.anchor = ftol(odo + 25.0) as i16;
            self.state = 0;
            snd.request(AMBIENT_POINT);
            snd.set_pitch(AMBIENT_POINT, 1);
            snd.set_volume(AMBIENT_POINT, 1);
        }
        if classes[1] == 1 {
            self.anchor = ftol(odo) as i16;
            self.state = 1;
            snd.set_volume(AMBIENT_POINT, 0x7f);
        }
        if left[1] == 1 {
            self.anchor = ftol(odo) as i16;
            self.state = 2;
        } else if self.state == -1 {
            return;
        }

        let delta = self.anchor as f64 - odo;
        let dist = ftol(delta.abs());
        let near = |x: i32| -> i32 {
            if x > 0x7f {
                0x7f
            } else if x <= 0 {
                1
            } else {
                x
            }
        };
        let x = near(dist.wrapping_mul(2));
        match self.state {
            0 => snd.set_volume(AMBIENT_POINT, (x.wrapping_neg() & 0x7f) as i16),
            2 => {
                snd.set_pitch(AMBIENT_POINT, (x & 0x7f) as i16);
                let y = near(dist);
                snd.set_volume(AMBIENT_POINT, (y.wrapping_neg() & 0x7f) as i16);
                if delta < -64.0 {
                    snd.release(AMBIENT_POINT);
                    self.anchor = -1;
                    self.state = -1;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixer::Command;
    use std::sync::Arc;

    fn tables() -> SoundTables {
        let mut t = SoundTables::default();
        for s in t.stages.iter_mut() {
            *s = StageSounds {
                motor: 0x31,
                joint: 0x32,
                horn: 4,
                motor_base_pitch: 1,
            };
        }
        t.stages[7].horn = MELODY_HORN;
        t.stages[1].horn = HORN_NO_RELEASE;
        let row = JointPattern {
            period: 120,
            clicks: [12, 24, 94, 104],
            flags: 7,
        };
        t.joints = vec![row; 21];
        t.joints[5].flags = 4;
        t
    }

    fn setup(stage: usize, train_type: usize) -> (TrainSound, SoundSystem) {
        let tables = Arc::new(tables());
        let mut snd = SoundSystem::new(tables.clone());
        let ts = TrainSound::new(&tables, stage, train_type, &mut snd);
        assert_eq!(snd.take_commands(), [Command::SetPitch(0x31, 1)]);
        (ts, snd)
    }

    fn moving(speed: f32) -> TrainTick {
        TrainTick {
            prev_speed: speed,
            speed,
            ..TrainTick::default()
        }
    }

    #[test]
    fn modes_by_type() {
        assert_eq!(NoiseMode::for_train_type(0), NoiseMode::Standard);
        assert_eq!(NoiseMode::for_train_type(9), NoiseMode::Type9);
        assert_eq!(NoiseMode::for_train_type(5), NoiseMode::Type45);
        assert_eq!(NoiseMode::for_train_type(8), NoiseMode::Fixed);
    }

    #[test]
    fn segment_start_and_reset() {
        let (ts, mut snd) = setup(0, 0);
        ts.start_segment(&mut snd);
        assert_eq!(
            snd.take_commands(),
            [
                Command::Play(9),
                Command::SetPitch(9, 0x7f),
                Command::SetVolume(9, 1),
                Command::Play(0x31),
                Command::SetPitch(0x31, 1),
            ]
        );
        ts.reset_sounds(&mut snd);
        assert_eq!(
            snd.take_commands(),
            [Command::SetVolume(9, 0x7f), Command::SetPitch(0x31, 1)]
        );
    }

    #[test]
    fn horn_edges() {
        let (mut ts, mut snd) = setup(0, 0);
        let mut t = TrainTick {
            driving: false,
            horn: HornEdge::Pressed,
            ..TrainTick::default()
        };
        ts.tick(&t, &mut snd);
        t.horn = HornEdge::Released;
        ts.tick(&t, &mut snd);
        let cmds = snd.take_commands();
        assert!(cmds.contains(&Command::Play(4)) && cmds.contains(&Command::Release(4)));
        t.horn_enabled = false;
        t.horn = HornEdge::Pressed;
        ts.tick(&t, &mut snd);
        assert!(!snd.take_commands().contains(&Command::Play(4)));

        // Code 8 is never released.
        let (mut ts, mut snd) = setup(1, 0);
        t.horn_enabled = true;
        t.horn = HornEdge::Released;
        ts.tick(&t, &mut snd);
        assert!(!snd.take_commands().contains(&Command::Release(8)));
    }

    #[test]
    fn melody_horn_rate_limit() {
        let (mut ts, mut snd) = setup(7, 0);
        let mut presses = Vec::new();
        for tick in 0..250 {
            let t = TrainTick {
                driving: false,
                horn: HornEdge::Pressed,
                ..TrainTick::default()
            };
            ts.tick(&t, &mut snd);
            if snd.take_commands().contains(&Command::Play(MELODY_HORN)) {
                presses.push(tick);
            }
        }
        // Playable once the counter exceeds 99, then every 100 ticks.
        assert_eq!(presses, [100, 200]);
    }

    #[test]
    fn brake_air() {
        let (mut ts, mut snd) = setup(0, 0);
        let t = TrainTick {
            driving: false,
            brake_moving_ticks: 1,
            prev_brake: 3,
            brake: 0,
            ..TrainTick::default()
        };
        ts.tick(&t, &mut snd);
        assert_eq!(
            snd.take_commands(),
            [
                Command::Play(0x14),
                Command::Play(0x13),
                Command::Release(0x1c)
            ]
        );
        // Type 5's flags lack the brake-air bit.
        let (mut ts, mut snd) = setup(0, 5);
        ts.tick(&t, &mut snd);
        assert_eq!(snd.take_commands(), [Command::Release(0x1c)]);
    }

    #[test]
    fn stopped_resets_noise() {
        let (mut ts, mut snd) = setup(0, 5);
        ts.tick(&TrainTick::default(), &mut snd);
        assert_eq!(
            snd.take_commands(),
            [
                Command::Release(0x1c),
                Command::SetVolume(9, 1),
                Command::SetPitch(9, 0x7f),
                Command::SetPitch(0x31, 1),
            ]
        );
    }

    #[test]
    fn motor_pitch_follows_speed() {
        let (mut ts, mut snd) = setup(0, 5);
        // 10 m/s = 36 km/h: 36·34.6597·1.149425287 = 1434.19, /3000·127 = 60.7 → 60.
        // Type 5 uses 0.0005: 1434.19·0.0005·127 = 91.07 → 91.
        ts.tick(&moving(10.0), &mut snd);
        assert_eq!(ts.params().0, 91);
        let (mut ts, mut snd) = setup(0, 0);
        ts.tick(&moving(10.0), &mut snd);
        assert_eq!(ts.params().0, 60);
        assert!(snd.take_commands().contains(&Command::SetPitch(0x31, 60)));
        // Clamped to 1..=127, and scaled by the traction fade.
        ts.tick(&moving(100.0), &mut snd);
        assert_eq!(ts.params().0, 127);
        let t = TrainTick {
            traction_fade: 0.0,
            ..moving(10.0)
        };
        ts.tick(&t, &mut snd);
        assert_eq!(ts.params().0, 1);
    }

    #[test]
    fn running_noise_slews() {
        let (mut ts, mut snd) = setup(0, 0);
        let t = TrainTick {
            prev_power: 3,
            ..moving(2.0)
        };
        // Volume target 2·32 = 64: rises by one per tick from 1.
        for i in 0..10 {
            ts.tick(&t, &mut snd);
            assert_eq!(ts.params().1, 2 + i);
        }
        // Pitch target 128 − 7.2/140·127 = 121.47; from 127 it steps down: on ticks where
        // stage_tick % (1 + trunc(2/8.5)) == 0 (every tick here) → −1 per tick.
        let (mut ts, mut snd) = setup(0, 0);
        let mut pitches = Vec::new();
        for tick in 0..8 {
            ts.tick(
                &TrainTick {
                    stage_tick: tick,
                    ..t
                },
                &mut snd,
            );
            pitches.push(ts.params().2);
        }
        assert_eq!(pitches, [126, 125, 124, 123, 122, 122, 122, 122]);
        // Coasting: pitch +3 (below 0x70) and volume −5 (above 0x40) each tick.
        let (mut ts, mut snd) = setup(0, 0);
        ts.noise_pitch = 0x10;
        ts.noise_volume = 0x60;
        ts.tick(&moving(2.0), &mut snd);
        // +1 toward the target, +3 idle; −1 toward 64, −5 idle.
        assert_eq!((ts.params().1, ts.params().2), (0x60 - 6, 0x10 + 4));
    }

    #[test]
    fn fixed_mode_and_type45_power() {
        let (mut ts, mut snd) = setup(0, 8);
        ts.tick(&moving(10.0), &mut snd);
        assert_eq!(ts.params().2, 0x7f);
        let (mut ts, mut snd) = setup(0, 4);
        let t = TrainTick {
            prev_power: 2,
            ..moving(10.0)
        };
        ts.tick(&t, &mut snd);
        assert_eq!(ts.params().1, 0x70);
    }

    #[test]
    fn rail_joints() {
        let (mut ts, mut snd) = setup(0, 0);
        let mut clicks = Vec::new();
        // 5 m/s along one 12 m pattern in 0.5 m steps.
        for step in 0..=48 {
            let t = TrainTick {
                odometer: step as f32 * 0.5,
                ..moving(5.0)
            };
            ts.tick(&t, &mut snd);
            if snd.take_commands().contains(&Command::Play(0x32)) {
                clicks.push(step as f32 * 0.5);
            }
        }
        // Clicks at 1.2, 2.4, 9.4, 10.4 m (first tick at or past each phase), then the next
        // period restarts once the phase falls back below the first click.
        assert_eq!(clicks, [1.5, 2.5, 9.5, 10.5, 13.5, 14.5, 21.5, 22.5]);

        // At ≥ 80 km/h only two clicks per period.
        let (mut ts, mut snd) = setup(0, 0);
        let mut n = 0;
        for step in 0..=24 {
            let t = TrainTick {
                odometer: step as f32 * 0.5,
                ..moving(25.0)
            };
            ts.tick(&t, &mut snd);
            n += snd
                .take_commands()
                .iter()
                .filter(|c| **c == Command::Play(0x32))
                .count();
        }
        assert_eq!(n, 2);
    }

    #[test]
    fn flange_squeal_and_departure() {
        let (mut ts, mut snd) = setup(0, 0);
        let mut squeals = Vec::new();
        for tick in 0..200 {
            let t = TrainTick {
                curve_sharpness: 3.0,
                odometer: 0.0,
                ..moving(10.0)
            };
            ts.tick(&t, &mut snd);
            if snd.take_commands().contains(&Command::Play(0x19)) {
                squeals.push(tick);
            }
        }
        assert_eq!(squeals, [80, 161]);
        let t = TrainTick {
            prev_speed: 0.0,
            speed: 0.1,
            ..TrainTick::default()
        };
        ts.tick(&t, &mut snd);
        assert!(snd.take_commands().contains(&Command::Play(0x30)));
    }

    #[test]
    fn ambient_speed_classes() {
        let mut snd = SoundSystem::new(Arc::new(tables()));
        let mut a = AmbientSound::new();
        a.state = -1;
        a.tick([0, 2, 0], 10.0, 0.0, &mut snd);
        // 10 m/s → trunc(36 + 64) = 100.
        assert_eq!(
            snd.take_commands(),
            [Command::Play(0x2e), Command::SetVolume(0x2e, 100)]
        );
        a.tick([0, 2, 0], 30.0, 0.0, &mut snd);
        assert_eq!(snd.take_commands(), [Command::SetVolume(0x2e, 0x7f)]);
        a.tick([0, 3, 0], 10.0, 0.0, &mut snd);
        assert_eq!(
            snd.take_commands(),
            [
                Command::Release(0x2e),
                Command::SetVolume(0x2e, 0x7f),
                Command::Play(0x29),
                Command::SetVolume(0x29, 100),
            ]
        );
        a.tick([0, 0, 0], 10.0, 0.0, &mut snd);
        assert_eq!(snd.take_commands(), [Command::Release(0x29)]);
    }

    #[test]
    fn ambient_point_sound() {
        let mut snd = SoundSystem::new(Arc::new(tables()));
        let mut a = AmbientSound::new();
        a.state = -1;
        // Approach: class 1 appears 24 points ahead at odometer 100 → anchor 125.
        a.tick([0, 0, 1], 10.0, 100.0, &mut snd);
        assert_eq!(
            snd.take_commands(),
            [
                Command::Play(0x2c),
                Command::SetPitch(0x2c, 1),
                Command::SetVolume(0x2c, 1),
                // distance 25 → x = 50 → volume 128 − 50.
                Command::SetVolume(0x2c, 78),
            ]
        );
        a.tick([0, 0, 1], 10.0, 120.0, &mut snd);
        assert_eq!(snd.take_commands(), [Command::SetVolume(0x2c, 118)]);
        // At the point: full volume, then hold.
        a.tick([0, 1, 1], 10.0, 125.0, &mut snd);
        assert_eq!(snd.take_commands(), [Command::SetVolume(0x2c, 0x7f)]);
        a.tick([0, 0, 0], 10.0, 130.0, &mut snd);
        // Left at 130: anchor 130, distance 0 → pitch 1, volume 127.
        assert_eq!(
            snd.take_commands(),
            [Command::SetPitch(0x2c, 1), Command::SetVolume(0x2c, 127)]
        );
        a.tick([0, 0, 0], 10.0, 150.0, &mut snd);
        // 20 m: pitch 40, volume 128 − 20.
        assert_eq!(
            snd.take_commands(),
            [Command::SetPitch(0x2c, 40), Command::SetVolume(0x2c, 108)]
        );
        a.tick([0, 0, 0], 10.0, 194.5, &mut snd);
        assert_eq!(
            snd.take_commands(),
            [
                Command::SetPitch(0x2c, 0x7f),
                Command::SetVolume(0x2c, 128 - 64),
                Command::Release(0x2c),
            ]
        );
        a.tick([0, 0, 0], 10.0, 200.0, &mut snd);
        assert!(snd.take_commands().is_empty());
    }

    #[test]
    fn ambient_anchor_wraps_like_i16() {
        let mut snd = SoundSystem::new(Arc::new(tables()));
        let mut a = AmbientSound::new();
        a.tick([0, 0, 1], 10.0, 40_000.0, &mut snd);
        // trunc(40025) as i16 = −25511: the distance saturates, volume 1.
        assert_eq!(a.anchor, (40_025i32 as u16) as i16);
        assert_eq!(
            snd.take_commands().last(),
            Some(&Command::SetVolume(0x2c, 1))
        );
    }

    /// Runs a clatter task to its end; returns the update index of every Play.
    fn clatter(speed: f32, flange: bool) -> Vec<(usize, u16)> {
        let mut snd = SoundSystem::new(Arc::new(tables()));
        let mut c = TurnoutClatter::new(speed);
        let mut out = Vec::new();
        for i in 0..10_000 {
            let alive = c.tick(0x32, flange, &mut snd);
            out.extend(snd.take_commands().into_iter().filter_map(|cmd| match cmd {
                Command::Play(code) => Some((i, code)),
                _ => None,
            }));
            if !alive {
                return out;
            }
        }
        panic!("clatter never ended");
    }

    #[test]
    fn turnout_clatter() {
        // 30 m/s = 1 m per tick. Wheel sets at 0 / −2.5 / −14.5 m click at 0, 4, 7; 3, 7, 10;
        // 15, 19 m travelled (the add follows the click test; at most three clicks each), and
        // the task ends once the last set has clicked twice.
        let p = clatter(30.0, true);
        let ticks: Vec<usize> = p.iter().map(|x| x.0).collect();
        assert_eq!(ticks, [0, 3, 4, 7, 7, 10, 15, 19]);
        assert!(p.iter().all(|x| x.1 == 0x32));
        // Below 40 km/h with the flange flag: squeal at the end.
        let p = clatter(10.0, true);
        assert_eq!(p.len(), 9);
        assert_eq!(p.last().unwrap().1, TURNOUT_SQUEAL);
        assert_eq!(clatter(10.0, false).len(), 8);
    }
}

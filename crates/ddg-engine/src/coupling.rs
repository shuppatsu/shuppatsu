//! The coupling minigame (`Coupling_Run` 0x4093c0, docs/re/rules.md §13).
//!
//! After a perfect stop at the coupling station (stage 2 station 24, stages 17–19 station 16)
//! the stop handler 0x40a0a0 runs a blocking loop: captions, a 270-tick intro with the train
//! held, then up to 1801 ticks in which the player drives the train the last 40 m onto the
//! waiting set and must couple at 0.5–2.5 km/h. Points (bonus seconds) come from sounding the
//! horn before moving off (+3), stopping at the 4.5–6.5 m mark (+5), the horn again after that
//! stop (+2) and the coupling speed (up to +10). Every loop iteration ends with one
//! `Task_Tick`, so one [`Coupling::step`] is one game tick.
//!
//! This module only keeps the minigame's own state; [`crate::rules::Rules`] runs it and the
//! train physics keep driving through [`crate::drive::Drive`] while [`Coupling::driving`] is
//! true. The sprite layout (slots 0x3f–0x4b) is presentation and lives in the game crate.

/// Ticks before the train may move: `Task_Tick` ×10 (0x425490), ×0x50, then the 0xb4-tick
/// sound loop.
pub const INTRO_TICKS: u32 = 10 + 0x50 + 0xb4;
/// The first caption (tbl 0x2d8 ボーナスゲーム) is up at tick 0, the instruction (tbl 0x2d9
/// 時間内に衝撃なく列車を連結せよ) at tick 90.
pub const CAPTION_TICK: u32 = 10 + 0x50;
/// Sprites and BGM 0x19a come after the first 10 ticks.
pub const SETUP_TICK: u32 = 10;
/// Time limit, ticks (`local_14 = 0x709`, shown with 0x417040).
pub const TIME_LIMIT_TICKS: i32 = 0x709;

/// The coupling point lies this far past the stop marker (f64 40.0 at 0x4594b0).
const TARGET_OFFSET_M: f64 = 40.0;
/// Distance calls (f64 at 0x458c48, 0x459490, 0x4594a0, 0x459478, 0x459470, 0x459468,
/// 0x457718): 8, 7, 6.5, (miss the mark below) 4.3, 3, 2, 1 m.
const CALL_8M: f64 = 8.0;
const CALL_7M: f64 = 7.0;
const MARK_FAR_M: f64 = 6.5;
const MARK_NEAR_M: f64 = 4.5;
const MARK_MISSED_M: f64 = 4.3;
const CALL_3M: f64 = 3.0;
const CALL_2M: f64 = 2.0;
const CALL_1M: f64 = 1.0;
/// "Standing still" at the mark: |speed| < 0.0001 m/s (f64 at 0x459480).
const STANDSTILL_MPS: f64 = 0.0001;
/// Warning chime zone: 9 m and more, or 3.2–6.5 m once stopped at the mark (f64 at 0x4594a8,
/// 0x459498), every 90 ticks (`local_14 % 0x5a`) with sound 0x37, 0x3f 70 ticks later.
const WARN_FAR_M: f64 = 9.0;
const WARN_NEAR_M: f64 = 3.2;
const WARN_PERIOD: i32 = 0x5a;
/// Coupling speed window, m/s: 0.5–2.5 km/h (f64 0.13888… at 0x459450, 0.69444… at 0x459448).
const COUPLE_MIN_MPS: f64 = 0.138_888_888_888_888_9;
const COUPLE_MAX_MPS: f64 = 0.694_444_444_444_444_4;
/// Speed points: `10 − trunc((v − 0.13888…) × 18.1818…)` (f64 at 0x459440).
const SPEED_POINTS_SCALE: f64 = 18.181_818_181_818_183;

/// Points for the horn before moving off, at the mark stop, after the mark stop.
const HORN_FIRST_POINTS: i32 = 3;
const MARK_POINTS: i32 = 5;
const HORN_AFTER_MARK_POINTS: i32 = 2;
const SPEED_POINTS: i32 = 10;

/// Outro lengths: success 200 ticks (0x4093c0 loop) and the cleanup tick; too fast and time-up
/// `0x425490(0x14)`; too weak `0x425490(100)`.
const SUCCESS_TICKS: u32 = 200;
const SHORT_OUTRO_TICKS: u32 = 0x14;
const WEAK_OUTRO_TICKS: u32 = 100;
/// In the success outro: the coupler frame switches at 60, the bonus message at 120.
const SUCCESS_COUPLED_AT: u32 = 60;
const SUCCESS_BONUS_AT: u32 = 120;
/// Sounds of the success outro (tick, code).
const SUCCESS_SOUNDS: [(u32, u16); 4] = [(40, 0x13), (58, 0x10), (65, 0xe), (95, 0x3e)];
/// Sounds of the intro's 0xb4-tick loop (loop tick, code).
const INTRO_SOUNDS: [(u32, u16); 6] = [
    (15, 0x11),
    (30, 0x34),
    (60, 0x3f),
    (70, 0x40),
    (130, 0x36),
    (175, 0x3f),
];
/// Camera jolts (0x40e370 with f32 −0.0005 / −0.004).
pub const JOLT_SOFT: f32 = -0.0005;
pub const JOLT_HARD: f32 = -0.004;
/// Navigator hint "the coupling was too weak" (0x411f20(0xf, 0x5a, 0)).
pub const WEAK_HINT: i16 = 0xf;
pub const WEAK_HINT_TICKS: i32 = 0x5a;

/// How the minigame ended; [`CouplingResult::code`] is 0x4093c0's return value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CouplingResult {
    /// Coupled at 0.5–2.5 km/h: 連結成功, returns 1.
    Coupled,
    /// Faster than 2.5 km/h: 連結失敗 with a hard jolt, returns 0.
    TooFast,
    /// Slower than 0.5 km/h: 連結失敗 and hint 0xf, returns −1.
    TooWeak,
    /// The time ran out: returns 0.
    TimeUp,
}

impl CouplingResult {
    pub fn code(self) -> i32 {
        match self {
            Self::Coupled => 1,
            Self::TooFast | Self::TimeUp => 0,
            Self::TooWeak => -1,
        }
    }
}

/// Where the minigame is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CouplingPhase {
    /// Captions and the countdown voice; the train is held. Ticks done so far.
    Intro(u32),
    /// The player drives (`0x855ff8 | 0x200`).
    Running,
    /// The result is shown; ticks done so far.
    Outro(CouplingResult, u32),
    Done(CouplingResult),
}

/// Something the presentation reacts to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CouplingEvent {
    /// `Sfx_Play(code)` (0x41fda0).
    Sound(u16),
    /// The result is decided (sprites 0x17f / 0x180 and 連結成功 0x2dc / 連結失敗 0x2dd).
    Result(CouplingResult),
    /// Camera jolt (0x40e370), see [`JOLT_SOFT`] / [`JOLT_HARD`].
    Jolt(f32),
}

/// The minigame's state (the locals of 0x4093c0).
#[derive(Debug, Clone, PartialEq)]
pub struct Coupling {
    pub phase: CouplingPhase,
    /// Ticks left (`local_14`).
    pub timer: i32,
    /// Driving ticks so far (`local_10`).
    pub run_ticks: i32,
    /// Bonus seconds collected (`local_18`).
    pub points: i32,
    /// Horn state (`iVar6`): 0 = the opening horn still scores, 1 = stopped at the mark (the
    /// second horn scores), −1 = nothing left to score.
    pub horn: i8,
    /// Distance calls made (`iVar7`, 0..=7; 4 = at or past the mark).
    pub calls: u8,
    /// Distance to the coupling point, m, clamped at 0 (`0x85708c` during the game).
    pub dist_m: f32,
    /// `trunc` of the unclamped distance (`0x85722c` during the game).
    pub dist_trunc: i32,
    /// Frame of the waiting set's coupler (`0x857244`), 1 once coupled or timed out.
    pub coupler_frame: u8,
}

/// The rules' inputs for one minigame tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CouplingInput {
    /// Speed after the last tick, m/s (`0x857234`).
    pub speed_mps: f32,
    /// Distance to the stop marker from the camera, m (`0x8500c8 × 10`).
    pub dist_to_stop_m: f32,
    /// Horn pressed this tick (edge bit 0x100 of 0x48c42c).
    pub horn_pressed: bool,
}

/// What one tick asks of the rules.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CouplingTick {
    pub events: Vec<CouplingEvent>,
    /// Zero the speed before the physics step (`0x857234 = 0`).
    pub stop_train: bool,
    /// The bonus seconds to show and add this tick (TimeMsg kind 0x103).
    pub bonus_s: Option<i32>,
    /// Hint to show (id, ticks).
    pub hint: Option<(i16, i32)>,
}

impl Default for Coupling {
    fn default() -> Self {
        Self::new()
    }
}

impl Coupling {
    pub fn new() -> Self {
        Self {
            phase: CouplingPhase::Intro(0),
            timer: TIME_LIMIT_TICKS,
            run_ticks: 0,
            points: 0,
            horn: 0,
            calls: 0,
            dist_m: TARGET_OFFSET_M as f32,
            dist_trunc: 0,
            coupler_frame: 0,
        }
    }

    /// The driving flag 0x200 is set: the train task runs on the player's notches.
    pub fn driving(&self) -> bool {
        self.phase == CouplingPhase::Running
    }

    pub fn result(&self) -> Option<CouplingResult> {
        match self.phase {
            CouplingPhase::Done(r) => Some(r),
            _ => None,
        }
    }

    /// One tick (one `Task_Tick` of 0x4093c0).
    pub fn step(&mut self, input: &CouplingInput) -> CouplingTick {
        let mut out = CouplingTick::default();
        match self.phase {
            CouplingPhase::Intro(t) => {
                if let Some(&(_, code)) = INTRO_SOUNDS
                    .iter()
                    .find(|&&(at, _)| t >= CAPTION_TICK && at == t - CAPTION_TICK)
                {
                    out.events.push(CouplingEvent::Sound(code));
                }
                self.phase = if t + 1 >= INTRO_TICKS {
                    CouplingPhase::Running
                } else {
                    CouplingPhase::Intro(t + 1)
                };
            }
            CouplingPhase::Running => self.run(input, &mut out),
            CouplingPhase::Outro(r, t) => self.outro(r, t, &mut out),
            CouplingPhase::Done(_) => {}
        }
        out
    }

    /// One iteration of the driving loop (0x4096f6..0x409c88).
    fn run(&mut self, input: &CouplingInput, out: &mut CouplingTick) {
        // fld 0x85708c; fadd 40.0; fst 0x85708c; ftol (of the unrounded sum).
        let d = f64::from(input.dist_to_stop_m) + TARGET_OFFSET_M;
        self.dist_m = d as f32;
        self.dist_trunc = d as i32;
        if self.dist_m < 0.0 {
            self.dist_m = 0.0;
        }
        let d = f64::from(self.dist_m);
        let v = f64::from(input.speed_mps);
        if v <= 0.0 && self.horn == 0 && input.horn_pressed {
            self.points += HORN_FIRST_POINTS;
            self.horn = -1;
        }
        if v <= 0.0 && self.horn == 1 && input.horn_pressed {
            self.points += HORN_AFTER_MARK_POINTS;
            self.horn = -1;
        }
        if v > 0.0 {
            self.horn = -1;
        }
        let at_mark =
            self.run_ticks > 30 && self.calls == 4 && (WARN_NEAR_M..=MARK_FAR_M).contains(&d);
        if d >= WARN_FAR_M || at_mark {
            if self.timer % WARN_PERIOD == 0 {
                out.events.push(CouplingEvent::Sound(0x37));
            }
            if self.timer % WARN_PERIOD == 0x46 {
                out.events.push(CouplingEvent::Sound(0x3f));
            }
        }
        let mut call = |reached: bool, from: u8, code: u16| {
            if reached && self.calls == from {
                out.events.push(CouplingEvent::Sound(code));
                self.calls = from + 1;
            }
        };
        call(d <= CALL_8M, 0, 0x38);
        call(d <= CALL_7M, 1, 0x11);
        call(d <= MARK_FAR_M, 2, 0x39);
        if (MARK_NEAR_M..=MARK_FAR_M).contains(&d) && v.abs() < STANDSTILL_MPS && self.calls == 3 {
            out.events.push(CouplingEvent::Sound(0x3a));
            self.points += MARK_POINTS;
            self.calls = 4;
            self.horn = 1;
        }
        if d <= MARK_MISSED_M && self.calls == 3 {
            self.calls = 4;
        }
        let mut call = |reached: bool, from: u8, code: u16| {
            if reached && self.calls == from {
                out.events.push(CouplingEvent::Sound(code));
                self.calls = from + 1;
            }
        };
        call(d <= CALL_3M, 4, 0x3b);
        call(d <= CALL_2M, 5, 0x3c);
        call(d <= CALL_1M, 6, 0x3d);

        if d <= 0.0 {
            let result = if v < COUPLE_MIN_MPS {
                CouplingResult::TooWeak
            } else if v <= COUPLE_MAX_MPS {
                let p = (v - COUPLE_MIN_MPS) * SPEED_POINTS_SCALE;
                self.points += SPEED_POINTS - p as i32;
                if self.points == 0 {
                    self.points = 1;
                }
                CouplingResult::Coupled
            } else {
                CouplingResult::TooFast
            };
            out.events.push(CouplingEvent::Result(result));
            match result {
                CouplingResult::Coupled => {
                    out.events.push(CouplingEvent::Sound(0x2a));
                    out.events.push(CouplingEvent::Jolt(JOLT_SOFT));
                }
                CouplingResult::TooFast => {
                    out.events.push(CouplingEvent::Sound(0x2b));
                    out.events.push(CouplingEvent::Sound(0x2f));
                    out.events.push(CouplingEvent::Jolt(JOLT_HARD));
                    self.points = 0;
                }
                _ => {
                    out.events.push(CouplingEvent::Sound(0x2b));
                    out.hint = Some((WEAK_HINT, WEAK_HINT_TICKS));
                    self.points = 0;
                }
            }
            self.outro(result, 0, out);
        } else if self.timer < 1 {
            self.coupler_frame = 1;
            out.events
                .push(CouplingEvent::Result(CouplingResult::TimeUp));
            self.outro(CouplingResult::TimeUp, 0, out);
        } else {
            self.run_ticks += 1;
            self.timer -= 1;
        }
    }

    /// The ticks after the result, the cleanup's `Task_Tick` included.
    fn outro(&mut self, r: CouplingResult, t: u32, out: &mut CouplingTick) {
        out.stop_train = true;
        let len = match r {
            CouplingResult::Coupled => SUCCESS_TICKS,
            CouplingResult::TooWeak => WEAK_OUTRO_TICKS,
            _ => SHORT_OUTRO_TICKS,
        };
        if r == CouplingResult::Coupled {
            if let Some(&(_, code)) = SUCCESS_SOUNDS.iter().find(|&&(at, _)| at == t) {
                out.events.push(CouplingEvent::Sound(code));
            }
            if t == SUCCESS_COUPLED_AT {
                self.coupler_frame = 1;
            }
            if t == SUCCESS_BONUS_AT {
                out.bonus_s = Some(self.points);
            }
        }
        // `len` loop ticks, then the cleanup tick.
        self.phase = if t >= len {
            CouplingPhase::Done(r)
        } else {
            CouplingPhase::Outro(r, t + 1)
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(v_kmh: f32, d: f32, horn: bool) -> CouplingInput {
        CouplingInput {
            speed_mps: v_kmh / 3.6,
            dist_to_stop_m: d - 40.0,
            horn_pressed: horn,
        }
    }

    fn skip_intro(c: &mut Coupling) {
        for _ in 0..INTRO_TICKS {
            assert!(!c.driving());
            c.step(&input(0.0, 40.0, false));
        }
        assert!(c.driving());
    }

    fn finish(c: &mut Coupling) -> (CouplingResult, u32, Option<i32>) {
        let mut n = 0;
        let mut bonus = None;
        while c.result().is_none() {
            let t = c.step(&input(0.0, 0.0, false));
            bonus = bonus.or(t.bonus_s);
            n += 1;
        }
        (c.result().unwrap(), n, bonus)
    }

    #[test]
    fn perfect_game_scores_twenty() {
        let mut c = Coupling::new();
        skip_intro(&mut c);
        c.step(&input(0.0, 40.0, true));
        assert_eq!(c.points, 3);
        // Creep to the mark and stop there.
        c.step(&input(3.0, 20.0, false));
        c.step(&input(1.0, 8.0, false));
        c.step(&input(1.0, 7.0, false));
        c.step(&input(1.0, 6.4, false));
        assert_eq!(c.calls, 3);
        c.step(&input(0.0, 5.5, false));
        assert_eq!((c.calls, c.points, c.horn), (4, 8, 1));
        c.step(&input(0.0, 5.5, true));
        assert_eq!(c.points, 10);
        // 0.5 km/h at the coupler: the full 10.
        let t = c.step(&input(0.5, 0.0, false));
        assert!(
            t.events
                .contains(&CouplingEvent::Result(CouplingResult::Coupled))
        );
        assert_eq!(c.points, 20);
        let (r, n, bonus) = finish(&mut c);
        assert_eq!(
            (r, n, bonus),
            (CouplingResult::Coupled, SUCCESS_TICKS, Some(20))
        );
        assert_eq!(r.code(), 1);
    }

    #[test]
    fn too_fast_and_too_weak() {
        let mut c = Coupling::new();
        skip_intro(&mut c);
        c.step(&input(5.0, 0.0, false));
        assert_eq!(
            finish(&mut c),
            (CouplingResult::TooFast, SHORT_OUTRO_TICKS, None)
        );
        let mut c = Coupling::new();
        skip_intro(&mut c);
        let t = c.step(&input(0.2, 0.0, false));
        assert_eq!(t.hint, Some((WEAK_HINT, WEAK_HINT_TICKS)));
        assert_eq!(c.result(), None);
        assert_eq!(finish(&mut c).0, CouplingResult::TooWeak);
    }

    #[test]
    fn time_runs_out_after_the_limit() {
        let mut c = Coupling::new();
        skip_intro(&mut c);
        let mut ticks = 0;
        while c.driving() {
            c.step(&input(0.0, 30.0, false));
            ticks += 1;
        }
        assert_eq!(ticks, TIME_LIMIT_TICKS + 1);
        assert_eq!(c.coupler_frame, 1);
        assert_eq!(finish(&mut c).0, CouplingResult::TimeUp);
    }

    #[test]
    fn minimum_speed_points_round_up_to_one() {
        let mut c = Coupling::new();
        skip_intro(&mut c);
        c.step(&input(2.4, 0.0, false));
        assert_eq!(c.points, 1);
    }
}

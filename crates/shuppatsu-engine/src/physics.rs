//! Train dynamics: notches → forces → speed → distance, one fixed 1/30 s tick at a time.
//!
//! This reproduces the player-train task of the 3000-bandai `dengo3k.exe` (task update
//! 0x422c20, created by 0x4229c0 in task list 2). The formulas, constants and their addresses
//! are documented in `docs/re/physics.md`; this module is a fresh implementation of that
//! behaviour, not a translation of the original code.
//!
//! # Numerics
//! The original keeps intermediate values on the x87 stack in double precision (the oracle
//! logs control word 0x027f under Wine) and rounds to `f32` only where it stores a value to a
//! float variable. This module computes in `f64` and converts to `f32` at exactly those stores;
//! operation order follows the original. Comparisons that the original makes against the
//! unrounded stack value use the `f64`.
//!
//! # Units
//! Speed is in m/s (`km/h ÷ 3.6`), distance in metres, track position in track units
//! (10 m). Forces are in the game's own unit: `accel [m/s²] = force × 0.032425421 × 0.27777777`.

use shuppatsu_formats::Result;
use shuppatsu_formats::exe::Exe;
use shuppatsu_formats::route::Route;
use shuppatsu_formats::scene::TrackPos;

/// Fixed simulation rate: 30 ticks per game second (main-loop.md §3).
pub const TICKS_PER_SECOND: u32 = 30;

/// Number of train types in the type table (0x471ad8, stride 0x3c, ends at 0x471f88).
pub const TRAIN_TYPE_COUNT: usize = 20;
/// Stages, as in [`shuppatsu_formats::stages::STAGE_COUNT`].
pub const STAGE_COUNT: usize = shuppatsu_formats::stages::STAGE_COUNT;
/// Difficulties, as in [`shuppatsu_formats::stages::DIFFICULTY_COUNT`].
pub const DIFFICULTY_COUNT: usize = shuppatsu_formats::stages::DIFFICULTY_COUNT;

/// Highest power notch (P5). Input word 0x829d00.
pub const MAX_POWER_NOTCH: u8 = 5;
/// Emergency brake position of the brake handle (0x829d04). Positions 1..=8 are B1..B8.
pub const EMERGENCY_BRAKE: u8 = 9;
/// Highest value the brake input word can hold; 10 brakes like 9.
pub const MAX_BRAKE_INPUT: u8 = 10;

/// Train type table: 20 × 15 f32 (3000-bandai exe).
const TRAIN_TABLE_VA: u32 = 0x471ad8;
const TRAIN_RECORD_LEN: u32 = 0x3c;
/// Stage → train type, i32[20] (read by 0x4229c0 into 0x84c3fc).
const STAGE_TRAIN_VA: u32 = 0x472228;
/// Gradient-force scale per stage × difficulty, f32[80] (0x40d040 → 0x84c404).
const GRADE_SCALE_VA: u32 = 0x459b90;
/// Curve-resistance scale per stage × difficulty, f32[80] (0x40d040 → 0x84c42c).
const CURVE_SCALE_VA: u32 = 0x459a50;

// Constants of the force model. Each is read by 0x422c20 or one of its helpers; the address
// is where the value lives in the exe, the type is how the FPU loads it.
/// m/s → km/h (f64 @ 0x459420).
const MPS_TO_KMH: f64 = 3.6;
/// Running resistance below this speed is a flat 3.0 (f64 @ 0x458b88 = 10, f32 3.0 @ 0x422fe7).
const RESISTANCE_LOW_SPEED_KMH: f64 = 10.0;
const RESISTANCE_LOW: f64 = 3.0;
/// Running resistance `0.0164 v + ((drag + 1) × 0.0078 + 0.028) v² / mass + 1.32`
/// (f64 @ 0x472618, 0x457718, 0x472628, 0x472620, 0x472610).
const RES_LINEAR: f64 = 0.0164;
const RES_DRAG_OFFSET: f64 = 1.0;
const RES_DRAG_SCALE: f64 = 0.0078;
const RES_QUAD_BASE: f64 = 0.028;
const RES_CONSTANT: f64 = 1.32;
/// Speed-dependent extra resistance (0x422860): k = max_kmh × 0.2777777 × 1.02,
/// `5 ((k² − v²)/k²)² + 0.2` (f64 @ 0x4725e0, 0x4725d8, 0x4594f8, 0x458b30).
const KMH_TO_MPS_A: f64 = 0.277_777_7;
const EXTRA_RES_SPEED_MARGIN: f64 = 1.02;
const EXTRA_RES_GAIN: f64 = 5.0;
const EXTRA_RES_FLOOR: f64 = 0.2;
/// Traction (0x422610). f32 @ 0x459428 (10), 0x4725b4 (0.1), 0x457728 (1), 0x4725b0 (0.25),
/// 0x4725ac (6), 0x4725a8 (1.1), 0x472598 (1.25); f64 @ 0x457720 (1.2), 0x4725a0 (0.6f), 0x457718 (1).
const TAPER_SPAN_KMH: f64 = 10.0;
const TAPER_SLOPE: f64 = 0.1_f32 as f64;
const TAPER_FORCE_SHARE: f64 = 0.25;
const MID_FLAT_FORCE: f64 = 6.0;
const MID_SLOPE_GAIN: f64 = 1.1_f32 as f64;
const LOW_FORCE_GAIN: f64 = 1.25;
const LOW_SPEED_HEADROOM: f64 = 1.2;
const LOW_SPEED_BOOST: f64 = 0.6_f32 as f64;
/// Brake force (0x422750). f32 @ 0x4725d0; f64 @ 0x4725c8, 0x469820, 0x4725c0, 0x467b00,
/// 0x469798, 0x4725b8, 0x459530.
const KMH_TO_MPS_B: f32 = 0.277_777_7;
const BRAKE_LOAD_PER_UNIT: f64 = 0.0018;
const BRAKE_FADE_SLOPE: f64 = 0.18;
const BRAKE_FADE_BASE: f64 = 0.45;
const BRAKE_LOW_SPEED_GAIN: f64 = 0.8;
const BRAKE_SHAPE_SLOPE: f64 = 0.12;
const BRAKE_SHAPE_BASE: f64 = 0.9;
const SLIPPERY_BRAKE_FACTOR: f64 = 0.8;
const FORCED_STOP_BRAKE_FACTOR: f64 = 4.0;
/// Brake cylinder model (0x4230aa..0x423159): f64 @ 0x44e8e0 (0.1 per notch), 0x457720
/// (1.2 × rate when easing off), 0x469738 (1.3 × rate on release), 0x458c50 (−0.4 floor),
/// f32 −0.4 @ 0x423152 (0xbecccccd), f64 1e-5 @ 0x469720 (change detector).
const BRAKE_PER_NOTCH_STEP: f64 = 0.1;
const BRAKE_EASE_FACTOR: f64 = 1.2;
const BRAKE_RELEASE_FACTOR: f64 = 1.3;
const BRAKE_RELEASED: f64 = -0.4;
const BRAKE_RELEASED_F32: f32 = -0.4;
const BRAKE_CHANGE_EPSILON: f64 = 1e-5;
/// Traction fade in/out per tick (f64 0.02 @ 0x458bd0), bounds 1.0 / 0.0.
const TRACTION_RAMP_STEP: f64 = 0.02;
/// Curve term (0x4228b0): |Δ|heading|| × 700 capped at 200 (f64 @ 0x4725e8, 0x4594b8).
const CURVE_GAIN: f64 = 700.0;
const CURVE_CAP: f64 = 200.0;
/// Gradient term: scale × gradient × 2000; uphill × 0.8; ≥ 100 ignored (f64 @ 0x4726c8,
/// 0x467b00, 0x458b38).
const GRADE_GAIN: f64 = 2000.0;
const GRADE_UPHILL_FACTOR: f64 = 0.8;
const GRADE_GLITCH: f64 = 100.0;
/// Force → acceleration: × 0.032425421 × 0.27777777 (f64 @ 0x472608, 0x472600).
const FORCE_TO_KMH_PER_S: f64 = 0.032_425_421;
const KMH_TO_MPS_C: f64 = 0.277_777_77;
/// Seconds per tick (f64 0.03333333 @ 0x4725f8) and track units per metre per tick
/// (f64 0.003333333 @ 0x4726b8).
const TICK_SECONDS: f64 = 0.033_333_33;
const UNITS_PER_MPS_TICK: f64 = 0.003_333_333;
/// Speed cap test and value (f64 @ 0x4726c0, 0x4725e0).
const CAP_TEST_KMH_TO_MPS: f64 = 0.277_777;
/// Brake deceleration readout (f64 0.277777777 @ 0x4726b0).
const KMH_TO_MPS_D: f64 = 0.277_777_777;
/// ATC trip: `speed > (limit + 3) × 0.27777777`, reset below `limit × 0.27`
/// (f64 @ 0x459470, 0x472600, 0x4726d0). While tripped the brake is forced to B8.
const ATC_MARGIN_KMH: f64 = 3.0;
const ATC_RESET_FACTOR: f64 = 0.27;
const ATC_FORCED_BRAKE: u8 = 8;
/// ATC limit at route load: f32 1000.0 (0x4479c000 @ 0x4229c0).
pub const ATC_NO_LIMIT_KMH: f32 = 1000.0;
/// Coast-down while the title/result flags are set: −0.15 m/s per tick (f64 @ 0x458ae8).
const COAST_DECEL: f64 = 0.15;
/// Leg-distance marker in 0x40c4e0: stage rank ≤ 3, trunc(leg) ∈ {50, 51}, skip 2.5 m (f64 @ 0x458c00).
const LEG_MARKER_MAX_RANK: i32 = 3;
const LEG_MARKER_METRES: [i32; 2] = [50, 51];
const LEG_MARKER_SKIP: f64 = 2.5;
/// Gradient scale: track height units (1/4096) per raw height step (f64 @ 0x458c38).
const HEIGHT_STEP: f32 = 1.0 / 4096.0;

/// Factor at task+0x34, written as 1.0 every tick by 0x422740 (an unused adhesion hook).
const TRACTION_ADHESION: f32 = 1.0;

/// One train type: the 15 floats of a 0x3c-byte record at 0x471ad8.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrainSpec {
    /// +0x00: end of the low-speed traction region, km/h.
    pub base_speed_kmh: f32,
    /// +0x04..+0x14: balancing speed of P1..P5, km/h. The P5 entry is also the top speed.
    pub notch_speed_kmh: [f32; 5],
    /// +0x18: traction force at top speed (end of the linear mid region).
    pub force_top: f32,
    /// +0x1c: traction force at the base speed.
    pub force_base: f32,
    /// +0x20: brake cylinder change per tick.
    pub brake_rate: f32,
    /// +0x24: brake cylinder target per notch, ×0.1 (target = notch × 0.1 × this).
    pub brake_per_notch: f32,
    /// +0x28 and +0x2c: brake force gains (multiplied together).
    pub brake_gain: [f32; 2],
    /// +0x30: mass divisor of the quadratic running resistance.
    pub mass: f32,
    /// +0x34: drag coefficient of the quadratic running resistance.
    pub drag: f32,
    /// +0x38: deceleration-per-tick limit for the jolt check (rules; see physics.md).
    pub jolt_limit: f32,
}

impl TrainSpec {
    /// Top speed in km/h (the P5 balancing speed, +0x14).
    pub fn max_speed_kmh(&self) -> f32 {
        self.notch_speed_kmh[4]
    }

    /// Parses one record from its 15 little-endian floats.
    pub fn from_floats(f: [f32; 15]) -> Self {
        Self {
            base_speed_kmh: f[0],
            notch_speed_kmh: [f[1], f[2], f[3], f[4], f[5]],
            force_top: f[6],
            force_base: f[7],
            brake_rate: f[8],
            brake_per_notch: f[9],
            brake_gain: [f[10], f[11]],
            mass: f[12],
            drag: f[13],
            jolt_limit: f[14],
        }
    }
}

/// Per-stage physics parameters (set at stage start by 0x40d040 / 0x4229c0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StageParams {
    /// Train type (0x84c3fc).
    pub train_type: usize,
    /// Gradient-force scale (0x84c404).
    pub grade_scale: f32,
    /// Curve-resistance scale (0x84c42c).
    pub curve_scale: f32,
}

/// Everything the physics reads from the exe.
#[derive(Debug, Clone, PartialEq)]
pub struct TrainTables {
    pub specs: Vec<TrainSpec>,
    /// Train type per stage (0x472228).
    pub stage_train: Vec<usize>,
    /// Indexed `stage * 4 + difficulty`.
    pub grade_scale: Vec<f32>,
    /// Indexed `stage * 4 + difficulty`.
    pub curve_scale: Vec<f32>,
}

impl TrainTables {
    /// Reads the tables from the 3000-bandai exe.
    pub fn read(exe: &Exe<'_>) -> Result<Self> {
        let specs = (0..TRAIN_TYPE_COUNT as u32)
            .map(|t| {
                let b = exe.reader_at(
                    TRAIN_TABLE_VA + t * TRAIN_RECORD_LEN,
                    TRAIN_RECORD_LEN as usize,
                    "train table",
                )?;
                let mut f = [0f32; 15];
                for (i, v) in f.iter_mut().enumerate() {
                    *v = b.f32(4 * i)?;
                }
                Ok(TrainSpec::from_floats(f))
            })
            .collect::<Result<Vec<_>>>()?;
        let stage_train = exe
            .reader_at(STAGE_TRAIN_VA, 4 * STAGE_COUNT, "stage train table")
            .and_then(|b| {
                (0..STAGE_COUNT)
                    .map(|s| {
                        let t = b.i32(4 * s)?;
                        usize::try_from(t)
                            .ok()
                            .filter(|&t| t < TRAIN_TYPE_COUNT)
                            .ok_or_else(|| shuppatsu_formats::FormatError::Invalid {
                                what: "stage train table",
                                detail: format!("stage {s}: train type {t} out of range"),
                            })
                    })
                    .collect::<Result<Vec<_>>>()
            })?;
        let floats = |va: u32, what: &'static str| -> Result<Vec<f32>> {
            let n = STAGE_COUNT * DIFFICULTY_COUNT;
            let b = exe.reader_at(va, 4 * n, what)?;
            (0..n).map(|i| b.f32(4 * i)).collect()
        };
        Ok(Self {
            specs,
            stage_train,
            grade_scale: floats(GRADE_SCALE_VA, "grade scale table")?,
            curve_scale: floats(CURVE_SCALE_VA, "curve scale table")?,
        })
    }

    /// Parameters for `stage` (0..20) at `difficulty` (0..4, the settings value 0x829d10).
    pub fn stage_params(&self, stage: usize, difficulty: usize) -> Option<StageParams> {
        let i = stage * DIFFICULTY_COUNT + difficulty.min(DIFFICULTY_COUNT - 1);
        Some(StageParams {
            train_type: *self.stage_train.get(stage)?,
            grade_scale: *self.grade_scale.get(i)?,
            curve_scale: *self.curve_scale.get(i)?,
        })
    }
}

/// Handle positions as the game latched them (0x829d00 / 0x829d04), one tick before use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Controls {
    /// 0 = off, 1..=5 = P1..P5.
    pub power: u8,
    /// 0 = released, 1..=8 = B1..B8, 9 = EB (10 also brakes like EB).
    pub brake: u8,
}

/// Game state the physics reads but does not own. Defaults describe ordinary driving.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TickContext {
    /// Driving phase: flag 0x200 of 0x855ff8. When false the train is held at rest.
    pub driving: bool,
    /// Coast-down: flag 0x400 or 0x10 of 0x855ff8 (title/result screens). −0.15 m/s per tick.
    pub coast_down: bool,
    /// During coast-down: `trunc(units_to_stop × 10) ≤` the next timetable row's field +0x20
    /// (0x422f53). Stops the train at once.
    pub coast_hold: bool,
    /// 0x857206 ≥ 4 (overrun handling): power off, brake forced to EB, brake force × 4.
    pub forced_stop: bool,
    /// 0x856fba ≠ 0: power cut (course event 10/11), traction fades out at 0.02 per tick.
    pub power_cut: bool,
    /// 0x857230 > 1: slippery rail, brake force × 0.8.
    pub slippery: bool,
    /// ATC/speed-check limit in km/h (0x84c424, set by course event 7).
    pub atc_limit_kmh: f32,
    /// Gradient (0x857228), see [`gradient_at`]. Positive = uphill.
    pub gradient: f32,
    /// Heading of the camera's track chord in radians (0x850288), see [`heading_at`].
    pub heading: f32,
    /// A stage is selected (0x8570a4 ≥ 0). Without it gradient and curve terms are skipped.
    pub track_effects: bool,
    /// Brake load value of the current timetable row (0x857084, row field +0x10).
    pub brake_load: i32,
}

impl Default for TickContext {
    fn default() -> Self {
        Self {
            driving: true,
            coast_down: false,
            coast_hold: false,
            forced_stop: false,
            power_cut: false,
            slippery: false,
            atc_limit_kmh: ATC_NO_LIMIT_KMH,
            gradient: 0.0,
            heading: 0.0,
            track_effects: true,
            brake_load: 0,
        }
    }
}

/// Things a tick produced that other subsystems act on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StepEvents {
    /// The ATC tripped this tick (`speed > limit + 3 km/h`). The original then zeroes
    /// 0x856fb0 (rules: ends the run) and forces B8.
    pub atc_tripped: bool,
}

/// Player train state. Field comments give the original's storage (task = the 0x68-byte
/// train task, fields relative to its base).
#[derive(Debug, Clone, PartialEq)]
pub struct TrainState {
    /// Train type, index into [`TrainTables::specs`] (0x84c3fc → task+0x5c).
    pub train_type: usize,
    /// Gradient-force scale (0x84c404).
    pub grade_scale: f32,
    /// Curve-resistance scale (0x84c42c).
    pub curve_scale: f32,
    /// Traction scale (0x84c3f4, 1.0 in play).
    pub traction_scale: f32,
    /// Brake scale (0x84c438, 1.0 in play).
    pub brake_scale: f32,

    /// Speed, m/s (0x857234 / task+0x3c).
    pub speed: f32,
    /// Acceleration of the last tick, m/s² (task+0x40).
    pub accel: f32,
    /// Net force of the last tick (task+0x44).
    pub net_force: f32,
    /// Brake cylinder value (task+0x4c). −0.4 = fully released; ≤ 0 gives no force.
    pub brake_cylinder: f32,
    /// Traction fade factor 0..1 (task+0x38).
    pub traction_ramp: f32,
    /// Power notch used last tick (task+0x60).
    pub prev_power: i32,
    /// Effective brake notch used last tick, after overrides (task+0x64).
    pub prev_brake: i32,
    /// |heading| seen by the previous curve term (task+0x54).
    pub prev_abs_heading: f32,
    /// ATC trip latch (task+0x8c).
    pub atc_latched: bool,
    /// Ticks the brake cylinder has been moving (task+0x58; brake-air sound).
    pub brake_moving_ticks: i32,

    /// Distance along the stage, metres (0x857004).
    pub odometer: f32,
    /// Distance since the last station event, metres (0x84c40c).
    pub leg_distance: f32,
    /// Track units the camera advances this tick (0x850208).
    pub units_per_tick: f32,
    /// Gradient force of the last tick (0x84c3d8).
    pub grade_force: f32,
    /// Curve sharpness |Δ|heading|| × 700, ≤ 200 (0x84c408; also the flange-squeal trigger).
    pub curve_sharpness: f32,

    /// Speed two ticks back (0x84c41c).
    pub speed_prev2: f32,
    /// Speed lost over the previous tick, m/s (0x84c3f8; also copied to 0x84c3e8).
    pub decel_prev_tick: f32,
    /// Largest [`Self::decel_prev_tick`] so far (0x84c3f0).
    pub decel_prev_tick_max: f32,
    /// Brake deceleration per tick from the brake force alone (0x84c420 = 0x84c434 = 0x84c3e4).
    pub brake_decel: f32,
    /// Largest [`Self::brake_decel`] so far (0x84c3e0).
    pub brake_decel_max: f32,
    /// Brake notch history (0x84c410[3]), written at `clock % 3`.
    pub brake_history: [i32; 3],
    /// `prev_brake − brake_history[0]` (0x84c3ec).
    pub brake_history_delta: i32,
}

impl TrainState {
    /// State right after the route load (0x4229c0) for the given stage parameters.
    /// `heading` is 0x850288 at that moment (stored without `abs`, as the original does).
    pub fn new(params: StageParams, heading: f32) -> Self {
        Self {
            train_type: params.train_type,
            grade_scale: params.grade_scale,
            curve_scale: params.curve_scale,
            traction_scale: 1.0,
            brake_scale: 1.0,
            speed: 0.0,
            accel: 0.0,
            net_force: 0.0,
            brake_cylinder: BRAKE_RELEASED_F32,
            traction_ramp: 0.0,
            prev_power: 0,
            prev_brake: 0,
            prev_abs_heading: heading,
            atc_latched: false,
            brake_moving_ticks: 0,
            odometer: 0.0,
            leg_distance: 0.0,
            units_per_tick: 0.0,
            grade_force: 0.0,
            curve_sharpness: 0.0,
            speed_prev2: 0.0,
            decel_prev_tick: 0.0,
            decel_prev_tick_max: 0.0,
            brake_decel: 0.0,
            brake_decel_max: 0.0,
            brake_history: [0; 3],
            brake_history_delta: 0,
        }
    }

    /// Speed in km/h (as the game computes it: `speed × 3.6`, stored as f32).
    pub fn speed_kmh(&self) -> f32 {
        (f64::from(self.speed) * MPS_TO_KMH) as f32
    }

    /// Advances one tick. `spec` must be `tables.specs[self.train_type]`; `clock` is the stage
    /// clock 0x8570b0 (only its value mod 3 is used, for the brake history).
    ///
    /// Intermediate values are `f64` (the x87 stack in double precision); a value becomes `f32`
    /// exactly where the original stores it to a float variable.
    pub fn step(
        &mut self,
        spec: &TrainSpec,
        controls: Controls,
        ctx: &TickContext,
        clock: i32,
    ) -> StepEvents {
        let mut events = StepEvents::default();
        let mut power = i32::from(controls.power.min(MAX_POWER_NOTCH));
        let mut brake = i32::from(controls.brake.min(MAX_BRAKE_INPUT));
        // `speed` is the task's working copy (task+0x3c); `self.speed` (0x857234) keeps the
        // value from the start of the tick until the end.
        let mut speed = self.speed;

        if ctx.forced_stop {
            brake = i32::from(EMERGENCY_BRAKE);
            power = 0;
        }

        // ATC: trip above limit + 3 km/h, reset below 97.2 % of the limit, B8 while tripped.
        let limit = f64::from(ctx.atc_limit_kmh);
        let start = f64::from(self.speed);
        if start > (limit + ATC_MARGIN_KMH) * KMH_TO_MPS_C {
            self.atc_latched = true;
            events.atc_tripped = true;
        } else if start < limit * ATC_RESET_FACTOR {
            self.atc_latched = false;
        }
        if self.atc_latched {
            brake = i32::from(ATC_FORCED_BRAKE);
        }

        if ctx.coast_down {
            let v = f64::from(speed) - COAST_DECEL;
            speed = v as f32;
            power = 0;
            if v < 0.0 || ctx.coast_hold {
                speed = 0.0;
            }
        }

        // km/h: the rounded copy feeds the traction curve, the unrounded one the resistance.
        let kmh = f64::from(speed) * MPS_TO_KMH;
        let kmh_stored = kmh as f32;
        let resistance = running_resistance(spec, kmh) as f32;
        let extra_resistance = speed_resistance(spec, speed) as f32;
        let traction = (traction_force(spec, power, kmh_stored, self.traction_scale) as f32)
            * TRACTION_ADHESION;

        // Traction fades in/out (power cut sections); the fade applies to the force, not the notch.
        self.traction_ramp = if ctx.power_cut {
            let r = f64::from(self.traction_ramp) - TRACTION_RAMP_STEP;
            if r < 0.0 { 0.0 } else { r as f32 }
        } else {
            let r = f64::from(self.traction_ramp) + TRACTION_RAMP_STEP;
            if r > 1.0 { 1.0 } else { r as f32 }
        };
        let mut net = (f64::from(traction) * f64::from(self.traction_ramp)) as f32;

        // Brake cylinder. Release vs apply depends on LAST tick's notch; the target on this one's.
        let before = self.brake_cylinder;
        let rate = f64::from(spec.brake_rate);
        if self.prev_brake < 1 {
            let c = f64::from(before) - rate * BRAKE_RELEASE_FACTOR;
            self.brake_cylinder = if c <= BRAKE_RELEASED {
                BRAKE_RELEASED_F32
            } else {
                c as f32
            };
        } else {
            net = 0.0; // no traction while the brake was applied
            let raised = (rate + f64::from(before)) as f32;
            let step = f64::from(brake.min(i32::from(EMERGENCY_BRAKE))) * BRAKE_PER_NOTCH_STEP;
            let target = f64::from(spec.brake_per_notch) * step;
            self.brake_cylinder = raised;
            if target <= f64::from(raised) {
                let eased = (f64::from(raised) - rate * BRAKE_EASE_FACTOR) as f32;
                self.brake_cylinder = if f64::from(eased) <= target {
                    target as f32
                } else {
                    eased
                };
            }
        }
        if (f64::from(self.brake_cylinder) - f64::from(before)).abs() > BRAKE_CHANGE_EPSILON {
            self.brake_moving_ticks += 1;
        } else {
            self.brake_moving_ticks = 0;
        }

        if !ctx.driving {
            self.hold_at_rest(ctx, brake);
            speed = 0.0;
        } else {
            let mut resistance = resistance;
            if ctx.track_effects {
                let g = f64::from(self.grade_scale) * f64::from(ctx.gradient) * GRADE_GAIN;
                self.grade_force = g as f32;
                if g >= GRADE_GLITCH {
                    self.grade_force = 0.0;
                } else if self.grade_force >= 0.0 {
                    self.grade_force = (f64::from(self.grade_force) * GRADE_UPHILL_FACTOR) as f32;
                }
                let abs_heading = ctx.heading.abs();
                let c =
                    (f64::from(self.prev_abs_heading) - f64::from(abs_heading)).abs() * CURVE_GAIN;
                self.curve_sharpness = if c >= CURVE_CAP {
                    CURVE_CAP as f32
                } else {
                    c as f32
                };
                let curve = f64::from(self.curve_scale) * f64::from(self.curve_sharpness);
                resistance = (curve + f64::from(resistance)) as f32;
                self.prev_abs_heading = abs_heading;
            }
            let brake_force = brake_force(
                spec,
                self.brake_cylinder,
                speed,
                ctx.brake_load,
                ctx.slippery,
                ctx.forced_stop,
                self.brake_scale,
            );
            let a =
                f64::from(net) - f64::from(resistance) - f64::from(extra_resistance) - brake_force;
            let a = a - f64::from(self.grade_force);
            self.net_force = a as f32;
            let accel = a * FORCE_TO_KMH_PER_S * KMH_TO_MPS_C;
            self.accel = accel as f32;
            let new_speed = accel * TICK_SECONDS + f64::from(speed);
            speed = if new_speed < 0.0 {
                0.0
            } else if f64::from(spec.max_speed_kmh()) * CAP_TEST_KMH_TO_MPS <= new_speed {
                (f64::from(spec.max_speed_kmh()) * KMH_TO_MPS_A) as f32
            } else {
                new_speed as f32
            };
            let advance = f64::from(speed) * TICK_SECONDS;
            self.odometer = (advance + f64::from(self.odometer)) as f32;
            self.leg_distance = (advance + f64::from(self.leg_distance)) as f32;
            self.units_per_tick = (f64::from(speed) * UNITS_PER_MPS_TICK) as f32;

            self.decel_prev_tick = self.speed_prev2 - self.speed;
            self.speed_prev2 = self.speed;
            self.brake_decel =
                (brake_force * FORCE_TO_KMH_PER_S * KMH_TO_MPS_D * TICK_SECONDS) as f32;
            if self.decel_prev_tick_max <= self.decel_prev_tick {
                self.decel_prev_tick_max = self.decel_prev_tick;
            }
            if self.brake_decel_max <= self.brake_decel {
                self.brake_decel_max = self.brake_decel;
            }
            self.brake_history_delta = self.prev_brake - self.brake_history[0];
            self.brake_history[clock.rem_euclid(3) as usize] = self.prev_brake;
        }

        self.prev_power = power;
        self.prev_brake = brake;
        self.speed = speed;
        events
    }

    /// The 50 m marker on [`Self::leg_distance`] (0x40c4e0, runs before the physics in the same
    /// tick): on stages whose rank (0x4656c4) is at most 3, once `trunc(leg)` reads 50 or 51
    /// the leg distance jumps by 2.5 m so the check fires only once. Returns true when it fired;
    /// the rules then decide on the announcement (sounds 0x20/0x21).
    pub fn leg_marker(&mut self, stage_rank: i32) -> bool {
        if stage_rank > LEG_MARKER_MAX_RANK {
            return false;
        }
        if !LEG_MARKER_METRES.contains(&(self.leg_distance as i32)) {
            return false;
        }
        self.leg_distance = (f64::from(self.leg_distance) + LEG_MARKER_SKIP) as f32;
        true
    }

    /// Passing a non-stop station clears the leg distance (0x40d620, before the physics).
    pub fn pass_station(&mut self) {
        self.leg_distance = 0.0;
    }

    /// The not-driving branch (0x423f5d): everything that moves is zeroed.
    fn hold_at_rest(&mut self, ctx: &TickContext, brake: i32) {
        self.accel = 0.0;
        self.net_force = 0.0;
        self.prev_abs_heading = ctx.heading.abs();
        self.units_per_tick = 0.0;
        self.brake_history = [brake; 3];
        self.leg_distance = 0.0;
        self.speed_prev2 = 0.0;
        self.decel_prev_tick = 0.0;
        self.brake_decel = 0.0;
        self.brake_history_delta = 0;
        self.atc_latched = false;
    }
}

/// Running resistance (in 0x422c20 itself), `kmh` unrounded: flat 3.0 when creeping, else
/// `0.0164 v + c v² / mass + 1.32` with `c = (drag + 1) × 0.0078 + 0.028`.
pub fn running_resistance(spec: &TrainSpec, kmh: f64) -> f64 {
    if kmh > RESISTANCE_LOW_SPEED_KMH {
        let c = (f64::from(spec.drag) + RES_DRAG_OFFSET) * RES_DRAG_SCALE + RES_QUAD_BASE;
        let quad = c * kmh * kmh / f64::from(spec.mass);
        kmh * RES_LINEAR + quad + RES_CONSTANT
    } else if kmh > 0.0 {
        RESISTANCE_LOW
    } else {
        0.0
    }
}

/// Extra resistance that falls with speed (0x422860): with `k` = 102 % of top speed in m/s,
/// `x = (k² − v²)/k²`, result `5x² + 0.2`. Note it is 5.2 at rest. `mps` in m/s.
pub fn speed_resistance(spec: &TrainSpec, mps: f32) -> f64 {
    let k = f64::from(spec.max_speed_kmh()) * KMH_TO_MPS_A * EXTRA_RES_SPEED_MARGIN;
    let k2 = k * k;
    let v = f64::from(mps);
    let x = (k2 - v * v) / k2;
    x * x * EXTRA_RES_GAIN + EXTRA_RES_FLOOR
}

/// Traction force for `notch` at `kmh` (0x422610). Three regions:
/// * above the notch's balancing speed: tapers from 25 % of `force_base` to 0 over 10 km/h;
/// * between base speed and balancing speed: linear from `force_base` towards `force_top`
///   (slope × 1.1);
/// * up to the base speed: `1.25 × force_base × (p − v)/p` with
///   `p = notch_speed × (1 + 0.6 (1.2 vmax − v)/(1.2 vmax))`.
pub fn traction_force(spec: &TrainSpec, notch: i32, kmh: f32, scale: f32) -> f64 {
    let Some(&notch_speed) = usize::try_from(notch)
        .ok()
        .and_then(|n| n.checked_sub(1))
        .and_then(|i| spec.notch_speed_kmh.get(i))
    else {
        return 0.0;
    };
    let (v, pt, scale) = (f64::from(kmh), f64::from(notch_speed), f64::from(scale));
    let (base, top) = (
        f64::from(spec.base_speed_kmh),
        f64::from(spec.max_speed_kmh()),
    );
    let force_base = f64::from(spec.force_base);
    if notch_speed < kmh {
        let over = v - pt;
        let over = if over < TAPER_SPAN_KMH {
            over
        } else {
            TAPER_SPAN_KMH
        };
        return (1.0 - over * TAPER_SLOPE) * force_base * TAPER_FORCE_SHARE * scale;
    }
    if kmh > spec.base_speed_kmh {
        if notch_speed < spec.base_speed_kmh {
            return MID_FLAT_FORCE * scale;
        }
        let slope = (force_base - f64::from(spec.force_top)) / (top - base) * MID_SLOPE_GAIN;
        return ((v - base) * slope + force_base) * scale;
    }
    let headroom = top * LOW_SPEED_HEADROOM;
    let p = ((headroom - v) / headroom * LOW_SPEED_BOOST + 1.0) * pt;
    (p - v) / p * force_base * LOW_FORCE_GAIN * scale
}

/// Brake force from the cylinder value (0x422750). Zero unless the cylinder is above 0.
/// With `vl` = top speed in m/s, `v = min(mps, vl)`, `x = (vl² − v²)/vl²`:
/// `cyl × gain0 × gain1 × (0.9 + 0.12x) × (0.45 − 0.18 v²/vl²)(1 + 0.8x) × (1 − 0.0018 load)`,
/// then × 0.8 on slippery rail, × 4 in a forced stop, × `scale`. The result is not rounded
/// (the caller keeps it on the FPU stack).
pub fn brake_force(
    spec: &TrainSpec,
    cylinder: f32,
    mps: f32,
    load: i32,
    slippery: bool,
    forced_stop: bool,
    scale: f32,
) -> f64 {
    if cylinder <= 0.0 {
        return 0.0;
    }
    let vl = f64::from(spec.max_speed_kmh()) * f64::from(KMH_TO_MPS_B);
    let v = if vl < f64::from(mps) { vl as f32 } else { mps };
    let vl2 = f64::from((vl * vl) as f32);
    let v2 = f64::from(v) * f64::from(v);
    let v2 = f64::from(v2 as f32);
    let load_factor = f64::from((1.0 - f64::from(load) * BRAKE_LOAD_PER_UNIT) as f32);
    let x = (vl2 - v2) / vl2;
    let fade = BRAKE_FADE_BASE - v2 / vl2 * BRAKE_FADE_SLOPE;
    let low = x * BRAKE_LOW_SPEED_GAIN + 1.0;
    let mut curve = (fade * low) as f32;
    if slippery {
        curve = (f64::from(curve) * SLIPPERY_BRAKE_FACTOR) as f32;
    }
    let gains = f64::from(spec.brake_gain[1]) * f64::from(spec.brake_gain[0]) * f64::from(cylinder);
    let shape = x * BRAKE_SHAPE_SLOPE + BRAKE_SHAPE_BASE;
    let mut f = gains * shape * (f64::from(curve) * load_factor);
    if forced_stop {
        f *= FORCED_STOP_BRAKE_FACTOR;
    }
    f * f64::from(scale)
}

/// Gradient as the track lookup computes it each tick (0x407c10 → 0x857228): height of the
/// point 6 slots ahead of the camera minus the one 4 ahead, in 1/4096 units. 0 off the end.
pub fn gradient_at(route: &Route, pos: TrackPos) -> f32 {
    let slot = pos.slot(route);
    let h = |s: usize| route.slots.get(s).map(|p| p.height_raw);
    match (h(slot + 6), h(slot + 4)) {
        (Some(a), Some(b)) => (f32::from(a) - f32::from(b)) * HEIGHT_STEP,
        _ => 0.0,
    }
}

/// Heading of the camera's track chord (0x41d160 → 0x850288): `atan2(Δz, Δx)` from the
/// camera's point to the next one. 0 off the end.
///
/// Δz is stored to a float temporary first (0x41d31f `fstp dword [esp+0xc]`, reloaded for
/// `fpatan` at 0x41d362) while Δx stays on the FPU stack; the rounding of Δz is visible in
/// the result (oracle: yamate-drive.csv row 6431).
pub fn heading_at(route: &Route, pos: TrackPos) -> f32 {
    let slot = pos.slot(route);
    match (route.slots.get(slot), route.slots.get(slot + 1)) {
        (Some(a), Some(b)) => {
            let dz = b.z - a.z;
            f64::from(dz).atan2(f64::from(b.x) - f64::from(a.x)) as f32
        }
        _ => 0.0,
    }
}

/// Distance to the stop point in metres (0x40dad0 → 0x85708c) from the camera's remaining
/// track units to the stop (0x8500c8) and the extra offset 0x85709c (0 in practice).
pub fn distance_to_stop(units_to_stop: f32, offset_m: i32) -> f32 {
    (f64::from(units_to_stop) * 10.0 + f64::from(offset_m)) as f32
}

/// The integer distance to the stop (0x85722c): [`distance_to_stop`] rounded half away from
/// zero (fmod/floor/ceil sequence in 0x40dad0).
pub fn distance_to_stop_rounded(distance_m: f32) -> i32 {
    distance_m.round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up train shaped like a commuter EMU.
    fn spec() -> TrainSpec {
        TrainSpec::from_floats([
            30.0, 10.0, 30.0, 50.0, 80.0, 100.0, 95.0, 65.0, 0.08, 4.0, 16.9, 3.8, 449.5, 7.0,
            0.045,
        ])
    }

    fn state() -> TrainState {
        TrainState::new(
            StageParams {
                train_type: 0,
                grade_scale: 1.0,
                curve_scale: 1.0,
            },
            0.0,
        )
    }

    fn run(st: &mut TrainState, c: Controls, ctx: &TickContext, ticks: i32) {
        for t in 0..ticks {
            st.step(&spec(), c, ctx, t);
        }
    }

    #[test]
    fn traction_regions() {
        let s = spec();
        assert_eq!(traction_force(&s, 0, 0.0, 1.0), 0.0);
        // At rest: p = 100 × 1.6, force = 1.25 × 65.
        assert!((traction_force(&s, 5, 0.0, 1.0) - 81.25).abs() < 1e-4);
        // Mid region starts at force_base just above base speed.
        let mid = traction_force(&s, 5, 30.001, 1.0);
        assert!((mid - 65.0).abs() < 0.01, "{mid}");
        // Above the balancing speed of P1 (10 km/h): 25 % of force_base, tapering to 0.
        assert!((traction_force(&s, 1, 10.0001, 1.0) - 16.25).abs() < 0.01);
        // 10 km/h over: 1 − 10 × 0.1f leaves a tiny negative force, as in the original.
        assert!(traction_force(&s, 1, 25.0, 1.0).abs() < 1e-5);
    }

    #[test]
    fn resistances() {
        let s = spec();
        assert_eq!(running_resistance(&s, 0.0), 0.0);
        assert_eq!(running_resistance(&s, 5.0), 3.0);
        let r = running_resistance(&s, 100.0);
        let expect = 0.0164 * 100.0 + (8.0 * 0.0078 + 0.028) * 10000.0 / 449.5 + 1.32;
        assert!((r - expect).abs() < 1e-3, "{r} vs {expect}");
        assert!((speed_resistance(&s, 0.0) - 5.2).abs() < 1e-6);
    }

    #[test]
    fn full_power_accelerates_monotonically_to_top_speed() {
        let mut st = state();
        let ctx = TickContext::default();
        // Pre-departure: the traction fade-in (0.02 per tick from 0) completes while standing.
        let waiting = TickContext {
            driving: false,
            ..TickContext::default()
        };
        run(&mut st, Controls::default(), &waiting, 60);
        assert_eq!(st.traction_ramp, 1.0);
        let mut last = 0.0;
        for t in 0..30 * 300 {
            st.step(&spec(), Controls { power: 5, brake: 0 }, &ctx, t);
            assert!(st.speed >= last, "tick {t}: {} < {last}", st.speed);
            last = st.speed;
        }
        let kmh = st.speed_kmh();
        assert!(kmh > 90.0 && kmh <= 100.0, "{kmh}");
        assert!(st.odometer > 0.0);
    }

    #[test]
    fn traction_needs_brake_released_last_tick() {
        let mut st = state();
        let ctx = TickContext::default();
        run(&mut st, Controls { power: 0, brake: 3 }, &ctx, 60);
        // Brake was applied last tick: power does nothing this tick.
        st.step(&spec(), Controls { power: 5, brake: 0 }, &ctx, 60);
        assert_eq!(st.speed, 0.0);
        // The cylinder still holds pressure for a while after release.
        assert!(st.brake_cylinder > 0.0);
    }

    #[test]
    fn brake_cylinder_ramps_to_target_and_releases_to_floor() {
        let mut st = state();
        let ctx = TickContext {
            driving: false,
            ..TickContext::default()
        };
        // First tick still sees prev_brake = 0, so it releases (stays at −0.4).
        st.step(&spec(), Controls { power: 0, brake: 8 }, &ctx, 0);
        assert_eq!(st.brake_cylinder, -0.4);
        run(&mut st, Controls { power: 0, brake: 8 }, &ctx, 100);
        assert!(
            (st.brake_cylinder - 3.2).abs() < 1e-5,
            "{}",
            st.brake_cylinder
        );
        run(&mut st, Controls { power: 0, brake: 0 }, &ctx, 100);
        assert_eq!(st.brake_cylinder, -0.4);
    }

    #[test]
    fn braking_stops_the_train() {
        let mut st = state();
        let ctx = TickContext::default();
        run(&mut st, Controls { power: 5, brake: 0 }, &ctx, 30 * 40);
        assert!(st.speed_kmh() > 60.0);
        let mut last = st.speed;
        for t in 0..30 * 60 {
            st.step(&spec(), Controls { power: 0, brake: 8 }, &ctx, t);
            assert!(st.speed <= last);
            last = st.speed;
        }
        assert_eq!(st.speed, 0.0);
    }

    #[test]
    fn atc_trip_forces_b8() {
        let mut st = state();
        st.speed = 60.0 / 3.6;
        st.prev_brake = 0;
        let ctx = TickContext {
            atc_limit_kmh: 40.0,
            ..TickContext::default()
        };
        let ev = st.step(&spec(), Controls { power: 5, brake: 0 }, &ctx, 0);
        assert!(ev.atc_tripped);
        assert_eq!(st.prev_brake, 8);
    }

    #[test]
    fn not_driving_holds_the_train() {
        let mut st = state();
        st.speed = 10.0;
        let ctx = TickContext {
            driving: false,
            ..TickContext::default()
        };
        st.step(&spec(), Controls { power: 5, brake: 0 }, &ctx, 0);
        assert_eq!(st.speed, 0.0);
        assert_eq!(st.units_per_tick, 0.0);
    }

    #[test]
    fn uphill_slows_more_than_flat() {
        let ctx_flat = TickContext::default();
        let ctx_up = TickContext {
            gradient: 0.002,
            ..TickContext::default()
        };
        let (mut a, mut b) = (state(), state());
        run(&mut a, Controls { power: 3, brake: 0 }, &ctx_flat, 600);
        run(&mut b, Controls { power: 3, brake: 0 }, &ctx_up, 600);
        assert!(b.speed < a.speed);
        assert!(b.grade_force > 0.0);
    }

    #[test]
    fn stop_distance_rounding() {
        assert_eq!(distance_to_stop_rounded(2.5), 3);
        assert_eq!(distance_to_stop_rounded(2.49), 2);
        assert_eq!(distance_to_stop_rounded(-2.5), -3);
        assert_eq!(distance_to_stop_rounded(-0.4), 0);
        assert_eq!(distance_to_stop(1.25, 0), 12.5);
    }
}

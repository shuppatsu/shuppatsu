//! Shared helpers for the data-driven engine tests: the install dir, a `.COM` loader and a
//! deterministic autopilot that drives a [`Drive`] from what a player sees.

use ddg_engine::drive::{Drive, DrivePhase, DriveTick, DriverInput, LoadError};
use ddg_engine::rules::RuleEvent;
use std::path::{Path, PathBuf};

/// `$DDG_DATA_DIR/install/Program_Executable_Files`, or `None` (test skips) when unset.
pub fn install_dir() -> Option<PathBuf> {
    let Some(root) = std::env::var_os("DDG_DATA_DIR") else {
        eprintln!("skipped: DDG_DATA_DIR not set");
        return None;
    };
    let dir = PathBuf::from(root).join("install/Program_Executable_Files");
    dir.is_dir().then_some(dir)
}

/// Route loader: `.COM` files from `dir`, names matched case-insensitively.
pub fn loader(dir: &Path) -> impl FnMut(&str) -> Result<Vec<u8>, LoadError> + Send + 'static {
    let dir = dir.to_path_buf();
    move |name: &str| -> Result<Vec<u8>, LoadError> {
        for e in std::fs::read_dir(&dir)? {
            let p = e?.path();
            if p.file_name()
                .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name))
            {
                return Ok(std::fs::read(p)?);
            }
        }
        Err(format!("{name} not found").into())
    }
}

/// Speeds at or above this are "no limit" (300 km/h, rules.md §4.2 / §5).
const NO_LIMIT_KMH: f32 = 299.0;
/// Cruise this far below the enforced limit (sign + tolerance) and the train's top speed.
const CRUISE_MARGIN_KMH: f32 = 3.0;
/// Planned service deceleration for station stops, m/s².
const PLAN_DECEL: f32 = 0.65;
/// Start the stop once the required deceleration reaches this, m/s².
const START_DECEL: f32 = 0.65;
/// Aim this far before the marker, m (the brake still bites in the last tick).
const AIM_SHORT_M: f32 = 0.05;
/// Creep speed when stopped short of the marker, km/h.
const CREEP_KMH: f32 = 8.0;
const TICK_S: f32 = 1.0 / 30.0;

/// What the cab shows the player this tick.
#[derive(Debug, Clone, Copy)]
pub struct View {
    pub speed_kmh: f32,
    /// The speed-limit sign in force, km/h (the HUD value, without the difficulty margin).
    pub limit_kmh: Option<f32>,
    /// The signal aspect's speed, km/h (without the margin); `Some(0)` for red.
    pub signal_kmh: Option<f32>,
    /// Distance to the stop marker (negative = overrun).
    pub dist_m: f32,
    /// The next station is a stop.
    pub stops: bool,
    /// ATS bell waiting for confirmation.
    pub ats_bell: bool,
    pub top_kmh: f32,
    /// Seconds until the scheduled time at the next station (the HUD clock and timetable).
    pub time_left_s: f32,
}

impl View {
    pub fn read(d: &Drive, margin_kmh: f32) -> Self {
        let r = d.rules();
        let shown = |mps: f32| {
            let kmh = mps * 3.6;
            (kmh < NO_LIMIT_KMH).then_some((kmh - margin_kmh).max(0.0))
        };
        Self {
            speed_kmh: d.speed_kmh(),
            limit_kmh: shown(r.limit_mps),
            signal_kmh: if r.aspect >= 4 {
                Some(0.0)
            } else {
                shown(r.signal_limit_mps)
            },
            dist_m: d.dist_to_stop_m(),
            stops: r.target_stops,
            ats_bell: r.ats == 2,
            top_kmh: d.train_spec().max_speed_kmh(),
            time_left_s: (r.scheduled - r.clock) as f32 / 30.0,
        }
    }
}

/// A simple player: cruise below the limits, brake to the marker by the required deceleration
/// (learning the train's deceleration per brake notch as it goes), acknowledge the ATS.
#[derive(Debug, Clone)]
pub struct Autopilot {
    margin_kmh: f32,
    /// Brake notch of the last output.
    brake: u8,
    /// Ticks the current brake notch has been held.
    held: u32,
    prev_speed_mps: f32,
    /// Smoothed measured deceleration, m/s².
    decel: f32,
    /// Learned deceleration per brake notch, m/s².
    per_notch: f32,
    /// Committed to the station stop.
    stopping: bool,
    /// ATC speed shown in the cab (course event 7 `a`), km/h.
    atc_shown_kmh: Option<f32>,
}

impl Autopilot {
    pub fn new(margin_kmh: f32) -> Self {
        Self {
            margin_kmh,
            brake: 9,
            held: 0,
            prev_speed_mps: 0.0,
            decel: 0.0,
            per_notch: 0.12,
            stopping: false,
            atc_shown_kmh: None,
        }
    }

    /// New segment / restart: forget the stop latch.
    pub fn reset_stop(&mut self) {
        self.stopping = false;
    }

    /// Reads the cab displays a tick changed: the ATC speed of the bonus section.
    pub fn observe(&mut self, tick: &DriveTick) {
        for e in &tick.events {
            if let RuleEvent::Script { kind: 7, args } = e {
                self.atc_shown_kmh = usize::try_from(args[0])
                    .ok()
                    .and_then(|i| ATC_SPEEDS_KMH.get(i))
                    .map(|&k| k as f32);
            }
        }
    }

    fn set(&mut self, power: u8, brake: u8) -> DriverInput {
        if brake != self.brake {
            self.held = 0;
        } else {
            self.held += 1;
        }
        self.brake = brake;
        DriverInput {
            power,
            brake,
            ..DriverInput::default()
        }
    }

    /// The coupling minigame: horn first, then creep the last 40 m and touch at about 1.5 km/h.
    fn couple(&mut self, d: &Drive, mps: f32) -> DriverInput {
        let Some(c) = d.rules().coupling.as_ref().filter(|c| c.driving()) else {
            return self.set(0, 8);
        };
        let left = c.dist_m;
        let target_kmh = if left > 10.0 {
            5.0
        } else if left > 4.0 {
            2.5
        } else {
            1.2
        };
        let kmh = mps * 3.6;
        let mut i = if kmh < target_kmh - 0.6 {
            self.set(1, 0)
        } else if kmh > target_kmh + 0.2 {
            self.set(0, if kmh > target_kmh + 1.0 { 4 } else { 2 })
        } else {
            self.set(0, 0)
        };
        i.horn_pressed = c.horn == 0 && mps <= 0.0;
        i
    }

    /// The bonus game: creep onto the car at about 1.5 km/h.
    fn bonus(&mut self, d: &Drive) -> DriverInput {
        let Some(b) = d.bonus() else {
            return self.set(0, 8);
        };
        let gap_m = (b.pair_z + 0.4) * 5.0;
        let target = if gap_m > 2.0 { 7.0 } else { 3.0 };
        let kmh = b.train.speed * 3.6;
        if kmh < target - 0.5 {
            self.set(1, 0)
        } else if kmh > target + 0.3 {
            self.set(0, if kmh > target + 1.5 { 4 } else { 2 })
        } else {
            self.set(0, 0)
        }
    }

    /// The controls for the next tick: notches, plus the horn at whistle signs (held while one
    /// is required, pressed in a horn bonus zone) and the ATS acknowledgement.
    pub fn control(&mut self, d: &Drive) -> DriverInput {
        let mut input = self.notches(d);
        if matches!(d.phase(), DrivePhase::Driving) {
            input.horn_held = d.rules().horn_state == 1;
            input.horn_pressed = d.rules().horn_state == 2;
        }
        input
    }

    fn notches(&mut self, d: &Drive) -> DriverInput {
        let v = View::read(d, self.margin_kmh);
        let mps = v.speed_kmh / 3.6;
        let measured = (self.prev_speed_mps - mps) / TICK_S;
        self.prev_speed_mps = mps;
        self.decel += (measured - self.decel) * 0.2;

        match d.phase() {
            DrivePhase::Loading | DrivePhase::Arrived | DrivePhase::Cleared => {
                self.stopping = false;
                return self.set(0, 9);
            }
            DrivePhase::PreDeparture if d.waiting_for_departure() => {
                self.stopping = false;
                return self.set(0, 9);
            }
            DrivePhase::Holding | DrivePhase::GameOver => return self.set(0, 8),
            DrivePhase::Coupling => return self.couple(d, mps),
            DrivePhase::Bonus => return self.bonus(d),
            DrivePhase::PreDeparture | DrivePhase::Driving => {}
        }

        // Learn the deceleration per notch from steady braking.
        if self.brake > 0 && self.brake < 9 && self.held > 20 && mps > 3.0 {
            let k = self.decel / f32::from(self.brake);
            if k.is_finite() && k > 0.02 {
                self.per_notch += (k - self.per_notch) * 0.05;
            }
        }

        if v.ats_bell {
            let mut i = self.set(0, self.brake.max(1));
            i.confirm_pressed = true;
            return i;
        }

        // An arcade player knows the difficulty's tolerance above the signs (rules.md §5).
        let mut cruise = v.top_kmh - CRUISE_MARGIN_KMH;
        for lim in [v.limit_kmh, v.signal_kmh].into_iter().flatten() {
            if lim > 0.0 {
                cruise = cruise.min(lim + self.margin_kmh - CRUISE_MARGIN_KMH);
            }
        }
        // Keep to the timetable: running early paints restrictive signals (rules.md §4.3).
        let atc_section = d.rules().derail_kmh < 999.0;
        if let Some(kmh) = schedule_kmh(v.dist_m, v.time_left_s, v.stops).filter(|_| !atc_section) {
            cruise = cruise.min(kmh.max(SCHEDULE_MIN_KMH));
        }
        // ATC section: the cab shows the coming ATC speed before it is enforced; above the
        // enforced speed + 3 km/h the run ends (physics.md §4.1).
        let atc = d.rules().derail_kmh;
        if atc < 999.0 {
            let shown = self.atc_shown_kmh.unwrap_or(atc).min(atc);
            cruise = cruise.min(shown - CRUISE_MARGIN_KMH - 2.0);
        }
        let red = v.signal_kmh == Some(0.0);

        if v.stops {
            if v.dist_m < 0.0 {
                // Overran: stop now.
                return self.set(0, if mps > 0.0 { 8 } else { 9 });
            }
            let d_eff = (v.dist_m - AIM_SHORT_M).max(0.01);
            let need = mps * mps / (2.0 * d_eff);
            if need >= START_DECEL {
                self.stopping = true;
            }
            if self.stopping {
                if mps <= 0.0 {
                    if v.dist_m > 0.5 {
                        // Short: creep up.
                        self.stopping = false;
                        return self.set(1, 0);
                    }
                    return self.set(0, 8);
                }
                let notch = (need / self.per_notch).round().clamp(1.0, 8.0) as u8;
                // At the last metres keep at least what we have, so the train settles.
                let notch = if v.dist_m < 2.0 && mps < 1.5 {
                    notch.max(self.brake.min(8))
                } else {
                    notch
                };
                return self.set(0, notch);
            }
            // Not braking yet: stay under the braking curve to the marker (creeps when close).
            let curve = (2.0 * PLAN_DECEL * d_eff).sqrt() * 3.6;
            cruise = cruise.min(curve.max(CREEP_KMH));
        }

        if red {
            // Stop for the red (the signal hold then clears it).
            return self.set(0, if mps > 0.0 { 5 } else { 8 });
        }
        let over = v.speed_kmh - cruise;
        if over > 2.0 {
            // Signs and signals give a few hundred metres: brake firmly.
            let notch = if over > 6.0 { 8 } else { 4 };
            return self.set(0, notch);
        }
        if over > -1.0 {
            return self.set(0, 0);
        }
        let power = if over < -10.0 { 5 } else { 3 };
        self.set(power, 0)
    }
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Cleared,
    /// Game over with no continue left (the target was the final station, or the cap).
    GameOver,
    TickCap,
}

/// One stage run's record.
#[derive(Debug, Clone)]
pub struct RunReport {
    pub stage: usize,
    pub difficulty: usize,
    pub stations: usize,
    pub ticks: u64,
    pub outcome: Outcome,
    /// Highest station index reached (arrived at or passed).
    pub reached: usize,
    pub continues: i32,
    /// Stop errors at each arrival, m (positive = short).
    pub stop_errors: Vec<f32>,
    pub ats_trips: u32,
    pub overspeed: u32,
    pub remaining_s: i32,
    /// `.COM` indices of the segments driven, in order of first use.
    pub segments: Vec<u32>,
    /// `.COM` indices of the stage's segments that hold a leg (one starting at the last station
    /// only serves a start there).
    pub stage_segments: Vec<u32>,
    /// Stage-specific events: the bonus section, coupling results.
    pub notes: Vec<String>,
    /// Stage 1 jumped from station 8 to the bonus section at 24 (rows 9–23 not driven).
    pub bonus_jump: bool,
    /// Sanity violations (first few).
    pub violations: Vec<String>,
}

/// Ticks of the `Arrived` phase before the result screen closes and the station is finished
/// (ddg-game's flow: 5 + 207 result ticks, then one more).
pub const FINISH_AFTER_ARRIVED_TICKS: u32 = 5 + 207 + 1;
/// Ticks of the `GameOver` phase before the continue screen is confirmed (22 + 8 + 30).
pub const CONTINUE_AFTER_GAME_OVER_TICKS: u32 = 60;
/// Continues the autopilot accepts before giving up.
pub const MAX_CONTINUES: i32 = 10;

/// Never cruise slower than this to keep to the timetable, km/h.
const SCHEDULE_MIN_KMH: f32 = 30.0;
/// Margin over the speed that would arrive exactly on time (limits and signals ahead cost time).
const SCHEDULE_SLACK: f32 = 1.08;

/// The cruise speed that reaches the next station on time, if there is time to spare: for a pass
/// `d / t`; for a stop, cruise then brake at [`PLAN_DECEL`]: `d/v + v/(2a) = t`.
fn schedule_kmh(dist_m: f32, time_left_s: f32, stops: bool) -> Option<f32> {
    if dist_m <= 0.0 || time_left_s <= 1.0 {
        return None;
    }
    let mps = if stops {
        let a = PLAN_DECEL;
        let disc = time_left_s * time_left_s - 2.0 * dist_m / a;
        if disc <= 0.0 {
            return None;
        }
        a * (time_left_s - disc.sqrt())
    } else {
        dist_m / time_left_s
    };
    Some(mps * 3.6 * SCHEDULE_SLACK + 2.0)
}

/// ATC readouts by the index of course event 7 (stack table in 0x416d10, docs/re/hud.md §3.6).
const ATC_SPEEDS_KMH: [i32; 8] = [0, 30, 70, 110, 160, 210, 245, 275];

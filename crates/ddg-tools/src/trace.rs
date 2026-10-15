//! Oracle traces (CSV written by ddg-hook) and replaying them through ddg-engine.
//!
//! The physics replay isolates the integrator: for each tick it takes the inputs and context
//! the original logged (notches, flags, gradient, ATC limit, …, from that tick's row; the
//! heading from the row before, see physics.md §8), steps the engine, and compares its outputs
//! with that row. Floats are compared bit for bit (the hook
//! logs `value:bits`).

use anyhow::{Context, Result, bail};
use ddg_engine::physics::{Controls, TickContext, TrainState, TrainTables};
use ddg_formats::exe::Exe;
use std::collections::HashMap;
use std::path::Path;

pub mod rules;

/// One logged value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    Int(i64),
    /// f32 logged with its exact bit pattern.
    Float(f32),
    Missing,
}

impl Value {
    fn parse(s: &str) -> Self {
        if let Some((_, bits)) = s.split_once(':')
            && let Ok(b) = u32::from_str_radix(bits, 16)
        {
            return Self::Float(f32::from_bits(b));
        }
        s.parse().map_or(Self::Missing, Self::Int)
    }

    pub fn f32(self) -> Option<f32> {
        match self {
            Self::Float(f) => Some(f),
            Self::Int(i) => Some(i as f32),
            Self::Missing => None,
        }
    }

    pub fn int(self) -> Option<i64> {
        match self {
            Self::Int(i) => Some(i),
            _ => None,
        }
    }
}

pub struct Trace {
    columns: HashMap<String, usize>,
    pub rows: Vec<Vec<Value>>,
}

impl Trace {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
        Self::parse(&text)
    }

    /// Parses CSV text (header line, then one row per tick).
    pub fn parse(text: &str) -> Result<Self> {
        let mut lines = text.lines();
        let header = lines.next().context("empty trace")?;
        let columns = header
            .split(',')
            .enumerate()
            .map(|(i, n)| (n.to_owned(), i))
            .collect();
        let rows = lines
            .filter(|l| !l.is_empty())
            .map(|l| l.split(',').map(Value::parse).collect())
            .collect();
        Ok(Self { columns, rows })
    }

    /// Whether the trace has a column.
    pub fn has(&self, name: &str) -> bool {
        self.columns.contains_key(name)
    }

    pub fn get(&self, row: usize, name: &str) -> Value {
        self.columns
            .get(name)
            .and_then(|&c| self.rows.get(row)?.get(c).copied())
            .unwrap_or(Value::Missing)
    }

    pub(crate) fn f(&self, row: usize, name: &str) -> Result<f32> {
        self.get(row, name)
            .f32()
            .with_context(|| format!("row {row}: column {name} missing"))
    }

    pub(crate) fn i(&self, row: usize, name: &str) -> Result<i64> {
        self.get(row, name)
            .int()
            .with_context(|| format!("row {row}: column {name} missing"))
    }
}

/// Phase flag bits (physics.md §8).
const DRIVING: i64 = 0x200;
const COAST: i64 = 0x400 | 0x10;

fn context(t: &Trace, r: usize) -> Result<TickContext> {
    let phase = t.i(r, "phase_flags")?;
    Ok(TickContext {
        driving: phase & DRIVING != 0,
        coast_down: phase & COAST != 0,
        // Not logged directly; only relevant on result screens.
        coast_hold: false,
        forced_stop: t.i(r, "overrun_state")? >= 4,
        power_cut: t.i(r, "power_cut")? != 0,
        slippery: t.i(r, "slippery")? > 1,
        atc_limit_kmh: t.f(r, "atc_limit_kmh")?,
        gradient: t.f(r, "gradient")?,
        heading: t.f(r, "heading")?,
        track_effects: t.i(r, "stage")? >= 0,
        brake_load: i32::try_from(t.i(r, "brake_load")?)?,
    })
}

/// Engine state taken from a logged row (to start the replay mid-run).
fn state_from_row(t: &Trace, r: usize, tables: &TrainTables) -> Result<TrainState> {
    let train_type = usize::try_from(t.i(r, "train_type")?)?;
    if train_type >= tables.specs.len() {
        bail!("row {r}: train type {train_type} out of range");
    }
    let mut s = TrainState::new(
        ddg_engine::physics::StageParams {
            train_type,
            grade_scale: t.f(r, "grade_scale")?,
            curve_scale: t.f(r, "curve_scale")?,
        },
        t.f(r, "t_prev_heading")?,
    );
    s.traction_scale = t.f(r, "traction_scale")?;
    s.brake_scale = t.f(r, "brake_scale")?;
    s.speed = t.f(r, "speed")?;
    s.accel = t.f(r, "t_accel")?;
    s.net_force = t.f(r, "t_force")?;
    s.brake_cylinder = t.f(r, "t_brake_cyl")?;
    s.traction_ramp = t.f(r, "t_fade")?;
    s.prev_power = i32::try_from(t.i(r, "t_last_power")?)?;
    s.prev_brake = i32::try_from(t.i(r, "t_last_brake")?)?;
    s.atc_latched = t.i(r, "t_atc_latch")? != 0;
    s.brake_moving_ticks = i32::try_from(t.i(r, "t_brake_ticks")?)?;
    s.odometer = t.f(r, "odometer")?;
    s.leg_distance = t.f(r, "leg_distance")?;
    s.speed_prev2 = t.f(r, "speed_two_back")?;
    s.decel_prev_tick = t.f(r, "speed_lost")?;
    s.decel_prev_tick_max = t.f(r, "speed_max")?;
    s.brake_decel = t.f(r, "brake_decel")?;
    s.brake_decel_max = t.f(r, "brake_decel_max")?;
    s.brake_history = [
        i32::try_from(t.i(r, "brake_hist0")?)?,
        i32::try_from(t.i(r, "brake_hist1")?)?,
        i32::try_from(t.i(r, "brake_hist2")?)?,
    ];
    s.brake_history_delta = i32::try_from(t.i(r, "brake_delta")?)?;
    Ok(s)
}

/// Compared outputs: (name in trace, engine value). Integers are compared as exact f32.
fn outputs(s: &TrainState) -> Vec<(&'static str, f32)> {
    vec![
        ("speed", s.speed),
        ("t_speed", s.speed),
        ("t_brake_cyl", s.brake_cylinder),
        ("t_fade", s.traction_ramp),
        ("t_force", s.net_force),
        ("t_accel", s.accel),
        ("t_prev_heading", s.prev_abs_heading),
        ("t_brake_ticks", s.brake_moving_ticks as f32),
        ("t_last_power", s.prev_power as f32),
        ("t_last_brake", s.prev_brake as f32),
        ("t_atc_latch", f32::from(u8::from(s.atc_latched))),
        ("gradient_force", s.grade_force),
        ("curve_sharpness", s.curve_sharpness),
        ("odometer", s.odometer),
        ("leg_distance", s.leg_distance),
        ("step_units", s.units_per_tick),
        ("speed_two_back", s.speed_prev2),
        ("speed_lost", s.decel_prev_tick),
        ("speed_max", s.decel_prev_tick_max),
        ("brake_decel", s.brake_decel),
        ("brake_decel_max", s.brake_decel_max),
        ("brake_hist0", s.brake_history[0] as f32),
        ("brake_hist1", s.brake_history[1] as f32),
        ("brake_hist2", s.brake_history[2] as f32),
        ("brake_delta", s.brake_history_delta as f32),
    ]
}

/// Result of a physics replay.
#[derive(Debug, Default)]
pub struct Report {
    pub compared: usize,
    pub exact: usize,
    /// Largest absolute error per field, for fields that ever differed.
    pub worst: Vec<(&'static str, f32)>,
    pub first_mismatch: Option<String>,
}

impl Report {
    fn note(&mut self, name: &'static str, err: f32) {
        match self.worst.iter_mut().find(|(n, _)| *n == name) {
            Some((_, w)) => *w = w.max(err),
            None => self.worst.push((name, err)),
        }
    }
}

/// Replays the physics over every driving stretch of the trace. Returns false on a mismatch.
pub fn replay_physics(trace_path: &Path, exe_path: &Path, verbose: bool) -> Result<bool> {
    let t = Trace::load(trace_path)?;
    let exe_bytes = std::fs::read(exe_path)?;
    let exe = Exe::parse(&exe_bytes)?;
    let tables = TrainTables::read(&exe)?;
    let ranks = ddg_engine::time::read_stage_ranks(&exe)?;
    let report = replay(&t, &tables, &ranks, verbose)?;
    println!(
        "compared {} values, {} bit-exact",
        report.compared, report.exact
    );
    for (name, err) in &report.worst {
        println!("  worst {name} error {err}");
    }
    if let Some(m) = &report.first_mismatch {
        println!("first mismatch: {m}");
    }
    Ok(report.first_mismatch.is_none())
}

/// Replays every driving stretch of `t`, each started from its first row's logged state.
pub fn replay(
    t: &Trace,
    tables: &TrainTables,
    stage_ranks: &[i32],
    verbose: bool,
) -> Result<Report> {
    let driving = |r: usize| {
        t.get(r, "phase_flags")
            .int()
            .is_some_and(|p| p & DRIVING != 0)
    };
    // Time over (0x10 set by the stage loop, physics.md §4.1) ends with one more logged tick where
    // 0x10 is clear but 0x200 still set; the original's physics leaves the train untouched on it
    // (hilly-drive trace: P5 at rest stays at 0). Treat that tick as the end of the stretch.
    let coast_released = |r: usize| {
        let flags = |r: usize| t.get(r, "phase_flags").int().unwrap_or(0);
        r > 0 && flags(r - 1) & 0x10 != 0 && flags(r) & 0x10 == 0
    };
    let mut report = Report::default();
    let mut r = 0;
    while r < t.rows.len() {
        if !driving(r) {
            r += 1;
            continue;
        }
        let mut state = state_from_row(t, r, tables)?;
        let spec = tables.specs[state.train_type];
        let start = r;
        while r + 1 < t.rows.len() && driving(r + 1) && !coast_released(r + 1) {
            // Row r+1 was logged at the end of the tick being replayed but before its input poll,
            // so its notches are the ones the physics used, and the driving-loop globals
            // (gradient, flags, limits, clock) were set before the physics ran. Only the heading
            // is written after the physics (by the camera task), so it comes from row r.
            let k = r + 1;
            let controls = Controls {
                power: u8::try_from(t.i(k, "power_notch")?).unwrap_or(0),
                brake: u8::try_from(t.i(k, "brake_notch")?).unwrap_or(0),
            };
            let mut ctx = context(t, k)?;
            ctx.heading = t.f(r, "heading")?;
            let clock = i32::try_from(t.i(k, "stage_clock")?)?;
            // Driving-loop writes to the leg distance that precede the physics in this tick.
            if t.get(k, "station_index") != t.get(r, "station_index") {
                state.pass_station();
            }
            let rank = usize::try_from(t.i(k, "stage")?)
                .ok()
                .and_then(|s| stage_ranks.get(s).copied())
                .unwrap_or(i32::MAX);
            state.leg_marker(rank);
            state.step(&spec, controls, &ctx, clock);
            r = k;
            for (name, mine) in outputs(&state) {
                let Some(theirs) = t.get(r, name).f32() else {
                    continue;
                };
                report.compared += 1;
                if mine.to_bits() == theirs.to_bits() {
                    report.exact += 1;
                    continue;
                }
                report.note(name, (mine - theirs).abs());
                if report.first_mismatch.is_none() {
                    report.first_mismatch = Some(format!(
                        "row {r} (stretch from row {start}): {name} engine {mine} ({:08x}) vs original {theirs} ({:08x})",
                        mine.to_bits(),
                        theirs.to_bits()
                    ));
                }
                if verbose {
                    println!("row {r}: {name} {mine} vs {theirs}");
                }
            }
        }
        r += 1;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every physics trace in `DDG_TRACE_DIR` must replay bit-exactly. Needs `DDG_DATA_DIR`
    /// for the exe tables; skips when either is unset.
    #[test]
    fn oracle_traces_replay_bit_exactly() {
        let (Some(traces), Some(data)) = (
            std::env::var_os("DDG_TRACE_DIR"),
            std::env::var_os("DDG_DATA_DIR"),
        ) else {
            eprintln!("skipped: DDG_TRACE_DIR or DDG_DATA_DIR not set");
            return;
        };
        let exe =
            std::path::PathBuf::from(data).join("install/Program_Executable_Files/BIN/dengo3k.exe");
        let bytes = std::fs::read(&exe).unwrap();
        let exe = Exe::parse(&bytes).unwrap();
        let tables = TrainTables::read(&exe).unwrap();
        let ranks = ddg_engine::time::read_stage_ranks(&exe).unwrap();
        let mut paths: Vec<_> = std::fs::read_dir(traces)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "csv"))
            .collect();
        paths.sort();
        for path in paths {
            let t = Trace::load(&path).unwrap();
            if !t.columns.contains_key("t_force") {
                continue; // not a physics trace
            }
            let report = replay(&t, &tables, &ranks, false).unwrap();
            eprintln!(
                "{}: {} / {} bit-exact",
                path.display(),
                report.exact,
                report.compared
            );
            assert_eq!(
                report.first_mismatch,
                None,
                "{}: worst {:?}",
                path.display(),
                report.worst
            );
        }
    }
}

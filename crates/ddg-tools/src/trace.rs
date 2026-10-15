//! Oracle traces (CSV written by ddg-hook) and replaying them through ddg-engine.
//!
//! The physics replay isolates the integrator: every tick it takes the context the original
//! logged (inputs, flags, gradient, heading, ATC limit, …) from the previous row, steps the
//! engine, and compares its outputs with the next row. Floats are compared bit for bit (the hook
//! logs `value:bits`).

use anyhow::{Context, Result, bail};
use ddg_engine::physics::{Controls, TickContext, TrainState, TrainTables};
use ddg_formats::exe::Exe;
use std::collections::HashMap;
use std::path::Path;

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

    pub fn get(&self, row: usize, name: &str) -> Value {
        self.columns
            .get(name)
            .and_then(|&c| self.rows.get(row)?.get(c).copied())
            .unwrap_or(Value::Missing)
    }

    fn f(&self, row: usize, name: &str) -> Result<f32> {
        self.get(row, name)
            .f32()
            .with_context(|| format!("row {row}: column {name} missing"))
    }

    fn i(&self, row: usize, name: &str) -> Result<i64> {
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

/// Compared outputs: (name in trace, engine value).
fn outputs(s: &TrainState) -> [(&'static str, f32); 6] {
    [
        ("speed", s.speed),
        ("t_brake_cyl", s.brake_cylinder),
        ("t_fade", s.traction_ramp),
        ("t_force", s.net_force),
        ("odometer", s.odometer),
        ("step_units", s.units_per_tick),
    ]
}

/// Replays the physics over every driving stretch of the trace. Returns false on a mismatch.
pub fn replay_physics(trace_path: &Path, exe_path: &Path, verbose: bool) -> Result<bool> {
    let t = Trace::load(trace_path)?;
    let exe_bytes = std::fs::read(exe_path)?;
    let tables = TrainTables::read(&Exe::parse(&exe_bytes)?)?;
    let driving = |r: usize| {
        t.get(r, "phase_flags")
            .int()
            .is_some_and(|p| p & DRIVING != 0)
    };

    let (mut compared, mut exact, mut worst) = (0usize, 0usize, 0f32);
    let mut first_mismatch: Option<String> = None;
    let mut r = 0;
    while r < t.rows.len() {
        if !driving(r) {
            r += 1;
            continue;
        }
        // Start a stretch at this driving row from its logged state.
        let mut state = state_from_row(&t, r, &tables)?;
        let spec = tables.specs[state.train_type];
        let start = r;
        while r + 1 < t.rows.len() && driving(r + 1) {
            let controls = Controls {
                power: u8::try_from(t.i(r, "power_notch")?).unwrap_or(0),
                brake: u8::try_from(t.i(r, "brake_notch")?).unwrap_or(0),
            };
            let ctx = context(&t, r)?;
            let clock = i32::try_from(t.i(r, "stage_clock")?)?;
            state.step(&spec, controls, &ctx, clock);
            r += 1;
            for (name, mine) in outputs(&state) {
                let Some(theirs) = t.get(r, name).f32() else {
                    continue;
                };
                compared += 1;
                if mine.to_bits() == theirs.to_bits() {
                    exact += 1;
                    continue;
                }
                let err = (mine - theirs).abs();
                worst = worst.max(if name == "speed" { err } else { 0.0 });
                if first_mismatch.is_none() {
                    first_mismatch = Some(format!(
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
    println!("compared {compared} values, {exact} bit-exact, worst speed error {worst} m/s");
    if let Some(m) = &first_mismatch {
        println!("first mismatch: {m}");
    }
    Ok(first_mismatch.is_none())
}

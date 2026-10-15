//! Rules replay: drives `ddg_engine::rules` from an oracle trace and compares the rule globals.
//!
//! Alignment (physics.md §8): a row is written at the end of `Task_Tick`, before that tick's
//! input poll. For the tick that produced row k the rules therefore saw
//! * the notches and buttons of row k (latched at the end of the previous tick);
//! * the speed, odometer and track position of row k−1 (the physics and camera of tick k run
//!   after the rules), so the distance to the stop is `row[k−1].units_to_stop × 10`.
//!
//! Row k holds the rule globals after the rules, the clock step and the HUD transfer.
//!
//! A replay *stretch* starts at the first row of a pre-departure wait (`0x855ff8 & 0x800`),
//! where the original has just built the segment's signals (0x40b970). The engine is set up from
//! that row's logged globals and stepped until it reaches the station (or game over). The
//! remaining-time transfer phase (HUD task counter, not logged) is inferred from the first
//! visible transfer in the stretch.
//!
//! Inputs the engine needs that a trace may lack (reported as gaps): `buttons_held` /
//! `buttons_pressed` (horn and ATS confirm; assumed released), `track_class_ahead` (tunnel test
//! for wet rails), `units_to_stop` (falls back to the previous row's `dist_to_stop`).

use anyhow::{Context, Result, bail};
use ddg_engine::physics::TrainTables;
use ddg_engine::rules::{Phase, RuleTables, Rules, TickInput, TrainReadout};
use ddg_formats::exe::Exe;
use ddg_formats::stages::{Stage, StageTables};
use std::path::Path;

use super::Trace;

const DRIVING: i64 = 0x200;
const PRE_DEPARTURE: i64 = 0x800;
/// Set by time over (0x4085b7) while the train coasts to a stop.
const COAST: i64 = 0x10;
/// Active-low button bits (input.md §6).
const HORN: i64 = 0x100;
const CONFIRM: i64 = 0x10_0000;
/// Course / limit event list pointer tables (routes.md §8) and record size.
const COURSE_LISTS: u32 = 0x485800;
const LIMIT_LISTS: u32 = 0x485850;
const EVENT_SIZE: i64 = 12;

/// Mismatch bookkeeping for one compared field.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldReport {
    pub name: &'static str,
    pub compared: usize,
    pub mismatches: usize,
    pub first: Option<String>,
}

/// Result of a rules replay.
#[derive(Debug, Default)]
pub struct RulesReport {
    pub stretches: usize,
    pub ticks: usize,
    pub fields: Vec<FieldReport>,
    /// Inputs or fields the trace does not have.
    pub gaps: Vec<String>,
}

impl RulesReport {
    pub fn mismatches(&self) -> usize {
        self.fields.iter().map(|f| f.mismatches).sum()
    }

    fn note(&mut self, name: &'static str, ok: bool, detail: impl FnOnce() -> String) {
        let i = match self.fields.iter().position(|f| f.name == name) {
            Some(i) => i,
            None => {
                self.fields.push(FieldReport {
                    name,
                    compared: 0,
                    mismatches: 0,
                    first: None,
                });
                self.fields.len() - 1
            }
        };
        let f = &mut self.fields[i];
        f.compared += 1;
        if !ok {
            f.mismatches += 1;
            if f.first.is_none() {
                f.first = Some(detail());
            }
        }
    }

    fn gap(&mut self, what: &str) {
        if !self.gaps.iter().any(|g| g == what) {
            self.gaps.push(what.to_owned());
        }
    }
}

/// Engine value of a compared field.
#[derive(Debug, Clone, Copy)]
enum Out {
    Int(i64),
    Float(f32),
}

/// Start addresses of each stage's course and limit lists, to turn logged cursors into indices.
pub type ListBases = Vec<(u32, u32)>;

fn engine_fields(r: &Rules, bases: (u32, u32)) -> Vec<(&'static str, Out)> {
    use Out::{Float as F, Int as I};
    let b = |v: bool| I(i64::from(v));
    let (course, limits) = r.event_cursors();
    vec![
        ("remaining_time", I(r.remaining_s.into())),
        ("pending_time", I(r.pending_s.into())),
        ("stage_clock", I(r.clock.into())),
        ("sched_arrival", I(r.scheduled.into())),
        ("clock_skew", I(r.clock_skew.into())),
        ("station_index", I(r.station as i64)),
        ("dist_to_stop_r", I(r.dist_round_m.into())),
        (
            "course_cursor",
            I(i64::from(bases.0) + course as i64 * EVENT_SIZE),
        ),
        (
            "limit_cursor",
            I(i64::from(bases.1) + limits as i64 * EVENT_SIZE),
        ),
        ("block_index", I(r.block as i64)),
        ("block_initial", I(r.block_count as i64)),
        ("aspect", I(r.aspect.into())),
        ("signal_limit", F(r.signal_limit_mps)),
        ("signal_enforce", I(r.signal_state.into())),
        ("signal_enforce_odo", I(r.signal_enforce_odo.into())),
        ("restricted_left", I(r.restricted_blocks.into())),
        ("repeater_offset", I(r.repeater_m.into())),
        ("signal_display", I(r.signal_variant.into())),
        ("speed_limit", F(r.limit_mps)),
        ("limit_state", I(r.limit_state.into())),
        ("limit_enforce_odo", I(r.limit_enforce_odo.into())),
        ("ats_state", I(r.ats.into())),
        ("ats_ticks", I(r.ats_ticks.into())),
        ("ats_chime_start", I(r.ats_chime_from.into())),
        ("horn_state", I(r.horn_state.into())),
        ("horn_deadline", I(r.horn_deadline.into())),
        ("no_power_state", I(r.coasting.into())),
        ("early_acc", I(r.early_ticks.into())),
        ("target_stops", b(r.target_stops)),
        ("final_station", b(r.final_station)),
        ("approach_state", I(r.approach.into())),
        ("approach_kmh", F(r.approach_kmh)),
        ("eb_ticks", I(r.eb_ticks.into())),
        ("eb_count", I(r.eb_count.into())),
        ("eb_ticks_approach", I(r.eb_approach_ticks.into())),
        ("repower", I(r.repowered.into())),
        ("excuse", I(r.repower_excused.into())),
        ("stopped_ticks", I(r.stopped_ticks.into())),
        ("forced_stop", b(r.forced_stop)),
        ("obstacle_state", I(r.obstacle.into())),
        ("obstacle_armed", b(r.obstacle_armed)),
        ("obstacle_used", b(r.obstacle_used)),
        ("obstacle_target", F(r.obstacle_odo)),
        ("derail_kmh", F(r.derail_kmh)),
        ("weather_on", b(r.weather_on)),
        ("weather_level", I(r.weather_level.into())),
        ("wet_rails", b(r.wet_rails)),
        ("last_lateness", I(r.late_s.into())),
        ("stop_streak", I(r.stop_streak.into())),
        ("pass_streak", I(r.pass_streak.into())),
        ("stop_error_sum", F(r.stop_error_sum)),
        ("stop_count", I(r.stops.into())),
        ("continues", I(r.continues.into())),
        ("stat0", I(r.stats_total[0].into())),
        ("stat1", I(r.stats_total[1].into())),
        ("stat2", I(r.stats_total[2].into())),
        ("stat3", I(r.stats_total[3].into())),
        ("stat4", I(r.stats_total[4].into())),
        ("stat5", I(r.stats_total[5].into())),
        ("stat6", I(r.stats_total[6].into())),
        ("stat7", I(r.stats_total[7].into())),
        ("stat8", I(r.stats_total[8].into())),
        ("stat9", I(r.stats_total[9].into())),
        ("stat10", I(r.stats_total[10].into())),
        ("stat11", I(r.stats_total[11].into())),
    ]
}

/// Columns a trace must have for a rules replay.
const REQUIRED: [&str; 7] = [
    "phase_flags",
    "stage",
    "difficulty",
    "station_index",
    "remaining_time",
    "pending_time",
    "stage_clock",
];

/// Whether a trace carries the rules schema.
pub fn has_rules_columns(t: &Trace) -> bool {
    REQUIRED.iter().all(|c| t.has(c))
}

/// Reads the per-stage list bases from the exe.
pub fn list_bases(exe: &Exe<'_>, stages: usize) -> Result<ListBases> {
    (0..stages as u32)
        .map(|s| {
            Ok((
                exe.u32_at(COURSE_LISTS + 4 * s)?,
                exe.u32_at(LIMIT_LISTS + 4 * s)?,
            ))
        })
        .collect()
}

/// CLI entry: replays the rules over a trace file and prints the report.
pub fn replay_rules(trace_path: &Path, exe_path: &Path, verbose: bool) -> Result<bool> {
    let t = Trace::load(trace_path)?;
    if !has_rules_columns(&t) {
        bail!("trace has no rules columns (needs oracle/rules.schema or full.schema)");
    }
    let bytes = std::fs::read(exe_path)?;
    let exe = Exe::parse(&bytes)?;
    let tables = RuleTables::read(&exe)?;
    let stages = StageTables::read(&exe)?;
    let bases = list_bases(&exe, stages.stages.len())?;
    let train = TrainTables::read(&exe)?;
    let report = replay(&t, &tables, Some(&train), &stages.stages, &bases, verbose)?;
    print_report(&report);
    Ok(report.mismatches() == 0)
}

pub fn print_report(report: &RulesReport) {
    println!(
        "rules: {} stretches, {} ticks, {} mismatching values",
        report.stretches,
        report.ticks,
        report.mismatches()
    );
    for f in &report.fields {
        if f.mismatches > 0 {
            println!(
                "  {:<20} {:>7} / {:<7} differ; first: {}",
                f.name,
                f.mismatches,
                f.compared,
                f.first.as_deref().unwrap_or("")
            );
        }
    }
    let exact: Vec<_> = report
        .fields
        .iter()
        .filter(|f| f.mismatches == 0)
        .map(|f| f.name)
        .collect();
    println!("  exact: {}", exact.join(" "));
    for g in &report.gaps {
        println!("  gap: {g}");
    }
}

/// Paused rows repeat the global tick (only the render list runs); the rules don't step.
fn paused(t: &Trace, r: usize) -> bool {
    r > 0 && t.has("global_tick") && t.get(r, "global_tick") == t.get(r - 1, "global_tick")
}

fn phase(t: &Trace, r: usize) -> i64 {
    t.get(r, "phase_flags").int().unwrap_or(0)
}

fn int_or(t: &Trace, r: usize, name: &str, default: i64) -> i64 {
    t.get(r, name).int().unwrap_or(default)
}

/// Replays every stretch of `t`.
pub fn replay(
    t: &Trace,
    tables: &RuleTables,
    train: Option<&TrainTables>,
    stages: &[Stage],
    bases: &[(u32, u32)],
    verbose: bool,
) -> Result<RulesReport> {
    let mut report = RulesReport::default();
    for (col, gap) in [
        (
            "buttons_held",
            "buttons_held not logged: horn assumed released",
        ),
        (
            "buttons_pressed",
            "buttons_pressed not logged: horn/ATS confirm presses assumed absent",
        ),
        (
            "track_class_ahead",
            "track_class_ahead not logged: never in a tunnel",
        ),
        (
            "units_to_stop",
            "units_to_stop not logged: distance input from the previous row's dist_to_stop",
        ),
    ] {
        if !t.has(col) {
            report.gap(gap);
        }
    }
    let mut r = 0;
    while r < t.rows.len() {
        let starts =
            phase(t, r) & PRE_DEPARTURE != 0 && (r == 0 || phase(t, r - 1) & PRE_DEPARTURE == 0);
        if !starts {
            r += 1;
            continue;
        }
        let cx = Ctx {
            tables,
            train,
            stages,
            bases,
            verbose,
        };
        r = replay_stretch(t, &cx, r, &mut report)?;
    }
    for f in &report.fields {
        if !t.has(f.name) && f.name != "phase" {
            report.gaps.push(format!("{} not logged", f.name));
        }
    }
    report.fields.retain(|f| t.has(f.name) || f.name == "phase");
    Ok(report)
}

/// Sets up the engine from row `s` (first pre-departure row) and steps it. Returns the next row.
/// Read-only inputs of a replay.
struct Ctx<'a> {
    tables: &'a RuleTables,
    train: Option<&'a TrainTables>,
    stages: &'a [Stage],
    bases: &'a [(u32, u32)],
    verbose: bool,
}

fn replay_stretch(t: &Trace, cx: &Ctx<'_>, s: usize, report: &mut RulesReport) -> Result<usize> {
    let Ctx {
        tables,
        train,
        stages,
        bases,
        verbose,
    } = *cx;
    let stage = usize::try_from(t.i(s, "stage")?).context("stage")?;
    let difficulty = usize::try_from(t.i(s, "difficulty")?).context("difficulty")?;
    let station = usize::try_from(t.i(s, "station_index")?).context("station")?;
    let data = stages.get(stage).context("stage out of range")?;
    let base = bases.get(stage).copied().unwrap_or((0, 0));
    let Ok(mut e) = Rules::new(tables, data, stage, difficulty, station) else {
        return Ok(s + 1); // e.g. the final station: nothing to drive
    };
    sync_from_row(&mut e, t, s, base);
    e.hud_counter = hud_phase(t, s);
    report.stretches += 1;
    compare(t, s, &e, base, report, verbose);

    let mut prev = s;
    let mut r = s + 1;
    while r < t.rows.len() {
        if paused(t, r) {
            r += 1;
            continue;
        }
        if matches!(e.phase, Phase::Arrived | Phase::GameOver | Phase::Cleared) {
            return Ok(r);
        }
        let input = tick_input(t, prev, r, report);
        let flags = phase(t, r);
        let mut out = e.step(&input);
        let readout = train_readout(t, r, train);
        if readout.is_none() {
            report.gap("physics readout columns missing: ride-comfort statistics skipped");
        }
        e.end_tick(&mut out, readout.as_ref());
        report.ticks += 1;
        let engine_waiting = e.phase == Phase::PreDeparture && !e.departure_done;
        if engine_waiting != (flags & PRE_DEPARTURE != 0) {
            report.note("phase", false, || {
                format!("row {r}: pre-departure wait engine {engine_waiting}, original {flags:#x}")
            });
        }
        // The arrival and recovery sequences clear 0x200 inside the same iteration, and time
        // over sets 0x10 (phase 0x210 while the train coasts); a still-driving engine against
        // such a row is a divergence.
        if e.phase == Phase::Driving && (flags & DRIVING == 0 || flags & COAST != 0) {
            report.note("phase", false, || {
                format!(
                    "row {r}: original left the driving loop ({flags:#x}), engine still driving"
                )
            });
            return Ok(r);
        }
        compare(t, r, &e, base, report, verbose);
        prev = r;
        r += 1;
    }
    Ok(r)
}

/// The train task's outputs for row `r` (physics columns of the same row).
fn train_readout(t: &Trace, r: usize, train: Option<&TrainTables>) -> Option<TrainReadout> {
    let spec = train?
        .specs
        .get(usize::try_from(t.get(r, "train_type").int()?).ok()?)?;
    Some(TrainReadout {
        speed_after: t.get(r, "speed").f32()?,
        decel_prev_tick: t.get(r, "speed_lost").f32()?,
        brake_decel: t.get(r, "brake_decel").f32()?,
        brake_delta: i32::try_from(t.get(r, "brake_delta").int()?).ok()?,
        brake_applied: i32::try_from(t.get(r, "t_last_brake").int()?).ok()?,
        jolt_limit: spec.jolt_limit,
        top_speed_kmh: spec.max_speed_kmh(),
    })
}

/// The engine state at a segment start, from the logged globals.
fn sync_from_row(e: &mut Rules, t: &Trace, r: usize, base: (u32, u32)) {
    let i = |n: &str| t.get(r, n).int();
    let f = |n: &str| t.get(r, n).f32();
    if let Some(v) = i("stage_clock") {
        e.clock = v as i32;
    }
    if let Some(v) = i("sched_arrival") {
        e.scheduled = v as i32;
    }
    if let Some(v) = i("remaining_time") {
        e.remaining_s = v as i32;
    }
    if let Some(v) = i("pending_time") {
        e.pending_s = v as i32;
    }
    if let Some(v) = i("clock_skew") {
        e.clock_skew = v as i32;
    }
    if let Some(v) = i("early_acc") {
        e.early_ticks = v as i32;
    }
    if let Some(v) = f("speed_limit") {
        e.limit_mps = v;
    }
    if let Some(v) = i("limit_state") {
        e.limit_state = v as i32;
    }
    if let Some(v) = i("limit_enforce_odo") {
        e.limit_enforce_odo = v as i32;
    }
    if let Some(v) = i("signal_display") {
        e.signal_variant = v as i16;
    }
    if let Some(v) = i("weather_on") {
        e.weather_on = v != 0;
    }
    if let Some(v) = i("weather_level") {
        e.weather_level = v as i32;
    }
    if let Some(v) = i("wet_rails") {
        e.wet_rails = v != 0;
    }
    if let Some(v) = i("stop_streak") {
        e.stop_streak = v as i32;
    }
    if let Some(v) = i("pass_streak") {
        e.pass_streak = v as i32;
    }
    if let Some(v) = f("stop_error_sum") {
        e.stop_error_sum = v;
    }
    if let Some(v) = i("stop_count") {
        e.stops = v as i32;
    }
    if let Some(v) = i("continues") {
        e.continues = v as i32;
    }
    if let Some(v) = i("obstacle_used") {
        e.obstacle_used = v != 0;
    }
    if let Some(v) = f("odometer") {
        e.odometer_m = v;
    }
    if let Some(v) = i("t_last_power") {
        e.prev_power = v as i32;
    }
    if let Some(v) = i("t_last_brake") {
        e.prev_brake = v as i32;
    }
    for (k, total) in e.stats_total.iter_mut().enumerate() {
        if let Some(v) = t.get(r, &format!("stat{k}")).int() {
            *total = v as i32;
        }
    }
    let idx = |ptr: Option<i64>, base: u32| {
        ptr.and_then(|p| usize::try_from((p - i64::from(base)) / EVENT_SIZE).ok())
    };
    let (c, l) = e.event_cursors();
    let course = idx(i("course_cursor"), base.0).unwrap_or(c);
    let limits = idx(i("limit_cursor"), base.1).unwrap_or(l);
    e.set_event_cursors(course, limits);
    // 0x40cf40 cleared the aspect before 0x40b970 built this segment's signals.
    e.aspect = 0;
    e.rebuild_signals();
    e.phase = Phase::PreDeparture;
    // State the setup tick (row r itself: 0x40dad0 priming, one `Task_Tick`) already changed.
    let set_i32 = |v: &mut i32, n: &str| {
        if let Some(x) = i(n) {
            *v = x as i32;
        }
    };
    set_i32(&mut e.dist_round_m, "dist_to_stop_r");
    set_i32(&mut e.eb_count, "eb_count");
    set_i32(&mut e.eb_ticks, "eb_ticks");
    set_i32(&mut e.eb_approach_ticks, "eb_ticks_approach");
    set_i32(&mut e.ats_chime_from, "ats_chime_start");
    set_i32(&mut e.ats_ticks, "ats_ticks");
    set_i32(&mut e.stopped_ticks, "stopped_ticks");
    set_i32(&mut e.late_s, "last_lateness");
    if let Some(v) = f("dist_to_stop") {
        e.dist_m = v;
    }
    if let Some(v) = i("ats_state") {
        e.ats = v as i16;
    }
    // The departure task ran once (counter 0) in the setup tick.
    e.departure_counter = 1;
}

/// HUD transfer phase: the first tick in the stretch whose row shows a ±1 s transfer had the HUD
/// counter at 2 (mod 3); the counter steps once per unpaused row.
fn hud_phase(t: &Trace, s: usize) -> u32 {
    let mut n = 0u32;
    let mut prev = s;
    for r in s + 1..t.rows.len() {
        if paused(t, r) {
            continue;
        }
        if phase(t, r) & PRE_DEPARTURE != 0 && phase(t, r - 1) & PRE_DEPARTURE == 0 {
            break; // next segment
        }
        n += 1;
        let rem = |k| t.get(k, "remaining_time").int();
        let pend = t.get(prev, "pending_time").int().unwrap_or(0);
        if let (Some(a), Some(b)) = (rem(prev), rem(r)) {
            let d = b - a;
            if (d == 1 && pend > 0) || (d == -1 && pend < 0) {
                // Counter before this (n-th) tick ≡ 2: start = 2 − (n − 1).
                return (2 + 3 * 1000 - (n - 1) % 3) % 3;
            }
        }
        prev = r;
    }
    0
}

fn tick_input(t: &Trace, prev: usize, r: usize, report: &mut RulesReport) -> TickInput {
    let f = |k: usize, n: &str| t.get(k, n).f32().unwrap_or(0.0);
    let dist = match t.get(prev, "units_to_stop").f32() {
        // 0x40dad0: fld units; fmul 10.0 (double); fiadd 0; fst.
        Some(u) => (f64::from(u) * 10.0) as f32,
        None => f(prev, "dist_to_stop"),
    };
    let held = t.get(r, "buttons_held").int();
    let pressed = t.get(r, "buttons_pressed").int();
    let mut input = TickInput {
        speed_mps: f(prev, "speed"),
        dist_to_stop_m: dist,
        dist_to_following_stop_m: f(r, "dist_to_stop"),
        odometer_m: f(prev, "odometer"),
        power: int_or(t, r, "power_notch", 0) as i32,
        brake: int_or(t, r, "brake_notch", 0) as i32,
        in_tunnel: int_or(t, r, "track_class_ahead", 0) == 3,
        ..TickInput::default()
    };
    // Active-low words: a clear bit means pressed.
    input.buttons.horn_held = held.is_some_and(|w| w & HORN == 0);
    input.buttons.horn_pressed = pressed.is_some_and(|w| w & HORN == 0);
    input.buttons.confirm_pressed = pressed.is_some_and(|w| w & CONFIRM == 0);
    if !t.has("speed") {
        report.gap("speed not logged");
    }
    input
}

fn compare(
    t: &Trace,
    r: usize,
    e: &Rules,
    base: (u32, u32),
    report: &mut RulesReport,
    verbose: bool,
) {
    for (name, mine) in engine_fields(e, base) {
        let mut theirs = t.get(r, name);
        // 0x857094 is an i16; the next short (0x857096, turnout flag of 0x407c10) shares the
        // dword when a schema logs it as i32.
        if name == "pass_streak"
            && let super::Value::Int(v) = theirs
        {
            theirs = super::Value::Int(i64::from(v as u16 as i16));
        }
        let (ok, shown) = match (mine, theirs) {
            (_, super::Value::Missing) => continue,
            (Out::Int(m), v) => match v.int() {
                Some(o) => (m == o, format!("engine {m} vs original {o}")),
                None => continue,
            },
            (Out::Float(m), v) => match v.f32() {
                Some(o) => (
                    m.to_bits() == o.to_bits(),
                    format!(
                        "engine {m} ({:08x}) vs original {o} ({:08x})",
                        m.to_bits(),
                        o.to_bits()
                    ),
                ),
                None => continue,
            },
        };
        if !ok && verbose {
            println!("row {r}: {name} {shown}");
        }
        report.note(name, ok, || format!("row {r}: {shown}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ddg_engine::rules::{DiffTable, ScriptedSignal};
    use ddg_formats::stages::{
        CourseEvent, DIFFICULTY_COUNT, STAGE_COUNT, StageSegment, TimetableRow,
    };
    use std::fmt::Write as _;

    fn diff(v: i16) -> DiffTable {
        DiffTable([[v; DIFFICULTY_COUNT]; STAGE_COUNT])
    }

    fn tables() -> RuleTables {
        RuleTables {
            limit_margin_kmh: diff(5),
            stop_tolerance_m: diff(2),
            start_time_s: diff(150),
            late_drain_s: diff(1),
            limit_overspeed_s: diff(10),
            emergency_brake_s: diff(5),
            signal_overspeed_min_s: diff(-1),
            signal_overspeed_cap_s: diff(15),
            false_start_s: diff(2),
            stopped_between_s: diff(5),
            horn_missed_s: diff(5),
            repower_s: diff(10),
            horn_bonus_s: diff(3),
            exact_stop_bonus_s: diff(10),
            on_time_stop_bonus_s: diff(5),
            clean_stop_bonus_s: diff(3),
            on_time_pass_bonus_s: diff(5),
            coasting_bonus_s: diff(5),
            limit_notice_m: diff(300),
            weather_min_time_s: diff(0),
            early_ticks: std::array::from_fn(|i| diff(240 + 60 * i as i16)),
            block_length_m: [330; STAGE_COUNT],
            weather_station: [99; STAGE_COUNT],
            weather_ticks: [9000; STAGE_COUNT],
            stage_kind: [3; STAGE_COUNT],
            aspect_patterns: [[1, 2, 4, 4, 4]; 8],
            scripted_patterns: [[1, 4, 4, 4, 4]; 2],
            scripted_signals: Vec::<ScriptedSignal>::new(),
            signal_speed_kmh: [70.0, 45.0, 25.0],
        }
    }

    fn stage() -> Stage {
        let row = |time_ticks, position_m, to_next_m| TimetableRow {
            time_ticks,
            position_m,
            to_next_m,
            stops: 1,
            params: [0, 300, 150, 0, -999],
        };
        // Scheduled tightly so the run is late and the lateness drain moves time.
        let tt = vec![
            row(1_000_000, 0, 1500),
            row(1_000_000 + 0x2b2 + 1800, 1500, 0),
        ];
        Stage {
            area: 1,
            train_sets: Vec::new(),
            segments: vec![StageSegment {
                com: 1,
                first_station: 0,
            }],
            track_sounds: [0; 32],
            timetables: std::array::from_fn(|_| tt.clone()),
            station_names: vec!["A".into(), "B".into()],
            course_events: Vec::new(),
            limit_events: vec![CourseEvent {
                station: 0,
                distance_m: 1200,
                kind: 4,
                args: [40, 0, -1],
            }],
            traffic_events: Vec::new(),
        }
    }

    const COLUMNS: &str = "global_tick,phase_flags,stage,difficulty,station_index,remaining_time,\
pending_time,stage_clock,sched_arrival,speed,odometer,units_to_stop,dist_to_stop,power_notch,\
brake_notch,buttons_held,buttons_pressed,aspect,speed_limit,limit_state,limit_cursor,ats_state,\
approach_state,stop_count,stop_error_sum,block_index,stat5";

    fn float(v: f32) -> String {
        format!("{v}:{:08x}", v.to_bits())
    }

    /// Writes a trace the way the hook would, from the engine itself plus a toy train: the
    /// rules of tick k see row k−1's speed/position and row k's notches.
    fn synthetic_trace() -> String {
        let mut e = Rules::new(&tables(), &stage(), 3, 1, 0).unwrap();
        let mut out = String::from(COLUMNS);
        out.push('\n');
        let (mut speed, mut odo, mut units) = (0.0f32, 0.0f32, 150.0f32);
        let mut tick = 0u32;
        let mut row = |e: &Rules,
                       flags: i64,
                       speed: f32,
                       odo: f32,
                       units: f32,
                       power: i32,
                       brake: i32| {
            tick += 1;
            let (_, limits) = e.event_cursors();
            writeln!(
                out,
                "{tick},{flags},3,1,{},{},{},{},{},{},{},{},{},{power},{brake},{},{},{},{},{},{},{},{},{},{},{},{}",
                e.station,
                e.remaining_s,
                e.pending_s,
                e.clock,
                e.scheduled,
                float(speed),
                float(odo),
                float(units),
                float(e.dist_m),
                0xffff_ffff_u32,
                0xffff_ffff_u32,
                e.aspect,
                float(e.limit_mps),
                e.limit_state,
                limits as i64 * 12,
                e.ats,
                e.approach,
                e.stops,
                float(e.stop_error_sum),
                e.block,
                e.stats_total[5],
            )
            .unwrap();
        };
        // The setup tick: one `Task_Tick` (train task, departure task, HUD) before the wait.
        let mut setup = ddg_engine::rules::TickOutput::default();
        e.end_tick(&mut setup, None);
        row(&e, PRE_DEPARTURE, speed, odo, units, 0, 4);
        // The departure countdown (brake held, so no brake-release statistic).
        while !e.departure_done {
            let input = TickInput {
                brake: 4,
                ..TickInput::default()
            };
            e.pre_departure_tick(&input);
            let flags = if e.departure_done { 0 } else { PRE_DEPARTURE };
            row(&e, flags, speed, odo, units, 0, 4);
        }
        for _ in 0..20_000 {
            let dist = (f64::from(units) * 10.0) as f32;
            let power = i32::from(speed < 15.0 && dist > 400.0);
            let brake = if dist < 400.0 { 4 } else { 0 };
            let input = TickInput {
                speed_mps: speed,
                dist_to_stop_m: dist,
                dist_to_following_stop_m: dist,
                odometer_m: odo,
                power,
                brake,
                ..TickInput::default()
            };
            let o = e.tick(&input);
            // Toy physics: accelerate to 15 m/s (over the 45 km/h limit), brake to a stop at 0.
            if o.force_stop || e.phase != Phase::Driving {
                speed = 0.0;
            } else if dist < 400.0 {
                let want = (2.0 * 0.3 * dist.max(0.0)).sqrt();
                speed = if dist <= 0.05 {
                    0.0
                } else {
                    want.min(speed).max(0.2)
                };
            } else if speed < 15.0 {
                speed += 0.03;
            }
            let step = (speed / 30.0).min(dist.max(0.0));
            odo += step;
            units -= step / 10.0;
            let flags = if e.phase == Phase::Driving {
                DRIVING
            } else {
                0
            };
            row(&e, flags, speed, odo, units, power, brake);
            if e.phase == Phase::Arrived {
                break;
            }
        }
        assert_eq!(e.phase, Phase::Arrived, "toy run must reach the station");
        out
    }

    fn run(text: &str) -> RulesReport {
        let t = Trace::parse(text).unwrap();
        assert!(has_rules_columns(&t));
        replay(
            &t,
            &tables(),
            None,
            &[stage(), stage(), stage(), stage()],
            &[(0, 0); 4],
            false,
        )
        .unwrap()
    }

    #[test]
    fn self_generated_trace_replays_exactly() {
        let text = synthetic_trace();
        // The toy run must exercise the overspeed penalty and the HUD transfer.
        let t = Trace::parse(&text).unwrap();
        let last = t.rows.len() - 1;
        assert_eq!(t.get(last, "stat5").int(), Some(5));
        assert!((0..t.rows.len()).any(|r| t.get(r, "pending_time").int().is_some_and(|p| p < -1)));
        let report = run(&text);
        assert_eq!(report.stretches, 1);
        assert!(report.ticks > 1000, "{}", report.ticks);
        let bad: Vec<_> = report.fields.iter().filter(|f| f.mismatches > 0).collect();
        assert!(bad.is_empty(), "{bad:?}");
        let names: Vec<_> = report.fields.iter().map(|f| f.name).collect();
        for n in [
            "remaining_time",
            "pending_time",
            "speed_limit",
            "limit_state",
            "stat5",
        ] {
            assert!(names.contains(&n), "{n} compared");
        }
        assert!(report.gaps.iter().any(|g| g.contains("track_class_ahead")));
    }

    #[test]
    fn a_wrong_value_is_reported_once_per_field() {
        let text = synthetic_trace();
        let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
        // Corrupt remaining_time (column 5) in a driving row.
        let k = lines.len() / 2;
        let mut cells: Vec<String> = lines[k].split(',').map(str::to_owned).collect();
        let v: i64 = cells[5].parse().unwrap();
        cells[5] = (v + 7).to_string();
        lines[k] = cells.join(",");
        let report = run(&(lines.join("\n") + "\n"));
        let f = report
            .fields
            .iter()
            .find(|f| f.name == "remaining_time")
            .unwrap();
        assert_eq!(f.mismatches, 1);
        assert!(
            f.first
                .as_deref()
                .unwrap()
                .starts_with(&format!("row {}", k - 1))
        );
    }

    /// Every rules trace in `DDG_TRACE_DIR` must replay without a single mismatch. Needs
    /// `DDG_DATA_DIR`; skips traces without rule columns.
    #[test]
    fn oracle_traces_replay_rules() {
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
        let tables = RuleTables::read(&exe).unwrap();
        let stages = StageTables::read(&exe).unwrap();
        let bases = list_bases(&exe, stages.stages.len()).unwrap();
        let mut paths: Vec<_> = std::fs::read_dir(traces)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "csv"))
            .collect();
        paths.sort();
        for path in paths {
            let t = Trace::load(&path).unwrap();
            if !has_rules_columns(&t) {
                continue;
            }
            let train = TrainTables::read(&exe).unwrap();
            let report = replay(&t, &tables, Some(&train), &stages.stages, &bases, false).unwrap();
            eprintln!("{}:", path.display());
            print_report(&report);
            assert!(
                report.stretches > 0,
                "{}: no stretch replayed",
                path.display()
            );
            assert_eq!(report.mismatches(), 0, "{}", path.display());
        }
    }
}

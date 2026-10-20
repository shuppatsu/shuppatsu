//! Drive replay: runs `shuppatsu_engine::drive::Drive` end to end from a trace's inputs alone and
//! compares every logged column.
//!
//! Inputs taken from the trace: the latched notches and buttons of each row, and the stage,
//! difficulty (inferred from the train tables if not logged) and start station of the first
//! pre-departure row. Everything else (gradient, heading, distance to the stop, station passing,
//! segment changes, odometer, clock, rules, physics) is computed by the engine.
//!
//! Alignment (rows are written at the end of `Task_Tick`, before the input poll; physics.md §8):
//! * The `Drive` is created at the route load, two rows before the first `0x800` row (the
//!   physics task's first row) and ticked once per unpaused row (`global_tick` repeats while
//!   paused).
//! * Menus: the trace shows when the original's screens closed and the replay makes the matching
//!   call. `finish_station` is called before the second row after the result screen closed
//!   (`0x855ff8 & 0x40` cleared), `continue_run` before the third row after the continue screen
//!   closed with "continue" (`0x10` cleared, `2` set). The engine then runs the original's fixed
//!   number of idle ticks, and the replay checks that its setup tick lands on the next `0x800`
//!   row.
//! * The HUD transfer phase (counter of the HUD task since the stage load, which depends on the
//!   stage intro) is inferred once from the first visible ±1 s transfer.
//!
//! Not compared (documented in physics.md §8):
//! * Before the first segment's setup tick: the camera globals (`heading`, `engine_index`,
//!   `units_to_stop`, `dist_to_stop[_r]`) and the physics copy of the heading still belong to
//!   the stage intro's camera.
//! * Rule columns while stopped at a station (`Arrived`): the result screen deducts the time
//!   over several ticks, the engine in `finish_station`. All columns are compared again from the
//!   row after `finish_station`.

use anyhow::{Context, Result, bail};
use shuppatsu_engine::drive::{Drive, DriveError, DrivePhase, DriverInput, LoadError};
use shuppatsu_engine::physics::TrainTables;
use shuppatsu_engine::rules::RuleTables;
use shuppatsu_engine::rules::{RuleEvent, TimeReason};
use shuppatsu_formats::exe::Exe;
use shuppatsu_formats::stages::StageTables;
use std::path::{Path, PathBuf};

use super::rules::{FieldReport, ListBases, Out, engine_fields, list_bases, paused, phase};
use super::{NotchTiming, Trace, Value};

const DRIVING: i64 = 0x200;
const PRE_DEPARTURE: i64 = 0x800;
const COAST: i64 = 0x10;
const RESULT: i64 = 0x40;
const STAGE_ACTIVE: i64 = 0x2;
const HORN: i64 = 0x100;
const CONFIRM: i64 = 0x10_0000;

/// Columns that hold the stage intro camera's values until the first setup tick.
const INTRO_CAMERA: [&str; 5] = [
    "heading",
    "engine_index",
    "units_to_stop",
    "dist_to_stop",
    "dist_to_stop_r",
];

/// Result of a drive replay.
#[derive(Debug, Default)]
pub struct DriveReport {
    pub ticks: usize,
    pub segments: usize,
    pub results: usize,
    pub continues: usize,
    /// Row at which the replay stopped early, and why.
    pub stopped: Option<String>,
    /// Row at which the original's run ended (game over without a continue).
    pub ended: Option<String>,
    pub fields: Vec<FieldReport>,
    pub gaps: Vec<String>,
}

impl DriveReport {
    pub fn mismatches(&self) -> usize {
        self.fields.iter().map(|f| f.mismatches).sum()
    }

    pub fn compared(&self) -> usize {
        self.fields.iter().map(|f| f.compared).sum()
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
}

/// Everything the replay reads from the exe.
pub struct Tables {
    pub train: TrainTables,
    pub rules: RuleTables,
    pub stages: StageTables,
    pub bases: ListBases,
}

impl Tables {
    pub fn read(exe: &Exe<'_>) -> Result<Self> {
        let stages = StageTables::read(exe)?;
        let bases = list_bases(exe, stages.stages.len())?;
        Ok(Self {
            train: TrainTables::read(exe)?,
            rules: RuleTables::read(exe)?,
            stages,
            bases,
        })
    }
}

/// A route loader over a `DATA` directory (case-insensitive names).
pub fn data_loader(dir: PathBuf) -> impl FnMut(&str) -> Result<Vec<u8>, LoadError> + Send {
    move |name: &str| -> Result<Vec<u8>, LoadError> {
        let path = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name))
            })
            .ok_or_else(|| format!("{name} not found in {}", dir.display()))?;
        Ok(std::fs::read(path)?)
    }
}

/// CLI entry.
pub fn replay_drive(
    trace_path: &Path,
    exe_path: &Path,
    data_dir: &Path,
    verbose: bool,
) -> Result<bool> {
    let t = Trace::load(trace_path)?;
    let bytes = std::fs::read(exe_path)?;
    let tables = Tables::read(&Exe::parse(&bytes)?)?;
    let report = replay(&t, &tables, data_dir, verbose)?;
    print_report(&report);
    Ok(report.mismatches() == 0 && report.stopped.is_none())
}

pub fn print_report(r: &DriveReport) {
    println!(
        "drive: {} ticks, {} segments, {} station results, {} continues; {} values compared, {} mismatching",
        r.ticks,
        r.segments,
        r.results,
        r.continues,
        r.compared(),
        r.mismatches()
    );
    for f in &r.fields {
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
    let exact: Vec<_> = r
        .fields
        .iter()
        .filter(|f| f.mismatches == 0)
        .map(|f| f.name)
        .collect();
    println!("  exact ({}): {}", exact.len(), exact.join(" "));
    if let Some(s) = &r.stopped {
        println!("  stopped early: {s}");
    }
    if let Some(s) = &r.ended {
        println!("  run ended: {s}");
    }
    for g in &r.gaps {
        println!("  gap: {g}");
    }
}

/// The `k`-th unpaused row after `r` (k ≥ 1).
fn after(t: &Trace, r: usize, k: usize) -> usize {
    let mut n = 0;
    let mut i = r;
    while i + 1 < t.rows.len() {
        i += 1;
        if !paused(t, i) {
            n += 1;
            if n == k {
                return i;
            }
        }
    }
    t.rows.len()
}

/// The previous unpaused row.
fn before(t: &Trace, r: usize) -> Option<usize> {
    (0..r).rev().find(|&i| !paused(t, i))
}

/// The inputs of the tick logged in row `r`. In [`NotchTiming::NextRow`] traces (`lag`) the
/// notches come from the previous unpaused row, except on a segment's setup row: 0x4080d0
/// forces them to N / EB before that tick (`input_force_notches(0, 9)`, 0x4332c0; input.md).
fn input(t: &Trace, r: usize, lag: bool) -> DriverInput {
    let int = |n: &str| t.get(r, n).int();
    let held = int("buttons_held");
    let pressed = int("buttons_pressed");
    let setup = phase(t, r) & PRE_DEPARTURE != 0
        && before(t, r).is_none_or(|p| phase(t, p) & PRE_DEPARTURE == 0);
    // The row whose notches tick `r` used; `None`: the setup's forced N / EB.
    let source = match (lag, before(t, r)) {
        (true, Some(_)) if setup => None,
        (true, Some(p)) => Some(p),
        _ => Some(r),
    };
    let notch = |n: &str, forced: i64| source.map_or(Some(forced), |k| t.get(k, n).int());
    DriverInput {
        power: notch("power_notch", 0)
            .and_then(|v| u8::try_from(v).ok())
            .unwrap_or(0),
        brake: notch("brake_notch", 9)
            .and_then(|v| u8::try_from(v).ok())
            .unwrap_or(0),
        // Active-low words: a clear bit means pressed (input.md §6).
        horn_held: held.is_some_and(|w| w & HORN == 0),
        horn_pressed: pressed.is_some_and(|w| w & HORN == 0),
        confirm_pressed: pressed.is_some_and(|w| w & CONFIRM == 0),
    }
}

/// Remaining time as logged (`remaining_time` in the rules schema, `remaining_counter` in the
/// physics schema).
fn remaining(t: &Trace, r: usize) -> Option<i64> {
    t.get(r, "remaining_time")
        .int()
        .or_else(|| t.get(r, "remaining_counter").int())
}

/// HUD counter before the tick of row `from`: the first row whose remaining time moved by ±1
/// without another cause had the counter at 2 (mod 3).
fn hud_counter(t: &Trace, from: usize) -> u32 {
    let mut n = 0u32;
    let mut prev = from;
    for r in from..t.rows.len() {
        if paused(t, r) {
            continue;
        }
        n += 1;
        if r > from
            && let (Some(a), Some(b)) = (remaining(t, prev), remaining(t, r))
        {
            let d = b - a;
            let pend = t.get(prev, "pending_time").int();
            let ok = match pend {
                Some(p) => (d == 1 && p > 0) || (d == -1 && p < 0),
                None => d.abs() == 1,
            };
            if ok {
                // Counter at this (n-th) tick ≡ 2: start ≡ 2 − (n − 1).
                return ((2 + 3 * 100_000 - (n - 1) as i64) % 3) as u32;
            }
        }
        prev = r;
    }
    0
}

/// Difficulty: logged, or the first one whose grade/curve scales and start time fit the trace.
fn difficulty(t: &Trace, s: usize, stage: usize, tables: &Tables) -> Result<usize> {
    if let Some(d) = t.get(s, "difficulty").int() {
        return Ok(usize::try_from(d)?);
    }
    let grade = t.get(s, "grade_scale").f32();
    let curve = t.get(s, "curve_scale").f32();
    let rem = remaining(t, s);
    (0..shuppatsu_formats::stages::DIFFICULTY_COUNT)
        .find(|&d| {
            let p = tables.train.stage_params(stage, d);
            p.is_some_and(|p| {
                grade.is_none_or(|g| g.to_bits() == p.grade_scale.to_bits())
                    && curve.is_none_or(|c| c.to_bits() == p.curve_scale.to_bits())
            }) && rem.is_none_or(|v| i64::from(tables.rules.start_time_s.get(stage, d)) == v)
        })
        .context("no difficulty matches the trace")
}

/// Engine values of every comparable column.
fn engine_values(d: &Drive, bases: (u32, u32)) -> Vec<(&'static str, Out)> {
    use Out::{Float as F, Int as I};
    let mut v = engine_fields(d.rules(), bases);
    for (name, x) in super::outputs(d.train()) {
        v.push((name, F(x)));
    }
    let r = d.rules();
    let tr = d.train();
    let (stage, difficulty) = r.stage();
    v.extend([
        ("global_tick", I(d.global_tick().into())),
        ("traffic_state", I(d.traffic().state.into())),
        ("signal_model", I(r.signal_model.into())),
        ("gradient", F(d.gradient())),
        ("heading", F(d.heading())),
        ("units_to_stop", F(d.units_to_stop())),
        ("engine_index", I(d.pos().engine_index as i64)),
        ("track_class_ahead", I(d.track_class_ahead().into())),
        ("dist_to_stop", F(r.dist_m)),
        ("overrun_state", I(r.ats.into())),
        ("power_cut", I(r.coasting.into())),
        ("slippery", I(r.weather_level.into())),
        ("atc_limit_kmh", F(r.derail_kmh)),
        (
            "brake_load",
            I(r.timetable()
                .get(r.station)
                .map_or(0, |row| row.params[0])
                .into()),
        ),
        ("train_type", I(tr.train_type as i64)),
        ("grade_scale", F(tr.grade_scale)),
        ("curve_scale", F(tr.curve_scale)),
        ("traction_scale", F(tr.traction_scale)),
        ("brake_scale", F(tr.brake_scale)),
        ("stage", I(stage as i64)),
        ("difficulty", I(difficulty as i64)),
        ("sched_next", I(r.scheduled.into())),
        ("seconds_late", I(r.late_s.into())),
        ("remaining_counter", I(r.remaining_s.into())),
        ("clock_speedup", I(r.clock_skew.into())),
        ("clock_freeze", I(r.obstacle.into())),
    ]);
    v
}

fn compare(
    t: &Trace,
    r: usize,
    d: &Drive,
    bases: (u32, u32),
    skip: &dyn Fn(&str) -> bool,
    report: &mut DriveReport,
    verbose: bool,
) {
    for (name, mine) in engine_values(d, bases) {
        if skip(name) {
            continue;
        }
        let mut theirs = t.get(r, name);
        if matches!(name, "pass_streak" | "weather_ticks")
            && let Value::Int(v) = theirs
        {
            // 0x857094 and 0x856fa8 are i16s; the next shorts are other globals (rules.md §14).
            theirs = Value::Int(i64::from(v as u16 as i16));
        }
        let (ok, shown) = match (mine, theirs) {
            (_, Value::Missing) => continue,
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
            println!("row {r}: {name} {shown} [{:?}]", d.phase());
        }
        report.note(name, ok, || format!("row {r} ({:?}): {shown}", d.phase()));
    }
    let flags = phase(t, r);
    let driving = flags & DRIVING != 0;
    let waiting = flags & PRE_DEPARTURE != 0;
    let ok = driving == d.train_driving() && waiting == d.waiting_for_departure();
    report.note("phase", ok, || {
        format!(
            "row {r}: flags {flags:#x}, engine {:?} (driving {}, waiting {})",
            d.phase(),
            d.train_driving(),
            d.waiting_for_departure()
        )
    });
}

/// Replays the whole trace with one `Drive`.
///
/// Traces without button columns cannot show horn presses. When the engine misses a whistle
/// sign there, the replay is repeated with the horn held from the sign to the miss (the
/// original then lost no time, which the logged remaining time shows); each such window is
/// listed as a gap.
pub fn replay(t: &Trace, tables: &Tables, data_dir: &Path, verbose: bool) -> Result<DriveReport> {
    let mut horn: Vec<(usize, usize)> = Vec::new();
    loop {
        let (mut report, missed) = replay_once(t, tables, data_dir, verbose, &horn)?;
        match missed {
            Some(w) if horn.len() < 64 && !horn.contains(&w) => horn.push(w),
            _ => {
                for (a, b) in &horn {
                    report
                        .gaps
                        .push(format!("horn held inferred for rows {a}..={b}"));
                }
                return Ok(report);
            }
        }
    }
}

fn replay_once(
    t: &Trace,
    tables: &Tables,
    data_dir: &Path,
    verbose: bool,
    horn: &[(usize, usize)],
) -> Result<(DriveReport, Option<(usize, usize)>)> {
    let mut report = DriveReport::default();
    let mut sign_row = None;
    for (col, gap) in [
        (
            "buttons_held",
            "buttons_held not logged: horn released except inferred windows",
        ),
        (
            "buttons_pressed",
            "buttons_pressed not logged: horn/ATS confirm presses assumed absent",
        ),
        (
            "difficulty",
            "difficulty not logged: inferred from the train tables",
        ),
    ] {
        if !t.has(col) {
            report.gaps.push(gap.to_owned());
        }
    }
    let s0 = (0..t.rows.len())
        .find(|&r| phase(t, r) & PRE_DEPARTURE != 0)
        .context("no pre-departure row")?;
    let stage = usize::try_from(t.i(s0, "stage")?)?;
    let station = usize::try_from(t.i(s0, "station_index").unwrap_or(0))?;
    let difficulty = difficulty(t, s0, stage, tables)?;
    let start = (0..s0)
        .rev()
        .filter(|&r| !paused(t, r))
        .nth(1)
        .context("no rows before the first setup")?;
    let mut drive = Drive::new(
        &tables.train,
        &tables.rules,
        &tables.stages,
        stage,
        difficulty,
        station,
        data_loader(data_dir.to_owned()),
    )?;
    drive.set_hud_counter(hud_counter(t, start));
    // 0x84c494 after the tick of row `start` is that row's value.
    if let Some(g) = t.get(start, "global_tick").int() {
        drive.set_global_tick((g as u32).wrapping_sub(1));
    }
    let bases = tables.bases.get(stage).copied().unwrap_or((0, 0));
    let lag = t.notch_timing() == NotchTiming::NextRow;

    let mut finish_at: Option<usize> = None;
    let mut continue_at: Option<usize> = None;
    let mut setup_seen = false;
    let mut r = start;
    while r < t.rows.len() {
        if paused(t, r) {
            r += 1;
            continue;
        }
        if finish_at == Some(r) {
            finish_at = None;
            drive.finish_station()?;
            report.results += 1;
        }
        if continue_at == Some(r) {
            continue_at = None;
            match drive.continue_run() {
                Ok(()) => report.continues += 1,
                Err(e @ DriveError::WrongPhase { .. }) => {
                    report.stopped = Some(format!("row {r}: {e}"));
                    break;
                }
                Err(e) => return Err(e.into()),
            }
        }
        let mut inp = input(t, r, lag);
        inp.horn_held |= horn.iter().any(|&(a, b)| (a..=b).contains(&r));
        let tick = drive.tick(inp);
        if tick
            .events
            .iter()
            .any(|e| matches!(e, RuleEvent::HornSign { .. }))
        {
            sign_row = Some(r);
        }
        if !t.has("buttons_pressed")
            && let Some(s) = sign_row
            && tick.events.iter().any(|e| {
                matches!(
                    e,
                    RuleEvent::Time {
                        reason: TimeReason::HornMissed,
                        ..
                    }
                )
            })
        {
            return Ok((report, Some((s + 1, r))));
        }
        if verbose && !tick.events.is_empty() {
            println!("row {r}: events {:?}", tick.events);
        }
        if !t.has("buttons_pressed")
            && tick.events.iter().any(|e| {
                matches!(
                    e,
                    RuleEvent::Time {
                        reason: TimeReason::Horn | TimeReason::HornMissed,
                        ..
                    }
                )
            })
        {
            // The horn outcome depends on presses the trace does not have.
            report.ended = Some(format!(
                "row {r}: horn outcome without logged buttons; compared up to the row before"
            ));
            report.ticks -= 1;
            break;
        }
        if drive.phase() == DrivePhase::Cleared {
            // The final station: the original goes on to the ending, the engine stops here.
            report.ended = Some(format!("row {r}, cleared"));
            break;
        }
        report.ticks += 1;
        if tick.segment_started {
            setup_seen = true;
            report.segments += 1;
        }
        let intro = !setup_seen;
        // The physics copy of the heading on the first setup row is still the intro camera's.
        let intro_heading = intro || (tick.segment_started && report.segments == 1);
        let arrived = drive.phase() == DrivePhase::Arrived;
        let rule_cols: Vec<&'static str> = if arrived {
            engine_fields(drive.rules(), bases)
                .into_iter()
                .map(|(n, _)| n)
                .chain(["remaining_counter", "sched_next", "seconds_late"])
                .collect()
        } else {
            Vec::new()
        };
        let skip = |name: &str| {
            (intro && INTRO_CAMERA.contains(&name))
                || (intro_heading && name == "t_prev_heading")
                || rule_cols.contains(&name)
        };
        compare(t, r, &drive, bases, &skip, &mut report, verbose);

        // Menus, as the trace shows them.
        let flags = phase(t, r);
        let prev = before(t, r).map_or(0, |p| phase(t, p));
        if drive.phase() == DrivePhase::Arrived && flags & RESULT == 0 && prev & RESULT != 0 {
            finish_at = Some(after(t, r, 2));
        }
        if drive.phase() == DrivePhase::GameOver && flags & COAST == 0 && prev & COAST != 0 {
            if flags & STAGE_ACTIVE != 0 {
                continue_at = Some(after(t, r, 1));
            } else {
                report.ended = Some(format!("row {r}, no continue"));
                break;
            }
        }
        r += 1;
    }
    if report.segments == 0 {
        bail!("the engine never reached a segment setup");
    }
    for f in &report.fields {
        if !t.has(f.name) && f.name != "phase" {
            report.gaps.push(format!("{} not logged", f.name));
        }
    }
    report.fields.retain(|f| t.has(f.name) || f.name == "phase");
    Ok((report, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every oracle trace with a pre-departure wait must replay through `Drive` without a
    /// single mismatch. Needs `SHUPPATSU_DATA_DIR` and `SHUPPATSU_TRACE_DIR`; skips when either is unset.
    #[test]
    fn oracle_traces_replay_through_drive() {
        let (Some(traces), Some(data)) = (
            std::env::var_os("SHUPPATSU_TRACE_DIR"),
            std::env::var_os("SHUPPATSU_DATA_DIR"),
        ) else {
            eprintln!("skipped: SHUPPATSU_TRACE_DIR or SHUPPATSU_DATA_DIR not set");
            return;
        };
        let install = PathBuf::from(data).join("install/Program_Executable_Files");
        let bytes = std::fs::read(install.join("BIN/dengo3k.exe")).unwrap();
        let tables = Tables::read(&Exe::parse(&bytes).unwrap()).unwrap();
        let mut paths: Vec<_> = std::fs::read_dir(traces)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "csv"))
            .collect();
        paths.sort();
        let mut replayed = 0;
        for path in paths {
            let t = Trace::load(&path).unwrap();
            if !t.has("units_to_stop")
                || !(0..t.rows.len()).any(|r| phase(&t, r) & PRE_DEPARTURE != 0)
            {
                continue;
            }
            let report = replay(&t, &tables, &install.join("DATA"), false).unwrap();
            eprintln!("{}:", path.display());
            print_report(&report);
            assert_eq!(report.mismatches(), 0, "{}", path.display());
            assert!(
                report.stopped.is_none(),
                "{}: {:?}",
                path.display(),
                report.stopped
            );
            assert!(
                report.ticks > 2000,
                "{}: only {} ticks",
                path.display(),
                report.ticks
            );
            replayed += 1;
        }
        assert!(replayed > 0, "no drive trace found");
    }
}

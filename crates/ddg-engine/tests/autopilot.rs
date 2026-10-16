//! Soak test: a deterministic autopilot drives every stage end to end through [`Drive`],
//! finishing stations and continuing after game overs the way the game's menus do, while the
//! run is checked for sanity (no panic, finite and monotonic state, bounded time).
//!
//! Needs `DDG_DATA_DIR`; skips when unset. The full soak is `#[ignore]`d; run it in release:
//!
//! ```text
//! DDG_DATA_DIR=$PWD/data/extracted cargo test --release -p ddg-engine --test autopilot \
//!     -- --ignored --nocapture
//! ```

mod support;

use ddg_engine::drive::{Drive, DrivePhase};
use ddg_engine::physics::TrainTables;
use ddg_engine::rules::{RuleEvent, RuleTables, StationOutcome, TimeReason};
use ddg_formats::exe::Exe;
use ddg_formats::stages::{DIFFICULTY_COUNT, StageTables};
use std::path::PathBuf;
use support::{
    Autopilot, CONTINUE_AFTER_GAME_OVER_TICKS, FINISH_AFTER_ARRIVED_TICKS, MAX_CONTINUES, Outcome,
    RunReport,
};

/// Game-time ticks a run may take (≈ 3.7 h of game time).
const TICK_CAP: u64 = 400_000;
/// The remaining time is clamped to three digits (`MAX_REMAINING_S` in rules.rs).
const MAX_REMAINING_S: i32 = 999;

struct Data {
    dir: PathBuf,
    train: TrainTables,
    rules: RuleTables,
    stages: StageTables,
}

fn data() -> Option<Data> {
    let dir = support::install_dir()?;
    let bytes = std::fs::read(dir.join("BIN/dengo3k.exe")).unwrap();
    let exe = Exe::parse(&bytes).unwrap();
    Some(Data {
        train: TrainTables::read(&exe).unwrap(),
        rules: RuleTables::read(&exe).unwrap(),
        stages: StageTables::read(&exe).unwrap(),
        dir,
    })
}

/// Runs one stage until cleared, out of continues or the tick cap.
fn run_stage(data: &Data, stage: usize, difficulty: usize) -> RunReport {
    let loader = support::loader(&data.dir.join("DATA"));
    let mut d = Drive::new(
        &data.train,
        &data.rules,
        &data.stages,
        stage,
        difficulty,
        0,
        loader,
    )
    .unwrap_or_else(|e| panic!("stage {stage} d{difficulty}: {e}"));
    let margin = data.rules.limit_margin_kmh.get(stage, difficulty) as f32;
    let top = d.train_spec().max_speed_kmh();
    let mut ap = Autopilot::new(margin);
    let mut rep = RunReport {
        stage,
        difficulty,
        stations: d.rules().timetable().len(),
        ticks: 0,
        outcome: Outcome::TickCap,
        reached: 0,
        continues: 0,
        stop_errors: Vec::new(),
        ats_trips: 0,
        overspeed: 0,
        remaining_s: 0,
        violations: Vec::new(),
        segments: Vec::new(),
        stage_segments: (d.stage_data().segments.iter())
            .filter(|s| (s.first_station as usize) + 1 < d.rules().timetable().len())
            .map(|s| s.com)
            .collect(),
    };
    let (mut arrived_ticks, mut over_ticks) = (0u32, 0u32);
    // AUTOPILOT_TRACE=<stage>/<difficulty>:<every n ticks> prints the cab state of that run.
    let trace = std::env::var("AUTOPILOT_TRACE").ok().and_then(|v| {
        let (run, n) = v.split_once(':')?;
        let (s, k) = run.split_once('/')?;
        let this = s.parse::<usize>().ok()? == stage && k.parse::<usize>().ok()? == difficulty;
        this.then(|| n.parse::<u64>().ok())?
    });
    if trace.is_some() {
        let early: Vec<i32> = (data.rules.early_ticks.iter())
            .map(|t| t.get(stage, difficulty))
            .collect();
        eprintln!("margin {margin} km/h, early-running thresholds {early:?} ticks");
        eprintln!("segments {:?}", d.stage_data().segments);
        for (i, row) in d.rules().timetable().iter().enumerate() {
            eprintln!("row {i}: {row:?}");
        }
    }
    let mut prev_odo = d.train().odometer;
    let mut rewound = false;

    while rep.ticks < TICK_CAP {
        let input = ap.control(&d);
        if trace.is_some_and(|n| rep.ticks.is_multiple_of(n)) {
            let r = d.rules();
            eprintln!(
                "t{} {:?} st{} v{:.1} lim{:.1} sig{:.1} asp{} d{:.1} stop{} ats{} rem{} in P{}B{}{}",
                rep.ticks,
                d.phase(),
                r.station,
                d.speed_kmh(),
                r.limit_mps * 3.6,
                r.signal_limit_mps * 3.6,
                r.aspect,
                d.dist_to_stop_m(),
                r.target_stops,
                r.ats,
                r.remaining_s,
                input.power,
                input.brake,
                if input.confirm_pressed { " A" } else { "" }
            );
        }
        let t = d.tick(input);
        rep.ticks += 1;
        ap.observe(&t);
        if t.segment_started {
            ap.reset_stop();
            if !rep.segments.contains(&d.com()) {
                rep.segments.push(d.com());
            }
        }
        for e in &t.events {
            match e {
                RuleEvent::Arrived(a) => {
                    rep.stop_errors.push(a.dist_m);
                    rep.reached = rep.reached.max(a.station);
                }
                RuleEvent::StationPassed { station } => rep.reached = rep.reached.max(*station),
                RuleEvent::SpeedLimit { .. }
                | RuleEvent::SpeedLimitEnd
                | RuleEvent::Derailed
                | RuleEvent::GameOver
                | RuleEvent::Script { kind: 7, .. }
                | RuleEvent::Time { .. }
                | RuleEvent::LateDrain { .. }
                    if trace.is_some() =>
                {
                    eprintln!(
                        "t{} {e:?} at d{:.1} v{:.1}",
                        rep.ticks,
                        d.dist_to_stop_m(),
                        d.speed_kmh()
                    );
                }
                RuleEvent::AtsBell => rep.ats_trips += 1,
                RuleEvent::Time {
                    reason: TimeReason::Overspeed,
                    ..
                } => rep.overspeed += 1,
                _ => {}
            }
        }

        // Sanity.
        let tr = d.train();
        let r = d.rules();
        let speed = d.speed_kmh();
        if !speed.is_finite() || speed < 0.0 || speed > top * 1.02 + 0.5 {
            violate(&mut rep, format!("speed {speed} km/h (top {top})"));
        }
        let odo = tr.odometer;
        if !odo.is_finite() {
            violate(&mut rep, format!("odometer {odo}"));
        } else if odo < prev_odo - 1e-3 {
            // Allowed: the stop error settles into the odometer (overrun), and a continue
            // rewinds it.
            let settle = d.phase() == DrivePhase::Arrived && (prev_odo - odo) <= -r.dist_m + 0.01;
            if !settle && !rewound {
                violate(&mut rep, format!("odometer went back {prev_odo} -> {odo}"));
            }
        }
        if d.phase() != DrivePhase::Loading {
            rewound = false;
        }
        prev_odo = odo;
        if !(0..=MAX_REMAINING_S).contains(&r.remaining_s) {
            violate(&mut rep, format!("remaining {} s", r.remaining_s));
        }
        if !d.dist_to_stop_m().is_finite() || !d.units_to_stop().is_finite() {
            violate(&mut rep, format!("distance {}", d.dist_to_stop_m()));
        }

        match d.phase() {
            DrivePhase::Arrived => {
                arrived_ticks += 1;
                if arrived_ticks == FINISH_AFTER_ARRIVED_TICKS {
                    arrived_ticks = 0;
                    let res = d
                        .finish_station()
                        .unwrap_or_else(|e| panic!("stage {stage} d{difficulty}: {e}"));
                    if res.outcome == StationOutcome::Cleared {
                        rep.outcome = Outcome::Cleared;
                        break;
                    }
                }
            }
            DrivePhase::GameOver => {
                over_ticks += 1;
                if over_ticks == CONTINUE_AFTER_GAME_OVER_TICKS {
                    over_ticks = 0;
                    if !d.can_continue() || d.rules().continues >= MAX_CONTINUES {
                        rep.outcome = Outcome::GameOver;
                        break;
                    }
                    d.continue_run()
                        .unwrap_or_else(|e| panic!("stage {stage} d{difficulty}: {e}"));
                    rewound = true;
                    ap.reset_stop();
                }
            }
            DrivePhase::Cleared => {
                rep.outcome = Outcome::Cleared;
                break;
            }
            _ => (arrived_ticks, over_ticks) = (0, 0),
        }
    }
    rep.continues = d.rules().continues;
    rep.remaining_s = d.rules().remaining_s;
    rep
}

fn violate(rep: &mut RunReport, msg: String) {
    if rep.violations.len() < 8 {
        rep.violations.push(format!("tick {}: {msg}", rep.ticks));
    }
}

fn fmt_errors(e: &[f32]) -> String {
    if e.is_empty() {
        return "-".into();
    }
    let max = e.iter().fold(0f32, |m, x| m.max(x.abs()));
    let mean = e.iter().map(|x| x.abs()).sum::<f32>() / e.len() as f32;
    let list: Vec<String> = e.iter().map(|x| format!("{x:.2}")).collect();
    format!("mean {mean:.2} max {max:.2} [{}]", list.join(" "))
}

fn print_table(reports: &[RunReport]) {
    println!(
        "{:>5} {:>4} {:>7} {:>9} {:>8} {:>4} {:>4} {:>4} {:>5} {:>5}  stop errors (m, + = short)",
        "stage", "diff", "ticks", "reached", "outcome", "cont", "ats", "ovsp", "time", "segs"
    );
    for r in reports {
        println!(
            "{:>5} {:>4} {:>7} {:>5}/{:<3} {:>8} {:>4} {:>4} {:>4} {:>5} {:>3}/{:<1}  {}",
            r.stage,
            r.difficulty,
            r.ticks,
            r.reached,
            r.stations - 1,
            format!("{:?}", r.outcome),
            r.continues,
            r.ats_trips,
            r.overspeed,
            r.remaining_s,
            r.segments.len(),
            r.stage_segments.len(),
            fmt_errors(&r.stop_errors)
        );
        for v in &r.violations {
            println!("      ! {v}");
        }
    }
}

/// The difficulty to run besides 0: the first one whose timetable differs, else (this exe
/// shares one timetable across difficulties, routes.md §9) the hardest, whose rule tables
/// (start time, limit margin, stop tolerance, penalties) differ.
fn second_difficulty(stages: &StageTables, stage: usize) -> usize {
    let s = &stages.stages[stage];
    (1..DIFFICULTY_COUNT)
        .find(|&k| s.timetables[k] != s.timetables[0])
        .unwrap_or(DIFFICULTY_COUNT - 1)
}

fn soak(data: &Data, runs: Vec<(usize, usize)>) -> Vec<RunReport> {
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let queue = std::sync::Mutex::new(runs.into_iter().enumerate().collect::<Vec<_>>());
    let results = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let Some((i, (stage, diff))) = queue.lock().unwrap().pop() else {
                        break;
                    };
                    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        run_stage(data, stage, diff)
                    }));
                    results.lock().unwrap().push((i, stage, diff, r));
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|r| r.0);
    let mut reports = Vec::new();
    let mut panics = Vec::new();
    for (_, stage, diff, r) in results {
        match r {
            Ok(rep) => reports.push(rep),
            Err(e) => {
                let msg = e
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| e.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                    .unwrap_or_default();
                panics.push(format!("stage {stage} d{diff}: panic: {msg}"));
            }
        }
    }
    print_table(&reports);
    for p in &panics {
        println!("{p}");
    }
    assert!(panics.is_empty(), "{} runs panicked", panics.len());
    reports
}

fn check(reports: &[RunReport]) {
    let mut bad = Vec::new();
    for r in reports {
        let tag = format!("stage {} d{}", r.stage, r.difficulty);
        if !r.violations.is_empty() {
            bad.push(format!("{tag}: {}", r.violations[0]));
        }
        match r.outcome {
            Outcome::Cleared => {
                if r.reached + 1 != r.stations {
                    bad.push(format!("{tag}: cleared at station {}", r.reached));
                }
                let missed: Vec<_> = (r.stage_segments.iter())
                    .filter(|c| !r.segments.contains(c))
                    .collect();
                if !missed.is_empty() {
                    bad.push(format!(
                        "{tag}: cleared without driving segments {missed:?}"
                    ));
                }
            }
            Outcome::GameOver => {}
            Outcome::TickCap => bad.push(format!("{tag}: tick cap at station {}", r.reached)),
        }
    }
    assert!(bad.is_empty(), "soak failures:\n{}", bad.join("\n"));
}

/// Every stage at difficulty 0 and at a second difficulty ([`second_difficulty`]).
#[test]
#[ignore = "soak: run in release with --ignored (see the module docs)"]
fn autopilot_drives_every_stage() {
    let Some(data) = data() else { return };
    let mut runs: Vec<(usize, usize)> = (0..data.stages.stages.len()).map(|s| (s, 0)).collect();
    for s in 0..data.stages.stages.len() {
        runs.push((s, second_difficulty(&data.stages, s)));
    }
    let reports = soak(&data, runs);
    check(&reports);
}

/// Quick version for the normal test run: stage 0 (one stop, the terminus) end to end.
#[test]
fn autopilot_drives_stage_0() {
    let Some(data) = data() else { return };
    let r = run_stage(&data, 0, 0);
    print_table(std::slice::from_ref(&r));
    check(std::slice::from_ref(&r));
    assert!(!r.stop_errors.is_empty(), "{r:?}");
}

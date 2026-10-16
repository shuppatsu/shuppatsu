//! `Drive` on the real data: every stage starts, departs and drives towards its first stop with
//! the track inputs derived from the route. Needs `DDG_DATA_DIR`; skips when unset.
//! Bit-exactness against the original is checked by `ddg-tools trace-replay --drive`.

use ddg_engine::drive::{Drive, DrivePhase, DriverInput, LoadError};
use ddg_engine::physics::TrainTables;
use ddg_engine::rules::RuleTables;
use ddg_formats::exe::Exe;
use ddg_formats::stages::StageTables;
use std::path::PathBuf;

fn install_dir() -> Option<PathBuf> {
    let Some(root) = std::env::var_os("DDG_DATA_DIR") else {
        eprintln!("skipped: DDG_DATA_DIR not set");
        return None;
    };
    let dir = PathBuf::from(root).join("install/Program_Executable_Files");
    dir.is_dir().then_some(dir)
}

fn loader(dir: PathBuf) -> impl FnMut(&str) -> Result<Vec<u8>, LoadError> + Send {
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

#[test]
fn every_stage_departs_and_follows_its_track() {
    let Some(dir) = install_dir() else { return };
    let bytes = std::fs::read(dir.join("BIN/dengo3k.exe")).unwrap();
    let exe = Exe::parse(&bytes).unwrap();
    let train = TrainTables::read(&exe).unwrap();
    let rules = RuleTables::read(&exe).unwrap();
    let stages = StageTables::read(&exe).unwrap();
    for stage in 0..stages.stages.len() {
        let mut d = Drive::new(
            &train,
            &rules,
            &stages,
            stage,
            1,
            0,
            loader(dir.join("DATA")),
        )
        .unwrap();
        assert_eq!(d.phase(), DrivePhase::Loading);
        let mut setup = false;
        let mut ticks = 0;
        while d.phase() != DrivePhase::Driving {
            let t = d.tick(DriverInput {
                brake: 9,
                ..DriverInput::default()
            });
            setup |= t.segment_started;
            ticks += 1;
            assert!(ticks < 1000, "stage {stage}: no departure");
        }
        assert!(setup, "stage {stage}: no setup tick");
        let units0 = d.units_to_stop();
        let odo0 = d.train().odometer;
        assert!(
            units0 > 1.0,
            "stage {stage}: {units0} units to the first stop"
        );
        // Accelerate for 20 s on P3.
        for _ in 0..600 {
            d.tick(DriverInput {
                power: 3,
                ..DriverInput::default()
            });
        }
        let run = d.train().odometer - odo0;
        assert!(run > 50.0, "stage {stage}: ran {run} m");
        // The camera walk moved by the same distance (10 m per track unit), within a passing.
        let walked = (units0 - d.units_to_stop()) * 10.0;
        assert!(
            (walked - run).abs() < 0.5 || d.rules().station > 0,
            "stage {stage}: walked {walked} m, odometer {run} m"
        );
        assert!(
            d.speed_kmh() > 10.0,
            "stage {stage}: {} km/h",
            d.speed_kmh()
        );
        assert_ne!(d.pos(), d.prev_pos());
    }
}

/// AI trains (docs/re/traffic.md): the first traffic events of stages 0 and 10 spawn their
/// cars on the way to the first stop, the moving ones run along their track and poses exist.
#[test]
fn traffic_events_spawn_and_move() {
    let Some(dir) = install_dir() else { return };
    let bytes = std::fs::read(dir.join("BIN/dengo3k.exe")).unwrap();
    let exe = Exe::parse(&bytes).unwrap();
    let train = TrainTables::read(&exe).unwrap();
    let rules = RuleTables::read(&exe).unwrap();
    let stages = StageTables::read(&exe).unwrap();
    for stage in [0, 10] {
        let mut d = Drive::new(
            &train,
            &rules,
            &stages,
            stage,
            1,
            0,
            loader(dir.join("DATA")),
        )
        .unwrap();
        let (mut spawned, mut moved, mut posed) = (0, false, false);
        for tick in 0..3000 {
            let power = if d.speed_kmh() < 60.0 { 4 } else { 0 };
            let before: Vec<_> = d.traffic().cars().iter().map(|c| c.track()).collect();
            d.tick(DriverInput {
                power: if tick < 400 { 0 } else { power },
                ..DriverInput::default()
            });
            let cars = d.traffic().cars();
            spawned = spawned.max(cars.len());
            moved |= cars.len() == before.len()
                && cars
                    .iter()
                    .zip(&before)
                    .any(|(c, b)| c.speed_kmh() > 1.0 && c.track() != *b);
            posed |= !d.traffic_poses(0.5).is_empty();
            if d.phase() != DrivePhase::Driving
                && d.phase() != DrivePhase::PreDeparture
                && tick > 400
            {
                break;
            }
        }
        eprintln!(
            "stage {stage}: up to {spawned} AI cars, cursor {}",
            d.traffic().cursor()
        );
        assert!(spawned > 0, "stage {stage}: no AI train");
        assert!(moved, "stage {stage}: no AI car moved");
        assert!(posed, "stage {stage}: no AI car pose");
        assert!(d.traffic().cursor() > 0, "stage {stage}");
    }
}

/// The attract demos: the replayed notches move the train, the lesson demo shows its hints
/// and keeps to its speed limits, and both end after their task's length.
#[test]
fn attract_demos_replay_their_notches() {
    use ddg_engine::demo::{DemoKind, DemoTables};
    use ddg_engine::rules::RuleEvent;
    let Some(dir) = install_dir() else { return };
    let bytes = std::fs::read(dir.join("BIN/dengo3k.exe")).unwrap();
    let exe = Exe::parse(&bytes).unwrap();
    let train = TrainTables::read(&exe).unwrap();
    let rules = RuleTables::read(&exe).unwrap();
    let stages = StageTables::read(&exe).unwrap();
    let demos = DemoTables::read(&exe).unwrap();
    for (stage, kind) in [
        (1, DemoKind::Route),
        (DemoKind::LESSON_STAGE, DemoKind::Lesson),
    ] {
        let mut d = Drive::new(
            &train,
            &rules,
            &stages,
            stage,
            0,
            0,
            loader(dir.join("DATA")),
        )
        .unwrap();
        d.start_demo(kind, &demos);
        let start = d.dist_to_stop_m();
        let (mut ticks, mut hints, mut overspeed) = (0, 0, 0);
        while !d.demo_finished() {
            for e in d.tick(DriverInput::default()).events {
                match e {
                    RuleEvent::HintShown { .. } => hints += 1,
                    RuleEvent::Time { seconds, .. } if seconds < 0 => overspeed += 1,
                    _ => {}
                }
            }
            ticks += 1;
        }
        assert_eq!(ticks, kind.ticks(), "stage {stage}");
        assert!(d.speed_kmh() > 30.0, "stage {stage}: {}", d.speed_kmh());
        assert!(start - d.dist_to_stop_m() > 300.0, "stage {stage}");
        if kind == DemoKind::Lesson {
            assert!(hints >= 8, "{hints}");
            assert_eq!(overspeed, 0);
        }
    }
}

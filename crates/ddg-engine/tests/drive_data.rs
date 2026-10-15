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

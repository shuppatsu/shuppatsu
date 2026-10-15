//! Physics against the user's own exe. Skips unless `DDG_DATA_DIR` points at the extracted disc
//! (`install/Program_Executable_Files/BIN/dengo3k.exe` below it). Exact per-tick values will come
//! from oracle traces; until then these check that the real tables give sane, monotonic runs.

use std::path::PathBuf;

use ddg_engine::physics::{Controls, TickContext, TrainState, TrainTables};
use ddg_engine::time::read_stage_ranks;
use ddg_formats::exe::Exe;

fn exe_bytes() -> Option<Vec<u8>> {
    let Some(root) = std::env::var_os("DDG_DATA_DIR") else {
        eprintln!("skipped: DDG_DATA_DIR not set");
        return None;
    };
    let path = PathBuf::from(root).join("install/Program_Executable_Files/BIN/dengo3k.exe");
    match std::fs::read(&path) {
        Ok(b) => Some(b),
        Err(e) => {
            eprintln!("skipped: {}: {e}", path.display());
            None
        }
    }
}

#[test]
fn tables_read_and_look_like_trains() {
    let Some(bytes) = exe_bytes() else { return };
    let exe = Exe::parse(&bytes).unwrap();
    let t = TrainTables::read(&exe).unwrap();
    assert_eq!(t.specs.len(), 20);
    for (i, s) in t.specs.iter().enumerate() {
        let n = s.notch_speed_kmh;
        assert!(n.windows(2).all(|w| w[0] < w[1]), "type {i}: notches {n:?}");
        assert!((60.0..=400.0).contains(&s.max_speed_kmh()), "type {i}");
        assert!(s.mass > 0.0 && s.brake_rate > 0.0 && s.brake_per_notch > 0.0);
    }
    assert_eq!(t.stage_train.len(), 20);
    assert!(
        t.grade_scale
            .iter()
            .chain(&t.curve_scale)
            .all(|&k| (1.0..=1.5).contains(&k))
    );
    let ranks = read_stage_ranks(&exe).unwrap();
    assert!(ranks.iter().all(|&r| (1..=5).contains(&r)), "{ranks:?}");
}

#[test]
fn every_stage_train_accelerates_and_brakes_to_a_stop() {
    let Some(bytes) = exe_bytes() else { return };
    let exe = Exe::parse(&bytes).unwrap();
    let tables = TrainTables::read(&exe).unwrap();
    let ctx = TickContext::default();
    for stage in 0..20 {
        let params = tables.stage_params(stage, 0).unwrap();
        let spec = tables.specs[params.train_type];
        let mut st = TrainState::new(params, 0.0);
        // Two seconds of pre-departure wait: the traction fade-in completes while standing.
        let waiting = TickContext {
            driving: false,
            ..TickContext::default()
        };
        for clock in 0..60 {
            st.step(&spec, Controls::default(), &waiting, clock);
        }
        let mut clock = 0;
        let mut last = 0.0;
        // 90 s at P5.
        for _ in 0..30 * 90 {
            clock += 1;
            st.step(&spec, Controls { power: 5, brake: 0 }, &ctx, clock);
            assert!(
                st.speed >= last,
                "stage {stage}: speed fell under full power"
            );
            last = st.speed;
        }
        let top = st.speed_kmh();
        assert!(top > 40.0, "stage {stage}: only {top} km/h after 90 s");
        assert!(
            top <= spec.max_speed_kmh() + 0.01,
            "stage {stage}: {top} km/h"
        );
        // Then B8 until stopped; it must stop within 2 minutes.
        let odometer = st.odometer;
        let mut ticks = 0;
        while st.speed > 0.0 {
            clock += 1;
            ticks += 1;
            st.step(&spec, Controls { power: 0, brake: 8 }, &ctx, clock);
            assert!(st.speed <= last, "stage {stage}: speed rose under braking");
            last = st.speed;
            assert!(ticks < 30 * 120, "stage {stage}: no stop from {top} km/h");
        }
        let braking_m = st.odometer - odometer;
        eprintln!(
            "stage {stage:2} type {:2}: {top:6.1} km/h after 90 s, B8 stop in {:5.1} s / {braking_m:6.1} m",
            params.train_type,
            ticks as f32 / 30.0
        );
    }
}

//! The coupling minigame's waiting set (course event 12, 0x40acd0 → 0x406c50; rules.md §7.5):
//! the autopilot drives the coupling leg of stage 17 (station 15 → 16) and the test checks
//! where the set stands, that the coupler frame switches, and that the player couples at its
//! rear.
//!
//! Needs `SHUPPATSU_DATA_DIR`; skips when unset. `COUPLING_SCRIPT=<file>` also writes the run's
//! notches as a `shuppatsu play --inputs` script (`tick notch brake` lines, drive ticks).

#[allow(dead_code)]
mod support;

use shuppatsu_engine::coupling::{CouplingEvent, CouplingResult};
use shuppatsu_engine::drive::{Drive, DrivePhase};
use shuppatsu_engine::physics::TrainTables;
use shuppatsu_engine::rules::{RuleEvent, RuleTables};
use shuppatsu_formats::exe::Exe;
use shuppatsu_formats::scene::cab_camera;
use shuppatsu_formats::stages::StageTables;
use std::fmt::Write as _;
use support::Autopilot;

const STAGE: usize = 17;
const STATION: usize = 15;
const TICK_CAP: u64 = 60_000;

#[test]
fn waiting_set_stands_at_the_coupler() {
    let Some(dir) = support::install_dir() else {
        return;
    };
    let bytes = std::fs::read(dir.join("BIN/dengo3k.exe")).unwrap();
    let exe = Exe::parse(&bytes).unwrap();
    let (train, rules, stages) = (
        TrainTables::read(&exe).unwrap(),
        RuleTables::read(&exe).unwrap(),
        StageTables::read(&exe).unwrap(),
    );
    let loader = support::loader(&dir.join("DATA"));
    let mut d = Drive::new(&train, &rules, &stages, STAGE, 0, STATION, loader).unwrap();
    let mut ap = Autopilot::new(rules.limit_margin_kmh.get(STAGE, 0) as f32);
    let mut script = String::new();
    let mut last = None;
    let mut placed_at = None;
    let mut result = None;
    let mut coupled_frame_seen = false;
    let mut couple_gap_m = None;
    for tick in 0..TICK_CAP {
        let input = ap.control(&d);
        if last != Some((input.power, input.brake)) {
            last = Some((input.power, input.brake));
            writeln!(script, "{} {} {}", d.tick_count(), input.power, input.brake).unwrap();
        }
        let t = d.tick(input);
        ap.observe(&t);
        for e in &t.events {
            match e {
                RuleEvent::Script { kind: 12, .. } => placed_at = Some((tick, d.dist_to_stop_m())),
                RuleEvent::Coupling(CouplingEvent::Result(r)) => {
                    result = Some(*r);
                    // The cab's distance to the nearest waiting car at the moment it couples.
                    let cam = cab_camera(d.route(), d.walker().pos(), 0.0, 0.0);
                    couple_gap_m = d
                        .traffic_poses(1.0)
                        .iter()
                        .map(|p| {
                            let o = p.origin();
                            let (dx, dz) = (o[0] - cam.eye[0], o[2] - cam.eye[2]);
                            (dx * dx + dz * dz).sqrt() * 10.0
                        })
                        .reduce(f32::min);
                }
                _ => {}
            }
        }
        coupled_frame_seen |= d.traffic().coupler_frame();
        if result.is_some() && d.phase() != DrivePhase::Coupling {
            break;
        }
    }
    if let Some(p) = std::env::var_os("COUPLING_SCRIPT") {
        std::fs::write(p, &script).unwrap();
    }
    let (tick, dist) = placed_at.expect("course event 12 did not fire");
    eprintln!("event 12 at tick {tick}, {dist:.1} m to the stop; result {result:?}");
    eprintln!("nearest waiting car origin at coupling: {couple_gap_m:?} m");
    assert_eq!(result, Some(CouplingResult::Coupled));
    assert!(coupled_frame_seen);
    // No marks before the leg: partner 1, rows 20 / 21 (ten cars).
    assert_eq!(d.rules().coupling_partner, 1);
    // The last car's origin is half a car (≈ 12.75 m) ahead of the coupled cab.
    let gap = couple_gap_m.unwrap();
    assert!((8.0..20.0).contains(&gap), "{gap}");
}

//! Rules against the user's own exe tables. Skips unless `DDG_DATA_DIR` points at the extracted
//! disc (`install/Program_Executable_Files/BIN/dengo3k.exe`).

use std::path::PathBuf;

use ddg_engine::rules::{Phase, RuleEvent, RuleTables, Rules, StationOutcome, TickInput};
use ddg_formats::exe::Exe;
use ddg_formats::stages::{DIFFICULTY_COUNT, STAGE_COUNT, StageTables};

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
fn rule_tables_read_with_expected_shapes() {
    let Some(bytes) = exe_bytes() else { return };
    let exe = Exe::parse(&bytes).unwrap();
    let t = RuleTables::read(&exe).unwrap();
    // Signal speeds above the margin: YG / Y / YY.
    assert_eq!(t.signal_speed_kmh, [70.0, 45.0, 25.0]);
    for s in 0..STAGE_COUNT {
        assert!(t.block_length_m[s] > 0, "stage {s} block length");
        for d in 0..DIFFICULTY_COUNT {
            assert!(t.start_time_s.get(s, d) > 0, "start time {s}/{d}");
            assert!((0..=5).contains(&t.stop_tolerance_m.get(s, d)));
            assert!(t.limit_margin_kmh.get(s, d) > 0);
            // Early thresholds rise with severity for the "left a stop" selection.
            let e: Vec<i32> = t.early_ticks.iter().map(|x| x.get(s, d)).collect();
            assert!(
                e[0] <= e[1] && e[1] <= e[3] && e[3] <= e[5] && e[5] <= e[6],
                "{e:?}"
            );
            assert!(e[2] <= e[4] && e[4] <= e[6], "{e:?}");
        }
    }
    // Every pattern ends in a red and contains only aspects 1..=4.
    for p in &t.aspect_patterns {
        assert_eq!(p[4], 4);
        assert!(p.iter().all(|&a| (1..=4).contains(&a)));
    }
    assert!(!t.scripted_signals.is_empty());
    assert!(
        t.scripted_signals
            .iter()
            .all(|s| (0..STAGE_COUNT as i16).contains(&s.stage) && (0..=1).contains(&s.kind))
    );
}

/// A crude scripted driver: cruise below the active limits, brake on a fixed profile to the
/// stop point, acknowledge the ATS. Runs every stage and difficulty from the first station to
/// the end and checks that the rules reach the end of every timetable and every event list.
#[test]
fn scripted_driver_completes_every_stage() {
    let Some(bytes) = exe_bytes() else { return };
    let exe = Exe::parse(&bytes).unwrap();
    let tables = RuleTables::read(&exe).unwrap();
    let stages = StageTables::read(&exe).unwrap();
    const DT: f32 = 1.0 / 30.0;

    for (s, stage) in stages.stages.iter().enumerate() {
        for d in 0..DIFFICULTY_COUNT {
            let tt = &stage.timetables[d];
            let mut r = Rules::new(&tables, stage, s, d, 0).unwrap();
            // Plenty of time so the run is never cut short by lateness.
            r.remaining_s = 999;
            let mut dist = tt[0].to_next_m as f32;
            let mut odo = r.odometer_m;
            let mut speed = 0.0f32;
            let mut target = 1usize;
            let mut ticks = 0u32;
            let mut arrivals = 0;
            for _ in 0..60 {
                r.pre_departure_tick(&TickInput::default());
            }
            r.depart();
            let outcome = loop {
                ticks += 1;
                assert!(ticks < 3_000_000, "stage {s}/{d} did not finish");
                let following = tt.get(target).map_or(0, |row| row.to_next_m) as f32;
                let mut input = TickInput {
                    speed_mps: speed,
                    dist_to_stop_m: dist,
                    dist_to_following_stop_m: dist + following,
                    odometer_m: odo,
                    power: 0,
                    brake: 0,
                    ..TickInput::default()
                };
                if r.ats == 2 {
                    input.brake = 4;
                    input.buttons.confirm_pressed = true;
                }
                if r.phase == Phase::Arrived {
                    arrivals += 1;
                    let res = r.finish_station();
                    r.remaining_s = 999;
                    match res.outcome {
                        StationOutcome::Continue => {}
                        other => break other,
                    }
                    target = r.station + 1;
                    dist = tt[r.station].to_next_m as f32;
                    for _ in 0..30 {
                        r.pre_departure_tick(&TickInput::default());
                    }
                    r.depart();
                    continue;
                }
                // The crude driver is often late and runs reds; keep it in the game so the whole
                // timetable and every event list are exercised.
                if r.remaining_s < 500 {
                    r.remaining_s = 999;
                }
                let out = r.tick(&input);
                for e in &out.events {
                    if let RuleEvent::StationPassed { station } = e {
                        target = station + 1;
                        dist += tt[station - 1].to_next_m as f32;
                    }
                }
                if r.phase == Phase::GameOver {
                    break StationOutcome::GameOver;
                }
                if out.force_stop || out.emergency_brake || !matches!(r.phase, Phase::Driving) {
                    speed = 0.0;
                } else {
                    let stops = tt.get(target).is_some_and(|row| row.stops == 1);
                    let limit = r
                        .limit_mps
                        .min(r.signal_limit_mps)
                        .min(r.derail_kmh / 3.6)
                        .min(25.0)
                        - 1.0;
                    let profile = if stops {
                        (2.0 * 0.25 * dist.max(0.0)).sqrt()
                    } else {
                        f32::MAX
                    };
                    let want = limit
                        .min(profile)
                        .max(if stops && dist > 0.3 { 0.5 } else { 0.0 });
                    if speed < want {
                        speed = (speed + 0.8 * DT).min(want);
                    } else {
                        speed = (speed - 1.2 * DT).max(want);
                    }
                    if stops && dist <= 0.0 {
                        speed = 0.0;
                    }
                    let step = speed * DT;
                    if stops && dist > 0.0 && step > dist {
                        odo += dist;
                        dist = 0.0;
                        speed = 0.0;
                    } else {
                        dist -= step;
                        odo += step;
                    }
                }
            };
            assert_eq!(outcome, StationOutcome::Cleared, "stage {s}/{d}");
            assert!(arrivals > 0);
            let (course, limits) = r.event_cursors();
            // Stages 1, 2 and 10 can end early (stage 1 stays at 7 when it ends at 8), and
            // stage 1's jump to the bonus section leaves the limit list where it was.
            let last = if s == 1 && r.station == 7 {
                8
            } else {
                r.station as i16
            };
            let c_end = reachable_end(&stage.course_events, last);
            let l_end = if r.vehicle_code.is_some() && s == 1 {
                reachable_end(&stage.limit_events, 9)
            } else {
                reachable_end(&stage.limit_events, last)
            };
            assert!(
                course >= c_end,
                "stage {s}/{d}: course event {course} not consumed: {:?}",
                stage.course_events.get(course)
            );
            assert!(
                limits >= l_end,
                "stage {s}/{d}: limit event {limits} not consumed: {:?}",
                stage.limit_events.get(limits)
            );
            let g = r.grades();
            assert!((0..=100).contains(&g.total), "stage {s}/{d}: {g:?}");
        }
    }
}

/// Index of the first event a run to the last station cannot be expected to consume: events of
/// the final segment (they may lie past the stop) or the start of an out-of-order tail (the game
/// never fires a record whose station is below the current one, e.g. stage 0's limit list ends
/// with three station-0 records).
fn reachable_end(events: &[ddg_formats::stages::CourseEvent], last: i16) -> usize {
    let mut prev = i16::MIN;
    for (i, e) in events.iter().enumerate() {
        if e.station >= last - 1 || e.station < prev {
            return i;
        }
        prev = e.station;
    }
    events.len()
}

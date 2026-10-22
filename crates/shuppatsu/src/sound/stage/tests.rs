//! Event → sound mapping, checked against a recording [`SoundSystem`] (no device, no data).
//! Sequences only play their first phrase here (the sequencer is not stepped), which is enough
//! to tell which call started.

use super::*;
use shuppatsu_audio::{Command, SoundTables, Step};
use shuppatsu_engine::rules::Arrival;
use std::sync::Arc;

const SEQ_BASE: i16 = 0x60;
const PAIR_BASE: i16 = 0x100;

fn stage_tables(stage: usize) -> StageTables {
    StageTables {
        stage,
        station_names: vec![10, 11, 12, 13],
        jingle: 0xc,
        navi: NaviTables {
            reactions: [2, 1, 3, 4],
            frames: (0..crate::navigator::ANIMS)
                .map(|_| {
                    [(537, 20), (538, 15), (543, 50), (-1, 0)]
                        .map(|(tbl, ticks)| crate::navigator::Frame { tbl, ticks })
                        .to_vec()
                })
                .collect(),
        },
    }
}

struct Rig {
    snd: SoundSystem,
    train: TrainSound,
    st: StageSound,
}

impl Rig {
    fn new(stage: usize) -> Self {
        let tables = SoundTables {
            // Sequence n plays 0x60 + n; pair n is 0x100 + n.
            sequences: (0..47).map(|i| vec![Step::new(SEQ_BASE + i, 0)]).collect(),
            call_pairs: (0..180).map(|i| Step::new(PAIR_BASE + i, 10)).collect(),
            ..SoundTables::default()
        };
        let mut snd = SoundSystem::new(Arc::new(tables.clone()));
        let train = TrainSound::new(&tables, stage, 0, &mut snd);
        snd.take_commands();
        Self {
            snd,
            train,
            st: StageSound::new(stage_tables(stage)),
        }
    }

    fn run(&mut self, info: &TickInfo<'_>) -> Vec<Command> {
        self.st.loop_part(&mut self.snd, &mut self.train, info);
        self.st.task_part(&mut self.snd, &self.train, info);
        self.snd.take_commands()
    }

    fn tick(&mut self, before: Snap, after: Snap, events: &[RuleEvent]) -> Vec<Command> {
        self.run(&TickInfo {
            before,
            after,
            events,
            segment_started: false,
            track_classes: None,
            turnout_entered: false,
            demo: false,
        })
    }

    /// A driving tick with `events` and the same state before and after.
    fn drive(&mut self, s: Snap, events: &[RuleEvent]) -> Vec<Command> {
        self.tick(s, s, events)
    }
}

fn plays(cmds: &[Command]) -> Vec<u16> {
    cmds.iter()
        .filter_map(|c| match c {
            Command::Play(code) => Some(*code),
            _ => None,
        })
        .collect()
}

fn shown(aspect: i16, display: i16) -> RuleEvent {
    RuleEvent::SignalShown {
        aspect,
        forced: false,
        display,
        variant: 0,
        ticks: 0,
        distance_m: 300,
        start_m: 0,
    }
}

fn limit(kmh: i32) -> RuleEvent {
    RuleEvent::SpeedLimit {
        kmh,
        notice_m: 300,
        offset_m: 0,
    }
}

#[test]
fn speed_limit_calls() {
    // Stage 1: a new limit plays only the prefix; a changed limit says nothing.
    let mut r = Rig::new(1);
    let s = Snap::default();
    assert_eq!(plays(&r.drive(s, &[limit(60)])), [LIMIT_PREFIX]);
    assert!(!r.snd.sequence_active());
    let active = Snap {
        limit_state: 1,
        ..s
    };
    assert!(r.drive(active, &[limit(45)]).is_empty());
    // Stage 5: prefix + speed for both kinds.
    let mut r = Rig::new(5);
    assert_eq!(plays(&r.drive(s, &[limit(60)])), [LIMIT_PREFIX]);
    assert!(r.snd.sequence_active());
    r.snd.clear_sequences();
    assert_eq!(plays(&r.drive(active, &[limit(45)])), [LIMIT_PREFIX]);
    assert!(r.snd.sequence_active());
    // Two limit events in one tick: the second sees the state the first left (0 = kind 4).
    let mut r = Rig::new(1);
    assert_eq!(
        plays(&r.drive(active, &[limit(45), limit(30)])),
        [LIMIT_PREFIX]
    );
    // Limit lifted: prefix, then 0xc3, on every stage.
    let mut r = Rig::new(1);
    assert_eq!(
        plays(&r.drive(s, &[RuleEvent::SpeedLimitEnd])),
        [LIMIT_PREFIX]
    );
    assert!(r.snd.sequence_active());
}

#[test]
fn signal_sighting_calls() {
    let s = Snap::default();
    let mut r = Rig::new(5);
    // Block signal (mode 0): one phrase per aspect.
    let at = |block, aspect| Snap { block, aspect, ..s };
    assert_eq!(plays(&r.drive(at(5, 2), &[shown(2, 0)])), [0xa9]);
    assert_eq!(plays(&r.drive(at(5, 4), &[shown(4, 0)])), [0xd2]);
    // Green: by block (sequence 5 at block 3), and 0x98 from block 8 on.
    assert_eq!(
        plays(&r.drive(at(3, 0), &[shown(0, 0)])),
        [(SEQ_BASE + 5) as u16]
    );
    assert_eq!(plays(&r.drive(at(9, 0), &[shown(0, 0)])), [GREEN_FAR]);
    let v1 = Snap {
        signal_variant: 1,
        ..at(9, 0)
    };
    assert_eq!(plays(&r.drive(v1, &[shown(0, 0)])), [GREEN_FAR_VARIANT1]);
    // Starting signal (block 0, mode 2) with a restrictive aspect: "出発 減速" call.
    assert_eq!(plays(&r.drive(at(0, 1), &[shown(1, 2)])), [0xc0]);
    // Passing-station route (stage 1), passing target: prefix 0xd4 first.
    let mut r = Rig::new(1);
    let pass = Snap {
        target_stops: false,
        scheduled: 100_000,
        ..at(2, 3)
    };
    assert_eq!(plays(&r.drive(pass, &[shown(3, 1)])), [0xd4]);
    // Stage 4 green at its home signal: "場内 進行" call (0xc1).
    let mut r = Rig::new(4);
    assert_eq!(plays(&r.drive(at(7, 0), &[shown(0, 1)])), [0xc1]);
    // Not a sighting: forced displays, and anything outside the loop body.
    let mut r = Rig::new(5);
    let forced = RuleEvent::SignalShown {
        aspect: 0,
        forced: true,
        display: 2,
        variant: 0,
        ticks: 0,
        distance_m: 10,
        start_m: 0,
    };
    assert!(r.drive(at(5, 0), &[forced]).is_empty());
    let waiting = Snap {
        phase: Phase::PreDeparture,
        ..at(5, 4)
    };
    assert!(r.drive(waiting, &[shown(4, 2)]).is_empty());
}

#[test]
fn repeater_call_without_display() {
    let mut r = Rig::new(5);
    let b = Snap {
        repeater_m: 100,
        ..Snap::default()
    };
    // Block 3 before the increment (mode 0): no prefix, "閉塞 進行".
    let a = Snap {
        repeater_m: 0,
        block: 4,
        ..b
    };
    assert_eq!(plays(&r.tick(b, a, &[])), [0xd1]);
    // Block 2 (home, mode 1) as the prefix, restrictive word 0xc2.
    let a = Snap {
        repeater_m: 0,
        block: 3,
        aspect: 2,
        ..b
    };
    assert_eq!(plays(&r.tick(b, a, &[])), [0xc1]);
}

#[test]
fn ats_bell_and_chime() {
    let mut r = Rig::new(5);
    let s = Snap::default();
    assert_eq!(r.drive(s, &[RuleEvent::AtsBell]), [Command::Play(ATS_BELL)]);
    assert_eq!(
        r.drive(s, &[RuleEvent::AtsAcknowledged]),
        [Command::Release(ATS_BELL)]
    );
    let chime = |clock| Snap {
        clock,
        ats_chime_from: 100,
        ..s
    };
    assert_eq!(plays(&r.drive(chime(100), &[])), [ATS_CHIME]);
    assert!(r.drive(chime(101), &[]).is_empty());
    assert_eq!(plays(&r.drive(chime(132), &[])), [ATS_CHIME]);
    // The next sighting releases it.
    let mut b = chime(140);
    b.block = 5;
    let mut a = b;
    a.ats_chime_from = -1;
    let cmds = r.tick(b, a, &[shown(0, 0)]);
    assert!(cmds.contains(&Command::Release(ATS_CHIME)));
}

/// The demo tasks (0x40e630, 0x40e860) never call 0x40c4e0: `0x8570d4` still holds its initial
/// 0 there (no departure reset it), but no chime and no approach call is requested.
#[test]
fn demo_drives_have_no_chime() {
    let mut r = Rig::new(1);
    let s = Snap {
        ats_chime_from: 0,
        leg_distance: 2000.0,
        ..Snap::default()
    };
    let mut demo = |clock| {
        let s = Snap { clock, ..s };
        r.run(&TickInfo {
            before: s,
            after: s,
            events: &[],
            segment_started: false,
            track_classes: None,
            turnout_entered: false,
            demo: true,
        })
    };
    for clock in [0, 1, 32, 64] {
        assert!(!plays(&demo(clock)).contains(&ATS_CHIME), "clock {clock}");
    }
    // The same state in a stage run chimes.
    assert!(plays(&r.drive(Snap { clock: 96, ..s }, &[])).contains(&ATS_CHIME));
}

#[test]
fn penalty_buzzer_and_pass_jingle() {
    let mut r = Rig::new(5);
    let s = Snap::default();
    let t = |reason, seconds| RuleEvent::Time { reason, seconds };
    assert_eq!(
        plays(&r.drive(s, &[t(TimeReason::Overspeed, -5)])),
        [BUZZER]
    );
    assert_eq!(
        plays(&r.drive(s, &[t(TimeReason::OnTimePass, 3)])),
        [ON_TIME_PASS]
    );
    assert!(r.drive(s, &[t(TimeReason::HornMissed, -3)]).is_empty());
    let waiting = Snap {
        phase: Phase::PreDeparture,
        ..s
    };
    assert_eq!(
        plays(&r.tick(waiting, waiting, &[t(TimeReason::FalseStart, -5)])),
        [BUZZER]
    );
}

#[test]
fn course_event_sounds() {
    let mut r = Rig::new(5);
    let s = Snap::default();
    let ev = |kind, a, b| RuleEvent::Script {
        kind,
        args: [a, b, 0],
    };
    assert_eq!(plays(&r.drive(s, &[ev(1, 0x45, 0)])), [0x45]);
    assert_eq!(plays(&r.drive(s, &[ev(7, 1, 210)])), [ATC_SECTION]);
    // Kind 2: station call of the next station (name id 11 → pair 90 + 11, code ≥ 0x147).
    assert_eq!(
        plays(&r.drive(s, &[ev(2, 1, 0)])),
        [(PAIR_BASE + 101) as u16]
    );
    r.snd.clear_sequences();
    // Kind 13: played, released 3 ticks later.
    assert_eq!(plays(&r.drive(s, &[ev(13, 0x50, 3)])), [0x50]);
    assert!(r.drive(s, &[]).is_empty());
    assert!(r.drive(s, &[]).is_empty());
    assert_eq!(r.drive(s, &[]), [Command::Release(0x50)]);
    // Kinds 18 / 19 toggle the flange flag.
    r.drive(s, &[ev(18, 0, 0)]);
    assert!(r.train.flange());
    r.drive(s, &[ev(19, 0, 0)]);
    assert!(!r.train.flange());
    // Kind 23: E3 coupling sequence on the 65th update.
    r.drive(s, &[ev(23, 0, 0)]);
    let at = (1..80)
        .find(|_| !r.drive(s, &[]).is_empty())
        .expect("coupling sound");
    assert_eq!(at, 64);
}

#[test]
fn departure_timeline() {
    let mut r = Rig::new(5);
    let mut log = Vec::new();
    for c in 1..=0xc9 {
        let b = Snap {
            phase: Phase::PreDeparture,
            departure_counter: c,
            ..Snap::default()
        };
        let a = Snap {
            departure_counter: c + 1,
            departure_done: c >= 0xc9,
            ..b
        };
        for cmd in r.tick(b, a, &[]) {
            log.push((c, cmd));
        }
    }
    assert_eq!(
        log,
        [
            (1, Command::Play(0xc)),
            (0x30, Command::Play(DEPART_CALL_LOCAL)),
            (100, Command::Release(0xc)),
            (100, Command::Play(DEPART_SIGNAL)),
            (200, Command::Release(DEPART_BELL)),
        ]
    );
    // The train moves later: the departure voice for the aspect in force.
    let b = Snap {
        phase: Phase::Driving,
        ..Snap::default()
    };
    let a = Snap {
        speed: 1.0,
        aspect: 2,
        ..b
    };
    assert_eq!(plays(&r.tick(b, b, &[])), Vec::<u16>::new());
    assert_eq!(plays(&r.tick(b, a, &[])), [DEPART_VOICES[2]]);
    assert!(r.tick(b, a, &[]).is_empty());

    // First station of stage 0 (offset 180): sequence 0x18 at 0x30.
    let mut r = Rig::new(0);
    let b = Snap {
        phase: Phase::PreDeparture,
        departure_counter: 0x30,
        departure_base: 180,
        ..Snap::default()
    };
    assert_eq!(plays(&r.tick(b, b, &[])), [(SEQ_BASE + 0x18) as u16]);
    // Stage 13: the extra jingle from offset + 90 to + 130.
    let mut r = Rig::new(13);
    let at = |c| Snap {
        phase: Phase::PreDeparture,
        departure_counter: c,
        ..Snap::default()
    };
    assert_eq!(plays(&r.tick(at(90), at(90), &[])), [DEPART_EXTRA_JINGLE]);
    assert_eq!(
        r.tick(at(130), at(130), &[]),
        [Command::Release(DEPART_EXTRA_JINGLE)]
    );
}

#[test]
fn segment_start() {
    let mut r = Rig::new(5);
    let s = Snap {
        weather_level: 40,
        ..Snap::default()
    };
    let cmds = r.run(&TickInfo {
        before: s,
        after: s,
        events: &[],
        segment_started: true,
        track_classes: None,
        turnout_entered: false,
        demo: false,
    });
    let p = plays(&cmds);
    // Running noise and motor (0 in these tables), the departure bell, then rain at level / 2.
    assert_eq!(p[0], RUNNING_NOISE);
    assert!(p.contains(&DEPART_BELL));
    assert!(cmds.ends_with(&[Command::Play(RAIN), Command::SetPitch(RAIN, 20)]));
}

#[test]
fn arrival_sequence() {
    let mut r = Rig::new(5);
    let driving = Snap::default();
    let draining = Snap {
        phase: Phase::Draining,
        speed: 0.0,
        ..driving
    };
    // Accepted stop: stop all.
    assert_eq!(r.tick(driving, draining, &[]), [Command::StopAll]);
    // After the drain: stop all again.
    let eval1 = Snap {
        phase: Phase::Evaluating(1),
        ..draining
    };
    let eval2 = Snap {
        phase: Phase::Evaluating(2),
        ..draining
    };
    assert_eq!(r.tick(eval1, eval2, &[]), [Command::StopAll]);
    // Clean stop on time at the final station: jingle, released after 100 ticks, then clear.
    let hold = Snap {
        phase: Phase::Holding {
            ticks: 220,
            then: AfterHold::Arrived,
        },
        final_station: true,
        ..draining
    };
    let ev = [
        RuleEvent::Time {
            reason: TimeReason::OnTimeStop,
            seconds: 5,
        },
        RuleEvent::Arrived(Arrival {
            station: 1,
            dist_m: 0.1,
            late_s: 0,
            mark: 2,
            forced: false,
        }),
    ];
    assert_eq!(plays(&r.tick(eval2, hold, &ev)), [JINGLE_ON_TIME]);
    let mut at = None;
    for i in 1..=120 {
        let cmds = r.tick(hold, hold, &[]);
        if !cmds.is_empty() {
            assert_eq!(
                cmds,
                [
                    Command::Release(JINGLE_ON_TIME),
                    Command::Play(JINGLE_ALL_CLEAR)
                ]
            );
            at = Some(i);
            break;
        }
    }
    assert_eq!(at, Some(100));
    // Leaving the loop: release the running noise; sequences off until the next segment.
    let arrived = Snap {
        phase: Phase::Arrived,
        ..draining
    };
    let cmds = r.tick(arrived, arrived, &[]);
    assert!(cmds.contains(&Command::Release(RUNNING_NOISE)));
    assert!(r.tick(arrived, arrived, &[]).is_empty());
}

#[test]
fn buffer_stop() {
    let mut r = Rig::new(5);
    let driving = Snap::default();
    let hold = Snap {
        phase: Phase::Holding {
            ticks: 80,
            then: AfterHold::Drain,
        },
        ..driving
    };
    let ev = [RuleEvent::Time {
        reason: TimeReason::ForcedStop,
        seconds: -20,
    }];
    assert_eq!(
        r.tick(driving, hold, &ev),
        [
            Command::StopAll,
            Command::Play(BUFFER_STOP_CRASH),
            Command::Play(0xdc)
        ]
    );
    let mut voices = Vec::new();
    for i in 1..=60 {
        for c in plays(&r.tick(hold, hold, &[])) {
            voices.push((i, c));
        }
    }
    assert_eq!(voices, [(15, 0xd6), (40, 0xe2), (50, 0xe6)]);
}

#[test]
fn red_signal_recovery() {
    let mut r = Rig::new(5);
    let driving = Snap::default();
    let hold = |ticks| Snap {
        phase: Phase::Holding {
            ticks,
            then: AfterHold::Resume,
        },
        ..driving
    };
    assert_eq!(
        r.tick(driving, hold(70), &[RuleEvent::SignalsReset]),
        [Command::Release(ATS_BELL)]
    );
    assert!(r.tick(hold(2), hold(1), &[]).is_empty());
    assert_eq!(plays(&r.tick(hold(1), driving, &[])), [RECOVERY_CALL]);
}

#[test]
fn game_over_and_time_up() {
    let mut r = Rig::new(5);
    let mut b = Snap::default();
    let mut a = b;
    a.stats[5] = 5;
    // Overspeed earlier in the run: the last statistic is category 5.
    r.tick(b, a, &[]);
    b = a;
    let over = Snap {
        phase: Phase::GameOver,
        ..b
    };
    assert_eq!(r.tick(b, over, &[RuleEvent::GameOver]), [Command::StopAll]);
    let mut heard = Vec::new();
    for n in 2..=80 {
        for c in plays(&r.tick(over, over, &[])) {
            heard.push((n, c));
        }
    }
    assert_eq!(heard, [(56, BUZZER), (76, 0xf2)]);
}

#[test]
fn approach_announcement() {
    let mut r = Rig::new(5);
    let s = Snap {
        home_signal_m: 300,
        dist_round_m: 1200,
        dist_m: 1200.0,
        ..Snap::default()
    };
    // 50 m into the leg: caption (state 1), no sound.
    let b = Snap {
        leg_distance: 50.0,
        ..s
    };
    assert!(r.tick(b, s, &[]).is_empty());
    // At f5 + 150: "<next station> 停車" (pair 11, then pair 3).
    let a = Snap {
        dist_round_m: 450,
        dist_m: 450.0,
        ..s
    };
    assert_eq!(plays(&r.tick(s, a, &[])), [(PAIR_BASE + 11) as u16]);
    assert!(r.tick(a, a, &[]).is_empty());
}

#[test]
fn navigator_reactions() {
    // EB held 40 ticks: animation 2, whose sound comes 5 + 0x14 updates after the show.
    let mut r = Rig::new(5);
    let b = Snap {
        eb_ticks: 0x28,
        ..Snap::default()
    };
    let eb = RuleEvent::Time {
        reason: TimeReason::EmergencyBrake,
        seconds: -10,
    };
    let s = Snap::default();
    let mut heard = Vec::new();
    for i in 0..40 {
        let cmds = if i == 0 {
            r.tick(b, b, std::slice::from_ref(&eb))
        } else {
            r.drive(s, &[])
        };
        heard.extend(plays(&cmds).into_iter().map(|c| (i, c)));
    }
    assert_eq!(heard, [(25, 0xdc)]);
    // The every-4th-application penalty (first tick) has no reaction.
    let mut r = Rig::new(5);
    let first = Snap {
        eb_ticks: 1,
        ..Snap::default()
    };
    r.tick(first, first, std::slice::from_ref(&eb));
    assert!((0..40).all(|_| r.drive(s, &[]).is_empty()));
    // Overrun reaction when the approach state turns 4, by the clock's low bit.
    let mut r = Rig::new(5);
    let b = Snap {
        approach: 3,
        clock: 7,
        ..Snap::default()
    };
    let a = Snap { approach: 4, ..b };
    r.tick(b, a, &[]);
    let at = (1..60).find(|_| !r.drive(a, &[]).is_empty());
    // Animation navi[2 + 1] = 4: sound 0x1012 (a sequence) at update 6 + 0x14.
    assert_eq!(at, Some(25));
}

#[test]
fn passing_countdown() {
    let mut r = Rig::new(5);
    let s = Snap {
        target_stops: false,
        scheduled: 1000,
        clock: 1000 - 30,
        ..Snap::default()
    };
    assert_eq!(plays(&r.drive(s, &[])), [COUNTDOWN, COUNTDOWN]);
    let off = Snap {
        clock: 1000 - 31,
        ..s
    };
    assert!(r.drive(off, &[]).is_empty());
    // A station just passed freezes the clocks (and the countdown) for 90 ticks.
    let ev = [RuleEvent::StationPassed { station: 1 }];
    assert!(plays(&r.drive(s, &ev)).is_empty());
}

#[test]
fn rain_start_and_tunnels() {
    let mut r = Rig::new(5);
    let b = Snap::default();
    let a = Snap {
        weather_on: true,
        weather_level: 2,
        ..b
    };
    assert_eq!(
        r.tick(b, a, &[]),
        [
            Command::Play(RAIN),
            Command::SetPitch(RAIN, 0),
            Command::SetPitch(RAIN, 1)
        ]
    );
    // Entering a tunnel (class 3 at probe +4) stops the rain; leaving restarts it.
    let mut info = TickInfo {
        before: a,
        after: a,
        events: &[],
        segment_started: false,
        track_classes: Some([0, 3, 0]),
        turnout_entered: false,
        demo: false,
    };
    let cmds = r.run(&info);
    assert!(cmds.contains(&Command::Release(RAIN)));
    info.track_classes = Some([0, 0, 0]);
    let cmds = r.run(&info);
    assert!(cmds.contains(&Command::Play(RAIN)));
    // Off: the level decays; released when it reaches 1.
    let decaying = Snap {
        weather_on: false,
        weather_level: 2,
        ..a
    };
    let gone = Snap {
        weather_on: false,
        weather_level: -1,
        ..a
    };
    assert_eq!(r.tick(decaying, gone, &[]), [Command::Release(RAIN)]);
}

/// A short run of stage 0 on the user's data: the tables read, and the departure, signal and
/// limit sounds come out of the real engine events. Skips without `SHUPPATSU_DATA_DIR`.
#[test]
fn stage_run_with_game_data() {
    use shuppatsu_engine::drive::{DriverInput, LoadError};
    use shuppatsu_engine::physics::TrainTables;
    use shuppatsu_engine::rules::RuleTables;
    use shuppatsu_formats::exe::Exe;

    let Ok(data) = crate::scene::GameData::from_env() else {
        eprintln!("SHUPPATSU_DATA_DIR not set; skipped");
        return;
    };
    let Ok(exe_bytes) = data.exe() else {
        eprintln!("no exe under SHUPPATSU_DATA_DIR; skipped");
        return;
    };
    let exe = Exe::parse(&exe_bytes).unwrap();
    let stage = 0;
    let tables = StageTables::read(&exe, stage).unwrap();
    assert_eq!(tables.jingle, 0xc);
    assert_eq!(tables.navi.reactions, [2, 1, 3, 3]);
    assert_eq!(tables.navi.frames.len(), crate::navigator::ANIMS);
    let mut st = StageSound::new(tables);

    let train = TrainTables::read(&exe).unwrap();
    let rules = RuleTables::read(&exe).unwrap();
    let stages = shuppatsu_formats::stages::StageTables::read(&exe).unwrap();
    let loader = move |name: &str| -> Result<Vec<u8>, LoadError> {
        data.read(name)
            .map_err(|e| LoadError::from(format!("{e:#}")))
    };
    let mut drive = Drive::new(&train, &rules, &stages, stage, 1, 0, loader).unwrap();
    let sound_tables = Arc::new(SoundTables::from_exe(&exe).unwrap());
    let mut snd = SoundSystem::new(sound_tables.clone());
    let mut ts = TrainSound::new(&sound_tables, stage, drive.train().train_type, &mut snd);

    let mut codes = Vec::new();
    for _ in 0..3000 {
        let before = Snap::of(&drive);
        let r = drive.rules();
        let v = drive.speed_mps();
        let input = if r.phase == Phase::PreDeparture && !r.departure_done {
            DriverInput {
                brake: 8,
                ..DriverInput::default()
            }
        } else if r.dist_m < f32::max(v * v, 30.0) {
            DriverInput {
                brake: 6,
                ..DriverInput::default()
            }
        } else if v < 15.0 {
            DriverInput {
                power: 4,
                ..DriverInput::default()
            }
        } else {
            DriverInput::default()
        };
        let out = drive.tick(input);
        let info = TickInfo {
            before,
            after: Snap::of(&drive),
            events: &out.events,
            segment_started: out.segment_started,
            track_classes: drive.track_classes(),
            turnout_entered: drive.turnout_entered(),
            demo: drive.demo_counter().is_some(),
        };
        st.loop_part(&mut snd, &mut ts, &info);
        st.task_part(&mut snd, &ts, &info);
        snd.frame();
        codes.extend(plays(&snd.take_commands()));
    }
    eprintln!("stage 0: {} plays: {codes:x?}", codes.len());
    for code in [0xc, DEPART_BELL, DEPART_SIGNAL, RUNNING_NOISE] {
        assert!(codes.contains(&code), "{code:#x} never played: {codes:x?}");
    }
    assert!(
        DEPART_VOICES.iter().any(|c| codes.contains(c)),
        "no departure voice: {codes:x?}"
    );
}

/// Both attract demos on the user's data, through the real engine: the ATS chime never rings
/// (it rang every 32 ticks while the demos ran 0x40c4e0's sounds), the running noise starts and
/// the lesson's signal call-outs still play. Skips without `SHUPPATSU_DATA_DIR`.
#[test]
fn demo_drives_with_game_data() {
    use shuppatsu_engine::demo::{DemoKind, DemoTables};
    use shuppatsu_engine::drive::{DriverInput, LoadError};
    use shuppatsu_engine::physics::TrainTables;
    use shuppatsu_engine::rules::RuleTables;
    use shuppatsu_formats::exe::Exe;

    let Ok(data) = crate::scene::GameData::from_env() else {
        eprintln!("SHUPPATSU_DATA_DIR not set; skipped");
        return;
    };
    let Ok(exe_bytes) = data.exe() else {
        eprintln!("no exe under SHUPPATSU_DATA_DIR; skipped");
        return;
    };
    let exe = Exe::parse(&exe_bytes).unwrap();
    let train = TrainTables::read(&exe).unwrap();
    let rules = RuleTables::read(&exe).unwrap();
    let stages = shuppatsu_formats::stages::StageTables::read(&exe).unwrap();
    let demos = DemoTables::read(&exe).unwrap();
    let sound_tables = Arc::new(SoundTables::from_exe(&exe).unwrap());
    for (kind, stage) in [
        (DemoKind::Route, 1),
        (DemoKind::Lesson, DemoKind::LESSON_STAGE),
    ] {
        let data = crate::scene::GameData::from_env().unwrap();
        let loader = move |name: &str| -> Result<Vec<u8>, LoadError> {
            data.read(name)
                .map_err(|e| LoadError::from(format!("{e:#}")))
        };
        let mut drive = Drive::new(&train, &rules, &stages, stage, 1, 0, loader).unwrap();
        drive.start_demo(kind, &demos);
        let mut st = StageSound::new(StageTables::read(&exe, stage).unwrap());
        let mut snd = SoundSystem::new(sound_tables.clone());
        let mut ts = TrainSound::new(&sound_tables, stage, drive.train().train_type, &mut snd);
        // `Sound::start_demo`: the creators start the running noise and the motor.
        ts.start_segment(&mut snd);
        let mut codes = plays(&snd.take_commands());
        while !drive.demo_finished() {
            let before = Snap::of(&drive);
            let out = drive.tick(DriverInput::default());
            let info = TickInfo {
                before,
                after: Snap::of(&drive),
                events: &out.events,
                segment_started: out.segment_started,
                track_classes: drive.track_classes(),
                turnout_entered: drive.turnout_entered(),
                demo: drive.demo_counter().is_some(),
            };
            st.loop_part(&mut snd, &mut ts, &info);
            st.task_part(&mut snd, &ts, &info);
            snd.frame();
            codes.extend(plays(&snd.take_commands()));
        }
        eprintln!("{kind:?} demo: {} plays: {codes:x?}", codes.len());
        assert!(!codes.contains(&ATS_CHIME), "{kind:?}: chime in {codes:x?}");
        // 0x8570d4 was never reset (no departure), which is what made the chime ring.
        if kind == DemoKind::Route {
            assert_ne!(drive.rules().ats_chime_from, -1);
        }
        assert_eq!(drive.demo_counter(), Some(kind.ticks()));
        assert_eq!(codes[0], RUNNING_NOISE, "{kind:?}");
        if kind == DemoKind::Lesson {
            assert!(codes.iter().any(|c| ASPECT_CALLS.contains(c)), "{codes:x?}");
        }
    }
}

/// A weather start split by the sky fade's `Task_Tick`: the rain starts in the split tick at
/// level 1 / 2; 0x40c4e0 (pitch level / 2) only runs in the tick that finishes the body.
#[test]
fn rain_start_across_the_sky_fade_tick() {
    let mut r = Rig::new(5);
    let b = Snap::default();
    let split = Snap {
        weather_on: true,
        weather_level: 1,
        fade_pending: Some(FadeSplit::CourseEvent),
        ..b
    };
    assert_eq!(
        r.tick(b, split, &[]),
        [Command::Play(RAIN), Command::SetPitch(RAIN, 0)]
    );
    let resumed = Snap {
        weather_level: 2,
        fade_pending: None,
        ..split
    };
    assert_eq!(r.tick(split, resumed, &[]), [Command::SetPitch(RAIN, 1)]);
}

/// Every rail-joint click of a turnout clatter is a force-feedback jolt (0x4204d0 calls
/// 0x434940 with each, input.md §11); the count is taken once.
#[test]
fn turnout_clicks_are_counted_for_the_vibration() {
    let mut r = Rig::new(0);
    let moving = Snap {
        speed: 10.0,
        ..Snap::default()
    };
    r.run(&TickInfo {
        before: moving,
        after: moving,
        events: &[],
        segment_started: false,
        track_classes: None,
        turnout_entered: true,
        demo: false,
    });
    let mut clicks = r.st.take_turnout_clicks();
    for _ in 0..200 {
        r.drive(moving, &[]);
        clicks += r.st.take_turnout_clicks();
    }
    assert_eq!(clicks, 8);
    assert_eq!(r.st.take_turnout_clicks(), 0);
}

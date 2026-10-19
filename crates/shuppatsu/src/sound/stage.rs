//! In-stage sound events: what the driving loop (0x4080d0 / 0x4084f5), its rule functions and
//! the HUD tasks request besides the train task (docs/re/audio-text.md §10).
//!
//! The engine reports rule events and exposes its state; this module maps them to the
//! original's `snd_request` (0x41fda0), release (0x420650) and stop-all (0x420660) calls in the
//! order the original makes them within a tick. Every mapping names its call site. A tick is
//! split like the original's: [`StageSound::loop_part`] is the loop body (track lookup, rule
//! functions, the stopped-train handler), then the caller runs the train task
//! ([`shuppatsu_audio::TrainSound`]), then [`StageSound::task_part`] is the rest of `Task_Tick`
//! (task lists: turnout clatter, navigator animations, departure task, HUD, time-up).
//!
//! Everything here is pure: it reads [`Snap`]shots and posts through a [`SoundSystem`], so it
//! is unit-tested with a recording sound system.

use crate::navigator::{Effect, NaviState, NaviTables, NaviTick, Navigator};
use anyhow::{Context, Result};
use shuppatsu_audio::{AmbientSound, SoundSystem, TrainSound, TurnoutClatter};
use shuppatsu_engine::drive::{Drive, DrivePhase};
use shuppatsu_engine::rules::{AfterHold, Phase, RuleEvent, STAT_COUNT, TimeReason};
use shuppatsu_formats::exe::Exe;
use shuppatsu_formats::hud::HudTables;

// ---------------------------------------------------------------------------------------------
// Sound codes (call sites in the comments; meanings from context, docs/re/audio-text.md §10)
// ---------------------------------------------------------------------------------------------

/// Penalty buzzer: overspeed (0x40b780, both branches), false start (0x4080d0 pre-departure
/// loop), time-up popup (0x4159c0).
const BUZZER: u16 = 0x11;
/// ATS bell: trip (0x40c4e0, ATS state 1), obstacle event (0x40acd0 kind 8). Released on
/// acknowledgement (0x40c4e0), recovery (0x40a0a0), departure (0x413370) and loop exit
/// (0x4085e0).
const ATS_BELL: u16 = 0xb;
/// ATS acknowledged chime, every 32 ticks (0x40c4e0); released by the next sighting
/// (0x40c010) and at loop exit (0x4085e0).
const ATS_CHIME: u16 = 0xd;
const ATS_CHIME_PERIOD: i32 = 32;
/// Course event 7 (ATC / derailment speed): 0x40acd0.
const ATC_SECTION: u16 = 0xe;
/// Speed-limit call prefix played alone on stages 0–2 (0x40acd0 / 0x40b4a0 kind 4).
const LIMIT_PREFIX: u16 = 0xc2;
/// On-time pass (0x40d620, after time message 13).
const ON_TIME_PASS: u16 = 0xbc;
/// Rain (0x4080d0, 0x40acd0 kind 21, 0x40c4e0, 0x40d620, 0x40a0a0); pitch = level / 2.
const RAIN: u16 = 0x28;
/// Departure bell: requested by the segment HUD setup 0x4124f0, released by the departure
/// task at offset + 200 (0x413370).
const DEPART_BELL: u16 = 0x2d;
/// Starting signal turns red at offset + 100 (0x413370).
const DEPART_SIGNAL: u16 = 0x23;
/// Extra BGM-slot jingle of stages 13–15 between offset + 90 and + 130 (0x413370).
const DEPART_EXTRA_JINGLE: u16 = 0x1a6;
/// Jingle of stage 10 station 7 instead of the table entry (0x4124f0).
const STAGE10_STATION7_JINGLE: u16 = 0x1a8;
/// Departure voice by aspect once the train moves (0x413370 stack table).
const DEPART_VOICES: [u16; 5] = [0x97, 0xce, 0xcf, 0xd0, 0xd2];
/// Voice at departure-task tick 0x30 (0x413370): sequence 0x18 at the first station of stages
/// 0–2 / 17–19, 0x16d at their other stations, 0xd5 on the other stages.
const DEPART_CALL_FIRST: u16 = 0x1018;
const DEPART_CALL_OTHER: u16 = 0x16d;
const DEPART_CALL_LOCAL: u16 = 0xd5;
/// "Green" call after a red-signal / ATS / obstacle recovery (0x40a0a0, both branches).
const RECOVERY_CALL: u16 = 0x97;
/// Forced stop (buffer stop): crash, then four voices at loop ticks 0, 15, 40, 50 of the
/// 80-tick hold (0x40a0a0).
const BUFFER_STOP_CRASH: u16 = 0x1b;
const BUFFER_STOP_VOICES: [(u32, u16); 4] = [(0, 0xdc), (15, 0xd6), (40, 0xe2), (50, 0xe6)];
/// Arrival jingles, BGM slots (0x40a0a0): clean stop on time / clean stop, released after the
/// 100-tick hold; all-round clear at the final station.
const JINGLE_ON_TIME: u16 = 0x199;
const JINGLE_CLEAN: u16 = 0x198;
const JINGLE_ALL_CLEAR: u16 = 0x19b;
const CLEAN_STOP_HOLD: u32 = 100;
const PLAIN_STOP_HOLD: u32 = 30;
/// Passing-time countdown, once per second for the last 5 s (Clock_Draw 0x417140, which
/// `Hud_Update` calls twice per tick).
const COUNTDOWN: u16 = 0x15;
/// The clocks freeze for 0x5a ticks after a station is passed (0x40d620 → 0x40ff20).
const CLOCK_FREEZE_TICKS: i32 = 0x5a;
/// Remaining-time transfer clicks (0x40ff20): +1 s, −1 s.
const TIME_UP_CLICK: u16 = 0x26;
const TIME_DOWN_CLICK: u16 = 0x27;
/// Running noise, released when the stage loop is left after an arrival (0x4085e0).
const RUNNING_NOISE: u16 = 9;
/// Obstacle recovery animation 0x41a7b0: 0x2c at pitch 0x10, then 0x1a.
const OBSTACLE_ANIM_POINT: u16 = 0x2c;
const OBSTACLE_ANIM_PITCH: i16 = 0x10;
const OBSTACLE_ANIM_SOUND: u16 = 0x1a;
/// E3 coupling animation (course event 23, 0x411240 → 0x411300): sequence 0x29 when the
/// animation counter reaches 0x3c, after a 6-update delay, i.e. on update 65.
const E3_COUPLING: u16 = 0x1029;
const E3_COUPLING_UPDATE: u32 = 5 + 0x3c;

/// 0x40c010: block-signal aspect calls (stack table `local_5c`), indexed by aspect 1..=4.
const ASPECT_CALLS: [u16; 5] = [0xa8, 0xa8, 0xa9, 0xaa, 0xd2];
/// 0x40c010: green calls by `block + 10 · display variant` (stack table `local_3c`): by
/// block for variants 0 and 2, 0xbe for variant 1.
const GREEN_CALLS: [u16; 30] = [
    0x97, 0x99, 0x99, 0x1005, 0x1006, 0x1007, 0x1008, 0x1009, 0x100a, 0x100b, //
    0xbe, 0xbe, 0xbe, 0xbe, 0xbe, 0xbe, 0xbe, 0xbe, 0xbe, 0xbe, //
    0x97, 0x99, 0x99, 0x1005, 0x1006, 0x1007, 0x1008, 0x1009, 0x100a, 0x100b,
];
/// 0x40c010: green call from block 8 on: 0xbe with display variant 1, else 0x98.
const GREEN_FAR_VARIANT1: u16 = 0xbe;
const GREEN_FAR: u16 = 0x98;
/// Block index from which 0x40c010 stops using [`GREEN_CALLS`].
const GREEN_TABLE_BLOCKS: usize = 8;

/// Station approach HUD state 0x856d68 (0x40c4e0): caption at 50 m into the leg (f32 50.0 at
/// 0x4594c8), banner and announcement at `f5 + 150` m, caption again at `f5 + 50`, done at
/// `f5 − 10`.
const APPROACH_LEG_M: f32 = 50.0;
const APPROACH_BANNER_M: i32 = 0x96;
const APPROACH_CAPTION_M: i32 = 0x32;
const APPROACH_DONE_M: i32 = -10;
/// Departure voice once the train moves at ≥ 0.04 m/s (double at 0x4594d8, 0x413723).
const DEPART_MOVING_MPS: f64 = 0.04;
/// Departure task milestones relative to its offset (0x413370).
const DEPART_EXTRA_AT: i32 = 0x5a;
const DEPART_RED_AT: i32 = 100;
const DEPART_EXTRA_END_AT: i32 = 0x82;
const DEPART_DOORS_AT: i32 = 200;
const DEPART_DONE_AT: i32 = 0xc9;
const DEPART_FIRST_CALL_AT: i32 = 0x30;
/// The stage loop runs 20 `Task_Tick`s after a game over before 0x4158b0 / 0x4085e0.
const GAME_OVER_TICKS: u32 = 0x14;
/// Time-up popup task 0x4159c0: buzzer on update 0x24 (counter 0x23), advice voice on update
/// 0x38 (counter 0x37).
const TIME_UP_BUZZER_UPDATE: u32 = 0x24;
const TIME_UP_VOICE_UPDATE: u32 = 0x38;
/// Holding ticks left (rules) at which the obstacle recovery's scripted steps fall: the
/// 0x41a7b0 animation after two `Task_Tick`s, rain, the train-sound restart one `Task_Tick`
/// before the green signal (which the rules show at 31). Guesses: the rules model the
/// obstacle hold as 36 ticks.
const OBSTACLE_ANIM_AT: u32 = 35;
const OBSTACLE_RAIN_AT: u32 = 33;
const OBSTACLE_RESTART_AT: u32 = 32;

/// Departure jingle per stage, i16 (0x466be0, copied to the departure task by 0x4124f0).
const DEPART_JINGLE_TABLE: u32 = 0x466be0;
const STAGES: usize = 20;

/// Weather type `0x857210`, a switch on the stage in 0x40d040; rain sounds only below 3.
fn weather_type(stage: usize) -> i16 {
    match stage {
        0..=2 | 17..=19 => 5,
        3 | 4 => 4,
        7 => 2,
        8..=11 => 1,
        _ => 0,
    }
}

/// Signal display mode of block `block` as 0x40c010 computes it (0 block, 1 home, 2 starting).
fn signal_mode(stage: usize, block: usize) -> i32 {
    match (stage == 4, block) {
        (true, 0 | 6 | 9) | (false, 0) => 2,
        (true, 2 | 7 | 10) | (false, 2) => 1,
        _ => 0,
    }
}

// ---------------------------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------------------------

/// Per-stage tables the stage sounds read from the exe.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StageTables {
    pub stage: usize,
    /// Station-name id per station (`0x4860bc[stage]`, copied to 0x856d58): the approach call
    /// 0x41ffe0 uses pair `id`, course event 2 (0x420040) pair `90 + id`.
    pub station_names: Vec<u16>,
    /// Departure jingle (0x466be0[stage]).
    pub jingle: u16,
    /// The navigator animations and the stage's choice of them (navigator.rs).
    pub navi: NaviTables,
}

impl StageTables {
    pub fn read(exe: &Exe<'_>, stage: usize) -> Result<Self> {
        anyhow::ensure!(stage < STAGES, "stage {stage} out of range");
        let i16_at = |va: u32| -> Result<i16> {
            let b = exe.bytes_at(va, 2)?;
            Ok(i16::from_le_bytes([b[0], b[1]]))
        };
        let hud = HudTables::read(exe).context("HUD tables")?;
        let station_names = hud
            .stage_station_ids
            .get(stage)
            .cloned()
            .unwrap_or_default();
        let jingle = i16_at(DEPART_JINGLE_TABLE + 2 * stage as u32)? as u16;
        Ok(Self {
            stage,
            station_names,
            jingle,
            navi: NaviTables::read(exe, stage)?,
        })
    }

    /// Name id of station `station` of the stage (0x856d58[station]).
    fn station_name(&self, station: usize) -> Option<usize> {
        self.station_names.get(station).map(|&id| usize::from(id))
    }
}

// ---------------------------------------------------------------------------------------------
// Snapshots
// ---------------------------------------------------------------------------------------------

/// The rule and train state the stage sounds read, at one instant (before or after a tick).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Snap {
    pub phase: Phase,
    /// `Drive::phase() == Loading`: between a station result and the next setup tick.
    pub loading: bool,
    pub station: usize,
    pub clock: i32,
    pub scheduled: i32,
    pub target_stops: bool,
    pub final_station: bool,
    pub aspect: i16,
    pub block: usize,
    pub signal_variant: i16,
    pub repeater_m: i32,
    pub limit_state: i32,
    pub ats: i16,
    pub ats_chime_from: i32,
    pub approach: i16,
    pub eb_ticks: i32,
    pub eb_approach_ticks: i32,
    pub obstacle: i16,
    pub weather_on: bool,
    pub weather_level: i32,
    pub departure_counter: i32,
    pub departure_base: i32,
    pub departure_done: bool,
    pub dist_m: f32,
    pub dist_round_m: i32,
    /// Home-signal distance of the target station (timetable field 5 of row `station + 1`).
    pub home_signal_m: i32,
    /// 0x84c40c: metres since the segment start or the last station passed.
    pub leg_distance: f32,
    pub speed: f32,
    pub odometer: f32,
    pub power: i32,
    pub brake: i32,
    /// 0x84c3f8 (copied to 0x84c3e8): speed lost over the last tick.
    pub decel_prev_tick: f32,
    pub early_ticks: i32,
    pub stats: [i32; STAT_COUNT],
    pub continues: i32,
    pub remaining_s: i32,
    /// The coupling minigame's `0x84c0f8 = 1` ([`shuppatsu_engine::coupling::Coupling::bottom_band`]).
    pub bottom_band: bool,
}

impl Default for Snap {
    fn default() -> Self {
        Self {
            phase: Phase::Driving,
            loading: false,
            station: 0,
            clock: 0,
            scheduled: 0,
            target_stops: true,
            final_station: false,
            aspect: 0,
            block: 0,
            signal_variant: 0,
            repeater_m: -1,
            limit_state: 0,
            ats: 0,
            ats_chime_from: -1,
            approach: 0,
            eb_ticks: 0,
            eb_approach_ticks: 0,
            obstacle: 0,
            weather_on: false,
            weather_level: 0,
            departure_counter: 0,
            departure_base: 0,
            departure_done: false,
            dist_m: 1000.0,
            dist_round_m: 1000,
            home_signal_m: 0,
            leg_distance: 0.0,
            speed: 0.0,
            odometer: 0.0,
            power: 0,
            brake: 0,
            decel_prev_tick: 0.0,
            early_ticks: 0,
            stats: [0; STAT_COUNT],
            continues: 0,
            remaining_s: 60,
            bottom_band: false,
        }
    }
}

impl Snap {
    pub fn of(drive: &Drive) -> Self {
        let r = drive.rules();
        let t = drive.train();
        Self {
            phase: r.phase,
            loading: drive.phase() == DrivePhase::Loading,
            station: r.station,
            clock: r.clock,
            scheduled: r.scheduled,
            target_stops: r.target_stops,
            final_station: r.final_station,
            aspect: r.aspect,
            block: r.block,
            signal_variant: r.signal_variant,
            repeater_m: r.repeater_m,
            limit_state: r.limit_state,
            ats: r.ats,
            ats_chime_from: r.ats_chime_from,
            approach: r.approach,
            eb_ticks: r.eb_ticks,
            eb_approach_ticks: r.eb_approach_ticks,
            obstacle: r.obstacle,
            weather_on: r.weather_on,
            weather_level: r.weather_level,
            departure_counter: r.departure_counter,
            departure_base: r.departure_base,
            departure_done: r.departure_done,
            dist_m: r.dist_m,
            dist_round_m: r.dist_round_m,
            home_signal_m: r
                .timetable()
                .get(r.station + 1)
                .map_or(0, |row| row.params[1]),
            leg_distance: t.leg_distance,
            speed: t.speed,
            odometer: t.odometer,
            power: t.prev_power,
            brake: t.prev_brake,
            decel_prev_tick: t.decel_prev_tick,
            early_ticks: r.early_ticks,
            stats: r.stats,
            continues: r.continues,
            remaining_s: r.remaining_s,
            bottom_band: r
                .coupling
                .as_ref()
                .is_some_and(|c| r.phase == Phase::Coupling && c.bottom_band()),
        }
    }

    /// What the navigator triggers read.
    fn navi(&self) -> NaviState {
        NaviState {
            phase: self.phase,
            loading: self.loading,
            departure_done: self.departure_done,
            clock: self.clock,
            ats: self.ats,
            approach: self.approach,
            eb_ticks: self.eb_ticks,
            eb_approach_ticks: self.eb_approach_ticks,
            obstacle: self.obstacle,
            slams: self.stats[11],
            final_station: self.final_station,
        }
    }

    /// The tick starting from this state runs the driving-loop body (0x4084f5): driving, or
    /// leaving the pre-departure wait.
    fn runs_loop_body(&self) -> bool {
        self.phase == Phase::Driving || (self.phase == Phase::PreDeparture && self.departure_done)
    }

    /// The departure task (0x413370) updates in the tick starting from this state.
    fn runs_departure_task(&self) -> bool {
        self.phase == Phase::PreDeparture && !self.departure_done
    }

    fn holding(&self) -> Option<(u32, AfterHold)> {
        match self.phase {
            Phase::Holding { ticks, then } => Some((ticks, then)),
            _ => None,
        }
    }
}

/// One tick as the stage sounds see it.
#[derive(Debug, Clone, Copy)]
pub struct TickInfo<'a> {
    pub before: Snap,
    pub after: Snap,
    pub events: &'a [RuleEvent],
    pub segment_started: bool,
    /// Classes at the ambient probes if 0x407c10 ran ([`Drive::track_classes`]).
    pub track_classes: Option<[i16; 3]>,
    /// [`Drive::turnout_entered`].
    pub turnout_entered: bool,
    /// An attract demo drive ([`Drive::demo_counter`] is `Some`): its task (0x40e630 /
    /// 0x40e860) calls only part of the loop body, never 0x40c4e0 or 0x40a0a0.
    pub demo: bool,
}

// ---------------------------------------------------------------------------------------------
// Tasks
// ---------------------------------------------------------------------------------------------

/// A sound posted from a task a fixed number of updates later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Timed {
    Request(u16),
    Release(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Timer {
    /// Fires in the `task_part` where this is 0; counts down by one per `task_part`.
    ticks: u32,
    action: Timed,
}

// ---------------------------------------------------------------------------------------------
// The stage sounds
// ---------------------------------------------------------------------------------------------

/// State the original keeps in globals and tasks for its in-stage sounds.
#[derive(Debug, Clone, PartialEq)]
pub struct StageSound {
    tables: StageTables,
    ambient: AmbientSound,
    /// Station approach HUD state 0x856d68.
    approach_hud: i16,
    /// Clock freeze 0x856c6c (the countdown is part of the clock drawing).
    clock_freeze: i32,
    /// The departure task reached its end but the train has not moved yet.
    depart_voice_pending: bool,
    /// Jingle the departure task started (task+0x48).
    jingle: u16,
    /// Arrival jingle to release, and the all-round-clear flag, after the hold.
    arrival: Option<(Option<u16>, bool, u32)>,
    timers: Vec<Timer>,
    turnouts: Vec<TurnoutClatter>,
    navi: Navigator,
    /// Ticks since the game over (the game-over tick is 1).
    game_over_ticks: Option<u32>,
    /// Category of the last statistic added (`0x856d84`, −1 after a reset).
    last_stat: i32,
    /// The stage loop was left; sequences re-enable when the next segment loads.
    sequences_off: bool,
}

impl StageSound {
    pub fn new(tables: StageTables) -> Self {
        Self {
            tables,
            ambient: AmbientSound::new(),
            approach_hud: 0,
            clock_freeze: 0,
            depart_voice_pending: false,
            jingle: 0,
            arrival: None,
            timers: Vec::new(),
            turnouts: Vec::new(),
            navi: Navigator::default(),
            game_over_ticks: None,
            last_stat: -1,
            sequences_off: false,
        }
    }

    fn stage(&self) -> usize {
        self.tables.stage
    }

    fn rain(&self) -> bool {
        weather_type(self.stage()) < 3
    }

    fn after(&mut self, ticks: u32, action: Timed) {
        self.timers.push(Timer { ticks, action });
    }

    /// The driving-loop body and the stopped-train handler of one tick, plus the segment setup
    /// on a segment's first tick.
    pub fn loop_part(&mut self, snd: &mut SoundSystem, train: &mut TrainSound, t: &TickInfo<'_>) {
        let (b, a) = (&t.before, &t.after);
        if self.sequences_off && (a.loading || t.segment_started) {
            // 0x4080d0 sets 0x84faf8 again after the station result (0x40d340).
            self.sequences_off = false;
            snd.set_sequences_enabled(true);
        }
        self.track_stats(b, a);
        if t.segment_started {
            self.segment_setup(snd, train, a);
            return;
        }

        let loop_body = b.runs_loop_body();
        // The demo tasks run 0x40dad0 (and in the lesson 0x40acd0, 0x40c010, 0x40b780) but not
        // 0x40c4e0 or the stopped-train handler 0x40a0a0, so no ATS chime, approach call or
        // rain from there (docs/re/audio-text.md §10.2).
        let tail_runs = loop_body && !t.demo;
        // 0x407c10: ambient sounds, and the probe-4 transitions the rain uses.
        let mut entered = 0;
        let mut left = 0;
        if let Some(classes) = t.track_classes {
            let old = self.ambient.classes();
            if old[1] != classes[1] {
                entered = classes[1];
                left = old[1];
            }
            self.ambient.tick(classes, b.speed, b.odometer, snd);
        }

        let mut limit_state = b.limit_state;
        let mut late_done = false;
        let mut repeater_done = false;
        let station_passed = t
            .events
            .iter()
            .any(|e| matches!(e, RuleEvent::StationPassed { .. }));
        for e in t.events {
            if tail_runs && !late_done && is_from_stop_handler(e) {
                late_done = true;
                self.loop_tail(snd, t, entered, left, station_passed);
                self.stop_handler_start(snd, t);
            }
            match *e {
                RuleEvent::StationPassed { .. } => {
                    // 0x40d620.
                    self.approach_hud = 0;
                    self.clock_freeze = CLOCK_FREEZE_TICKS;
                }
                RuleEvent::Time { reason, .. } => self.time_message(snd, reason),
                RuleEvent::Script { kind, args } => self.course_event(snd, train, a, kind, args),
                RuleEvent::SpeedLimit { kmh, .. } => {
                    // 0x40acd0 / 0x40b4a0: kind 4 applies with no limit in force, kind 5 with
                    // one; both leave 0x856fc4 at 0.
                    let first = limit_state == 0;
                    let stage = self.stage();
                    if first && stage < 3 {
                        snd.request(LIMIT_PREFIX);
                    } else if first || stage > 2 {
                        snd.call_prefix_speed(0, kmh);
                    }
                    limit_state = 0;
                }
                RuleEvent::SpeedLimitEnd => {
                    // Kind 6: "limit lifted" when a sign was ever shown (0x856d24), which the
                    // kind's own 制限解除 sign guarantees.
                    snd.call_prefix_speed(0, 0);
                    limit_state = 0;
                }
                RuleEvent::SignalShown {
                    aspect,
                    forced,
                    display,
                    ..
                } if loop_body && !forced => {
                    // 0x40c010: a sighting. Display 3 is the repeater path, which calls even
                    // when the display is a no-op; see below.
                    if display == 3 {
                        repeater_done = true;
                        self.repeater_call(snd, a);
                    } else {
                        self.sighting_call(snd, a, aspect);
                    }
                    if b.ats_chime_from != -1 {
                        snd.release(ATS_CHIME);
                    }
                }
                RuleEvent::AtsBell => snd.request(ATS_BELL),
                RuleEvent::AtsAcknowledged => snd.release(ATS_BELL),
                RuleEvent::SignalsReset => snd.release(ATS_BELL),
                RuleEvent::ObstacleStop => {
                    // 0x40acd0 kind 8 (`NaviAnim(0)` follows in 0x40c4e0: navigator.rs).
                    snd.request(ATS_BELL);
                }
                RuleEvent::GameOver => {
                    // 0x40857b: stop all, then 20 `Task_Tick`s.
                    snd.stop_all();
                    self.game_over_ticks = Some(0);
                    self.depart_voice_pending = false;
                }
                RuleEvent::Arrived(arr) => {
                    let hold = if arr.mark > 0 {
                        CLEAN_STOP_HOLD
                    } else {
                        PLAIN_STOP_HOLD
                    };
                    let jingle = self.arrival.and_then(|j| j.0);
                    self.arrival = Some((jingle, a.final_station, hold));
                }
                _ => {}
            }
        }
        if loop_body {
            // The repeater path of 0x40c010 consumed 0x856fb4 without a new display.
            if !repeater_done && b.repeater_m > 0 && a.repeater_m == 0 {
                self.repeater_call(snd, a);
                if b.ats_chime_from != -1 {
                    snd.release(ATS_CHIME);
                }
            }
            if tail_runs && !late_done {
                self.loop_tail(snd, t, entered, left, station_passed);
                self.stop_handler_start(snd, t);
            }
        }
        self.holds(snd, train, t);
        self.leave_loop(snd, t);
    }

    /// The rest of `Task_Tick` after the train task.
    pub fn task_part(&mut self, snd: &mut SoundSystem, train: &TrainSound, t: &TickInfo<'_>) {
        let (b, a) = (&t.before, &t.after);
        // Train task 0x422c20: turnout clatter (moving branch).
        if t.turnout_entered && a.speed > 0.0 {
            self.turnouts.push(TurnoutClatter::new(a.speed));
        }
        // List 0x84c180: turnout clatter (0x4204d0) and timed releases (0x420440).
        let joint = train.stage_sounds().joint;
        let flange = train.flange();
        self.turnouts.retain_mut(|c| c.tick(joint, flange, snd));
        let mut i = 0;
        while i < self.timers.len() {
            if self.timers[i].ticks == 0 {
                match self.timers.remove(i).action {
                    Timed::Request(code) => snd.request(code),
                    Timed::Release(code) => snd.release(code),
                }
            } else {
                self.timers[i].ticks -= 1;
                i += 1;
            }
        }
        // The tick's `NaviAnim` calls (navigator.rs), then list 3: navigator animations, the
        // departure task, the HUD.
        let navi_tick = NaviTick {
            before: b.navi(),
            after: a.navi(),
            events: t.events,
            segment_started: t.segment_started,
            game_over: self.game_over_ticks.is_some(),
            loop_exit: self.game_over_ticks == Some(GAME_OVER_TICKS),
        };
        let mut navi_sound = |e: Effect| {
            if let Effect::Sound(code) = e {
                snd.request(code);
            }
        };
        for call in crate::navigator::calls(&navi_tick, &self.tables.navi) {
            // `NaviAnim` does nothing under the coupling minigame's bottom band (cab.rs).
            if b.bottom_band && call != crate::navigator::Call::Clear {
                continue;
            }
            self.navi.apply(call, &self.tables.navi, &mut navi_sound);
        }
        self.navi.update(&self.tables.navi, &mut navi_sound);
        self.departure_task(snd, t);
        self.hud(snd, b, a);
        if let Some((jingle, all_clear, hold)) = self.arrival {
            if hold > 0 {
                self.arrival = Some((jingle, all_clear, hold - 1));
            } else {
                // 0x40a0a0 after the 100 / 30 `Task_Tick`s of the hold (the first is in the
                // evaluation tick): release the jingle (0x420630), then the all-round clear at
                // the final station (0x40aaa5).
                if let Some(code) = jingle {
                    snd.release(code);
                }
                if all_clear {
                    snd.request(JINGLE_ALL_CLEAR);
                }
                self.arrival = None;
            }
        }
    }

    /// 0x4080d0 at a segment start, 0x40cf40 (segment reset) and 0x4124f0 (segment HUD).
    fn segment_setup(&mut self, snd: &mut SoundSystem, train: &TrainSound, a: &Snap) {
        train.start_segment(snd);
        self.ambient.reset();
        self.approach_hud = 0;
        self.clock_freeze = 0;
        self.depart_voice_pending = false;
        self.arrival = None;
        self.game_over_ticks = None;
        // 0x4124f0: the departure bell rings until the doors close (offset + 200).
        snd.request(DEPART_BELL);
        // 0x4080d0 after 0x40dad0: rain resumes at the segment start.
        if a.weather_level > 0 && self.rain() {
            snd.request(RAIN);
            snd.set_pitch(RAIN, ((a.weather_level / 2) & 0x7f) as i16);
        }
    }

    /// `0x856d84`: category of the last 0x40dd60 call, reset by 0x40dd10 (stage start,
    /// continue). Several categories in one tick: the highest wins (call order not modelled).
    fn track_stats(&mut self, b: &Snap, a: &Snap) {
        if a.continues != b.continues {
            self.last_stat = -1;
            return;
        }
        if let Some(cat) = (0..STAT_COUNT).rev().find(|&c| a.stats[c] > b.stats[c]) {
            self.last_stat = cat as i32;
        }
    }

    /// Sounds raised with a time message (0x415080 callers).
    fn time_message(&mut self, snd: &mut SoundSystem, reason: TimeReason) {
        match reason {
            // 0x40d620 after message 13.
            TimeReason::OnTimePass => snd.request(ON_TIME_PASS),
            // 0x40b780 (before the message) and the pre-departure loop of 0x4080d0 (after).
            TimeReason::Overspeed | TimeReason::FalseStart => snd.request(BUZZER),
            TimeReason::ForcedStop | TimeReason::ForcedStopFinal => {
                // 0x40a0a0: crash, then the voices of the 80-tick hold.
                snd.request(BUFFER_STOP_CRASH);
                for (at, code) in BUFFER_STOP_VOICES {
                    if at == 0 {
                        snd.request(code);
                    } else {
                        self.after(at, Timed::Request(code));
                    }
                }
            }
            TimeReason::OnTimeStop | TimeReason::CleanStop => {
                // 0x40a0a0: the jingle starts before the message (0x42ff20).
                let code = if reason == TimeReason::OnTimeStop {
                    JINGLE_ON_TIME
                } else {
                    JINGLE_CLEAN
                };
                snd.request(code);
                self.arrival = Some((Some(code), false, CLEAN_STOP_HOLD));
            }
            _ => {}
        }
    }

    /// Course events that are sounds (0x40acd0).
    fn course_event(
        &mut self,
        snd: &mut SoundSystem,
        train: &mut TrainSound,
        a: &Snap,
        kind: i16,
        args: [i16; 3],
    ) {
        match kind {
            // Kind 1: a sound or sequence code.
            1 => snd.request(args[0] as u16),
            // Kind 2: station announcement 0x420040(a, name of the next station).
            2 => {
                if let Some(name) = self.tables.station_name(a.station + 1) {
                    snd.call_station(i32::from(args[0]), name);
                }
            }
            // Kind 7: ATC / derailment speed section.
            7 => snd.request(ATC_SECTION),
            // Kind 13: 0x420470(a, b) plays `a` and a task releases it `b` updates later.
            13 => {
                snd.request(args[0] as u16);
                let ticks = u32::try_from(args[1]).unwrap_or(0);
                self.after(ticks, Timed::Release(args[0] as u16));
            }
            // Kinds 18 / 19: 0x84c400, the flange-squeal / departure-sound flag.
            18 => train.set_flange(true),
            19 => train.set_flange(false),
            // Kind 23: E3 coupling animation 0x411240 → 0x411300.
            23 => self.after(E3_COUPLING_UPDATE - 1, Timed::Request(E3_COUPLING)),
            _ => {}
        }
    }

    /// 0x40c010's call-out for a sighted signal whose display task was created.
    fn sighting_call(&mut self, snd: &mut SoundSystem, a: &Snap, aspect: i16) {
        let stage = self.stage();
        let mode = signal_mode(stage, a.block);
        let passing_route = !(3..=16).contains(&stage);
        let aspect = i32::from(aspect);
        let green = || {
            usize::try_from(a.signal_variant)
                .ok()
                .and_then(|v| GREEN_CALLS.get(a.block + 10 * v))
                .copied()
        };
        if aspect != 0 {
            if mode == 0 {
                if let Some(&code) = usize::try_from(aspect)
                    .ok()
                    .and_then(|i| ASPECT_CALLS.get(i))
                {
                    snd.request(code);
                }
            } else if passing_route && !a.target_stops {
                snd.call_signal(3, mode, aspect);
            } else {
                snd.call_signal(0, mode, aspect);
            }
        } else if a.block < GREEN_TABLE_BLOCKS {
            let by_table =
                mode == 0 || (passing_route && a.target_stops) || (!passing_route && stage != 4);
            if by_table {
                if let Some(code) = green() {
                    snd.request(code);
                }
            } else if passing_route {
                snd.call_signal(3, mode, 0);
            } else {
                snd.call_signal(0, mode, 0);
            }
        } else if mode != 0 {
            snd.call_signal(0, mode, 0);
        } else if a.signal_variant == 1 {
            snd.request(GREEN_FAR_VARIANT1);
        } else {
            snd.request(GREEN_FAR);
        }
    }

    /// 0x40c010's repeater path: `0x420320(mode, 0, word)`, the mode of the block before the
    /// increment as the prefix.
    fn repeater_call(&mut self, snd: &mut SoundSystem, a: &Snap) {
        let mode = signal_mode(self.stage(), a.block.saturating_sub(1));
        let (prefix, word) = match (a.signal_variant == 1, a.aspect == 0) {
            (true, true) => (0, 5),
            (true, false) | (false, false) => (mode, 6),
            (false, true) => (mode, 0),
        };
        snd.call_signal(prefix, 0, word);
    }

    /// 0x40c4e0's sounds: approach announcement, EB-in-approach reaction, overrun reaction,
    /// ATS chime, rain.
    fn loop_tail(
        &mut self,
        snd: &mut SoundSystem,
        t: &TickInfo<'_>,
        entered: i16,
        left: i16,
        station_passed: bool,
    ) {
        let (b, a) = (&t.before, &t.after);
        // Approach HUD state 0x856d68: one step per tick.
        let leg = if station_passed { 0.0 } else { b.leg_distance };
        let f5 = a.home_signal_m;
        match self.approach_hud {
            0 if leg >= APPROACH_LEG_M => self.approach_hud = 1,
            1 if a.dist_round_m <= f5 + APPROACH_BANNER_M => {
                // 0x41ffe0(3 = stop / 2 = pass, name): "<station>, <suffix>".
                if let Some(name) = self.tables.station_name(a.station + 1) {
                    snd.call_pair(if a.target_stops { 3 } else { 2 }, name);
                }
                self.approach_hud = 2;
            }
            2 if a.dist_m <= (f5 + APPROACH_CAPTION_M) as f32 => self.approach_hud = 3,
            3 if a.dist_m <= (f5 + APPROACH_DONE_M) as f32 => self.approach_hud = 4,
            _ => {}
        }
        // ATS chime every 32 ticks from the acknowledgement (clock before this tick's ++).
        if a.ats_chime_from != -1 && (b.clock - a.ats_chime_from) % ATS_CHIME_PERIOD == 0 {
            snd.request(ATS_CHIME);
        }
        // Rain.
        if self.rain() {
            let started = b.weather_level <= 0 && a.weather_level > 0;
            if started && !(station_passed && t.track_classes.is_some_and(|c| c[1] == 3)) {
                // 0x40acd0 kind 21 / 0x40d620 (not in a tunnel): start at the old level / 2.
                snd.request(RAIN);
                snd.set_pitch(RAIN, 0);
            }
            let pitch = ((a.weather_level / 2) & 0x7f) as i16;
            // In a tunnel (class 3 at probe +4) the rain stops; it resumes on leaving.
            let tunnel = |snd: &mut SoundSystem| {
                if entered == 3 {
                    snd.release(RAIN);
                }
                if left == 3 {
                    snd.request(RAIN);
                }
            };
            if a.weather_on {
                snd.set_pitch(RAIN, pitch);
                tunnel(snd);
            } else if a.weather_level == -1 && b.weather_level != -1 {
                // The level decayed to 1: released, level −1.
                snd.release(RAIN);
            } else if a.weather_level > 1 {
                tunnel(snd);
                snd.set_pitch(RAIN, pitch);
            }
        }
    }

    /// The start of the stopped-train handler's blocking branches (0x40a0a0).
    fn stop_handler_start(&mut self, snd: &mut SoundSystem, t: &TickInfo<'_>) {
        let (b, a) = (&t.before, &t.after);
        if !b.runs_loop_body() {
            return;
        }
        match a.phase {
            // Arrival: 0x420660 before anything else.
            Phase::Holding {
                then: AfterHold::Drain,
                ..
            }
            | Phase::Draining
            | Phase::Evaluating(_) => snd.stop_all(),
            // Obstacle cleared: stop all (and `NaviAnim(-1)`, navigator.rs).
            Phase::Holding {
                then: AfterHold::ObstacleCleared,
                ..
            } => snd.stop_all(),
            _ => {}
        }
    }

    /// Sounds of the blocking sequences while they run.
    fn holds(&mut self, snd: &mut SoundSystem, train: &TrainSound, t: &TickInfo<'_>) {
        let (b, a) = (&t.before, &t.after);
        if let Some((ticks, then)) = b.holding() {
            let recovery = matches!(then, AfterHold::Resume | AfterHold::ObstacleCleared);
            if then == AfterHold::ObstacleCleared {
                match ticks {
                    OBSTACLE_ANIM_AT => {
                        snd.request(OBSTACLE_ANIM_POINT);
                        snd.set_pitch(OBSTACLE_ANIM_POINT, OBSTACLE_ANIM_PITCH);
                        snd.request(OBSTACLE_ANIM_SOUND);
                    }
                    OBSTACLE_RAIN_AT if a.weather_level > 1 && self.rain() => {
                        snd.request(RAIN);
                        snd.set_pitch(RAIN, ((a.weather_level / 2) & 0x7f) as i16);
                    }
                    OBSTACLE_RESTART_AT => train.start_segment(snd),
                    _ => {}
                }
            }
            if recovery && ticks <= 1 {
                // After the hold's last `Task_Tick`s, before the loop's own.
                snd.request(RECOVERY_CALL);
            }
        }
        if b.phase == Phase::Evaluating(1) {
            // 0x40a0a0: the second 0x420660, after the pending time drained (0x407ff0).
            snd.stop_all();
        }
    }

    /// Leaving the stage loop (0x4085e0), and the time-up popup (0x4158b0) on a game over.
    fn leave_loop(&mut self, snd: &mut SoundSystem, t: &TickInfo<'_>) {
        let (b, a) = (&t.before, &t.after);
        let arrived = b.phase == Phase::Arrived && !self.sequences_off && !b.loading;
        let game_over = match self.game_over_ticks.as_mut() {
            Some(n) => {
                *n += 1;
                *n == GAME_OVER_TICKS
            }
            None => false,
        };
        if game_over {
            // 0x4158b0, then 0x4085e0. The popup's first update is in the next tick.
            let voice = self.time_up_voice(a);
            self.after(TIME_UP_BUZZER_UPDATE, Timed::Request(BUZZER));
            self.after(TIME_UP_VOICE_UPDATE, Timed::Request(voice));
        }
        if !(arrived || game_over) {
            return;
        }
        if b.ats > 0 {
            snd.release(ATS_BELL);
        }
        if b.ats_chime_from != -1 {
            snd.release(ATS_CHIME);
        }
        // 0x84faf8 = 0 drops running sequences at the next sequencer tick.
        snd.set_sequences_enabled(false);
        self.sequences_off = true;
        if arrived {
            // Stage still active (0x855ff8 & 2): release the running noise.
            snd.release(RUNNING_NOISE);
        }
    }

    /// 0x40ddb0: the advice voice of the time-up popup, from the last statistic category.
    fn time_up_voice(&self, a: &Snap) -> u16 {
        let cause = if a.stats[2] > 1 { 2 } else { self.last_stat };
        match cause {
            0 => 0xef,
            1 if a.early_ticks < 1 => 0xf6,
            1 => 0xf0,
            2 => 0xf1,
            // double 0.17 at 0x459548 against 0x84c3e8.
            3 if f64::from(a.decel_prev_tick) > 0.17 => 0xfb,
            3 => 0xea,
            4 => 0xfc,
            5 => 0xf2,
            6 if a.stats[0] < 1 => 0xeb,
            6 => 0xf3,
            7 => 0xec,
            9 => 0xf5,
            0xb => 0xfb,
            _ => 0xf6,
        }
    }

    /// The departure task 0x413370 (one update per `Task_Tick` while it lives).
    fn departure_task(&mut self, snd: &mut SoundSystem, t: &TickInfo<'_>) {
        let (b, a) = (&t.before, &t.after);
        if self.game_over_ticks.is_some() {
            // 0x855ff8 & 0x400 (time-up popup) ends the task.
            self.depart_voice_pending = false;
        }
        if !b.runs_departure_task() || t.segment_started {
            if self.depart_voice_pending && f64::from(a.speed) >= DEPART_MOVING_MPS {
                snd.request(DEPART_VOICES[usize::try_from(a.aspect).unwrap_or(0).min(4)]);
                self.depart_voice_pending = false;
            }
            return;
        }
        let c = b.departure_counter;
        let o = b.departure_base;
        let stage = self.stage();
        let extra = (13..=15).contains(&stage);
        if c == 1 {
            self.jingle = if stage == 10 && a.station == 7 {
                STAGE10_STATION7_JINGLE
            } else {
                self.tables.jingle
            };
            snd.request(self.jingle);
        }
        if c == DEPART_FIRST_CALL_AT {
            snd.request(if !(3..=16).contains(&stage) {
                if a.station == 0 {
                    DEPART_CALL_FIRST
                } else {
                    DEPART_CALL_OTHER
                }
            } else {
                DEPART_CALL_LOCAL
            });
        }
        if c == o + DEPART_EXTRA_AT && extra {
            snd.request(DEPART_EXTRA_JINGLE);
        }
        if c == o + DEPART_RED_AT {
            snd.release(self.jingle);
            snd.request(DEPART_SIGNAL);
        }
        if c == o + DEPART_EXTRA_END_AT && extra {
            snd.release(DEPART_EXTRA_JINGLE);
        }
        if c == o + DEPART_DOORS_AT {
            if a.ats > 0 {
                snd.release(ATS_BELL);
            }
            snd.release(DEPART_BELL);
        }
        if c >= o + DEPART_DONE_AT {
            if f64::from(a.speed) >= DEPART_MOVING_MPS {
                snd.request(DEPART_VOICES[usize::try_from(a.aspect).unwrap_or(0).min(4)]);
            } else {
                self.depart_voice_pending = true;
            }
        }
    }

    /// `Hud_Update` 0x40ff20: the passing countdown of the clock drawing and the remaining-time
    /// transfer clicks.
    fn hud(&mut self, snd: &mut SoundSystem, b: &Snap, a: &Snap) {
        if self.clock_freeze == 0 {
            let left = a.scheduled - a.clock + 30;
            if !a.target_stops && left > 0 && left / 30 < 6 && left % 30 == 0 {
                // Clock_Draw runs for the clock and for the scheduled time; the test reads
                // globals, so both calls request it.
                snd.request(COUNTDOWN);
                snd.request(COUNTDOWN);
            }
        } else {
            self.clock_freeze -= 1;
        }
        match a.remaining_s - b.remaining_s {
            1 => snd.request(TIME_UP_CLICK),
            -1 => snd.request(TIME_DOWN_CLICK),
            _ => {}
        }
    }
}

/// Events raised by the stopped-train handler 0x40a0a0 or after it (game over), which come
/// after everything 0x40c4e0 does in the same tick.
fn is_from_stop_handler(e: &RuleEvent) -> bool {
    matches!(
        e,
        RuleEvent::Time {
            reason: TimeReason::ForcedStop
                | TimeReason::ForcedStopFinal
                | TimeReason::StoppedBetweenStations
                | TimeReason::StoppedBetweenStationsAlt
                | TimeReason::StoppedInDeadSection
                | TimeReason::OnTimeStop
                | TimeReason::CleanStop,
            ..
        } | RuleEvent::Arrived(_)
            | RuleEvent::SignalsReset
            | RuleEvent::GameOver
    )
}

#[cfg(test)]
mod tests;

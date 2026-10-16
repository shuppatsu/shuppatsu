//! Game rules: signals, speed limits, ATS, the remaining-time ("持ち時間") system, station stop
//! evaluation and the final grade.
//!
//! This is a per-tick reimplementation of the rule half of the original driving loop
//! (`Stage_Run` @ 0x4080d0, loop at 0x4084f5). Physics (speed integration, braking curves) is out of
//! scope: the caller passes the train's speed, odometer and distance to the stop point every tick
//! and applies the outputs (forced stop, emergency brake). Behaviour, addresses and confidence
//! levels are documented in `docs/re/rules.md`; the section numbers below refer to it.
//!
//! All tunable numbers (penalty seconds, tolerances, thresholds, signal patterns) are read from
//! the user's `dengo3k.exe` by [`RuleTables::read`]. Only small code constants are hardcoded, each
//! with the address it comes from.
//!
//! Units: speeds in m/s (the game's `0x857234`), distances in metres, time in ticks (30 per game
//! second) or whole seconds for the remaining time.

use ddg_formats::FormatError;
use ddg_formats::exe::Exe;
use ddg_formats::stages::{CourseEvent, DIFFICULTY_COUNT, STAGE_COUNT, Stage, TimetableRow};

/// Errors raised while preparing the rules for a stage.
#[derive(Debug, thiserror::Error)]
pub enum RulesError {
    #[error("reading rule tables: {0}")]
    Format(#[from] FormatError),
    #[error("stage {0} out of range")]
    BadStage(usize),
    #[error("difficulty {0} out of range")]
    BadDifficulty(usize),
    #[error("station {station} out of range for a timetable of {rows} rows")]
    BadStation { station: usize, rows: usize },
}

/// Game ticks per game second (`% 0x1e` on the stage clock, 0x4084f5).
pub const TICKS_PER_SECOND: i32 = 30;
/// Clock advance for the station dwell: `clock += 0x2b2` (690 ticks = 23 s) at 0x40d340 and as
/// the start offset at 0x40d040 / 0x40d930.
const DWELL_TICKS: i32 = 0x2b2;
/// km/h → m/s factor, double at 0x459508.
const KMH_TO_MPS: f64 = 0.277_777_777_777_777_8;
/// m/s → km/h factor, double at 0x459420.
const MPS_TO_KMH: f64 = 3.6;
/// "No limit": 0x42a6aaab = 83.333336 m/s = 300 km/h (0x40cf40, 0x40b4a0 kind 6).
const NO_LIMIT_MPS: f32 = 83.333_336;
/// Signal speed for aspect 0 (green): 300.0 immediate in 0x40b970 / 0x40c010.
const GREEN_KMH: f32 = 300.0;
/// "Stopped" threshold on |speed|: double 0.0001 at 0x459480 (0x40a0a0).
const STOPPED_MPS: f32 = 0.0001;
/// Sighting distance of a signal = block length × 0.66666 (f32 at 0x459510, 0x40b970).
const SIGHT_FACTOR: f32 = 0.66666;
/// Overrun distance that trips the ATS: f32 −50 at 0x459540 (0x40dad0).
const ATS_OVERRUN_M: f32 = -50.0;
/// Approach state 2→3 when the distance drops below `f6 × 0.8` (f32 at 0x45952c, 0x40c4e0).
const APPROACH_FACTOR: f32 = 0.8;
/// Approach state 3→4 below −20 m (f32 at 0x459528, 0x40c4e0).
const APPROACH_OVERRUN_M: f32 = -20.0;
/// ATS confirmation window: 0x5a ticks (0x40c4e0).
const ATS_CONFIRM_TICKS: i32 = 0x5a;
/// EB penalty arms after the EB was held this many ticks above [`EB_MIN_MPS`] (0x28, 0x40c4e0).
const EB_HOLD_TICKS: i32 = 0x28;
/// Minimum speed for the EB checks: double 4.0 at 0x459530 (0x40c4e0, 0x422dec).
const EB_MIN_MPS: f32 = 4.0;
/// Signal overspeed allowance for the stage-5 hint: double 1.1 at 0x457710 (0x40b780).
const SIGNAL_HINT_MPS: f32 = 1.1;
/// Derailment: speed above `(limit + 3) × 0.27777777` (doubles at 0x459470 and 0x472600,
/// 0x422e51) zeroes the remaining time.
const DERAIL_SLACK_KMH: f64 = 3.0;
const DERAIL_FACTOR: f64 = 0.277_777_77;
/// Default ATC / derailment speed: 0x4479c000 = 999.0 km/h (0x40b970), i.e. none. Oracle:
/// `derail_kmh` reads 999 (hilly-drive.csv).
const DERAIL_NONE_KMH: f32 = 999.0;
/// Jolt / notch-slam cooldown `0x8571f8`: 100 at segment start (0x40cf40), +1 per driving tick up
/// to 200 (0x40cc04), a slam needs > 150 (0x42349e).
const COOLDOWN_START: i32 = 100;
const COOLDOWN_MAX: i32 = 200;
const COOLDOWN_READY: i32 = 150;
/// Notch slam: brake history delta of 5 notches (0x423494); B8 from below under 12 m/s
/// (double 12.0 at 0x469770).
const SLAM_DELTA: i32 = 5;
const SLAM_B8_MPS: f64 = 12.0;
/// 95 % of the top speed (double 0.95 at 0x4726a8, 0x423564).
const TOP_SPEED_SHARE: f64 = 0.95;
/// Departure task (0x413370): window and timings relative to the task counter.
const DEPART_FALSE_START_FROM: i32 = 0x20;
const DEPART_RELEASE_FROM: i32 = 0xa0;
const DEPART_CHECK_AT: i32 = 200;
const DEPART_AT: i32 = 0xc9;
const DEPART_FREEZE: i32 = 0x46;
const DEPART_HOLD_AT: i32 = 0xc7;
/// Departure signal (0x413370): red at offset + 100 for 100 ticks (pushes at 0x413612), then
/// the real aspect at offset + 0xa0 until 10 m travelled (0x413683).
const DEPART_SIGNAL_RED_AT: i32 = 100;
const DEPART_SIGNAL_RED_TICKS: i32 = 100;
const DEPART_SIGNAL_AT: i32 = 0xa0;
const DEPART_SIGNAL_DISTANCE_M: i32 = 10;
/// The task ends once the train moves at ≥ 0.04 m/s (double at 0x4594d8, 0x413723).
const DEPART_MOVING_MPS: f64 = 0.04;
/// Maximum remaining time: 0x3e7 (0x410104).
const MAX_REMAINING_S: i32 = 999;
/// Block array size: 0x856dc0..0x856fa8 holds 40 records of 12 bytes (0x40bfd0).
const MAX_BLOCKS: usize = 40;
/// Ticks at the end of the red-signal / ATS recovery in which 0x40a0a0 no longer zeroes the
/// speed: 10 `Task_Tick`s after the 60-tick standstill (0x40a548) plus the loop's own.
pub const RECOVERY_RELEASE_TICKS: u32 = 11;
/// Hold ticks left when the obstacle recovery shows its green signal: it precedes the final
/// 30 standstill `Task_Tick`s of 0x40a0a0's obstacle branch and the driving loop's own one.
const OBSTACLE_SIGNAL_AT: u32 = 31;
/// Extra standstill after stopping at the final station (0x40aaa5: 0x78 `Task_Tick`s).
const FINAL_STATION_HOLD: u32 = 0x78;
/// Sentinel signal position for "no signal" (0xfffffc19 = −999, 0x40b970).
const NO_SIGNAL: i32 = -999;

// --------------------------------------------------------------------------------------------
// Tables read from the exe
// --------------------------------------------------------------------------------------------

/// A `[stage][difficulty]` table of `i16` (20 × 4, indexed `(difficulty + stage*4)`), the shape
/// of every tuning table the rules use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffTable(pub [[i16; DIFFICULTY_COUNT]; STAGE_COUNT]);

impl DiffTable {
    fn read(exe: &Exe<'_>, va: u32) -> Result<Self, RulesError> {
        let r = exe.reader_at(va, 2 * DIFFICULTY_COUNT * STAGE_COUNT, "rule table")?;
        let mut t = [[0i16; DIFFICULTY_COUNT]; STAGE_COUNT];
        for (s, row) in t.iter_mut().enumerate() {
            for (d, v) in row.iter_mut().enumerate() {
                *v = r.i16(2 * (s * DIFFICULTY_COUNT + d))?;
            }
        }
        Ok(Self(t))
    }

    /// Value for a stage and difficulty, widened like the game's `movsx`.
    pub fn get(&self, stage: usize, difficulty: usize) -> i32 {
        i32::from(self.0[stage][difficulty])
    }
}

/// One entry of the scripted-signal list at 0x459234 (6 × i16, ends with stage −1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScriptedSignal {
    pub stage: i16,
    pub station: i16,
    /// 0 = fixed restrictive pattern (a train ahead), 1 = arm the obstacle-stop event.
    pub kind: i16,
    /// Pattern starts this many blocks before the first block.
    pub blocks_back: i16,
    /// Passed to the passing-train effect (0x857000, 0x8570cc).
    pub train: i16,
    pub train_distance: i16,
}

/// Every tuning table the rules read from the exe (§2 of `docs/re/rules.md`).
#[derive(Debug, Clone)]
pub struct RuleTables {
    /// 0x459870: km/h tolerance added to every speed limit and signal speed.
    pub limit_margin_kmh: DiffTable,
    /// 0x4597d0: stop-position tolerance in m (0x8570d0), both short and long.
    pub stop_tolerance_m: DiffTable,
    /// 0x4599b0: remaining time at the start of a run / after a continue, s.
    pub start_time_s: DiffTable,
    /// 0x45a310: seconds lost per second while behind the timetable (0x40dad0).
    pub late_drain_s: DiffTable,
    /// 0x45a3b0: speed-limit overspeed penalty, s (0x40b780).
    pub limit_overspeed_s: DiffTable,
    /// 0x45a450: emergency-brake penalty, s (0x40c4e0).
    pub emergency_brake_s: DiffTable,
    /// 0x45a590: signal overspeed minimum penalty, s, or −1 (0x40b780).
    pub signal_overspeed_min_s: DiffTable,
    /// 0x45a630: signal overspeed penalty cap, s, used when the minimum is −1 (0x40b780).
    pub signal_overspeed_cap_s: DiffTable,
    /// 0x45a6d0: power applied before departure was allowed, s (0x4080d0).
    pub false_start_s: DiffTable,
    /// 0x45a770: stopping between stations, s every 50 ticks after the grace period (0x40a0a0).
    pub stopped_between_s: DiffTable,
    /// 0x45a810: horn not sounded at a whistle sign, s (0x40acd0).
    pub horn_missed_s: DiffTable,
    /// 0x45a8b0: re-applying power during the station approach, s (0x413d90).
    pub repower_s: DiffTable,
    /// 0x459910 → 0x856fb8 (0x40d040): seconds per metre of overrun on the station result
    /// (task 0x413f30 at 0x4143fa multiplies the overrun by it; banner sprite 0x4654ec[factor]).
    pub overrun_factor: DiffTable,
    /// 0x45a950: horn bonus at full value, s (0x40acd0).
    pub horn_bonus_s: DiffTable,
    /// 0x45a9f0: bonus for a stop within 1 cm, s (station result task 0x413f30).
    pub exact_stop_bonus_s: DiffTable,
    /// 0x45aa90: bonus for a clean stop on time (< 3 s), s, plus the streak (0x40a0a0).
    pub on_time_stop_bonus_s: DiffTable,
    /// 0x45ab30: bonus for a clean stop that is not on time, s (0x40a0a0).
    pub clean_stop_bonus_s: DiffTable,
    /// 0x45abd0: bonus for passing a station within the scheduled second, s, plus streak.
    pub on_time_pass_bonus_s: DiffTable,
    /// 0x45ad10: bonus for coasting through a no-power section, s (0x40acd0 kind 11).
    pub coasting_bonus_s: DiffTable,
    /// 0x45db58: default advance notice for speed limits whose `c` field is 0, m (0x40b4a0).
    pub limit_notice_m: DiffTable,
    /// 0x459d20: remaining time needed before scripted weather starts, s.
    pub weather_min_time_s: DiffTable,
    /// Early-running thresholds in ticks (0x8570c4) that pick the signal pattern (0x40b970):
    /// 0x459eb0 (any restriction), 0x459f50, 0x459ff0, 0x45a090, 0x45a130, 0x45a1d0, 0x45a270.
    pub early_ticks: [DiffTable; 7],
    /// 0x459cd0: block (signal spacing) length per stage, m.
    pub block_length_m: [i16; STAGE_COUNT],
    /// 0x459cf8: first station where scripted weather may start (99 = never).
    pub weather_station: [i16; STAGE_COUNT],
    /// 0x459dc0: weather duration per stage, ticks.
    pub weather_ticks: [i32; STAGE_COUNT],
    /// 0x4656c4 (stride 0x20): stage kind; 1 = lesson stage, > 3 = special ending.
    pub stage_kind: [i32; STAGE_COUNT],
    /// 0x4591d0: signal aspect patterns, 5 aspects each, read from the first restricted block on.
    pub aspect_patterns: [[i16; 5]; 8],
    /// 0x459220 (other stages) and 0x45922a (stage 4): patterns for scripted signals.
    pub scripted_patterns: [[i16; 5]; 2],
    /// 0x459234: scripted signal entries.
    pub scripted_signals: Vec<ScriptedSignal>,
    /// Signal speeds above the margin for aspects 1, 2, 3 (f32 at 0x45951c, 0x459518, 0x459514).
    pub signal_speed_kmh: [f32; 3],
    /// 0x466c08: per stage `[three_lamp_set, repeater_set]` (i32 pairs, stride 8), the sprite
    /// sets `Signal_Show` 0x411500 uses for display variant 1 and for repeaters.
    pub signal_sets: [[i32; 2]; STAGE_COUNT],
    /// Navigator advice speeds per stage (via its train type), km/h: `[n * 2 + wet]` for
    /// `n` 0..=4 (`FUN_00422990`, 0x472278).
    pub advice_kmh: [[i32; 10]; STAGE_COUNT],
}

/// Addresses of the early-running threshold tables, in [`RuleTables::early_ticks`] order.
const EARLY_TABLES: [u32; 7] = [
    0x459eb0, 0x459f50, 0x459ff0, 0x45a090, 0x45a130, 0x45a1d0, 0x45a270,
];

impl RuleTables {
    /// Reads every rule table from the user's exe.
    pub fn read(exe: &Exe<'_>) -> Result<Self, RulesError> {
        let t = |va| DiffTable::read(exe, va);
        let early_ticks = [
            t(EARLY_TABLES[0])?,
            t(EARLY_TABLES[1])?,
            t(EARLY_TABLES[2])?,
            t(EARLY_TABLES[3])?,
            t(EARLY_TABLES[4])?,
            t(EARLY_TABLES[5])?,
            t(EARLY_TABLES[6])?,
        ];

        let per_stage_i16 = |va: u32| -> Result<[i16; STAGE_COUNT], RulesError> {
            let r = exe.reader_at(va, 2 * STAGE_COUNT, "rule table")?;
            let mut out = [0i16; STAGE_COUNT];
            for (i, v) in out.iter_mut().enumerate() {
                *v = r.i16(2 * i)?;
            }
            Ok(out)
        };
        let mut weather_ticks = [0i32; STAGE_COUNT];
        let r = exe.reader_at(0x459dc0, 4 * STAGE_COUNT, "rule table")?;
        for (i, v) in weather_ticks.iter_mut().enumerate() {
            *v = r.i32(4 * i)?;
        }
        let mut stage_kind = [0i32; STAGE_COUNT];
        let r = exe.reader_at(0x4656c4, 0x20 * STAGE_COUNT, "rule table")?;
        for (i, v) in stage_kind.iter_mut().enumerate() {
            *v = r.i32(0x20 * i)?;
        }
        let pattern = |va: u32| -> Result<[i16; 5], RulesError> {
            let r = exe.reader_at(va, 10, "rule table")?;
            let mut p = [0i16; 5];
            for (i, v) in p.iter_mut().enumerate() {
                *v = r.i16(2 * i)?;
            }
            Ok(p)
        };
        let mut aspect_patterns = [[0i16; 5]; 8];
        for (i, p) in aspect_patterns.iter_mut().enumerate() {
            *p = pattern(0x4591d0 + 10 * i as u32)?;
        }
        let scripted_patterns = [pattern(0x459220)?, pattern(0x45922a)?];
        let mut scripted_signals = Vec::new();
        for i in 0..64u32 {
            let r = exe.reader_at(0x459234 + 12 * i, 12, "rule table")?;
            let stage = r.i16(0)?;
            if stage == -1 {
                break;
            }
            scripted_signals.push(ScriptedSignal {
                stage,
                station: r.i16(2)?,
                kind: r.i16(4)?,
                blocks_back: r.i16(6)?,
                train: r.i16(8)?,
                train_distance: r.i16(10)?,
            });
        }
        let r = exe.reader_at(0x459514, 12, "rule table")?;
        let signal_speed_kmh = [r.f32(8)?, r.f32(4)?, r.f32(0)?];
        let mut signal_sets = [[0i32; 2]; STAGE_COUNT];
        let r = exe.reader_at(SIGNAL_SETS_VA, 8 * STAGE_COUNT, "rule table")?;
        for (i, v) in signal_sets.iter_mut().enumerate() {
            *v = [r.i32(8 * i)?, r.i32(8 * i + 4)?];
        }

        let mut advice_kmh = [[0i32; 10]; STAGE_COUNT];
        let types = exe.reader_at(STAGE_TRAIN_TYPE_VA, 4 * STAGE_COUNT, "rule table")?;
        for (stage, row) in advice_kmh.iter_mut().enumerate() {
            let ty = u32::try_from(types.i32(4 * stage)?).unwrap_or(0);
            let r = exe.reader_at(ADVICE_SPEEDS_VA + 40 * ty, 40, "rule table")?;
            for (i, v) in row.iter_mut().enumerate() {
                *v = r.i32(4 * i)?;
            }
        }

        Ok(Self {
            advice_kmh,
            limit_margin_kmh: t(0x459870)?,
            stop_tolerance_m: t(0x4597d0)?,
            start_time_s: t(0x4599b0)?,
            late_drain_s: t(0x45a310)?,
            limit_overspeed_s: t(0x45a3b0)?,
            emergency_brake_s: t(0x45a450)?,
            signal_overspeed_min_s: t(0x45a590)?,
            signal_overspeed_cap_s: t(0x45a630)?,
            false_start_s: t(0x45a6d0)?,
            stopped_between_s: t(0x45a770)?,
            horn_missed_s: t(0x45a810)?,
            repower_s: t(0x45a8b0)?,
            overrun_factor: t(0x459910)?,
            horn_bonus_s: t(0x45a950)?,
            exact_stop_bonus_s: t(0x45a9f0)?,
            on_time_stop_bonus_s: t(0x45aa90)?,
            clean_stop_bonus_s: t(0x45ab30)?,
            on_time_pass_bonus_s: t(0x45abd0)?,
            coasting_bonus_s: t(0x45ad10)?,
            limit_notice_m: t(0x45db58)?,
            weather_min_time_s: t(0x459d20)?,
            early_ticks,
            block_length_m: per_stage_i16(0x459cd0)?,
            weather_station: per_stage_i16(0x459cf8)?,
            weather_ticks,
            stage_kind,
            aspect_patterns,
            scripted_patterns,
            scripted_signals,
            signal_speed_kmh,
            signal_sets,
        })
    }
}

// --------------------------------------------------------------------------------------------
// Signal sprite (Signal_Show 0x411500 and its task 0x411940), docs/re/hud.md §3.9
// --------------------------------------------------------------------------------------------

/// Signal sprites, built from immediates on the stack in `Signal_Show` (0x411500) and indexed
/// `aspect + base`, see [`signal_sprite`]. Verified against the code listing.
pub const SIGNAL_SPRITES: [u16; 65] = [
    0x451, 0x454, 0x454, 0x454, 0x457, // home: variant 1 set 0 (3-lamp)
    0x47d, 0x47a, 0x477, 0x474, 0x471, // home: variant 0, or variant 1 set 1 (4-lamp)
    0x47f, 0x47a, 0x477, 0x474, 0x471, // home: variant 1 set 2
    0x452, 0x455, 0x455, 0x455, 0x458, // starting: variant 1 set 0
    0x47e, 0x47b, 0x478, 0x475, 0x472, // starting: variant 0 / set 1
    0x47f, 0x47b, 0x478, 0x475, 0x472, // starting: variant 1 set 2
    0x450, 0x453, 0x453, 0x453, 0x456, // block: variant 1 set 0
    0x47c, 0x479, 0x476, 0x473, 0x470, // block: variant 0 / set 1
    0x47f, 0x479, 0x476, 0x473, 0x470, // block: variant 1 set 2
    0x485, 0x484, 0x483, 0x482, 0x480, // repeater set 0
    0x485, 0x481, 0x481, 0x481, 0x480, // repeater set 1
    0x486, 0x484, 0x483, 0x482, 0x480, // repeater set 2
    0x486, 0x481, 0x481, 0x481, 0x480, // repeater set 3
];
/// Per-stage `{i32 three_lamp_set, i32 repeater_set}`, stride 8 (read by 0x411500).
const SIGNAL_SETS_VA: u32 = 0x466c08;
/// Bit of the aspect argument that asks for a forced redisplay (0x411500).
pub const SIGNAL_FORCED: i32 = 0x100;
/// A new signal task waits this many updates before it first draws (`+0x38 = 5`, 0x4118c7).
const SIGNAL_DELAY_TICKS: i32 = 5;
/// A forced redisplay keeps the old sprite this many more updates (`old+0x34 = 5`, 0x411863).
const SIGNAL_HANDOVER_TICKS: i32 = 5;
/// Showing a green releases the pending green-signal hint as `Hint_Show(id, 0, 220)` (0x411774).
const GREEN_HINT_DISTANCE_M: i32 = 0xdc;
/// ATS trip above the signal speed: `Hint_Show(5, 0x5a, 0)` when the speed exceeds the signal
/// limit + 0.3 m/s (double at 0x458af8), else `Hint_Show(-1)` (0x40c4e0).
const ATS_HINT_ID: i16 = 5;
const ATS_HINT_MARGIN_MPS: f32 = 0.3;
const ATS_HINT_TICKS: i32 = 0x5a;
/// Stage-5 hint id 6 "do not accelerate yet" inside a signal restriction (0x40b780): shown
/// until 2 m past the block's sighting point.
const SIGNAL_HINT_ID: i16 = 6;
const SIGNAL_HINT_EXTRA_M: i32 = 2;
/// Navigator advice (0x40c4e0 tail): only on stages of rank ≤ 3 (`0x4656c4`), late/early
/// advice for 0x5a ticks, pass advice for 0x96 m, stop advice for 0x78 m.
const ADVICE_MAX_RANK: i32 = 3;
const ADVICE_TICKS: i32 = 0x5a;
const ADVICE_PASS_DISTANCE_M: i32 = 0x96;
const ADVICE_STOP_DISTANCE_M: i32 = 0x78;
/// The pass advice compares the speed in m/s with the table km/h × 0.25 (double 0.25 at
/// 0x459520, used as f32).
const ADVICE_PASS_FACTOR: f32 = 0.25;
/// Advice speeds (`FUN_00422990`): i32 at 0x472278 + 4 × (2n + 10 × train type), the next i32
/// for wet rails (`0x857230 > 1`); train type per stage at 0x472228 (0x4229c0).
const ADVICE_SPEEDS_VA: u32 = 0x472278;
const STAGE_TRAIN_TYPE_VA: u32 = 0x472228;
/// Departure task hints on the lesson stage's first station (0x413370): `(offset, id, ticks,
/// distance)`; and the stage-1 bonus section's hint 0x10 at station 0x18.
const DEPART_LESSON_HINTS: [(i32, i16, i32, i32); 3] =
    [(0x14, 0, 0x78, 0), (0x8c, 9, 0x50, 0), (0xdc, 0x13, 0, 5)];
const DEPART_BONUS_HINT: (i32, i16, i32, i32) = (0x14, 0x10, 0xb4, 0);

/// The sprite `Signal_Show` (0x411500) picks for `aspect` 0..=4 (without the forced bit),
/// display `mode` (0 block, 1 home, 2 starting, 3 repeater), display variant `0x8570c2` and the
/// stage's `[three_lamp_set, repeater_set]` (0x466c08). `None` for an invalid aspect or mode,
/// where the original returns without showing anything.
pub fn signal_sprite(aspect: i32, mode: i32, variant: i16, sets: [i32; 2]) -> Option<u16> {
    let a = usize::try_from(aspect).ok().filter(|&a| a <= 4)?;
    let set = |s: i32| usize::try_from(s).unwrap_or(0) * 5;
    let base = match (variant & 1, mode) {
        (_, 3) => set(sets[1]) + 0x2d,
        (0, 0) => 0x23,
        (0, 1) => 5,
        (0, 2) => 0x14,
        (_, 0) => set(sets[0]) + 0x1e,
        (_, 1) => set(sets[0]),
        (_, 2) => set(sets[0]) + 0xf,
        _ => return None,
    };
    SIGNAL_SPRITES.get(a + base).copied()
}

/// One signal display task (0x411940).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SignalTask {
    /// Tilemap id (`+0x30`).
    sprite: u16,
    /// Tick counter (`+0x34`): the display lasts while it is ≥ 0 when no distance is set.
    ticks: i32,
    /// Updates left before the first draw (`+0x38`); −1 once drawing.
    delay: i32,
    /// Enabled (`+0x3c`); cleared to stop the task at its next update.
    enabled: bool,
    /// Odometer at which the display ends (`+0x48` = trunc(odometer) + distance), 0 = none.
    until_m: i32,
}

/// The signal sprite in HUD slot 0x60: `Signal_Show` 0x411500 creates a display task (0x411940)
/// and keeps a pointer to the newest one (`0x856d30`); every task draws the slot while it lives
/// and hides it when it ends (destructor 0x4119d0). Tasks update in creation order, after the
/// calls of the same tick. The rules need this for the "already shown" test, the HUD for the
/// sprite itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SignalHud {
    /// Live tasks in creation order.
    tasks: Vec<SignalTask>,
    /// Index of the newest task in `tasks` (`0x856d30`), until it ends.
    current: Option<usize>,
    /// Slot 0x60's tilemap after the last update; sticky between draws.
    slot: Option<u16>,
}

impl SignalHud {
    /// The task part of 0x411500 for a valid sprite: does nothing (`false`) if the newest task
    /// already shows `sprite`. Otherwise the newest task is stopped, or, when `forced`, kept for
    /// five more updates without its distance, and a new task is created. `start_m` is the
    /// truncated odometer at the call; `distance_m` 0 means "for `ticks` updates".
    pub fn show(
        &mut self,
        sprite: u16,
        forced: bool,
        ticks: i32,
        distance_m: i32,
        start_m: i32,
    ) -> bool {
        if let Some(cur) = self.current.and_then(|i| self.tasks.get_mut(i)) {
            if cur.sprite == sprite {
                return false;
            }
            if forced {
                cur.until_m = 0;
                cur.ticks = SIGNAL_HANDOVER_TICKS;
            } else {
                cur.enabled = false;
            }
        }
        self.tasks.push(SignalTask {
            sprite,
            ticks,
            delay: SIGNAL_DELAY_TICKS,
            enabled: true,
            until_m: if distance_m == 0 {
                0
            } else {
                start_m.wrapping_add(distance_m)
            },
        });
        self.current = Some(self.tasks.len() - 1);
        true
    }

    /// 0x411500 with a negative aspect: the newest task stops at its next update.
    pub fn hide(&mut self) {
        if let Some(cur) = self.current.and_then(|i| self.tasks.get_mut(i)) {
            cur.enabled = false;
        }
    }

    /// One update of every task (0x411940) with the odometer `0x857004` as it is then.
    pub fn tick(&mut self, odometer_m: f32) {
        let mut i = 0;
        while i < self.tasks.len() {
            let t = &mut self.tasks[i];
            let alive = if t.delay > 0 {
                t.delay -= 1;
                true
            } else {
                if t.delay == 0 {
                    t.delay = -1;
                } else {
                    t.ticks -= 1;
                }
                let showing = if t.until_m == 0 {
                    t.ticks >= 0
                } else {
                    // fild +0x48; fsub 0x857004; fcomp 0.0f (0x458b10) at 0x411990.
                    f64::from(t.until_m) - f64::from(odometer_m) >= 0.0
                };
                if showing && t.enabled {
                    self.slot = Some(t.sprite);
                }
                showing && t.enabled
            };
            if alive {
                i += 1;
                continue;
            }
            // Destructor 0x4119d0: hide the slot, forget the task if it is the newest.
            self.tasks.remove(i);
            self.slot = None;
            self.current = match self.current {
                Some(c) if c == i => None,
                Some(c) if c > i => Some(c - 1),
                c => c,
            };
        }
    }

    /// Removes every task and hides the slot.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Slot 0x60's tilemap after the last update, `None` = hidden.
    pub fn slot(&self) -> Option<u16> {
        self.slot
    }

    /// The sprite of the newest live task (`0x856d30 + 0x30`).
    pub fn current_sprite(&self) -> Option<u16> {
        self.current.map(|i| self.tasks[i].sprite)
    }
}

// --------------------------------------------------------------------------------------------
// Inputs, outputs, events
// --------------------------------------------------------------------------------------------

/// Buttons the rules read. The game keeps active-low words (`0x48c428` held, `0x48c42c` pressed
/// this tick); here `true` means the button is down / was pressed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Buttons {
    /// Horn held (`0x48c428` bit 0x100).
    pub horn_held: bool,
    /// Horn pressed this tick (`0x48c42c` bit 0x100).
    pub horn_pressed: bool,
    /// A / confirm pressed this tick (`0x48c42c` bit 0x100000): ATS acknowledgement.
    pub confirm_pressed: bool,
}

/// What the rules need from the train each tick. All values are the ones the original loop sees
/// before this tick's physics step (the physics task runs inside `Task_Tick`, after the rules).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TickInput {
    /// Train speed, m/s (`0x857234`).
    pub speed_mps: f32,
    /// Distance to the current target stop point, m (`0x85708c`; negative = overrun).
    pub dist_to_stop_m: f32,
    /// Distance to the stop point after the current target, m. Used only on the tick a passing
    /// station is passed (0x40dad0 re-targets the next stop in the same tick).
    pub dist_to_following_stop_m: f32,
    /// Odometer, m (`0x857004`), counting up along the route.
    pub odometer_m: f32,
    /// Power notch 0..=5 (`0x829d00`).
    pub power: i32,
    /// Brake notch 0..=10, ≥ 9 = emergency (`0x829d04`).
    pub brake: i32,
    pub buttons: Buttons,
    /// The point 4 ahead has track sound class 3 (tunnel), `0x857202 == 3` (0x407c10).
    pub in_tunnel: bool,
}

/// Why the remaining time changed. The discriminant is the message kind passed to `0x415080`,
/// which also decides the sign: kinds 1, 2, 3, 12, 13 add time, the others subtract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeReason {
    /// 0: stopped between stations.
    StoppedBetweenStations = 0,
    /// 1: clean stop on time.
    OnTimeStop = 1,
    /// 2: clean stop.
    CleanStop = 2,
    /// 3: horn bonus.
    Horn = 3,
    /// 4: emergency brake.
    EmergencyBrake = 4,
    /// 5: horn not sounded at a whistle sign.
    HornMissed = 5,
    /// 6: overspeed (speed limit or signal).
    Overspeed = 6,
    /// 7: overran the stop by more than 50 m (message only).
    Overrun = 7,
    /// 8: stopped inside a no-power section.
    StoppedInDeadSection = 8,
    /// 9: power before departure was allowed.
    FalseStart = 9,
    /// 10: ran past the forced-stop point (buffer stop).
    ForcedStop = 10,
    /// 12: coasted through a no-power section.
    Coasting = 12,
    /// 13: passed a station on time.
    OnTimePass = 13,
    /// 14: stopped between stations, alternate message.
    StoppedBetweenStationsAlt = 14,
    /// 15: forced stop at the final station variant (message only).
    ForcedStopFinal = 15,
}

impl TimeReason {
    /// Whether this kind adds time (sign marker 0x1ac in the table at 0x415080).
    pub fn is_bonus(self) -> bool {
        matches!(
            self,
            Self::OnTimeStop | Self::CleanStop | Self::Horn | Self::Coasting | Self::OnTimePass
        )
    }
}

/// Things the presentation layer and the trace care about.
#[derive(Debug, Clone, PartialEq)]
pub enum RuleEvent {
    /// A HUD time message; `seconds` is signed (0 = message without a number, no change).
    Time { reason: TimeReason, seconds: i32 },
    /// One second of lateness drained (0x40dad0), `seconds` negative.
    LateDrain { seconds: i32 },
    /// `Signal_Show` 0x411500 created a signal display task (HUD slot 0x60, hud.md §3.9).
    /// Raised only when the call was not a no-op; feed it to a [`SignalHud`] to draw it.
    SignalShown {
        /// 0 green … 4 stop (the forced bit 0x100 masked off).
        aspect: i16,
        /// The call had the 0x100 bit: the previous sprite stays 5 more ticks.
        forced: bool,
        /// 0 block, 1 home (場内), 2 starting (出発), 3 repeater (中継).
        display: i16,
        /// Display variant `0x8570c2` in effect for the call.
        variant: i16,
        /// Shown for this many ticks after the 5-tick delay when `distance_m` is 0.
        ticks: i32,
        /// Shown until the odometer passes `start_m + distance_m`, 0 = use `ticks`.
        distance_m: i32,
        /// Truncated odometer `0x857004` at the call.
        start_m: i32,
    },
    /// `Signal_Show(-1, …)`: the signal sprite stops at the next HUD update.
    SignalHidden,
    /// `Hint_Show(id, ticks, distance)` 0x411f20 (hud.md §3.12): a navigator hint. Raised by
    /// the green signal that releases the pending hint (`0x857224`), course event 9, the
    /// departure task (lesson stage), the ATS trip (id 5), the stage-5 signal hint (id 6), the
    /// late/early advice (ids 0x20/0x21) and the pass/stop advice (ids 0x1d/0x1e). Not raised
    /// while the ATS awaits confirmation or brakes (`0x857206 ≥ 2`), as 0x411f20 ignores calls
    /// then.
    HintShown {
        id: i16,
        /// Shown for this many ticks after the 5-tick delay when `distance_m` is 0.
        ticks: i32,
        /// Shown until the odometer passes `start_m + distance_m`, 0 = use `ticks`.
        distance_m: i32,
        /// Truncated odometer `0x857004` at the call.
        start_m: i32,
    },
    /// `Hint_Show(-1, …)`: the hint on screen ends at its next update.
    HintHidden,
    /// A speed limit sign: limit in km/h, notice distance and enforcement offset in m.
    SpeedLimit {
        kmh: i32,
        notice_m: i32,
        offset_m: i32,
    },
    /// End of a speed limit.
    SpeedLimitEnd,
    /// The ATS bell started.
    AtsBell,
    /// The ATS was acknowledged in time.
    AtsAcknowledged,
    /// The ATS applied the emergency brake.
    AtsEmergency,
    /// Signals reset after stopping at a red signal or after an ATS stop.
    SignalsReset,
    /// A whistle sign (horn required within `within_m`).
    HornSign { within_m: i32 },
    /// A passing station was passed (station index after the change).
    StationPassed { station: usize },
    /// The train stopped at the target station; the stop has been evaluated.
    Arrived(Arrival),
    /// Remaining time reached zero (or derailment).
    GameOver,
    /// Speed above the derailment limit.
    Derailed,
    /// The scripted obstacle-stop event started.
    ObstacleStop,
    /// Course events the rules don't act on (sounds, announcements, HUD, scenery), passed through.
    Script { kind: i16, args: [i16; 3] },
}

/// Result of a station stop, computed in the stop handler 0x40a0a0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Arrival {
    pub station: usize,
    /// Stop error, m (`0x85708c`, positive = short).
    pub dist_m: f32,
    /// `0x856fc0`: arrival second minus scheduled second.
    pub late_s: i32,
    /// 0 none, 1 clean stop, 2 clean stop on time (`0x856fe1[station]`).
    pub mark: u8,
    /// The forced-stop point was passed.
    pub forced: bool,
}

/// Outcome of [`Rules::finish_station`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StationOutcome {
    Continue,
    Cleared,
    GameOver,
}

/// Station result screen deductions (0x413d90 + task 0x413f30).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StationResult {
    /// Overrun beyond the tolerance, m (= seconds deducted), capped at 999.
    pub overrun_s: i32,
    /// Re-power during the approach, s.
    pub repower_s: i32,
    /// Emergency brake during the approach, s (10).
    pub eb_s: i32,
    /// Sum actually deducted (capped 999, or 20 on lesson stages).
    pub total_s: i32,
    /// Bonus for stopping within 1 cm, s.
    pub exact_bonus_s: i32,
    pub remaining_s: i32,
    pub outcome: StationOutcome,
}

/// What the train task (0x422c20) computed this tick, for the rule checks it contains (ride
/// comfort statistics). Pass it to [`Rules::end_tick`] after the physics step.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TrainReadout {
    /// Speed after this tick's physics, m/s (new `0x857234`).
    pub speed_after: f32,
    /// Speed lost over the last tick (`0x84c3f8`).
    pub decel_prev_tick: f32,
    /// Brake deceleration per tick (`0x84c420`).
    pub brake_decel: f32,
    /// Brake notch change over three ticks (`0x84c3ec`).
    pub brake_delta: i32,
    /// Brake notch the physics applied after its overrides (task+0x64 at the end of the tick,
    /// also `0x856cfa`).
    pub brake_applied: i32,
    /// Train spec +0x38: jolt limit.
    pub jolt_limit: f32,
    /// Train spec +0x14: top speed, km/h.
    pub top_speed_kmh: f32,
}

/// Per-tick output.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TickOutput {
    pub events: Vec<RuleEvent>,
    /// The rules set the speed to 0 this tick (`0x857234 = 0`).
    pub force_stop: bool,
    /// ATS emergency: the physics must apply B9 and P0 (`0x857206 >= 4`, 0x422e3b).
    pub emergency_brake: bool,
}

/// Where the rules are in the stage flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Doors / countdown before departure (`0x855ff8 & 0x800`).
    PreDeparture,
    /// The driving loop (`0x855ff8 & 0x200`).
    Driving,
    /// A blocking sequence of the original (`Task_Tick` loops in 0x40a0a0): speed held at 0,
    /// clock stopped, the HUD keeps draining pending time.
    Holding {
        ticks: u32,
        then: AfterHold,
    },
    /// Waiting for the pending time to drain (0x407ff0), then evaluating the stop.
    Draining,
    /// Stop evaluation of 0x40a0a0 after the drain: part 1 (stop error, count) and part 2
    /// (lateness, bonus), one `Task_Tick` apart.
    Evaluating(u8),
    /// Stopped at the station; call [`Rules::finish_station`].
    Arrived,
    GameOver,
    Cleared,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterHold {
    /// Red-signal / ATS recovery (0x40a0a0): `0x200` stays set, so the train task keeps
    /// driving; the speed is forced to 0 except in the last [`RECOVERY_RELEASE_TICKS`] ticks.
    Resume,
    /// Obstacle cleared (0x40a0a0, first branch): back to driving afterwards.
    ObstacleCleared,
    Drain,
    Arrived,
}

/// One signal block (12-byte record at 0x856dc0 + 12·i).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Block {
    /// Distance to the stop at which the signal comes into sight, m.
    pub sight_pos_m: i32,
    /// Distance from the sighting point to the signal, m.
    pub sight_m: i32,
    /// Aspect of the signal: 0 green, 1 YG, 2 Y, 3 YY, 4 red.
    pub aspect: i16,
}

/// The twelve statistic categories (`0x857020[12]` since the last continue,
/// `0x857050[12]` for the run), filled by `0x40dd60` and read by the final grade (0x40dfd0).
pub const STAT_COUNT: usize = 12;

/// Final grade components (0x40dfd0), each 0..=20.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grades {
    pub parts: [i32; 5],
    /// Sum (`0x856c88`).
    pub total: i32,
}

// --------------------------------------------------------------------------------------------
// State
// --------------------------------------------------------------------------------------------

/// Complete rule state for one run of one stage. Field comments give the original global.
#[derive(Debug, Clone)]
pub struct Rules {
    tables: RuleTables,
    stage: usize,
    difficulty: usize,
    timetable: Vec<TimetableRow>,
    course: Vec<CourseEvent>,
    limits: Vec<CourseEvent>,

    pub phase: Phase,
    /// Station index of the segment's start (`0x856d78`); the target is `station + 1`.
    pub station: usize,
    /// Game clock in ticks since midnight (`0x8570b0`).
    pub clock: i32,
    /// Scheduled arrival at the target (`0x8571e8`).
    pub scheduled: i32,
    /// Remaining time, s (`0x856fb0`).
    pub remaining_s: i32,
    /// Pending time change still to be counted into the remaining time (`0x856d7c`).
    pub pending_s: i32,
    /// HUD task tick counter (task +0x34 of the task created by 0x40f7e0).
    pub hud_counter: u32,
    /// Extra clock ticks added once per game second (`0x856fac`).
    pub clock_skew: i32,

    /// Working copies of the inputs (`0x857234`, `0x85708c`, `0x85722c`, `0x857004`).
    pub speed_mps: f32,
    pub dist_m: f32,
    pub dist_round_m: i32,
    pub odometer_m: f32,

    course_cursor: usize,
    limit_cursor: usize,

    /// Signal blocks (`0x856dc0`), current index (`0x857080`) and initial index (`0x85720c`).
    pub blocks: Vec<Block>,
    pub block: usize,
    pub block_count: usize,
    /// Aspect in force (`0x8570a6`).
    pub aspect: i16,
    /// Signal speed limit, m/s (`0x857090`).
    pub signal_limit_mps: f32,
    /// Signal enforcement: 0 armed, 1 enforced, 2 enforced + hint shown, 3 violated (`0x856fa0`).
    pub signal_state: i32,
    /// Odometer at which the signal limit starts (`0x856fcc`, 0 = none).
    pub signal_enforce_odo: i32,
    /// Restricted blocks left before the signals clear (`0x856d70`).
    pub restricted_blocks: i32,
    /// Repeater signal offset (`0x856fb4`, −1 none).
    pub repeater_m: i32,
    /// Signal display variant (`0x8570c2`).
    pub signal_variant: i16,
    /// The HUD signal tasks (`Signal_Show` 0x411500), for its "already shown" test.
    pub signal_hud: SignalHud,
    /// A `Task_Tick` ran since the last signal-task update (the update needs the odometer after
    /// the physics step, which the next tick's input carries).
    signal_hud_pending: bool,
    /// The stage loop was left (0x4085e0): report its `Signal_Show(-1)` with the next step.
    signal_hide_pending: bool,
    /// Pending green-signal hint (`0x857224`, i16, −1 none): course event 20, shown by the next
    /// green signal.
    pub green_hint: i16,
    /// The pass/stop advice was shown since the last signal set-up (`0x856fae`, cleared by
    /// 0x40b970).
    pub advice_shown: bool,
    /// Signal speeds per aspect, km/h (stack array in 0x40b970 / 0x40c010).
    speed_by_aspect: [f32; 5],

    /// Speed limit, m/s (`0x8570bc`).
    pub limit_mps: f32,
    /// 0 none/armed, 1 enforced, 2 violated (`0x856fc4`).
    pub limit_state: i32,
    /// Odometer at which the speed limit starts (`0x856da0`, 0 = none).
    pub limit_enforce_odo: i32,

    /// ATS: 0 idle, 1 bell, 2 awaiting confirmation, 4 emergency (`0x857206`).
    pub ats: i16,
    /// Ticks since the ATS tripped (`0x857240`).
    pub ats_ticks: i32,
    /// Clock at confirmation, chime every 32 ticks (`0x8570d4`, −1 none).
    pub ats_chime_from: i32,

    /// Horn sign: 1 required, 2 bonus zone, −1 none (`0x857208`); deadline odometer (`0x856d6a`).
    pub horn_state: i16,
    pub horn_deadline: i16,
    /// No-power section: 1 inside, 2 power used, 0 none (`0x856fba`).
    pub coasting: i16,

    /// Early running accumulator, ticks (`0x8570c4`).
    pub early_ticks: i32,
    /// Target station is a stop (`0x856d74`).
    pub target_stops: bool,
    /// The target is the final station (`0x8570ae`).
    pub final_station: bool,
    /// Station approach: 0, 1 latched, 2, 3 inside 80 % of f6, 4 overran (`0x856d9a`).
    pub approach: i16,
    /// Speed at the approach latch, km/h (`0x856d8c`).
    pub approach_kmh: f32,
    /// EB held ticks (`0x856d64`), EB applications (`0x8571e0`), EB ticks in the approach
    /// (`0x856da8`).
    pub eb_ticks: i32,
    pub eb_count: i32,
    pub eb_approach_ticks: i32,
    /// Re-power during the approach (`0x8571f6`) and its excuse (`0x857238`).
    pub repowered: i16,
    pub repower_excused: i16,
    /// Notches the train task saw last tick (task +0x60 / +0x64).
    pub prev_power: i32,
    pub prev_brake: i32,
    input_power: i32,
    input_brake: i32,
    /// Ticks stopped away from a station (`0x8571ec`).
    pub stopped_ticks: i32,
    /// Ran past the forced-stop point (`0x8570b4`).
    pub forced_stop: bool,
    /// Obstacle event: state (`0x856d80`), armed (`0x856d76`), used (`0x85721e`), stop
    /// odometer (`0x856dac`).
    pub obstacle: i16,
    pub obstacle_armed: bool,
    pub obstacle_used: bool,
    pub obstacle_odo: f32,
    /// Derailment speed, km/h (`0x84c424`).
    pub derail_kmh: f32,
    /// Weather: active (`0x84c3a0`), level (`0x857230`), ticks (`0x856fa8`), wet rails (`0x84c2e0`).
    pub weather_on: bool,
    pub weather_level: i32,
    pub weather_ticks: i32,
    pub wet_rails: bool,
    /// Notch-slam cooldown (`0x8571f8`).
    pub jolt_cooldown: i32,
    /// Departure task (0x413370): counter (task+0x34), offset (+0x38), false-start freeze
    /// (+0x50), brake-released ticks (+0x40); `departure_done` = `0x800` cleared.
    pub departure_counter: i32,
    pub departure_base: i32,
    pub departure_freeze: i32,
    pub departure_released: i32,
    pub departure_done: bool,

    /// Last arrival lateness, s (`0x856fc0`).
    pub late_s: i32,
    /// On-time stop streak (`0x8570c0`) and on-time pass streak (`0x857094`).
    pub stop_streak: i32,
    pub pass_streak: i32,
    /// Sum of stop errors, m (`0x856fa4`), and number of stops (`0x856fc8`).
    pub stop_error_sum: f32,
    pub stops: i32,
    /// Statistics since the last continue (`0x857020`) and for the run (`0x857050`).
    pub stats: [i32; STAT_COUNT],
    pub stats_total: [i32; STAT_COUNT],
    /// Continues used (`0x8570b8`).
    pub continues: i32,
    /// The signal blocks still have to be built for the segment (0x40b970 runs in the segment
    /// setup, after 0x40d040 / 0x40d340 / 0x40d930).
    signals_pending: bool,
    /// Per-station marks (`0x856fe1`).
    pub marks: Vec<u8>,
    /// Arrival records `(clock, distance)` (`0x8570e8`).
    pub arrivals: Vec<(i32, i32)>,
    /// Odometer at the final stop, for the ranking score (`0x8570c8`).
    pub final_odometer: f32,
}

fn round_half_away(v: f32) -> i32 {
    // 0x40dad0: fmod(|d|, 1) < 0.5 picks floor or ceil toward/away from zero.
    f64::from(v).round() as i32
}

fn trunc(v: f64) -> i32 {
    // MSVC __ftol (0x437b30) truncates toward zero.
    v as i32
}

impl Rules {
    /// Prepares a run (0x40d040 stage start + 0x40b970 first segment).
    pub fn new(
        tables: &RuleTables,
        stage_data: &Stage,
        stage: usize,
        difficulty: usize,
        start_station: usize,
    ) -> Result<Self, RulesError> {
        if stage >= STAGE_COUNT {
            return Err(RulesError::BadStage(stage));
        }
        if difficulty >= DIFFICULTY_COUNT {
            return Err(RulesError::BadDifficulty(difficulty));
        }
        let timetable = stage_data.timetables[difficulty].clone();
        if start_station + 1 >= timetable.len() {
            return Err(RulesError::BadStation {
                station: start_station,
                rows: timetable.len(),
            });
        }
        let stations = timetable.len();
        let mut r = Self {
            tables: tables.clone(),
            stage,
            difficulty,
            timetable,
            course: stage_data.course_events.clone(),
            limits: stage_data.limit_events.clone(),
            phase: Phase::PreDeparture,
            station: start_station,
            clock: 0,
            scheduled: 0,
            remaining_s: 0,
            pending_s: 0,
            hud_counter: 0,
            clock_skew: 0,
            speed_mps: 0.0,
            dist_m: 0.0,
            dist_round_m: 0,
            odometer_m: 0.0,
            course_cursor: 0,
            limit_cursor: 0,
            blocks: Vec::new(),
            block: 0,
            block_count: 0,
            aspect: 0,
            signal_limit_mps: NO_LIMIT_MPS,
            signal_state: 0,
            signal_enforce_odo: 0,
            restricted_blocks: 0,
            repeater_m: -1,
            signal_variant: 0,
            signal_hud: SignalHud::default(),
            signal_hud_pending: false,
            signal_hide_pending: false,
            green_hint: -1,
            advice_shown: false,
            speed_by_aspect: [GREEN_KMH, 0.0, 0.0, 0.0, 0.0],
            limit_mps: NO_LIMIT_MPS,
            limit_state: 0,
            limit_enforce_odo: 0,
            ats: 0,
            ats_ticks: 0,
            // 0x8570d4 is only set to −1 when leaving a wait / the loop (0x4080d0); before the
            // first departure it holds its initial 0 (oracle: hilly-drive.csv).
            ats_chime_from: 0,
            signals_pending: true,
            horn_state: -1,
            horn_deadline: 0,
            coasting: 0,
            early_ticks: 0,
            target_stops: false,
            final_station: false,
            approach: 0,
            approach_kmh: 0.0,
            eb_ticks: 0,
            eb_count: 0,
            eb_approach_ticks: 0,
            repowered: 0,
            repower_excused: 0,
            prev_power: 0,
            prev_brake: 0,
            input_power: 0,
            input_brake: 0,
            stopped_ticks: 0,
            forced_stop: false,
            obstacle: 0,
            obstacle_armed: false,
            obstacle_used: false,
            obstacle_odo: 0.0,
            derail_kmh: DERAIL_NONE_KMH,
            jolt_cooldown: COOLDOWN_START,
            departure_counter: 0,
            departure_base: 0,
            departure_freeze: 0,
            departure_released: 0,
            departure_done: false,
            weather_on: false,
            weather_level: 0,
            weather_ticks: 0,
            wet_rails: false,
            late_s: 0,
            stop_streak: 0,
            pass_streak: 0,
            stop_error_sum: 0.0,
            stops: 0,
            stats: [0; STAT_COUNT],
            stats_total: [0; STAT_COUNT],
            continues: 0,
            marks: vec![0; stations],
            arrivals: vec![(0, 999_999); stations],
            final_odometer: 0.0,
        };
        r.start_run();
        Ok(r)
    }

    fn table(&self, t: &DiffTable) -> i32 {
        t.get(self.stage, self.difficulty)
    }

    fn row(&self, i: usize) -> Option<&TimetableRow> {
        self.timetable.get(i)
    }

    fn target_row(&self) -> TimetableRow {
        // The game reads row d78+1 unconditionally; past the end it would hit the −1 terminator.
        self.row(self.station + 1).copied().unwrap_or(TimetableRow {
            time_ticks: -1,
            position_m: -1,
            to_next_m: -1,
            stops: -1,
            params: [-1; 5],
        })
    }

    fn is_final_next(&self) -> bool {
        // `row[d78+2].time == −1` (0x40d340 / 0x40d620).
        self.row(self.station + 2).is_none()
    }

    /// 0x40d040 (stage start) without the presentation parts.
    fn start_run(&mut self) {
        self.stats = [0; STAT_COUNT];
        self.stats_total = [0; STAT_COUNT];
        self.stops = 0;
        self.stop_error_sum = 0.0;
        self.stop_streak = 0;
        self.pass_streak = 0;
        self.early_ticks = 0;
        self.pending_s = 0;
        self.clock_skew = 0;
        self.weather_level = 0;
        self.weather_ticks = 0;
        self.weather_on = false;
        self.obstacle_used = false;
        self.signal_variant = 0;
        self.reset_segment_state();
        self.course_cursor = 0;
        self.limit_cursor = 0;
        self.seek_cursors();
        self.clock = self.timetable[self.station].time_ticks + DWELL_TICKS;
        self.scheduled = self.target_row().time_ticks;
        self.remaining_s = self.table(&self.tables.start_time_s);
        let start = self.timetable[self.station].position_m - self.timetable[0].position_m;
        self.odometer_m = start as f32;
        if self.tables.weather_station[self.stage] == 0 {
            self.weather_on = true;
            self.weather_level = 1;
        }
        self.final_station = self.is_final_next();
        // 0x40b970 runs in the segment setup (`segment_setup` or the first `step`).
        self.signals_pending = true;
    }

    fn seek_cursors(&mut self) {
        let st = self.station as i16;
        while self
            .course
            .get(self.course_cursor)
            .is_some_and(|e| e.station < st)
        {
            self.course_cursor += 1;
        }
        while self
            .limits
            .get(self.limit_cursor)
            .is_some_and(|e| e.station < st)
        {
            self.limit_cursor += 1;
        }
    }

    /// 0x40cf40: per-segment reset.
    fn reset_segment_state(&mut self) {
        self.obstacle = 0;
        self.green_hint = -1;
        self.horn_deadline = 0;
        self.horn_state = -1;
        self.ats = 0;
        self.ats_ticks = 0;
        self.repower_excused = 0;
        self.limit_state = 0;
        self.limit_mps = NO_LIMIT_MPS;
        self.signal_state = 0;
        self.signal_limit_mps = NO_LIMIT_MPS;
        self.aspect = 0;
        self.approach = 0;
        self.coasting = 0;
        self.repowered = 0;
        self.stopped_ticks = 0;
        self.eb_ticks = 0;
        self.eb_approach_ticks = 0;
        self.eb_count = 0;
        self.forced_stop = false;
        self.repeater_m = -1;
        self.obstacle_armed = false;
        self.late_s = 0;
        self.jolt_cooldown = COOLDOWN_START;
        self.reset_departure();
    }

    /// A new departure task (created per segment by 0x4124f0): the offset is 180 ticks at the
    /// first station of stages 0–2 and 17–19, else 0 (0x412fad).
    fn reset_departure(&mut self) {
        self.departure_counter = 0;
        self.departure_freeze = 0;
        self.departure_released = 0;
        self.departure_done = false;
        let long = (self.stage <= 2 || self.stage >= 17) && self.station == 0;
        self.departure_base = if long { 0xb4 } else { 0 };
    }

    /// One update of the departure task (0x413370), inside `Task_Tick` after the train task.
    fn departure_task(&mut self, out: &mut TickOutput, speed_after: f32, brake_applied: i32) {
        let c = self.departure_counter;
        let base = self.departure_base;
        if c > DEPART_FALSE_START_FROM
            && c < base + DEPART_CHECK_AT
            && self.input_power > 0
            && self.ats == 0
        {
            self.ats = 1;
            self.departure_freeze = DEPART_FREEZE;
            self.stat(8, 2);
        }
        // Navigator hints of the lesson stage's first station and the stage-1 bonus section.
        let lesson = self.stage_rank() == 1 && self.station == 0;
        for (at, id, ticks, distance) in DEPART_LESSON_HINTS {
            if lesson && c == base + at {
                self.hint(out, id, ticks, distance);
            }
        }
        let (at, id, ticks, distance) = DEPART_BONUS_HINT;
        if self.stage == 1 && self.station == 0x18 && c == base + at {
            self.hint(out, id, ticks, distance);
        }
        if c == base + DEPART_CHECK_AT && self.departure_released > 0 {
            // Brake released during the last 40 ticks of the countdown (0x4136fd).
            self.stat(2, 1);
        }
        if c == base + DEPART_SIGNAL_RED_AT {
            // 0x41361a: the starting signal shows red for 100 ticks.
            let variant = self.signal_variant;
            self.signal_show(out, 4, 2, DEPART_SIGNAL_RED_TICKS, 0, variant);
        }
        if c == base + DEPART_SIGNAL_AT {
            // 0x413696: then the aspect in force, forced, until 10 m travelled, looked up with
            // `0x8570c2` set to 0 for the call.
            let aspect = i32::from(self.aspect) | SIGNAL_FORCED;
            self.signal_show(out, aspect, 2, 0, DEPART_SIGNAL_DISTANCE_M, 0);
        }
        if c >= base + DEPART_AT {
            self.departure_done = true;
            if f64::from(speed_after) >= DEPART_MOVING_MPS {
                return;
            }
        }
        if self.departure_freeze > 0 {
            self.departure_freeze -= 1;
        }
        if !(self.departure_freeze > 0 && self.departure_counter == DEPART_HOLD_AT) {
            self.departure_counter += 1;
        }
        if self.departure_counter >= base + DEPART_RELEASE_FROM && brake_applied == 0 {
            self.departure_released += 1;
        }
    }

    // ----------------------------------------------------------------------------------------
    // Time
    // ----------------------------------------------------------------------------------------

    /// 0x415080 + 0x4122c0: show a time message and queue the change.
    fn time(&mut self, out: &mut TickOutput, reason: TimeReason, amount: i32) {
        let seconds = if reason.is_bonus() { amount } else { -amount };
        if amount != 0 {
            self.pending_s += seconds;
        }
        out.events.push(RuleEvent::Time { reason, seconds });
    }

    /// `Hint_Show(id, ticks, distance)` 0x411f20: the whole call is skipped while the ATS
    /// awaits confirmation or brakes (`0x857206 < 2` test); id −1 cancels the hint on screen.
    fn hint(&mut self, out: &mut TickOutput, id: i16, ticks: i32, distance_m: i32) {
        if self.ats >= 2 {
            return;
        }
        if id < 0 {
            out.events.push(RuleEvent::HintHidden);
            return;
        }
        out.events.push(RuleEvent::HintShown {
            id,
            ticks,
            distance_m,
            start_m: trunc(f64::from(self.odometer_m)),
        });
    }

    /// The navigator advice at the end of 0x40c4e0, run only in the driving loop body, after
    /// the 50 m leg marker of the train (`Train::leg_marker`, which fired when `leg_marker`
    /// is true): running late / early (ids 0x20 / 0x21), then once per signal set-up
    /// (`0x856fae`) "the next station is passed" (0x1d) or "is a stop" (0x1e) when the speed
    /// fits the distance left.
    pub fn advice_hints(&mut self, out: &mut TickOutput, leg_marker: bool) {
        if self.stage_rank() > ADVICE_MAX_RANK {
            return;
        }
        if leg_marker {
            if (self.late_s < -2 || self.early_ticks > 0xb3)
                && self.restricted_blocks == 0
                && self.aspect == 0
            {
                self.hint(out, 0x21, ADVICE_TICKS, 0);
            }
            if self.late_s > 3 {
                self.hint(out, 0x20, ADVICE_TICKS, 0);
            }
        }
        if self.advice_shown {
            return;
        }
        let d = self.dist_round_m;
        let speed = self.speed_mps;
        let wet = usize::from(self.weather_level > 1);
        let kmh = |n: usize| self.tables.advice_kmh[self.stage][n * 2 + wet];
        let mut shows = 0;
        if !self.target_stops {
            // Passing: slow enough, braking, green and under both limits (f32 maths).
            let ok = |n: usize| {
                let v = kmh(n) as f32 * ADVICE_PASS_FACTOR;
                speed <= v
                    && self.input_brake > 0
                    && self.aspect == 0
                    && v < self.signal_limit_mps
                    && v < self.limit_mps
            };
            if (300..=400).contains(&d) && ok(2) {
                shows += 1;
            }
            if (200..=300).contains(&d) && ok(1) {
                shows += 1;
            }
            for _ in 0..shows {
                self.hint(out, 0x1d, 0, ADVICE_PASS_DISTANCE_M);
            }
        } else {
            // Stopping: still too fast for the distance left.
            let fast = |n: usize| f64::from(speed) >= f64::from(kmh(n)) * KMH_TO_MPS;
            if (500..=600).contains(&d) && fast(4) {
                shows += 1;
            }
            if (300..=400).contains(&d) && fast(2) {
                shows += 1;
            }
            if (200..=300).contains(&d) && fast(1) {
                shows += 1;
            }
            for _ in 0..shows {
                self.hint(out, 0x1e, 0, ADVICE_STOP_DISTANCE_M);
            }
        }
        if shows > 0 {
            self.advice_shown = true;
        }
    }

    /// 0x40dd60: statistics, except on stage 2 station 24 (the bonus section).
    fn stat(&mut self, cat: usize, n: i32) {
        if self.stage == 2 && self.station == 24 {
            return;
        }
        self.stats[cat] += n;
        self.stats_total[cat] += n;
    }

    /// Adds to a statistic category from outside (ride-comfort counters that live in the
    /// physics task: categories 0, 3 and 11, see `docs/re/rules.md` §9).
    pub fn add_stat(&mut self, cat: usize, n: i32) {
        if cat < STAT_COUNT {
            self.stat(cat, n);
        }
    }

    /// HUD task 0x40ff20 (transfer part, 0x4100b2): one second of the pending change is moved
    /// into the remaining time every third HUD tick.
    fn hud_tick(&mut self) {
        if self.pending_s != 0 && self.hud_counter % 3 == 2 {
            if self.pending_s > 0 {
                self.remaining_s = (self.remaining_s + 1).min(MAX_REMAINING_S);
                self.pending_s -= 1;
            } else {
                self.remaining_s = (self.remaining_s - 1).max(0);
                self.pending_s += 1;
            }
        }
        self.hud_counter = self.hud_counter.wrapping_add(1);
    }

    /// The part of the player-train task (0x422c20, run inside `Task_Tick` after the rules)
    /// that feeds the rules: EB counters, re-power detection and derailment.
    fn train_task_tick(&mut self, out: &mut TickOutput, train: Option<&TrainReadout>) {
        let speed = self.speed_mps;
        let mut brake = self.input_brake;
        let mut power = self.input_power;
        if self.ats == 0 && self.obstacle == 0 {
            if brake != self.prev_brake && brake >= 9 && self.prev_brake < 9 {
                self.eb_count += 1;
            }
            if brake >= 9 && speed >= EB_MIN_MPS {
                self.eb_ticks += 1;
            } else {
                self.eb_ticks = 0;
            }
            if speed > 0.0 && brake >= 9 && self.approach > 0 && self.stage_active() {
                self.eb_approach_ticks += 1;
            }
        } else {
            self.eb_ticks = 0;
            if self.ats >= 4 {
                brake = 9;
                power = 0;
            }
        }
        let limit = f64::from(self.derail_kmh);
        if f64::from(speed) > (limit + DERAIL_SLACK_KMH) * DERAIL_FACTOR {
            self.remaining_s = 0;
            out.events.push(RuleEvent::Derailed);
        }
        if self.approach > 0 && self.repower_excused == 0 && self.repowered == 0 {
            let notched_up = self.prev_power < power && self.ats == 0;
            if notched_up || (self.approach >= 3 && power > 0) {
                self.stat(3, 5);
                self.repowered += 1;
            }
        }
        if self.ats >= 4 || self.obstacle > 0 {
            self.jolt_cooldown = 0;
        }
        if let Some(t) = train {
            self.comfort_checks(t, speed);
            brake = t.brake_applied;
        }
        self.prev_brake = brake;
        self.prev_power = power;
    }

    /// Ride-comfort statistics of the train task (0x4233a1..0x42358a, physics.md §7). `old` is
    /// the speed before this tick's physics step; task+0x64 still holds last tick's brake here.
    fn comfort_checks(&mut self, t: &TrainReadout, old: f32) {
        // Jolt: both decelerations at or above the train's jolt limit.
        if self.remaining_s > 0
            && self.ats <= 1
            && self.obstacle == 0
            && t.jolt_limit <= t.decel_prev_tick
            && t.jolt_limit <= t.brake_decel
        {
            self.stat(3, 1);
        }
        // Notch slams (the coast flags 0x410 are clear while driving).
        if old > 0.0 {
            if t.brake_delta.abs() >= SLAM_DELTA
                && self.jolt_cooldown > COOLDOWN_READY
                && self.prev_brake <= 8
            {
                self.jolt_cooldown = 0;
                self.stat(11, 2);
            }
            if t.brake_applied == 8
                && self.prev_brake < 8
                && f64::from(old) < SLAM_B8_MPS
                && self.jolt_cooldown > COOLDOWN_READY
            {
                self.jolt_cooldown = 0;
                self.stat(11, 2);
            }
        }
        // Accelerating through 95 % of the top speed.
        let threshold = f64::from(t.top_speed_kmh) * KMH_TO_MPS * TOP_SPEED_SHARE;
        if old < t.speed_after
            && f64::from(t.speed_after) >= threshold
            && f64::from(old) < threshold
        {
            self.stat(0, 2);
        }
    }

    fn stage_active(&self) -> bool {
        matches!(
            self.phase,
            Phase::Driving
                | Phase::PreDeparture
                | Phase::Holding { .. }
                | Phase::Draining
                | Phase::Evaluating(_)
        )
    }

    // ----------------------------------------------------------------------------------------
    // Signals
    // ----------------------------------------------------------------------------------------

    /// 0x40b970: builds the signal blocks for the segment and picks the aspects.
    fn build_blocks(&mut self) {
        self.signals_pending = false;
        self.advice_shown = false;
        let margin = self.table(&self.tables.limit_margin_kmh) as f32;
        let [s1, s2, s3] = self.tables.signal_speed_kmh;
        self.speed_by_aspect = [GREEN_KMH, margin + s1, margin + s2, margin + s3, 0.0];
        let prev = self.timetable[self.station];
        let next = self.target_row();
        if prev.stops != 0 {
            self.limit_mps = NO_LIMIT_MPS;
            self.limit_state = 0;
        }
        let len = i32::from(self.tables.block_length_m[self.stage]).max(1);
        let home = next.params[1];
        let mut n = ((prev.to_next_m - home) / len).max(0);
        let sight = trunc(f64::from(len as f32 * SIGHT_FACTOR));
        let top = (n + 2) as usize;
        let size = (top + 2).clamp(5, MAX_BLOCKS);
        let mut blocks = vec![Block::default(); size];
        let mut boundary = prev.to_next_m;
        if top > 3 {
            for j in (4..=top.min(size - 2)).rev() {
                boundary -= len;
                blocks[j] = Block {
                    sight_pos_m: sight + boundary,
                    sight_m: sight,
                    aspect: 0,
                };
            }
        }
        blocks[3] = Block {
            sight_pos_m: sight + home,
            sight_m: sight,
            aspect: 0,
        };
        if n == 0 {
            n = 1;
        }
        if blocks[3].sight_pos_m >= boundary {
            blocks[3].sight_pos_m = boundary - 20;
            blocks[3].sight_m = blocks[3].sight_pos_m - home;
        }
        blocks[2] = Block {
            sight_pos_m: home,
            sight_m: 50,
            aspect: 0,
        };
        blocks[1] = if next.stops == 0 {
            let p = trunc(f64::from(home as f32 * SIGHT_FACTOR));
            Block {
                sight_pos_m: p,
                sight_m: p,
                aspect: 0,
            }
        } else {
            Block {
                sight_pos_m: NO_SIGNAL,
                sight_m: 0,
                aspect: 0,
            }
        };
        blocks[0] = Block {
            sight_pos_m: NO_SIGNAL,
            sight_m: 0,
            aspect: 0,
        };
        let k = ((n + 2) as usize).min(size - 2);
        blocks[k + 1] = blocks[k];
        self.blocks = blocks;
        self.block = k;
        self.block_count = k;
        self.target_stops = next.stops != 0;

        // Hand-placed signals on stage 4, station 0 (immediates at 0x40b970).
        if self.stage == 4 && self.station == 0 && self.blocks.len() > 10 {
            self.blocks[10].sight_pos_m = 0xa00;
            self.blocks[10].sight_m = 0xa0;
            self.blocks[7].sight_pos_m = 0x68b;
            self.blocks[7].sight_m = 200;
        }
        self.derail_kmh = DERAIL_NONE_KMH;
        if (self.stage == 1 || self.stage == 2) && self.station == 24 {
            self.aspect = 0;
            self.block = 2;
            self.derail_kmh = 210.0;
            return;
        }
        if self.stage == 4 && self.station == 0 {
            self.early_ticks = 600;
        }
        if self.aspect == 0 {
            self.restricted_blocks = 0;
            self.pick_pattern(n, prev.stops == 1);
        } else {
            let mut a = (self.aspect + 1).min(4);
            for j in (1..=k).rev() {
                if j == 2 {
                    continue;
                }
                self.blocks[j].aspect = a;
                if a < 4 {
                    a += 1;
                }
                if a == 3 {
                    a = 4;
                }
            }
        }
        if self.stage == 4 && self.station == 0 {
            self.early_ticks = 0;
        }
    }

    fn early(&self, i: usize) -> i32 {
        self.table(&self.tables.early_ticks[i])
    }

    /// Pattern selection of 0x40b970: running early puts a "train ahead" in front of you.
    fn pick_pattern(&mut self, n: i32, left_a_stop: bool) {
        let c4 = self.early_ticks;
        if c4 < self.early(0) {
            return;
        }
        let k = self.block;
        if c4 >= self.early(2) {
            let found = self
                .tables
                .scripted_signals
                .iter()
                .find(|s| {
                    usize::try_from(s.stage) == Ok(self.stage)
                        && i32::from(s.station) == self.station as i32
                })
                .copied();
            if let Some(s) = found {
                if s.kind == 0 {
                    let (pat, count) = if self.stage == 4 {
                        (self.tables.scripted_patterns[1], 3)
                    } else {
                        (self.tables.scripted_patterns[0], 2)
                    };
                    self.restricted_blocks = count;
                    let start = k as i32 - i32::from(s.blocks_back);
                    self.paint(start, &pat, 0);
                    if self.stage == 6 && self.station == 11 && self.blocks.len() > 6 {
                        self.blocks[6].sight_pos_m = 0x5e6;
                        self.blocks[6].sight_m = 0xaa;
                    }
                    self.early_ticks = 0;
                    return;
                }
                if s.kind == 1 {
                    if !self.obstacle_used {
                        self.obstacle_armed = true;
                    }
                    self.early_ticks = 0;
                    return;
                }
            }
        }
        // (mode, pattern row, restricted blocks)
        let (mode, pat, count) = if left_a_stop {
            if c4 >= self.early(6) {
                (1, 6, 3)
            } else if c4 >= self.early(5) {
                (2, 5, 2)
            } else if c4 >= self.early(3) {
                (2, 3, 2)
            } else if c4 >= self.early(1) {
                (2, 1, 1)
            } else {
                (0, 0, 1)
            }
        } else if c4 >= self.early(6) {
            (0, 6, 3)
        } else if c4 >= self.early(4) {
            (0, 4, 2)
        } else if c4 >= self.early(2) {
            (0, 2, 2)
        } else {
            (0, 0, 1)
        };
        self.restricted_blocks = count;
        let start = match mode {
            0 if count < n => self.clock % (n - count) + 3 + count,
            0 => k as i32,
            1 => self.clock % n + 4,
            _ => k as i32 + 1,
        };
        let pattern = self.tables.aspect_patterns[pat];
        let mut first = 0;
        let mut j = start;
        if start > k as i32 {
            let a = pattern[0];
            self.aspect = a;
            self.signal_state = 0;
            // fcc = ftol(odometer + 10.0) (double at 0x458b88, 0x40bef2).
            self.signal_enforce_odo = trunc(f64::from(self.odometer_m) + 10.0);
            self.signal_limit_mps = self.aspect_speed_mps(a);
            j = start - 1;
            first = 1;
        }
        self.paint(j, &pattern, first);
    }

    /// Writes a pattern onto blocks `from` down to 1, skipping block 2 (the station's home
    /// signal); the pattern index stops advancing once it reaches a red.
    fn paint(&mut self, from: i32, pattern: &[i16; 5], mut p: usize) {
        let mut j = from;
        while j > 0 {
            let ju = j as usize;
            if ju != 2 && ju < self.blocks.len() {
                let a = pattern[p.min(4)];
                self.blocks[ju].aspect = a;
                if a < 4 {
                    p += 1;
                }
            }
            j -= 1;
        }
    }

    fn aspect_speed_mps(&self, aspect: i16) -> f32 {
        let kmh = self.speed_by_aspect[aspect.clamp(0, 4) as usize];
        (f64::from(kmh) * KMH_TO_MPS) as f32
    }

    /// 0x40bfd0: all signals green, no signal limit.
    fn clear_signals(&mut self) {
        for b in &mut self.blocks {
            b.aspect = 0;
        }
        self.aspect = 0;
        self.signal_limit_mps = NO_LIMIT_MPS;
        self.signal_state = 0;
        self.signal_enforce_odo = 0;
    }

    /// `Signal_Show(aspect, mode, ticks, distance)` 0x411500 with the display variant in
    /// effect: `aspect` may carry [`SIGNAL_FORCED`]; a negative aspect hides the sprite.
    /// Returns whether a new display task was created (the original's return value).
    fn signal_show(
        &mut self,
        out: &mut TickOutput,
        aspect: i32,
        mode: i16,
        ticks: i32,
        distance_m: i32,
        variant: i16,
    ) -> bool {
        if aspect < 0 {
            if self.signal_hud.current_sprite().is_some() {
                self.signal_hud.hide();
                out.events.push(RuleEvent::SignalHidden);
            }
            return false;
        }
        let forced = aspect & SIGNAL_FORCED != 0;
        let aspect = if forced { aspect & 0xff } else { aspect };
        if aspect > 4 {
            return false;
        }
        // The hint goes out before the "already shown" test (0x411760).
        if aspect == 0 && self.green_hint != -1 {
            self.hint(out, self.green_hint, 0, GREEN_HINT_DISTANCE_M);
            self.green_hint = -1;
        }
        let sets = self.tables.signal_sets[self.stage];
        let Some(sprite) = signal_sprite(aspect, i32::from(mode), variant, sets) else {
            return false;
        };
        let start_m = trunc(f64::from(self.odometer_m));
        if !self
            .signal_hud
            .show(sprite, forced, ticks, distance_m, start_m)
        {
            return false;
        }
        out.events.push(RuleEvent::SignalShown {
            aspect: aspect as i16,
            forced,
            display: mode,
            variant,
            ticks,
            distance_m,
            start_m,
        });
        true
    }

    /// One update of the signal display tasks for the last `Task_Tick`, with the odometer the
    /// physics left (0x411940 runs in list 3, after the train task).
    fn flush_signal_hud(&mut self, odometer_m: f32) {
        if self.signal_hud_pending {
            self.signal_hud_pending = false;
            self.signal_hud.tick(odometer_m);
        }
    }

    fn display_type(&self) -> i16 {
        if self.stage == 4 {
            match self.block {
                0 | 6 | 9 => 2,
                2 | 7 | 10 => 1,
                _ => 0,
            }
        } else {
            match self.block {
                0 => 2,
                2 => 1,
                _ => 0,
            }
        }
    }

    /// 0x40c010: block tracking, signal sighting.
    fn signal_step(&mut self, out: &mut TickOutput) {
        let old = self.block;
        if self.block > 0 && self.dist_m < self.blocks[self.block].sight_pos_m as f32 {
            self.block -= 1;
        }
        if self.dist_m <= 0.0 || self.obstacle != 0 || old == self.block {
            return;
        }
        let k = self.block;
        if k != 1 {
            if matches!(self.signal_state, 1 | 2) && self.aspect > 0 {
                if self.repeater_m != 0 {
                    self.restricted_blocks -= 1;
                }
                if self.restricted_blocks < 1 {
                    self.clear_signals();
                }
            }
            self.aspect = self.blocks.get(k + 1).map_or(0, |b| b.aspect);
        }
        let display = self.display_type();
        if self.repeater_m > 0 {
            if self.ats <= 3 {
                // Distance = the repeater offset `0x856fb4` (0x40c010).
                let (aspect, offset, variant) = (self.aspect, self.repeater_m, self.signal_variant);
                self.signal_show(out, i32::from(aspect), 3, 0, offset, variant);
                let delta = self.repeater_m;
                self.repeater_m = 0;
                if let Some(b) = self.blocks.get_mut(self.block + 1) {
                    self.block += 1;
                    b.sight_pos_m -= delta;
                    b.sight_m -= delta;
                }
            }
            self.stop_chime();
            return;
        }
        if self.ats > 3 || k == 1 {
            self.repeater_m = -1;
            return;
        }
        // Shown until the signal itself: distance = the sight distance of block k + 1.
        let sight = self.blocks.get(k + 1).map_or(0, |b| b.sight_m);
        let (aspect, variant) = (self.aspect, self.signal_variant);
        if !self.signal_show(out, i32::from(aspect), display, 0, sight, variant) {
            self.repeater_m = -1;
            return;
        }
        self.signal_state = 0;
        if self.aspect != 0 {
            self.signal_enforce_odo = trunc(f64::from(sight) + f64::from(self.odometer_m));
            self.signal_limit_mps = self.aspect_speed_mps(self.aspect);
        } else {
            self.signal_limit_mps = NO_LIMIT_MPS;
        }
        self.stop_chime();
        self.repeater_m = -1;
    }

    fn stop_chime(&mut self) {
        if self.ats_chime_from != -1 {
            self.ats_chime_from = -1;
        }
    }

    // ----------------------------------------------------------------------------------------
    // Per-tick rule functions, in the driving-loop order of 0x4084f5
    // ----------------------------------------------------------------------------------------

    /// 0x40dad0: lateness drain, distance, passing stations, overrun → ATS, approach latch.
    fn distance_step(&mut self, out: &mut TickOutput, input: &TickInput) {
        if self.clock > self.scheduled {
            let diff = (self.clock - self.scheduled).abs();
            if diff % TICKS_PER_SECOND == 0 {
                let s = self.table(&self.tables.late_drain_s);
                self.pending_s -= s;
                out.events.push(RuleEvent::LateDrain { seconds: -s });
            }
        }
        self.dist_m = input.dist_to_stop_m;
        if self.dist_m < 0.0 {
            if self.target_row().stops == 0 {
                self.pass_station(out);
                self.dist_m = input.dist_to_following_stop_m;
                self.build_blocks();
            } else if self.dist_m < ATS_OVERRUN_M {
                self.time(out, TimeReason::Overrun, 0);
                if self.ats == 0 {
                    self.ats = 1;
                }
            }
        }
        let f6 = self.target_row().params[2];
        if self.approach == 0 && self.dist_m < f6 as f32 && self.target_stops {
            self.approach_kmh = (f64::from(self.speed_mps) * MPS_TO_KMH) as f32;
            self.approach = 1;
        }
        self.dist_round_m = round_half_away(self.dist_m);
    }

    /// 0x40d620: passing a non-stop station.
    fn pass_station(&mut self, out: &mut TickOutput) {
        self.late_s = self.clock / TICKS_PER_SECOND - self.scheduled / TICKS_PER_SECOND;
        self.stat(1, self.late_s.abs());
        if let Some(a) = self.arrivals.get_mut(self.station) {
            *a = (self.clock, trunc(f64::from(self.dist_m)));
        }
        let diff = self.clock - self.scheduled;
        if (0..=29).contains(&diff) {
            let s = self.table(&self.tables.on_time_pass_bonus_s) + self.pass_streak;
            self.time(out, TimeReason::OnTimePass, s);
            self.pass_streak += 1;
            if let Some(m) = self.marks.get_mut(self.station) {
                *m = 1;
            }
        } else {
            self.pass_streak = 0;
        }
        if diff < 0 {
            self.early_ticks -= diff;
        } else {
            self.early_ticks = 0;
        }
        self.station += 1;
        self.scheduled = self.target_row().time_ticks;
        self.final_station = self.is_final_next();
        out.events.push(RuleEvent::StationPassed {
            station: self.station,
        });
    }

    /// 0x409fc0: forced stops (buffer stop past f8, obstacle).
    fn forced_stop_step(&mut self, out: &mut TickOutput) {
        if self.dist_round_m <= self.target_row().params[4] {
            self.speed_mps = 0.0;
            out.force_stop = true;
            self.forced_stop = true;
        }
        if self.obstacle >= 1 && self.obstacle_odo - self.odometer_m <= -1.0 {
            self.speed_mps = 0.0;
            out.force_stop = true;
        }
    }

    /// 0x40acd0: one course event per tick, plus horn and no-power-section bookkeeping.
    fn course_event_step(&mut self, out: &mut TickOutput, input: &TickInput) {
        if let Some(&e) = self.course.get(self.course_cursor)
            && i32::from(e.distance_m) >= self.dist_round_m
            && i32::from(e.station) == self.station as i32
        {
            let [a, b, c] = e.args;
            let mut advance = true;
            match e.kind {
                0 => {
                    let k = self.block;
                    let mut v = a;
                    if let Some(bk) = self.blocks.get_mut(k) {
                        bk.aspect = v;
                    }
                    for j in (1..k).rev() {
                        if j == 2 {
                            continue;
                        }
                        if v < 4 {
                            v += 1;
                        }
                        if v == 3 && b == 0 {
                            v = 4;
                        }
                        self.blocks[j].aspect = v;
                    }
                    self.restricted_blocks = i32::from(c);
                }
                3 => {
                    if a == 1 {
                        if self.obstacle < 1 {
                            self.horn_state = 1;
                            self.horn_deadline =
                                trunc(f64::from(b) + f64::from(self.odometer_m)) as i16;
                            out.events.push(RuleEvent::HornSign {
                                within_m: i32::from(b),
                            });
                        }
                    } else if a == 0 {
                        self.horn_state = 2;
                        self.horn_deadline =
                            trunc(f64::from(b) + f64::from(self.odometer_m)) as i16;
                    } else {
                        out.events.push(RuleEvent::HornSign {
                            within_m: i32::from(b),
                        });
                    }
                }
                4 | 5 => {
                    let idx = i32::from(a) / 5 - 4;
                    if !(0..=21).contains(&idx) {
                        advance = false;
                    } else if (e.kind == 4) == (self.limit_state == 0) {
                        self.limit_state = 0;
                        self.limit_enforce_odo = trunc(f64::from(c) + f64::from(self.odometer_m));
                        self.set_limit(i32::from(a));
                        out.events.push(RuleEvent::SpeedLimit {
                            kmh: i32::from(a),
                            notice_m: i32::from(b),
                            offset_m: i32::from(c),
                        });
                    }
                }
                6 => self.end_limit(out),
                7 => {
                    self.derail_kmh = f32::from(b);
                    out.events.push(RuleEvent::Script {
                        kind: e.kind,
                        args: e.args,
                    });
                }
                8 => {
                    if self.obstacle < 1 && self.obstacle_armed {
                        self.obstacle = 1;
                        self.obstacle_used = true;
                        // 0x856dac = a × 1.25 + odometer − 5 (doubles 0x459500, 0x4594f8).
                        self.obstacle_odo =
                            (f64::from(a) * 1.25 + f64::from(self.odometer_m) - 5.0) as f32;
                        out.events.push(RuleEvent::ObstacleStop);
                    }
                }
                10 => self.coasting = 1,
                11 => {
                    if self.coasting == 1 {
                        let s = self.table(&self.tables.coasting_bonus_s);
                        self.time(out, TimeReason::Coasting, s);
                    }
                    self.coasting = 0;
                }
                14 => self.repeater_m = i32::from(a),
                15 => self.signal_variant = a,
                16 => self.signal_variant = 0,
                21 => {
                    let min = self.table(&self.tables.weather_min_time_s);
                    if self.weather_level == 0 && min <= self.remaining_s && !self.weather_on {
                        self.weather_on = true;
                        self.weather_level = 1;
                        self.weather_ticks = 0;
                    }
                }
                22 => self.weather_on = false,
                20 => self.green_hint = a,
                9 => self.hint(out, a, i32::from(b), i32::from(c)),
                _ => out.events.push(RuleEvent::Script {
                    kind: e.kind,
                    args: e.args,
                }),
            }
            if !advance {
                return;
            }
            self.course_cursor += 1;
        }
        if self.coasting == 1 && input.power > 0 {
            self.coasting = 2;
        }
        if input.buttons.horn_held && self.horn_state == 1 {
            self.horn_state = -1;
        }
        if input.buttons.horn_pressed && self.horn_state == 2 {
            // Bonus scales with the distance left in the zone: min(1, left × 0.04)
            // (double 0.04 at 0x4594d8, 0x40b35e).
            let left = (f64::from(self.horn_deadline) - f64::from(self.odometer_m)) * 0.04;
            let f = if left < 1.0 { left } else { 1.0 };
            let s = trunc(f64::from(self.table(&self.tables.horn_bonus_s)) * f).max(1);
            self.time(out, TimeReason::Horn, s);
            self.horn_deadline = -1;
            self.horn_state = -1;
        }
        if f32::from(self.horn_deadline) <= self.odometer_m {
            if self.horn_state == 1 {
                let s = self.table(&self.tables.horn_missed_s);
                self.time(out, TimeReason::HornMissed, s);
                self.stat(10, 2);
            }
            self.horn_state = -1;
            self.horn_deadline = -1;
        }
    }

    fn set_limit(&mut self, kmh: i32) {
        let margin = self.table(&self.tables.limit_margin_kmh);
        self.limit_mps = (f64::from(margin + kmh) * KMH_TO_MPS) as f32;
    }

    fn end_limit(&mut self, out: &mut TickOutput) {
        self.limit_mps = NO_LIMIT_MPS;
        self.limit_state = 0;
        out.events.push(RuleEvent::SpeedLimitEnd);
    }

    /// 0x40b4a0: one speed-limit event per tick.
    fn limit_event_step(&mut self, out: &mut TickOutput) {
        let Some(&e) = self.limits.get(self.limit_cursor) else {
            return;
        };
        let [a, b, c] = e.args;
        let (offset, trigger, notice) = match c {
            -1 => (0, i32::from(e.distance_m), i32::from(b)),
            0 => {
                let d = self.table(&self.tables.limit_notice_m);
                (d, i32::from(e.distance_m) + d, i32::from(b) + d)
            }
            v => (i32::from(v), i32::from(e.distance_m), i32::from(b)),
        };
        if trigger < self.dist_round_m || i32::from(e.station) != self.station as i32 {
            return;
        }
        match e.kind {
            4 | 5 => {
                let idx = i32::from(a) / 5 - 4;
                if !(0..=21).contains(&idx) {
                    return;
                }
                if (e.kind == 4) == (self.limit_state == 0) {
                    self.limit_state = 0;
                    self.limit_enforce_odo = trunc(f64::from(offset) + f64::from(self.odometer_m));
                    self.set_limit(i32::from(a));
                    out.events.push(RuleEvent::SpeedLimit {
                        kmh: i32::from(a),
                        notice_m: notice,
                        offset_m: offset,
                    });
                }
            }
            6 => self.end_limit(out),
            _ => {}
        }
        self.limit_cursor += 1;
    }

    /// 0x40b780: speed limit and signal speed enforcement.
    fn enforcement_step(&mut self, out: &mut TickOutput) {
        let odo = self.odometer_m;
        let check_limit = if self.limit_enforce_odo == 0 || odo <= self.limit_enforce_odo as f32 {
            self.limit_state == 1
        } else {
            self.limit_enforce_odo = 0;
            self.limit_state = 1;
            true
        };
        if check_limit && self.limit_mps < self.speed_mps {
            let s = self.table(&self.tables.limit_overspeed_s);
            self.time(out, TimeReason::Overspeed, s);
            self.stat(5, 5);
            self.limit_state = 2;
        }

        if self.signal_enforce_odo != 0 && odo > self.signal_enforce_odo as f32 {
            self.signal_enforce_odo = 0;
            self.signal_state = 1;
        } else if !matches!(self.signal_state, 1 | 2) {
            return;
        }
        if self.speed_mps <= self.signal_limit_mps {
            if self.signal_limit_mps - SIGNAL_HINT_MPS < self.speed_mps && self.stage == 5 {
                // Until 2 m past the sighting point of the current block (block 1 for block 2).
                let k = if self.block == 2 { 1 } else { self.block };
                let pos = self.blocks.get(k).map_or(0, |b| b.sight_pos_m);
                let d = self.dist_round_m - pos + SIGNAL_HINT_EXTRA_M;
                self.hint(out, SIGNAL_HINT_ID, 0, d);
                self.signal_state = 2;
            }
            return;
        }
        let margin = f64::from(self.table(&self.tables.limit_margin_kmh)) * KMH_TO_MPS;
        let over = f64::from(self.speed_mps) - (f64::from(self.signal_limit_mps) - margin);
        let v = trunc(over * MPS_TO_KMH);
        let min = self.table(&self.tables.signal_overspeed_min_s);
        let s = if min == -1 {
            v.min(self.table(&self.tables.signal_overspeed_cap_s))
        } else {
            v.max(min)
        };
        self.time(out, TimeReason::Overspeed, s);
        self.stat(5, 5);
        self.signal_state = 3;
        self.ats = 1;
    }

    /// 0x40c4e0: approach state, EB penalty, ATS, weather.
    fn misc_step(&mut self, out: &mut TickOutput, input: &TickInput) {
        // EB penalty: the 1st..3rd application costs time only when held 40 ticks above
        // 4 m/s; every 4th application costs immediately.
        if self.eb_ticks == EB_HOLD_TICKS
            && self.eb_approach_ticks < 20
            && self.ats == 0
            && self.obstacle == 0
            && self.speed_mps >= EB_MIN_MPS
            && self.eb_count % 4 != 0
        {
            let s = self.table(&self.tables.emergency_brake_s);
            self.time(out, TimeReason::EmergencyBrake, s);
            self.stat(4, 6);
        }
        if self.eb_ticks == 1 && self.eb_count % 4 == 0 {
            let s = self.table(&self.tables.emergency_brake_s);
            self.time(out, TimeReason::EmergencyBrake, s);
            self.stat(4, 6);
        }
        if self.jolt_cooldown < COOLDOWN_MAX {
            self.jolt_cooldown += 1;
        }
        if self.obstacle & 0xf == 1 {
            // NaviAnim(0) 0x410cb0: the emergency-stop signal takes over slot 0x60 and hides the
            // signal sprite (`Signal_Show(-1, …)`).
            let variant = self.signal_variant;
            self.signal_show(out, -1, -1, -1, -1, variant);
            self.obstacle += 1;
        }

        if self.ats < 1 {
            self.ats_ticks = 0;
        } else {
            self.ats_ticks += 1;
        }
        if self.ats == 1 {
            out.events.push(RuleEvent::AtsBell);
            self.ats_chime_from = -1;
            if self.dist_m >= 0.0 {
                if self.speed_mps <= self.signal_limit_mps + ATS_HINT_MARGIN_MPS {
                    self.hint(out, -1, -1, -1);
                } else {
                    self.hint(out, ATS_HINT_ID, ATS_HINT_TICKS, 0);
                }
                self.stat(7, i32::from(self.aspect) * 2);
                self.ats = if self.aspect == 4 { 4 } else { 2 };
                if self.ats == 4 {
                    out.events.push(RuleEvent::AtsEmergency);
                }
            } else {
                self.stat(6, 10);
                self.ats = 4;
                out.events.push(RuleEvent::AtsEmergency);
            }
        } else if self.ats == 2 {
            if self.ats_ticks < ATS_CONFIRM_TICKS {
                if input.buttons.confirm_pressed && input.brake > 0 {
                    self.ats_chime_from = self.clock;
                    self.ats = 0;
                    out.events.push(RuleEvent::AtsAcknowledged);
                }
            } else {
                self.ats = 4;
                out.events.push(RuleEvent::AtsEmergency);
            }
        }

        // Weather (0x40c4e0 tail): level ramps 1..254 while on, down while off.
        if !self.weather_on {
            if self.weather_level > 0 {
                self.weather_level -= 1;
            }
            if self.weather_level == 1 {
                self.weather_level = -1;
            }
        } else {
            if self.weather_level > 0 && self.weather_level < 0xfe {
                self.weather_level += 1;
            }
            if self.weather_level > 199 {
                self.wet_rails = true;
            }
            if input.in_tunnel {
                self.wet_rails = false;
            }
        }
        if self.weather_level > 0 {
            self.weather_ticks += 1;
            if self.tables.weather_ticks[self.stage] <= self.weather_ticks {
                self.weather_on = false;
            }
        }

        // Approach state: 1 → 2, below 80 % of f6 → 3, past −20 m → 4.
        let f6 = self.target_row().params[2] as f32;
        if self.approach == 1 {
            self.approach = 2;
        }
        if self.approach == 2 && self.dist_m < f6 * APPROACH_FACTOR {
            self.approach = 3;
        }
        if self.approach == 3 && self.dist_m < APPROACH_OVERRUN_M && !self.final_station {
            self.approach = 4;
        }
    }

    /// 0x40a0a0: called every tick while time remains; acts only when the train is stopped.
    fn stopped_step(&mut self, out: &mut TickOutput) {
        if self.speed_mps.abs() > STOPPED_MPS {
            self.stopped_ticks = 0;
            return;
        }
        let tol = self.table(&self.tables.stop_tolerance_m);
        let short = tol < self.dist_round_m;
        if self.obstacle > 0 {
            // Obstacle cleared: short scripted sequence, then 30 ticks at standstill.
            self.ats = 0;
            self.ats_chime_from = -1;
            self.obstacle_armed = false;
            self.wet_rails = true;
            self.obstacle = 0;
            self.stopped_ticks = 0;
            self.eb_ticks = 0;
            self.phase = Phase::Holding {
                ticks: 36,
                then: AfterHold::ObstacleCleared,
            };
            return;
        }
        if (self.ats > 3 || self.aspect == 4)
            && ((short && self.target_stops) || !self.target_stops)
        {
            self.signal_stop_recovery(out);
            return;
        }
        if short || !self.target_stops {
            self.stopped_between(out);
            return;
        }
        self.arrive(out);
    }

    /// The red-signal / ATS branch of 0x40a0a0.
    fn signal_stop_recovery(&mut self, out: &mut TickOutput) {
        self.clear_signals();
        self.ats = 0;
        self.ats_chime_from = -1;
        out.events.push(RuleEvent::SignalsReset);
        // The first of the 60 standstill ticks (0x40a238: 0x857234 = 0 before each `Task_Tick`).
        self.speed_mps = 0.0;
        out.force_stop = true;
        self.phase = Phase::Holding {
            ticks: 60 + 10,
            then: AfterHold::Resume,
        };
    }

    /// The end of the red-signal / ATS recovery (0x40a45e, after the 60 standstill ticks): is a
    /// signal in sight ahead? Past the home signal the block index drops to 0; with no signal in
    /// sight, re-powering in the approach is excused. Then the HUD shows a green (forced) until
    /// that signal, or for 60 ticks (`Signal_Show(0x100, mode, ticks, distance)` at 0x40a540).
    fn recovery_signal(&mut self, out: &mut TickOutput) {
        let (mode, ticks, distance) = if self.repeater_m == 0 {
            // `0x856fb4` = 0 (a repeater was just shown): mode 3 for this distance (0x40a529).
            let pos0 = self.blocks.first().map_or(NO_SIGNAL, |b| b.sight_pos_m);
            (3, 0, self.dist_round_m - pos0 + 1)
        } else {
            let sight = self.blocks.get(self.block + 1).map_or(0, |b| b.sight_m);
            let mut found = None;
            let mut past_home = false;
            for i in (1..=self.block_count.min(self.blocks.len().saturating_sub(1))).rev() {
                if i == 2 {
                    continue;
                }
                if self.blocks[i].sight_pos_m == NO_SIGNAL {
                    if self.dist_m < self.blocks[2].sight_pos_m as f32 {
                        self.block = 0;
                        past_home = true;
                    }
                    break;
                }
                let ahead = self.blocks[i].sight_pos_m - self.dist_round_m;
                if ahead > 0 && ahead < sight {
                    found = Some((i, ahead));
                    break;
                }
            }
            let mode = if past_home || self.block == 0 {
                2
            } else if self.block <= 2 {
                1
            } else {
                0
            };
            match found {
                Some((i, ahead)) => (mode, 0, self.blocks[i].sight_m - ahead),
                None => {
                    if self.target_stops && self.approach > 0 {
                        self.repower_excused = 1;
                    }
                    // 0x3c ticks (0x40a513).
                    (mode, 0x3c, 0)
                }
            }
        };
        let variant = self.signal_variant;
        self.signal_show(out, SIGNAL_FORCED, mode, ticks, distance, variant);
    }
    /// The "stopped short / between stations" branch of 0x40a0a0.
    fn stopped_between(&mut self, out: &mut TickOutput) {
        self.stopped_ticks += 1;
        let t = self.stopped_ticks;
        if self.coasting == 1 {
            self.coasting = 0;
            self.time(out, TimeReason::StoppedInDeadSection, 10);
        }
        let kind = if self.stage == 1 && self.station == 24 {
            TimeReason::StoppedBetweenStationsAlt
        } else {
            TimeReason::StoppedBetweenStations
        };
        let penalty = self.table(&self.tables.stopped_between_s);
        if self.approach == 0 {
            if t > 149 && (t - 150) % 50 == 0 {
                let s = if t < 601 { 0 } else { penalty };
                self.time(out, kind, s);
            }
        } else if t > 29 && (t - 30) % 50 == 0 {
            if t < 401 {
                let r = if ((t - 30) / 50) % 2 != 0 {
                    kind
                } else {
                    TimeReason::StoppedBetweenStationsAlt
                };
                self.time(out, r, 0);
            } else {
                self.time(out, kind, penalty);
            }
        }
    }

    /// The arrival branch of 0x40a0a0 (part 1, before the pending time drains).
    fn arrive(&mut self, out: &mut TickOutput) {
        let mut hold = 0;
        if self.forced_stop {
            if self.final_station {
                self.time(out, TimeReason::ForcedStop, 0);
            } else {
                self.time(out, TimeReason::ForcedStop, 20);
            }
            hold = 80;
            // 0x856fa4 += 20.0 (double at 0x4594d0).
            self.stop_error_sum += 20.0;
        }
        self.phase = if hold > 0 {
            Phase::Holding {
                ticks: hold,
                then: AfterHold::Drain,
            }
        } else {
            Phase::Draining
        };
        if self.phase == Phase::Draining {
            self.drain_check();
        }
    }

    /// 0x407ff0: `while pending != 0 { Task_Tick }` then one more `Task_Tick`. The tick on which
    /// pending is found at 0 is that extra `Task_Tick`; the evaluation follows on the next one.
    fn drain_check(&mut self) {
        if self.pending_s == 0 {
            self.phase = Phase::Evaluating(1);
        }
    }

    /// The arrival branch of 0x40a0a0, part 1 (0x40a5e0..): stop error, stop count, ATS reset.
    /// Oracle: stop_count rises one row before last_lateness (stops-drive.csv rows 4138/4139).
    fn settle_stop(&mut self) {
        self.stop_error_sum += self.dist_m.abs();
        self.stops += 1;
        if self.ats > 0 {
            self.ats = 0;
        }
        self.phase = Phase::Evaluating(2);
    }

    /// The arrival branch of 0x40a0a0, part 2 (after one more `Task_Tick`): lateness, bonus.
    fn evaluate_stop(&mut self, out: &mut TickOutput) {
        self.late_s = self.clock / TICKS_PER_SECOND - self.scheduled / TICKS_PER_SECOND;
        let late = self.late_s.abs();
        self.stat(1, late);
        let diff = self.clock - self.scheduled;
        if diff < 0 && self.dist_round_m > -20 {
            self.early_ticks -= diff;
        } else {
            self.early_ticks = 0;
        }
        let clean = self.dist_round_m == 0
            && self.eb_approach_ticks == 0
            && self.eb_ticks == 0
            && self.repowered == self.repower_excused;
        let mut mark = 0;
        let hold = if clean {
            if late < 3 {
                let s = self.table(&self.tables.on_time_stop_bonus_s) + self.stop_streak;
                self.time(out, TimeReason::OnTimeStop, s);
                self.stop_streak += 1;
                mark = 2;
            } else {
                let s = self.table(&self.tables.clean_stop_bonus_s);
                self.time(out, TimeReason::CleanStop, s);
                mark = 1;
            }
            100
        } else {
            self.stop_streak = 0;
            30
        };
        // 0x40aaa5: 120 more ticks at the final station (stage-specific endings of stages 1, 2,
        // 10 and 17–19 are not modelled).
        let hold = if self.final_station {
            hold + FINAL_STATION_HOLD
        } else {
            hold
        };
        if let Some(m) = self.marks.get_mut(self.station) {
            *m = mark;
        }
        if self.tables.stage_kind[self.stage] == 1 {
            self.eb_approach_ticks = 0;
        }
        if let Some(a) = self.arrivals.get_mut(self.station) {
            *a = (self.clock, trunc(f64::from(self.dist_m)));
        }
        out.events.push(RuleEvent::Arrived(Arrival {
            station: self.station + 1,
            dist_m: self.dist_m,
            late_s: self.late_s,
            mark,
            forced: self.forced_stop,
        }));
        self.phase = Phase::Holding {
            ticks: hold,
            then: AfterHold::Arrived,
        };
    }

    // ----------------------------------------------------------------------------------------
    // Public driving API
    // ----------------------------------------------------------------------------------------

    /// One tick before departure (the `0x800` wait in 0x4080d0): the clock runs, the departure
    /// task counts down (false start, brake release) and ends the wait. Same as [`Rules::tick`].
    pub fn pre_departure_tick(&mut self, input: &TickInput) -> TickOutput {
        self.tick(input)
    }

    /// The pre-departure loop body (0x4084xx): clock, false-start penalty.
    fn pre_departure_step(&mut self, out: &mut TickOutput) {
        self.clock += 1;
        if self.ats == 1 {
            self.ats = 2;
            let s = self.table(&self.tables.false_start_s);
            self.time(out, TimeReason::FalseStart, s);
        }
    }

    /// Indices of the next course event and limit event (`0x857088`, `0x8571fc` as list indices).
    pub fn event_cursors(&self) -> (usize, usize) {
        (self.course_cursor, self.limit_cursor)
    }

    /// Moves the event cursors (to resume a run mid-way, e.g. from an oracle trace).
    pub fn set_event_cursors(&mut self, course: usize, limits: usize) {
        self.course_cursor = course.min(self.course.len());
        self.limit_cursor = limits.min(self.limits.len());
    }

    /// Rebuilds the signal blocks of the current segment from the current clock, early
    /// accumulator and aspect (0x40b970), as done at segment start.
    pub fn rebuild_signals(&mut self) {
        self.build_blocks();
    }

    /// Power applied while the doors were closing (task at 0x4133c5).
    pub fn false_start(&mut self) {
        if self.ats == 0 {
            self.ats = 1;
            self.stat(8, 2);
        }
    }

    fn latch_input(&mut self, input: &TickInput) {
        self.flush_signal_hud(input.odometer_m);
        self.input_power = input.power;
        self.input_brake = input.brake;
        self.speed_mps = input.speed_mps;
        self.odometer_m = input.odometer_m;
    }

    /// The segment setup in 0x4080d0 before its `Task_Tick`: the priming call of 0x40dad0
    /// (distance to the stop and its rounding, approach latch) with the notches that tick uses.
    /// Run the physics afterwards, then [`Rules::end_tick`], which runs the segment's new
    /// departure task for the first time (counter 0 → 1).
    pub fn segment_setup(&mut self, input: &TickInput) -> TickOutput {
        let mut out = TickOutput::default();
        self.latch_input(input);
        if self.signals_pending {
            self.build_blocks();
        }
        self.distance_step(&mut out, input);
        out
    }

    /// A tick outside the stage loops (after the route load, on the result and continue screens
    /// and between them): only the train task's rule part (EB counters, re-power, derailment,
    /// comfort statistics) and the HUD time transfer run. The departure task does not exist yet.
    pub fn idle_tick(&mut self, input: &TickInput, train: Option<&TrainReadout>) -> TickOutput {
        let mut out = TickOutput::default();
        self.latch_input(input);
        self.train_task_tick(&mut out, train);
        self.hud_tick();
        self.signal_hud_pending = true;
        out
    }

    /// The timetable of the run (one row per station, without the terminator).
    pub fn timetable(&self) -> &[TimetableRow] {
        &self.timetable
    }

    /// Stage index and difficulty of the run.
    pub fn stage(&self) -> (usize, usize) {
        (self.stage, self.difficulty)
    }

    /// The stage's rank / kind (i32 at 0x4656c4 + stage × 0x20; also read by the 50 m leg
    /// marker of 0x40c4e0 and the clock skew).
    pub fn stage_rank(&self) -> i32 {
        self.tables.stage_kind[self.stage]
    }

    /// Whether the driving flag `0x855ff8 & 0x200` is set, i.e. the train task runs its driving
    /// branch: while driving and during the red-signal / ATS recovery (0x40a0a0 keeps the flag;
    /// the arrival branch clears it).
    pub fn train_driving(&self) -> bool {
        matches!(
            self.phase,
            Phase::Driving
                | Phase::Holding {
                    then: AfterHold::Resume,
                    ..
                }
        )
    }

    /// 0x4085e0, when the stage loop is left after an arrival or a game over: the ATS and its
    /// chime are reset (`0x857206 = 0`, `0x8570d4 = −1`).
    pub fn leave_driving_loop(&mut self) {
        self.ats = 0;
        self.ats_chime_from = -1;
        // 0x4158b0 (game over) and 0x4085e0 both call `Signal_Show(-1, …)`; the event goes out
        // with the next step.
        let odometer = self.odometer_m;
        self.flush_signal_hud(odometer);
        if self.signal_hud.current_sprite().is_some() {
            self.signal_hud.hide();
            self.signal_hide_pending = true;
        }
    }

    /// Departure: leave the pre-departure wait (0x4080d0 after the `0x800` loop).
    pub fn depart(&mut self) {
        self.ats = 0;
        self.ats_chime_from = -1;
        self.phase = Phase::Driving;
    }

    /// One iteration of the driving loop.
    /// One tick: [`Rules::step`] then [`Rules::end_tick`] without a physics readout (the
    /// ride-comfort statistics are then skipped).
    pub fn tick(&mut self, input: &TickInput) -> TickOutput {
        let mut out = self.step(input);
        self.end_tick(&mut out, None);
        out
    }

    /// The rule part of one tick (the loop body of 0x4080d0 before `Task_Tick`). Run the
    /// physics afterwards, then [`Rules::end_tick`].
    pub fn step(&mut self, input: &TickInput) -> TickOutput {
        let mut out = TickOutput::default();
        // Callers that skip `segment_setup` get the blocks on the first tick of the segment.
        if self.signals_pending && matches!(self.phase, Phase::PreDeparture | Phase::Driving) {
            self.build_blocks();
        }
        if self.phase == Phase::PreDeparture && self.departure_done {
            // The 0x800 wait ended in the last `Task_Tick`: 0x857206 = 0, 0x8570d4 = −1.
            self.depart();
        }
        self.latch_input(input);
        if std::mem::take(&mut self.signal_hide_pending) {
            out.events.push(RuleEvent::SignalHidden);
        }
        match self.phase {
            Phase::Driving => self.drive(&mut out, input),
            Phase::Holding { ticks, then } => {
                if then == AfterHold::Resume && ticks == RECOVERY_RELEASE_TICKS {
                    self.recovery_signal(&mut out);
                }
                if then == AfterHold::ObstacleCleared && ticks == OBSTACLE_SIGNAL_AT {
                    // 0x40a0a0, obstacle branch: `Signal_Show(0, 0, 0, 5)`, a green block signal
                    // for 5 m, before the last 30 standstill ticks.
                    let variant = self.signal_variant;
                    self.signal_show(&mut out, 0, 0, 0, 5, variant);
                }
                // 0x40a0a0 zeroes 0x857234 before each `Task_Tick` of its wait loops, except in
                // the recovery's last 10 ticks and the loop's own `Task_Tick` after it.
                if !(then == AfterHold::Resume && ticks <= RECOVERY_RELEASE_TICKS) {
                    self.speed_mps = 0.0;
                    out.force_stop = true;
                }
                if ticks <= 1 {
                    if then == AfterHold::Resume && self.coasting == 1 {
                        // 0x40a555: leaving a no-power section is forgotten.
                        self.coasting = 0;
                    }
                    self.phase = match then {
                        AfterHold::Resume | AfterHold::ObstacleCleared => Phase::Driving,
                        AfterHold::Drain => Phase::Draining,
                        AfterHold::Arrived => Phase::Arrived,
                    };
                } else {
                    self.phase = Phase::Holding {
                        ticks: ticks - 1,
                        then,
                    };
                }
            }
            Phase::Draining => {
                self.speed_mps = 0.0;
                out.force_stop = true;
                self.drain_check();
            }
            Phase::Evaluating(part) => {
                self.speed_mps = 0.0;
                out.force_stop = true;
                if part == 1 {
                    self.settle_stop();
                } else {
                    self.evaluate_stop(&mut out);
                }
            }
            Phase::PreDeparture => self.pre_departure_step(&mut out),
            Phase::Arrived | Phase::GameOver | Phase::Cleared => {}
        }
        out.emergency_brake = self.ats >= 4;
        out
    }

    /// The rule parts of `Task_Tick` after the physics step: the train task's counters and
    /// comfort checks, the departure task, the HUD time transfer.
    pub fn end_tick(&mut self, out: &mut TickOutput, train: Option<&TrainReadout>) {
        self.train_task_tick(out, train);
        if self.phase == Phase::PreDeparture && !self.departure_done {
            let speed = train.map_or(self.speed_mps, |t| t.speed_after);
            let brake = train.map_or(self.input_brake, |t| t.brake_applied);
            self.departure_task(out, speed, brake);
        }
        self.hud_tick();
        self.signal_hud_pending = true;
    }

    fn drive(&mut self, out: &mut TickOutput, input: &TickInput) {
        self.distance_step(out, input);
        self.forced_stop_step(out);
        self.course_event_step(out, input);
        self.limit_event_step(out);
        self.enforcement_step(out);
        self.signal_step(out);
        self.misc_step(out, input);
        if self.obstacle == 0 {
            self.clock += 1;
        }
        if self.clock % TICKS_PER_SECOND == 2 {
            self.clock += self.clock_skew;
        }
        if self.remaining_s > 0 {
            self.stopped_step(out);
        } else {
            self.game_over(out);
        }
    }

    fn game_over(&mut self, out: &mut TickOutput) {
        // 0x40859d: the HUD keeps draining for 20 more ticks; 0x4085d0 then discards the rest of
        // the pending change (oracle: pending still −1 on the game-over row, hilly-drive.csv).
        self.phase = Phase::GameOver;
        out.events.push(RuleEvent::GameOver);
    }

    /// Station result and the move to the next segment (0x413d90, task 0x413f30, 0x40d340,
    /// 0x40b970). Call once in [`Phase::Arrived`]; the remaining pending time is counted in
    /// first (the HUD keeps draining it during the result screen).
    pub fn finish_station(&mut self) -> StationResult {
        self.remaining_s = (self.remaining_s + self.pending_s).clamp(0, MAX_REMAINING_S);
        self.pending_s = 0;
        let tol = self.table(&self.tables.stop_tolerance_m);
        let overrun_s = if self.dist_round_m < -tol {
            let v = -self.dist_round_m;
            let m = if self.dist_round_m != -999 && v > 998 {
                999
            } else {
                v
            };
            // 0x4143fa: the metres are multiplied by 0x856fb8 (1–3) on the result screen.
            m * self.table(&self.tables.overrun_factor)
        } else {
            0
        };
        let repower_s = if self.repowered > 0 {
            self.table(&self.tables.repower_s)
        } else {
            0
        };
        let lesson = self.tables.stage_kind[self.stage] == 1;
        let mut eb_s = 0;
        if !lesson && self.eb_approach_ticks > 0 {
            eb_s = 10;
            self.stat(4, 2);
        }
        let mut total = (overrun_s + repower_s + eb_s).min(MAX_REMAINING_S);
        if lesson {
            total = total.min(20);
        }
        self.remaining_s -= total;
        // Stop within 1 cm (|d| × 100 truncated, double 100.0 at 0x458b38) on a marked station.
        let cm = trunc(f64::from(self.dist_m.abs()) * 100.0);
        let mut exact_bonus_s = 0;
        if cm == 0 && self.marks.get(self.station).copied().unwrap_or(0) > 0 {
            exact_bonus_s = self.table(&self.tables.exact_stop_bonus_s);
            self.remaining_s += exact_bonus_s;
        }
        self.remaining_s = self.remaining_s.max(0);
        self.final_odometer = self.odometer_m + self.dist_m;

        // 0x4080d0: clock skew from station 4 on, every other station.
        if self.tables.stage_kind[self.stage] > 0
            && self.station > 3
            && (self.station - 4).is_multiple_of(2)
            && self.clock_skew < 1
        {
            self.clock_skew += 1;
        }
        let outcome = self.next_segment();
        StationResult {
            overrun_s,
            repower_s,
            eb_s,
            total_s: total,
            exact_bonus_s,
            remaining_s: self.remaining_s,
            outcome,
        }
    }

    /// 0x40d340 + segment setup.
    fn next_segment(&mut self) -> StationOutcome {
        let was_final = self.final_station;
        self.station += 1;
        if was_final {
            self.phase = Phase::Cleared;
            return StationOutcome::Cleared;
        }
        // Out of time: 0x40d340 still updates the target, resets the segment state and adds
        // the dwell (oracle: stops-drive.csv row 4381, clock +690 with the game over).
        let game_over = self.remaining_s < 1;
        self.scheduled = self.target_row().time_ticks;
        self.final_station = self.is_final_next();
        self.reset_segment_state();
        self.clock += DWELL_TICKS;
        // 0x40d340 may also start scripted weather here when the global frame counter is a
        // multiple of 4; that depends on non-rule state and is left to the caller.
        self.signals_pending = true;
        if game_over {
            self.phase = Phase::GameOver;
            return StationOutcome::GameOver;
        }
        self.phase = Phase::PreDeparture;
        StationOutcome::Continue
    }

    /// Continue after a game over (0x40d930): back to the last stopping station with a fresh
    /// remaining time.
    pub fn continue_run(&mut self) {
        if self.approach > 0 {
            // 0x4080d0 (continue branch): out of time inside the station approach
            // (0x856d9a ≥ 1). The run first moves on to the target station as if it had been
            // reached (0x856fb0 = 1, 0x40d340), then 0x40d930 rewinds from there. Oracle:
            // yamate-drive.csv continues at station 2 after timing out 11 m short of it.
            self.remaining_s = 1;
            self.next_segment();
        }
        self.station = self.rewind_station(self.station);
        self.stats = [0; STAT_COUNT];
        self.early_ticks = 0;
        self.pending_s = 0;
        self.clock = self.timetable[self.station].time_ticks + DWELL_TICKS;
        self.scheduled = self.target_row().time_ticks;
        self.reset_segment_state();
        self.stop_streak = 0;
        self.pass_streak = 0;
        self.remaining_s = self.table(&self.tables.start_time_s);
        let start = self.timetable[self.station].position_m - self.timetable[0].position_m;
        self.odometer_m = start as f32;
        self.course_cursor = 0;
        self.limit_cursor = 0;
        self.seek_cursors();
        self.continues += 1;
        self.clock_skew = 0;
        self.final_station = self.is_final_next();
        self.signal_hud.clear();
        self.signal_hud_pending = false;
        self.signal_hide_pending = false;
        if self.stage == 0 {
            self.weather_level = 0;
            self.weather_ticks = 0;
            self.weather_on = false;
        }
        self.signals_pending = true;
        self.phase = Phase::PreDeparture;
    }

    /// 0x40d930: the last stopping station at or before `station`.
    fn rewind_station(&self, mut station: usize) -> usize {
        let stops = |i: usize| self.timetable.get(i).is_some_and(|t| t.stops == 1);
        while !stops(station) && station != 0 {
            station -= 1;
        }
        station
    }

    /// The station [`Rules::continue_run`] would restart from (to load its route in advance).
    pub fn continue_station(&self) -> usize {
        let from = if self.approach > 0 {
            self.station + 1
        } else {
            self.station
        };
        self.rewind_station(from)
    }

    // ----------------------------------------------------------------------------------------
    // Scoring
    // ----------------------------------------------------------------------------------------

    /// Final grade (0x40dfd0): five parts worth up to 20 each, scaled by how far the run got.
    pub fn grades(&self) -> Grades {
        let t = &self.stats_total;
        let c = self.continues;
        let stations = self.timetable.len() as f64;
        // Progress scale (doubles 18/16/21 at 0x459568, 0x458be0, 0x458c58; f32 0.125).
        let progress = if self.stage == 1 && self.station <= 8 {
            (self.station + 1) as f64 * 0.125 * 18.0
        } else if self.stage == 10
            && (self.station <= 7 || (self.station == 8 && self.final_station))
        {
            self.station as f64 * 0.125 * 16.0
        } else {
            (self.station + 1) as f64 / stations * 21.0
        };
        // − continues / 3 (double 0.3333333 at 0x459560), clamped to 1..=20.
        let m = (progress - f64::from(c) * 0.333_333_3).clamp(1.0, 20.0);
        let div = |x: i32| -> i32 {
            if c > 0 {
                // / (continues + 0.2) (double at 0x458b30)
                trunc(f64::from(x) / (f64::from(c) + 0.2))
            } else {
                x
            }
        };
        let part = |bad: i32, cap: i32, scale: f64| -> i32 {
            trunc(f64::from(cap - bad.min(cap)) * scale * m)
        };
        let p0 = if self.stops == 0 {
            0
        } else {
            part(div((t[1] - 1).max(0)), 50, 0.02)
        };
        let p1 = part(div(t[3] / 8 + t[11] + t[4]), 50, 0.02);
        let p2 = if self.stops == 0 {
            0
        } else {
            part(div(trunc(f64::from(self.stop_error_sum))), 30, 1.0 / 30.0)
        };
        let p3 = part(div(t[7] + t[5] + t[6] + t[9]), 20, 0.05);
        let p4 = part(
            div(t[11] / 2 + t[3] / 16 + t[8] + t[2] + t[0]),
            30,
            1.0 / 30.0,
        );
        let parts = [p0, p1, p2, p3, p4];
        Grades {
            parts,
            total: parts.iter().sum(),
        }
    }

    /// Ranking value (0x4080d0 → 0x41e8d0): `(remaining×1000 [if cleared] + grade×1000 /
    /// (continues+1) + odometer) × 0.01`, capped at 9999.
    pub fn ranking_score(&self, cleared: bool) -> i32 {
        // Computed on the x87 stack, stored as f32 at 0x8570c8, then × 0.01 (double at
        // 0x469750) and capped at 0x270f.
        let g = f64::from(self.grades().total);
        let mut score = g * 1000.0 / f64::from(self.continues + 1) + f64::from(self.final_odometer);
        if cleared {
            score += f64::from(self.remaining_s) * 1000.0;
        }
        trunc(f64::from(score as f32) * 0.01).min(9999)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ddg_formats::stages::StageSegment;

    fn diff(v: i16) -> DiffTable {
        DiffTable([[v; DIFFICULTY_COUNT]; STAGE_COUNT])
    }

    fn tables() -> RuleTables {
        RuleTables {
            limit_margin_kmh: diff(5),
            stop_tolerance_m: diff(2),
            start_time_s: diff(30),
            late_drain_s: diff(1),
            limit_overspeed_s: diff(10),
            emergency_brake_s: diff(5),
            signal_overspeed_min_s: diff(-1),
            signal_overspeed_cap_s: diff(15),
            false_start_s: diff(2),
            stopped_between_s: diff(5),
            horn_missed_s: diff(5),
            repower_s: diff(10),
            overrun_factor: diff(1),
            horn_bonus_s: diff(3),
            exact_stop_bonus_s: diff(10),
            on_time_stop_bonus_s: diff(5),
            clean_stop_bonus_s: diff(3),
            on_time_pass_bonus_s: diff(5),
            coasting_bonus_s: diff(5),
            limit_notice_m: diff(300),
            weather_min_time_s: diff(0),
            early_ticks: [
                diff(240),
                diff(330),
                diff(390),
                diff(450),
                diff(630),
                diff(690),
                diff(750),
            ],
            block_length_m: [330; STAGE_COUNT],
            weather_station: [99; STAGE_COUNT],
            weather_ticks: [9000; STAGE_COUNT],
            stage_kind: [3; STAGE_COUNT],
            aspect_patterns: [
                [1, 2, 4, 4, 4],
                [3, 4, 4, 4, 4],
                [1, 1, 2, 4, 4],
                [3, 2, 4, 4, 4],
                [1, 2, 4, 4, 4],
                [3, 2, 4, 4, 4],
                [1, 2, 4, 4, 4],
                [1, 2, 3, 4, 4],
            ],
            scripted_patterns: [[1, 4, 4, 4, 4], [1, 2, 4, 4, 4]],
            scripted_signals: Vec::new(),
            signal_speed_kmh: [70.0, 45.0, 25.0],
            signal_sets: [[0, 1]; STAGE_COUNT],
            advice_kmh: [[0; 10]; STAGE_COUNT],
        }
    }

    fn row(time_ticks: i32, position_m: i32, to_next_m: i32, stops: i32) -> TimetableRow {
        TimetableRow {
            time_ticks,
            position_m,
            to_next_m,
            stops,
            params: [0, 300, 150, 0, -999],
        }
    }

    /// Two stations 2000 m apart, scheduled 120 s apart.
    fn stage(course: Vec<CourseEvent>, limits: Vec<CourseEvent>) -> Stage {
        let tt = vec![
            row(1_000_000, 0, 2000, 1),
            row(1_000_000 + 0x2b2 + 3600, 2000, 0, 1),
        ];
        Stage {
            area: 1,
            train_sets: Vec::new(),
            segments: vec![StageSegment {
                com: 1,
                first_station: 0,
            }],
            track_sounds: [0; 32],
            timetables: [tt.clone(), tt.clone(), tt.clone(), tt],
            station_names: vec!["A".into(), "B".into()],
            course_events: course,
            limit_events: limits,
            traffic_events: Vec::new(),
        }
    }

    fn ev(station: i16, distance_m: i16, kind: i16, args: [i16; 3]) -> CourseEvent {
        CourseEvent {
            station,
            distance_m,
            kind,
            args,
        }
    }

    fn rules(course: Vec<CourseEvent>, limits: Vec<CourseEvent>) -> Rules {
        let mut r = Rules::new(&tables(), &stage(course, limits), 3, 1, 0).unwrap();
        r.rebuild_signals();
        r.depart();
        r
    }

    fn input(speed: f32, dist: f32, odo: f32) -> TickInput {
        TickInput {
            speed_mps: speed,
            dist_to_stop_m: dist,
            dist_to_following_stop_m: dist,
            odometer_m: odo,
            ..TickInput::default()
        }
    }

    fn time_events(out: &TickOutput) -> Vec<(TimeReason, i32)> {
        out.events
            .iter()
            .filter_map(|e| match e {
                RuleEvent::Time { reason, seconds } => Some((*reason, *seconds)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn pending_time_moves_one_second_every_third_tick() {
        let mut r = rules(Vec::new(), Vec::new());
        r.remaining_s = 5;
        r.pending_s = -7;
        let mut seen = Vec::new();
        for _ in 0..30 {
            r.hud_tick();
            seen.push(r.remaining_s);
        }
        assert_eq!(&seen[..9], &[5, 5, 4, 4, 4, 3, 3, 3, 2]);
        assert_eq!(r.remaining_s, 0, "clamped at zero");
        assert_eq!(r.pending_s, 0, "the rest is consumed even at zero");
    }

    #[test]
    fn speed_limit_is_enforced_after_its_offset_and_penalised_once() {
        // 40 km/h limit at 1500 m, enforced 100 m after the notice.
        let mut r = rules(Vec::new(), vec![ev(0, 1500, 4, [40, 0, 100])]);
        let fast = 60.0 / 3.6;
        let out = r.tick(&input(fast, 1600.0, 400.0));
        assert!(
            !out.events
                .iter()
                .any(|e| matches!(e, RuleEvent::SpeedLimit { .. }))
        );
        let out = r.tick(&input(fast, 1500.0, 500.0));
        assert!(out.events.contains(&RuleEvent::SpeedLimit {
            kmh: 40,
            notice_m: 0,
            offset_m: 100
        }));
        assert!(
            (r.limit_mps - 45.0 / 3.6).abs() < 1e-4,
            "limit includes the margin"
        );
        let out = r.tick(&input(fast, 1450.0, 550.0));
        assert!(
            time_events(&out).is_empty(),
            "not enforced before the offset"
        );
        let out = r.tick(&input(fast, 1390.0, 610.0));
        assert_eq!(time_events(&out), vec![(TimeReason::Overspeed, -10)]);
        assert_eq!(r.limit_state, 2);
        let out = r.tick(&input(fast, 1380.0, 620.0));
        assert!(time_events(&out).is_empty(), "one penalty per limit");
        assert_eq!(r.stats_total[5], 5);
    }

    #[test]
    fn red_signal_trips_the_ats_and_brakes_without_acknowledgement() {
        let mut r = rules(Vec::new(), Vec::new());
        // Force a restrictive aspect for the next block and run into it.
        let k = r.block;
        r.blocks[k].aspect = 4;
        r.tick(&input(10.0, (r.blocks[k].sight_pos_m + 1) as f32, 0.0));
        let out = r.tick(&input(10.0, (r.blocks[k].sight_pos_m - 1) as f32, 1.0));
        assert!(
            out.events
                .iter()
                .any(|e| matches!(e, RuleEvent::SignalShown { aspect: 4, .. }))
        );
        assert_eq!(r.signal_limit_mps, 0.0);
        // Reach the signal: enforcement begins, any speed is overspeed.
        let past = r.signal_enforce_odo as f32 + 1.0;
        let out = r.tick(&input(10.0, (r.blocks[k].sight_pos_m - 2) as f32, past));
        assert!(matches!(time_events(&out)[..], [(TimeReason::Overspeed, s)] if s < 0));
        // The bell is handled by 0x40c4e0 later in the same tick: red → immediate EB.
        assert_eq!(r.ats, 4);
        assert!(out.events.contains(&RuleEvent::AtsEmergency));
        assert!(out.emergency_brake);
    }

    #[test]
    fn ats_is_cleared_by_confirm_with_brake() {
        let mut r = rules(Vec::new(), Vec::new());
        r.aspect = 2;
        r.ats = 1;
        r.tick(&input(10.0, 1500.0, 0.0));
        assert_eq!(r.ats, 2);
        let mut i = input(10.0, 1490.0, 1.0);
        i.brake = 3;
        i.buttons.confirm_pressed = true;
        let out = r.tick(&i);
        assert!(out.events.contains(&RuleEvent::AtsAcknowledged));
        assert_eq!(r.ats, 0);
    }

    #[test]
    fn ats_brakes_after_the_window() {
        let mut r = rules(Vec::new(), Vec::new());
        r.aspect = 2;
        r.ats = 1;
        for t in 0..ATS_CONFIRM_TICKS {
            r.tick(&input(10.0, 1500.0 - t as f32, t as f32));
        }
        assert_eq!(r.ats, 4);
    }

    #[test]
    fn lateness_drains_one_second_per_second() {
        let mut r = rules(Vec::new(), Vec::new());
        r.clock = r.scheduled + 29;
        let out = r.tick(&input(10.0, 1500.0, 0.0));
        assert!(
            !out.events
                .iter()
                .any(|e| matches!(e, RuleEvent::LateDrain { .. }))
        );
        let out = r.tick(&input(10.0, 1490.0, 1.0));
        assert!(out.events.contains(&RuleEvent::LateDrain { seconds: -1 }));
    }

    #[test]
    fn clean_on_time_stop_earns_bonus_and_exact_bonus() {
        let mut r = rules(Vec::new(), Vec::new());
        r.clock = r.scheduled - 30;
        r.remaining_s = 30;
        r.tick(&input(0.0, 0.004, 2000.0));
        // Nothing pending: the arrival tick is 0x407ff0's extra `Task_Tick`.
        assert_eq!(r.phase, Phase::Evaluating(1));
        r.tick(&input(0.0, 0.004, 2000.0));
        assert_eq!(r.stops, 1, "part 1: stop count and error");
        let out = r.tick(&input(0.0, 0.004, 2000.0));
        assert!(
            out.events
                .iter()
                .any(|e| matches!(e, RuleEvent::Arrived(Arrival { mark: 2, .. })))
        );
        assert_eq!(time_events(&out), vec![(TimeReason::OnTimeStop, 5)]);
        while r.phase != Phase::Arrived {
            r.tick(&input(0.0, 0.004, 2000.0));
        }
        let res = r.finish_station();
        assert_eq!(res.exact_bonus_s, 10);
        assert_eq!(res.total_s, 0);
        assert_eq!(res.remaining_s, 45);
        assert_eq!(res.outcome, StationOutcome::Cleared);
    }

    #[test]
    fn overrun_beyond_tolerance_costs_its_metres() {
        let mut r = rules(Vec::new(), Vec::new());
        r.remaining_s = 30;
        r.tick(&input(0.0, -6.2, 2006.0));
        while r.phase != Phase::Arrived {
            r.tick(&input(0.0, -6.2, 2006.0));
        }
        let res = r.finish_station();
        assert_eq!(res.overrun_s, 6);
        assert_eq!(res.remaining_s, 24);
    }

    #[test]
    fn stopping_short_starts_the_between_stations_timer() {
        let mut r = rules(Vec::new(), Vec::new());
        r.approach = 0;
        for _ in 0..600 {
            r.tick(&input(0.0, 800.0, 1200.0));
        }
        assert_eq!(r.phase, Phase::Driving);
        assert_eq!(r.stopped_ticks, 600);
        let out = r.tick(&input(0.0, 800.0, 1200.0));
        assert!(time_events(&out).is_empty());
        for _ in 0..48 {
            r.tick(&input(0.0, 800.0, 1200.0));
        }
        let out = r.tick(&input(0.0, 800.0, 1200.0));
        assert_eq!(
            time_events(&out),
            vec![(TimeReason::StoppedBetweenStations, -5)]
        );
    }

    #[test]
    fn horn_sign_missed_and_horn_bonus() {
        let mut r = rules(
            vec![ev(0, 1500, 3, [1, 30, -1]), ev(0, 1000, 3, [0, 30, -1])],
            Vec::new(),
        );
        r.tick(&input(10.0, 1500.0, 500.0));
        assert_eq!(r.horn_state, 1);
        let out = r.tick(&input(10.0, 1460.0, 531.0));
        assert_eq!(time_events(&out), vec![(TimeReason::HornMissed, -5)]);
        r.tick(&input(10.0, 1000.0, 1000.0));
        assert_eq!(r.horn_state, 2);
        let mut i = input(10.0, 995.0, 1005.0);
        i.buttons.horn_pressed = true;
        let out = r.tick(&i);
        // 25 m left × 0.04 = 1.0 → full 3 s.
        assert_eq!(time_events(&out), vec![(TimeReason::Horn, 3)]);
    }

    #[test]
    fn running_out_of_time_is_game_over() {
        let mut r = rules(Vec::new(), Vec::new());
        r.remaining_s = 1;
        r.pending_s = -1;
        let mut over = false;
        for t in 0..10 {
            let out = r.tick(&input(10.0, 1500.0 - t as f32, t as f32));
            over |= out.events.contains(&RuleEvent::GameOver);
        }
        assert!(over);
        assert_eq!(r.phase, Phase::GameOver);
    }

    #[test]
    fn perfect_run_grades_100() {
        let mut r = rules(Vec::new(), Vec::new());
        r.station = 1;
        r.stops = 1;
        let g = r.grades();
        assert_eq!(g.parts, [20, 20, 20, 20, 20]);
        assert_eq!(g.total, 100);
    }

    #[test]
    fn blocks_cover_the_segment() {
        let r = rules(Vec::new(), Vec::new());
        // (2000 − 300) / 330 = 5 blocks + station blocks.
        assert_eq!(r.block, 7);
        assert_eq!(r.blocks[2].sight_pos_m, 300);
        assert_eq!(r.blocks[1].sight_pos_m, NO_SIGNAL);
        let sight = (330.0f32 * SIGHT_FACTOR) as i32;
        assert_eq!(r.blocks[7].sight_pos_m, sight + 2000 - 330);
        assert!(
            r.blocks[3..=7]
                .windows(2)
                .all(|w| w[0].sight_pos_m < w[1].sight_pos_m)
        );
    }

    #[test]
    fn signal_sprites_follow_the_table() {
        let sets = [0, 1];
        // 4-lamp block signal: green 0x47c … red 0x470; starting green 0x47e.
        assert_eq!(signal_sprite(0, 0, 0, sets), Some(0x47c));
        assert_eq!(signal_sprite(4, 0, 0, sets), Some(0x470));
        assert_eq!(signal_sprite(0, 2, 0, sets), Some(0x47e));
        // 3-lamp set 0 home; repeaters use their own set in both variants.
        assert_eq!(signal_sprite(2, 1, 1, sets), Some(0x454));
        assert_eq!(signal_sprite(1, 3, 0, sets), Some(0x481));
        assert_eq!(signal_sprite(1, 3, 1, sets), Some(0x481));
        assert_eq!(signal_sprite(5, 0, 0, sets), None);
        assert_eq!(signal_sprite(0, 4, 0, sets), None);
    }

    fn shown(out: &TickOutput) -> Vec<RuleEvent> {
        out.events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    RuleEvent::SignalShown { .. } | RuleEvent::HintShown { .. }
                )
            })
            .cloned()
            .collect()
    }

    #[test]
    fn departure_shows_red_then_the_aspect_in_force() {
        let mut r = Rules::new(&tables(), &stage(Vec::new(), Vec::new()), 3, 1, 0).unwrap();
        r.rebuild_signals();
        r.signal_variant = 1;
        r.green_hint = 7;
        let mut seen = Vec::new();
        for t in 1..=170 {
            let out = r.tick(&input(0.0, 2000.0, 0.0));
            for e in shown(&out) {
                seen.push((t, e));
            }
        }
        // Counter 100 on the 101st update, 0xa0 on the 161st (offset 0 on stage 3).
        assert_eq!(
            seen,
            vec![
                (
                    101,
                    RuleEvent::SignalShown {
                        aspect: 4,
                        forced: false,
                        display: 2,
                        variant: 1,
                        ticks: 100,
                        distance_m: 0,
                        start_m: 0,
                    }
                ),
                (
                    161,
                    RuleEvent::HintShown {
                        id: 7,
                        ticks: 0,
                        distance_m: 220,
                        start_m: 0,
                    }
                ),
                (
                    161,
                    RuleEvent::SignalShown {
                        aspect: 0,
                        forced: true,
                        display: 2,
                        variant: 0,
                        ticks: 0,
                        distance_m: 10,
                        start_m: 0,
                    }
                ),
            ]
        );
        assert_eq!(r.green_hint, -1);
        assert_eq!(
            r.signal_hud.current_sprite(),
            Some(0x47e),
            "4-lamp starting green"
        );
    }

    #[test]
    fn the_same_sprite_shows_again_once_its_task_has_ended() {
        let mut r = rules(Vec::new(), Vec::new());
        let mut out = TickOutput::default();
        assert!(r.signal_show(&mut out, 2, 0, 0, 10, 0));
        assert!(!r.signal_show(&mut out, 2, 0, 0, 10, 0), "already shown");
        // 3-lamp set 0 (variant 1): aspects 1..3 share a sprite.
        assert!(r.signal_show(&mut out, 1, 0, 0, 10, 1));
        assert!(!r.signal_show(&mut out, 3, 0, 0, 10, 1));
        // Pass the signal: the task ends at the first update beyond its distance.
        r.signal_hud_pending = true;
        r.flush_signal_hud(11.0);
        assert_eq!(
            r.signal_hud.current_sprite(),
            Some(0x453),
            "still in its delay"
        );
        for _ in 0..6 {
            r.signal_hud_pending = true;
            r.flush_signal_hud(11.0);
        }
        assert_eq!(r.signal_hud.current_sprite(), None);
        assert!(r.signal_show(&mut out, 3, 0, 0, 10, 1));
    }

    #[test]
    fn recovery_shows_a_forced_green_before_the_last_ten_ticks() {
        let mut r = rules(Vec::new(), Vec::new());
        r.phase = Phase::Holding {
            ticks: RECOVERY_RELEASE_TICKS + 1,
            then: AfterHold::Resume,
        };
        let out = r.tick(&input(0.0, 1500.0, 500.0));
        assert!(shown(&out).is_empty());
        let out = r.tick(&input(0.0, 1500.0, 500.0));
        assert!(matches!(
            shown(&out)[..],
            [RuleEvent::SignalShown {
                aspect: 0,
                forced: true,
                ..
            }]
        ));
    }

    #[test]
    fn course_hints_are_dropped_while_the_ats_awaits_confirmation() {
        let ev9 = ev(1, 900, 9, [3, 60, 0]);
        let mut r = rules(vec![ev9], vec![]);
        r.odometer_m = 120.7;
        let mut out = TickOutput::default();
        r.hint(&mut out, 3, 60, 0);
        assert_eq!(
            out.events,
            [RuleEvent::HintShown {
                id: 3,
                ticks: 60,
                distance_m: 0,
                start_m: 120,
            }]
        );
        r.ats = 2;
        let mut out = TickOutput::default();
        r.hint(&mut out, 3, 60, 0);
        r.hint(&mut out, -1, -1, -1);
        assert!(out.events.is_empty());
        r.ats = 1;
        r.hint(&mut out, -1, -1, -1);
        assert_eq!(out.events, [RuleEvent::HintHidden]);
    }

    #[test]
    fn stop_advice_shows_once_per_signal_setup() {
        let mut r = rules(vec![], vec![]);
        r.tables.advice_kmh[3] = [55, 50, 76, 70, 85, 79, 93, 87, 100, 94];
        r.target_stops = true;
        r.dist_round_m = 550;
        r.speed_mps = 100.0 / 3.6 + 0.01;
        let mut out = TickOutput::default();
        r.advice_hints(&mut out, false);
        assert!(matches!(
            out.events[..],
            [RuleEvent::HintShown {
                id: 0x1e,
                distance_m: 120,
                ..
            }]
        ));
        let mut out = TickOutput::default();
        r.dist_round_m = 350;
        r.advice_hints(&mut out, false);
        assert!(out.events.is_empty(), "once until the next signal set-up");
        r.build_blocks();
        r.advice_hints(&mut out, false);
        assert_eq!(out.events.len(), 1);
    }
}

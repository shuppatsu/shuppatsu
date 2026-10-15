//! Stage clock: the in-game time of day shown on the HUD and compared with the timetable.
//!
//! The clock is the global 0x8570b0, in ticks since midnight (30 ticks = one game second; the
//! timetable rows store their times in the same unit, see `ddg_formats::stages`). Behaviour and
//! addresses: `docs/re/physics.md` §6.

use ddg_formats::Result;
use ddg_formats::exe::Exe;

/// Ticks per game second.
pub const TICKS_PER_SECOND: i32 = 30;
/// Dwell added to the scheduled arrival when a segment starts: 0x2b2 = 690 ticks = 23 s
/// (0x40d040, 0x40d930 at stage start/continue; 0x40d340 when moving on to the next segment).
pub const DWELL_TICKS: i32 = 0x2b2;
/// The speed-up is added when `clock % 30 == 2` (0x40855d).
const SPEEDUP_PHASE: i32 = 2;
/// Per-stage rank table: i32 at 0x4656c4 + stage × 0x20 (3000-bandai exe).
const STAGE_RANK_VA: u32 = 0x4656c4;
const STAGE_RANK_STRIDE: u32 = 0x20;

/// The stage clock (0x8570b0) and its speed-up (0x856fac).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageClock {
    /// Ticks since midnight.
    pub ticks: i32,
    /// Extra ticks added whenever the clock reaches phase 2 of a second while driving (0 or 1
    /// in practice; with 1 the clock gains a tick every 29 driving ticks).
    pub speedup: i32,
}

impl StageClock {
    /// The clock when a run starts or continues at a station whose timetable row has
    /// `scheduled_ticks` (row field +0x00): the scheduled time plus the 23 s dwell.
    pub fn at_station(scheduled_ticks: i32) -> Self {
        Self {
            ticks: scheduled_ticks + DWELL_TICKS,
            speedup: 0,
        }
    }

    /// One tick of the pre-departure wait (doors, departure signal): the clock always runs.
    pub fn tick_waiting(&mut self) {
        self.ticks += 1;
    }

    /// One tick of the driving loop. `frozen` is 0x856d80 ≠ 0 (end-of-line sequence), which
    /// stops the clock; the speed-up check runs either way.
    pub fn tick_driving(&mut self, frozen: bool) {
        if !frozen {
            self.ticks += 1;
        }
        if self.ticks % TICKS_PER_SECOND == SPEEDUP_PHASE {
            self.ticks += self.speedup;
        }
    }

    /// After a station stop has been evaluated (0x4080d0, before 0x40d340): from the 5th
    /// station on, at every other station, the speed-up becomes 1 if the stage has a positive
    /// rank (every stage does).
    pub fn after_stop(&mut self, stage_rank: i32, station: i32) {
        if stage_rank > 0 && station > 3 && (station - 4) % 2 == 0 && self.speedup < 1 {
            self.speedup += 1;
        }
    }

    /// Moving on to the next segment after a stop (0x40d340): the dwell is added.
    pub fn next_segment(&mut self) {
        self.ticks += DWELL_TICKS;
    }

    /// Continue after a failed stop: the speed-up is cleared (0x4080d0).
    pub fn on_continue(&mut self) {
        self.speedup = 0;
    }

    /// Whole seconds late (negative = early) against a scheduled time (0x856fc0, computed
    /// at a stop in 0x40a0a0 and when passing a station in 0x40d620). Both times are divided
    /// by 30 separately, as the original does.
    pub fn seconds_late(&self, scheduled_ticks: i32) -> i32 {
        self.ticks / TICKS_PER_SECOND - scheduled_ticks / TICKS_PER_SECOND
    }

    /// Passing a non-stop station within the first second of its scheduled time
    /// (`0 ≤ clock − scheduled ≤ 29`, 0x40d620) earns the on-time passing bonus.
    pub fn passes_on_time(&self, scheduled_ticks: i32) -> bool {
        (0..TICKS_PER_SECOND).contains(&(self.ticks - scheduled_ticks))
    }

    /// Hours, minutes, seconds as the replay/HUD format them (0x42c020).
    pub fn hms(&self) -> (i32, i32, i32) {
        let t = self.ticks;
        (t / 108_000, (t / 1800) % 60, (t / 30) % 60)
    }
}

/// The per-stage rank values (0x4656c4 + stage × 0x20) used by [`StageClock::after_stop`].
pub fn read_stage_ranks(exe: &Exe<'_>) -> Result<Vec<i32>> {
    (0..ddg_formats::stages::STAGE_COUNT as u32)
        .map(|s| {
            exe.reader_at(STAGE_RANK_VA + s * STAGE_RANK_STRIDE, 4, "stage rank")?
                .i32(0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_with_dwell_and_counts_ticks() {
        let mut c = StageClock::at_station(1_010_520);
        assert_eq!(c.ticks, 1_011_210);
        c.tick_waiting();
        c.tick_driving(false);
        c.tick_driving(true);
        assert_eq!(c.ticks, 1_011_212);
        assert_eq!(c.hms(), (9, 21, 47));
    }

    #[test]
    fn speedup_adds_one_tick_per_second() {
        let mut c = StageClock::at_station(0);
        c.after_stop(2, 3);
        assert_eq!(c.speedup, 0);
        c.after_stop(2, 5);
        assert_eq!(c.speedup, 0);
        c.after_stop(2, 4);
        assert_eq!(c.speedup, 1);
        let start = c.ticks;
        for _ in 0..300 {
            c.tick_driving(false);
        }
        // After a jump to phase 3 the next phase 2 is 29 ticks away: hits at 2, 31, …, 292.
        assert_eq!(c.ticks - start, 311);
        c.on_continue();
        assert_eq!(c.speedup, 0);
    }

    #[test]
    fn lateness_uses_whole_seconds() {
        let c = StageClock {
            ticks: 59,
            speedup: 0,
        };
        assert_eq!(c.seconds_late(30), 0);
        assert_eq!(c.seconds_late(29), 1);
        assert!(c.passes_on_time(30));
        assert!(!c.passes_on_time(60));
        assert!(!c.passes_on_time(29));
    }
}

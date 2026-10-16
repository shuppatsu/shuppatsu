//! The stop-approach record behind the station-result replay (docs/re/scene.md §8).
//!
//! Every driving tick, 0x409fc0 writes the distance to the stop into a 400-entry ring
//! (`0x850300`, index `0x850a80`, wrapped flag `0x850a78`) while the train is within 50 m of a
//! stopping station or approaching an obstacle stop, and rewinds the ring otherwise. The
//! result camera (0x419730) replays the last 120 samples. Presentation only: nothing in the
//! rules reads it back.

/// Ring length (`0x850300..0x850940`, f32).
pub const RING_LEN: usize = 400;
/// Samples the result replays: the last 120 ticks, 4 s (0x419730: `0x78`).
pub const REPLAY_LEN: usize = 120;
/// Recording starts at 50 m to a stopping station (f64 at 0x4594c8).
const STATION_ZONE_M: f32 = 50.0;
/// An obstacle approach is recorded from 200 m (f64 at 0x4594b8).
const OBSTACLE_ZONE_M: f32 = 200.0;
/// 0x419730 replaces the record with a made-up approach when the last sample is at or past
/// −10 m (f64 at 0x459538).
const OVERRUN_REPLACE_M: f32 = -10.0;
/// The made-up approach starts 20 m out (f32 at 0x459558; f64 20.0 at 0x4594d0).
const MADE_UP_START_M: f32 = 20.0;
/// Its step starts at `2(|d| + 20)/120` (f64 1/120 at 0x469768) and shrinks each sample by
/// `2(|d| + 20)/14400` (f64 −1/14400 at 0x469760).
const MADE_UP_STEP: f64 = 1.0 / 120.0;
const MADE_UP_DECAY: f64 = -1.0 / 14400.0;

/// The ring and its write position.
#[derive(Debug, Clone, PartialEq)]
pub struct StopRecord {
    samples: Box<[f32; RING_LEN]>,
    /// Next write index (`0x850a80`).
    head: usize,
    /// The ring has wrapped since the last rewind (`0x850a78`).
    wrapped: bool,
}

impl Default for StopRecord {
    fn default() -> Self {
        Self {
            samples: Box::new([0.0; RING_LEN]),
            head: 0,
            wrapped: false,
        }
    }
}

/// What 0x409fc0 sees on a driving tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecordInput {
    /// Distance to the stop, m (`0x85708c`, positive = short).
    pub dist_m: f32,
    /// The target station is a stop (`0x856d74 == 1`).
    pub target_stops: bool,
    /// Obstacle-stop state (`0x856d80`).
    pub obstacle: i16,
    /// Obstacle stop odometer (`0x856dac`) and the odometer (`0x857004`), m.
    pub obstacle_odo: f32,
    pub odometer_m: f32,
}

impl StopRecord {
    /// The record step of 0x409fc0 (driving ticks only).
    pub fn record(&mut self, i: RecordInput) {
        if i.dist_m <= STATION_ZONE_M && i.target_stops {
            self.samples[self.head] = i.dist_m;
        } else if i.obstacle < 1 {
            self.head = 0;
            self.wrapped = false;
            return;
        } else {
            let d = i.obstacle_odo - i.odometer_m;
            self.samples[self.head] = d;
            if d > OBSTACLE_ZONE_M {
                return;
            }
        }
        self.head = (self.head + 1) % RING_LEN;
        if self.head == 0 {
            self.wrapped = true;
        }
    }

    /// The samples the result camera plays, one per tick, as 0x419730 picks them: the last
    /// [`REPLAY_LEN`] written (fewer when the ring has not wrapped), or a made-up 121-sample
    /// approach from 20 m when the last sample is at or past −10 m (the replay then plays the
    /// first 120). `shift_m` is added to the whole ring first (40 m at the coupling stations).
    ///
    /// Empty when nothing was recorded; the camera then keeps its creation pose, which the
    /// original computes from the stale sample at index 0 ([`StopRecord::stale_first`]).
    pub fn replay(&mut self, shift_m: f32) -> Vec<f32> {
        if shift_m != 0.0 {
            for s in self.samples.iter_mut() {
                *s += shift_m;
            }
        }
        let (mut start, mut count) = if self.wrapped {
            ((self.head + RING_LEN - REPLAY_LEN) % RING_LEN, REPLAY_LEN)
        } else if self.head >= REPLAY_LEN {
            (self.head - REPLAY_LEN, REPLAY_LEN)
        } else {
            (0, self.head)
        };
        let last = self.samples[(self.head + RING_LEN - 1) % RING_LEN];
        if last <= OVERRUN_REPLACE_M {
            self.make_up(last);
            (start, count) = (0, REPLAY_LEN);
        }
        (0..count)
            .map(|k| self.samples[(start + k) % RING_LEN])
            .collect()
    }

    /// The sample the creation pose reads when [`StopRecord::replay`] came back empty.
    pub fn stale_first(&self) -> f32 {
        self.samples[0]
    }

    /// 0x419730's replacement for an overrun record: a decelerating approach from 20 m that
    /// ends about `|last|` metres past the stop, written over samples 0..=120.
    fn make_up(&mut self, last: f32) {
        let over = f64::from(last.abs());
        let span = over + 20.0 + over + 20.0;
        let mut step = span * MADE_UP_STEP;
        let mut d = MADE_UP_START_M;
        for s in self.samples.iter_mut().take(REPLAY_LEN + 1) {
            *s = d;
            d = (f64::from(d) - step) as f32;
            step += span * MADE_UP_DECAY;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(d: f32) -> RecordInput {
        RecordInput {
            dist_m: d,
            target_stops: true,
            obstacle: 0,
            obstacle_odo: 0.0,
            odometer_m: 0.0,
        }
    }

    #[test]
    fn records_only_inside_50_m() {
        let mut r = StopRecord::default();
        for d in [80.0, 60.0, 50.0, 30.0, 10.0, 0.5] {
            r.record(at(d));
        }
        assert_eq!(r.replay(0.0), vec![50.0, 30.0, 10.0, 0.5]);
    }

    #[test]
    fn leaving_the_zone_rewinds() {
        let mut r = StopRecord::default();
        r.record(at(20.0));
        r.record(at(70.0));
        r.record(at(10.0));
        assert_eq!(r.replay(0.0), vec![10.0]);
    }

    #[test]
    fn keeps_the_last_120_across_the_wrap() {
        let mut r = StopRecord::default();
        for k in 0..450 {
            r.record(at(50.0 - k as f32 * 0.1));
        }
        let s = r.replay(0.0);
        assert_eq!(s.len(), REPLAY_LEN);
        assert_eq!(s[REPLAY_LEN - 1], 50.0 - 449.0 * 0.1);
        assert_eq!(s[0], 50.0 - 330.0 * 0.1);
    }

    #[test]
    fn an_overrun_gets_a_made_up_approach() {
        let mut r = StopRecord::default();
        for k in 0..60 {
            r.record(at(30.0 - k as f32));
        }
        let s = r.replay(0.0);
        assert_eq!(s.len(), REPLAY_LEN);
        assert_eq!(s[0], 20.0);
        assert!(s.windows(2).all(|w| w[1] < w[0]));
        // 120 steps cover about |d| + 20 m: the replay ends near the real overrun (−29 m).
        assert!(
            (s[REPLAY_LEN - 1] + 29.0).abs() < 1.0,
            "{}",
            s[REPLAY_LEN - 1]
        );
    }

    #[test]
    fn obstacle_approach_from_200_m() {
        let mut r = StopRecord::default();
        let obstacle = |odo: f32| RecordInput {
            dist_m: 900.0,
            target_stops: true,
            obstacle: 1,
            obstacle_odo: 1000.0,
            odometer_m: odo,
        };
        r.record(obstacle(700.0));
        r.record(obstacle(850.0));
        r.record(obstacle(900.0));
        assert_eq!(r.replay(0.0), vec![150.0, 100.0]);
    }
}

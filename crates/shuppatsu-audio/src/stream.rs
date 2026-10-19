//! A PCM stream fed by the game thread and played by the [`crate::Mixer`]: the Komachi movie's
//! soundtrack (docs/re/movie.md). The original sent it to DirectShow's default audio renderer,
//! beside the game's DirectSound voices and at full level, so here it is one extra source
//! mixed at unity gain and resampled to the device rate like the voices (setting
//! `audio_resampler`).
//!
//! The mixer counts the source frames it has played ([`PcmStream::played`]); that count is the
//! movie's clock, so the picture follows the sound as heard. When the queue runs dry before
//! [`PcmStream::finish`], the stream holds (silence, the clock stops) until more arrives.
//!
//! The count moves in whole device buffers, once per audio callback (a few to tens of
//! milliseconds), so [`PcmStream::position_at`] reads it as a smooth clock: the count at the
//! start of the last callback ([`PcmStream::mark_callback`]) plus the time since then, less
//! the device's output latency, and never past what has been rendered.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Output latencies above this are taken as a bad report and capped.
const MAX_LATENCY: Duration = Duration::from_millis(250);

#[derive(Debug, Default)]
pub(crate) struct StreamState {
    pub(crate) queue: VecDeque<[f32; 2]>,
    pub(crate) finished: bool,
    /// Source frames that have passed the play position.
    pub(crate) played: u64,
    /// The last audio callback ([`PcmStream::mark_callback`]).
    anchor: Option<Anchor>,
}

/// One audio callback: when it began, the frames played before it, and how long its first
/// frame takes to reach the speaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Anchor {
    at: Instant,
    played: u64,
    latency: Duration,
}

/// The position heard at `now`, in source frames at `rate`: `played` as is without an
/// anchor; else the anchor's count advanced by the time since its callback less the latency,
/// clamped to `0..=played` (not past what has been rendered: an underrun holds it).
fn position(played: u64, anchor: Option<Anchor>, rate: u32, now: Instant) -> f64 {
    let Some(a) = anchor else {
        return played as f64;
    };
    let since = now.saturating_duration_since(a.at).as_secs_f64() - a.latency.as_secs_f64();
    (a.played as f64 + since * f64::from(rate)).clamp(0.0, played as f64)
}

/// A queue of stereo frames at a fixed rate, shared between the game and audio threads.
#[derive(Debug)]
pub struct PcmStream {
    rate: u32,
    state: Mutex<StreamState>,
}

impl PcmStream {
    pub fn new(rate: u32) -> Arc<Self> {
        Arc::new(Self {
            rate: rate.max(1),
            state: Mutex::new(StreamState::default()),
        })
    }

    pub(crate) fn lock(&self) -> MutexGuard<'_, StreamState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The source rate in Hz.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Appends interleaved samples of `channels` channels (1: mono to both sides; beyond 2:
    /// the first two).
    pub fn push(&self, samples: &[f32], channels: usize) {
        let channels = channels.max(1);
        let mut s = self.lock();
        for f in samples.chunks_exact(channels) {
            let l = f[0];
            let r = if channels > 1 { f[1] } else { l };
            s.queue.push_back([l, r]);
        }
    }

    /// No more data will come: the stream plays out what is queued, then stays silent.
    pub fn finish(&self) {
        self.lock().finished = true;
    }

    /// Frames waiting to be played.
    pub fn queued(&self) -> usize {
        self.lock().queue.len()
    }

    /// Source frames played so far (the clock, in units of 1 / [`PcmStream::rate`] s). It
    /// moves once per audio callback; [`PcmStream::position_at`] is the smooth form.
    pub fn played(&self) -> u64 {
        self.lock().played
    }

    /// The output calls this at the start of each audio callback, before the mixer renders:
    /// `at` is now, `latency` the time until the callback's first frame is heard (capped at
    /// 250 ms).
    pub fn mark_callback(&self, at: Instant, latency: Duration) {
        let mut s = self.lock();
        s.anchor = Some(Anchor {
            at,
            played: s.played,
            latency: latency.min(MAX_LATENCY),
        });
    }

    /// The play position heard at `now`, in source frames (fractional): smooth between audio
    /// callbacks, behind the rendered count by the output latency, held by an underrun.
    /// Without callback marks (no device, tests) it is [`PcmStream::played`].
    pub fn position_at(&self, now: Instant) -> f64 {
        let s = self.lock();
        position(s.played, s.anchor, self.rate, now)
    }

    /// Finished and played out.
    pub fn drained(&self) -> bool {
        let s = self.lock();
        s.finished && s.queue.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    /// 1 kHz: 1 frame per ms. A callback of 100 frames at t0 with no latency: the position
    /// runs from the anchor at 1 frame per ms up to the rendered count, then holds.
    #[test]
    fn position_runs_between_callbacks() {
        let t0 = Instant::now();
        let a = Some(Anchor {
            at: t0,
            played: 500,
            latency: Duration::ZERO,
        });
        assert_eq!(position(600, a, 1000, t0), 500.0);
        assert_eq!(position(600, a, 1000, t0 + 37 * MS), 537.0);
        assert_eq!(position(600, a, 1000, t0 + 100 * MS), 600.0);
        assert_eq!(
            position(600, a, 1000, t0 + 140 * MS),
            600.0,
            "late callback: holds"
        );
        // Before the anchor (a reader's `now` taken before the callback's): no earlier than it.
        assert_eq!(position(600, a, 1000, t0 - 5 * MS), 500.0);
        // Without an anchor: the count.
        assert_eq!(position(600, None, 1000, t0), 600.0);
    }

    /// The latency puts the position behind the anchor (what is heard now was rendered by an
    /// earlier callback); never below 0.
    #[test]
    fn position_lags_by_the_latency() {
        let t0 = Instant::now();
        let a = Anchor {
            at: t0,
            played: 500,
            latency: 20 * MS,
        };
        assert_eq!(position(600, Some(a), 1000, t0), 480.0);
        assert_eq!(position(600, Some(a), 1000, t0 + 50 * MS), 530.0);
        let start = Some(Anchor { played: 0, ..a });
        assert_eq!(position(100, start, 1000, t0 + 5 * MS), 0.0);
    }

    /// Callbacks of 1024 frames (23 ms at 44.1 kHz): read at 120 Hz, the position advances
    /// evenly (every step within 1 frame of the elapsed time), and it never goes back.
    #[test]
    fn position_is_smooth_across_large_callbacks() {
        let rate = 44_100;
        let s = PcmStream::new(rate);
        let t0 = Instant::now();
        let chunk = 1024u64;
        let period = Duration::from_secs_f64(chunk as f64 / f64::from(rate));
        let mut last = 0.0f64;
        let mut callbacks = 0u32;
        for i in 0..240u32 {
            let now = t0 + Duration::from_secs_f64(f64::from(i) / 120.0);
            // Every callback due by `now` has happened.
            while t0 + period * callbacks <= now {
                s.mark_callback(t0 + period * callbacks, Duration::ZERO);
                s.lock().played += chunk;
                callbacks += 1;
            }
            let p = s.position_at(now);
            let expect = (now - t0).as_secs_f64() * f64::from(rate);
            assert!((p - expect).abs() < 1.0, "{i}: {p} vs {expect}");
            assert!(p >= last);
            last = p;
            // The raw count jumps by whole callbacks.
            assert_eq!(s.played() % chunk, 0);
        }
    }
}

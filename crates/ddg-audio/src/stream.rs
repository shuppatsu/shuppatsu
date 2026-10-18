//! A PCM stream fed by the game thread and played by the [`crate::Mixer`]: the Komachi movie's
//! soundtrack (docs/re/movie.md). The original sent it to DirectShow's default audio renderer,
//! beside the game's DirectSound voices and at full level, so here it is one extra source
//! mixed at unity gain and resampled to the device rate like the voices (setting
//! `audio_resampler`).
//!
//! The mixer counts the source frames it has played ([`PcmStream::played`]); that count is the
//! movie's clock, so the picture follows the sound as heard. When the queue runs dry before
//! [`PcmStream::finish`], the stream holds (silence, the clock stops) until more arrives.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Debug, Default)]
pub(crate) struct StreamState {
    pub(crate) queue: VecDeque<[f32; 2]>,
    pub(crate) finished: bool,
    /// Source frames that have passed the play position.
    pub(crate) played: u64,
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

    /// Source frames played so far (the clock, in units of 1 / [`PcmStream::rate`] s).
    pub fn played(&self) -> u64 {
        self.lock().played
    }

    /// Finished and played out.
    pub fn drained(&self) -> bool {
        let s = self.lock();
        s.finished && s.queue.is_empty()
    }
}

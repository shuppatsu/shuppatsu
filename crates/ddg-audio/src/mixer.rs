//! The audio-thread side: ten voices mixed in software (audio-text.md §1, §2, §4).
//!
//! The original gives each voice its own DirectSound buffer, refilled half a buffer at a time,
//! and lets DirectSound mix them. Here every voice reads its sample data directly, resampled to
//! the output rate, and the mixer sums them into interleaved `f32` stereo. Observable behaviour
//! matches the original:
//!
//! * slots 0–7 play effects, 8–9 play BGM; replaying a sounding code restarts its slot; when no
//!   slot is free the request is dropped (no stealing);
//! * effect loops repeat `[loop_start, loop_end)` until released; release jumps to the tail;
//! * a voice frees its slot at the end of the streaming half-buffer that held its last sample
//!   (0x1000 samples for effects, 0x2000 frames for BGM), counted from (re)start;
//! * pitch `hz = rate·(256 − p)/256` (inverted for code 0x31, fixed 44100 for BGM);
//! * volume is linear in millibels: `mB = −10000·(1 − tbl[code]/127 · v/127)`;
//! * no pan: mono effects go to both channels at full level.

use crate::bank::{EffectInfo, SoundBank};
use crate::code::{INVERTED_PITCH_CODE, LAST_EFFECT_CODE, Source};
use crate::sinc::{self, HISTORY, TAPS, Window};
use crate::tables::CodeVolumes;
use std::sync::Arc;

/// Number of voices (0x42ffa0 builds 10).
pub const VOICES: usize = 10;
/// Effect voices are slots `0..SE_VOICES`, BGM voices the rest (0x430820).
pub const SE_VOICES: usize = 8;
/// Streaming half-buffer length: effects 0x1000 u8 samples, BGM 0x8000 bytes = 0x2000 frames.
const SE_HALF: u64 = 0x1000;
const BGM_HALF: u64 = 0x2000;
/// IDirectSoundBuffer::SetFrequency range (DSBFREQUENCY_MIN/MAX); outside it the call fails and
/// the previous frequency stays.
const FREQ_MIN: u32 = 100;
const FREQ_MAX: u32 = 100_000;
/// Default volume parameter after a (re)start (0x430720).
pub const FULL_VOLUME: i16 = 0x7f;

/// A message to the audio thread (the original's thread messages 0x464–0x469).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// 0x464: start or restart `code`.
    Play(u16),
    /// 0x465: key off (jump to the release tail, or stop).
    Release(u16),
    /// 0x466: stop every voice.
    StopAll,
    /// 0x467: pitch parameter (0..=127, larger = lower; inverted for code 0x31).
    SetPitch(u16, i16),
    /// 0x468: volume parameter (0..=127).
    SetVolume(u16, i16),
    /// 0x469: pause (hold every voice) or resume.
    Pause(bool),
}

/// Sample interpolation of the resampler (setting `audio_resampler`, docs/settings.md).
///
/// The original left resampling to DirectSound: every voice buffer (`LOCSOFTWARE`, its rate set
/// with SetFrequency) was converted by the Windows 9x software mixer into the never-formatted
/// primary buffer, by default 22.05 kHz 8-bit (audio-text.md §1). That mixer steps through the
/// source with a fixed-point increment; whether it took the nearest or a linearly interpolated
/// sample is not known (audio-text.md open question 8). [`Interpolation::Nearest`] and
/// [`Interpolation::Linear`] are those two candidates; [`Interpolation::Sinc`] is an
/// enhancement the original never had. None of them reproduces the 22.05 kHz primary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Interpolation {
    /// Sample-and-hold.
    Nearest,
    /// Linear between neighbouring source samples (the arcade default of [`MixerConfig`]).
    Linear,
    /// Band-limited: a 32-tap Kaiser-windowed sinc ([`crate::sinc`]) whose cutoff follows the
    /// pitch when a voice plays faster than the output rate. No images or aliasing, reads
    /// across loop seams, zero-padded at the ends of one-shots. The port's default (the user's
    /// decision, docs/settings.md).
    #[default]
    Sinc,
}

impl Interpolation {
    /// Every mode, in the settings screen's order.
    pub const ALL: [Self; 3] = [Self::Linear, Self::Nearest, Self::Sinc];

    /// The `audio_resampler` value.
    pub fn name(self) -> &'static str {
        match self {
            Self::Nearest => "nearest",
            Self::Linear => "linear",
            Self::Sinc => "sinc",
        }
    }

    /// Parses an `audio_resampler` value (case-insensitive).
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|m| m.name() == s)
    }
}

/// Mixer options. The defaults are the arcade behaviour; every non-default value is an
/// enhancement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MixerConfig {
    /// Output gain applied after mixing (1.0 = original level). Enhancement when ≠ 1.
    pub master_volume: f32,
    pub interpolation: Interpolation,
    /// Ramp volume changes over a few milliseconds instead of stepping (avoids zipper noise on
    /// the per-tick motor/noise volume updates). Enhancement; off for arcade.
    pub smooth_volume: bool,
}

impl Default for MixerConfig {
    fn default() -> Self {
        Self {
            master_volume: 1.0,
            interpolation: Interpolation::Linear,
            smooth_volume: false,
        }
    }
}

/// Frequency in Hz for `code` at pitch parameter `p` (0x430e00).
///
/// `native_rate` is the effect's header rate; BGM codes always give 44100.
pub fn pitch_hz(code: u16, p: i16, native_rate: u32) -> i64 {
    if code > LAST_EFFECT_CODE {
        return 44_100;
    }
    let p = if code == INVERTED_PITCH_CODE {
        127 - p as i64
    } else {
        p as i64
    };
    // C division truncates toward zero, which `/` on i64 also does.
    native_rate as i64 * (256 - p) / 256
}

/// DirectSound volume in hundredths of a dB for a code-volume byte and a volume parameter
/// (0x430da0: `trunc` of a double product, clamped to [−10000, 0]).
pub fn volume_millibels(code_volume: i8, v: i16) -> i32 {
    let g = (code_volume as f64 / 127.0) * (v as f64 / 127.0);
    let mb = -10_000i64 - (-10_000.0 * g) as i64;
    mb.clamp(-10_000, 0) as i32
}

/// Linear amplitude of a DirectSound volume (−10000 is treated as silence).
pub fn millibels_to_gain(mb: i32) -> f32 {
    if mb <= -10_000 {
        0.0
    } else {
        10f32.powf(mb as f32 / 2000.0)
    }
}

/// Read-only view of a voice for diagnostics and tests.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoiceInfo {
    pub slot: usize,
    pub code: u16,
    pub pitch: i16,
    pub volume: i16,
    pub hz: u32,
    pub gain: f32,
    /// Current source position (sample or frame index).
    pub position: usize,
    /// The sample data has ended; the slot is waiting for its half-buffer to drain.
    pub draining: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Voice {
    /// 0 = free.
    code: u16,
    source: Source,
    cursor: usize,
    frac: f64,
    /// Source samples streamed since the last (re)start of the DirectSound buffer.
    streamed: u64,
    /// Set when the data ran out: free the slot once `streamed` reaches this.
    stop_at: Option<u64>,
    pitch: i16,
    volume: i16,
    hz: u32,
    gain: f32,
    /// Gain actually applied (differs from `gain` only while smoothing).
    applied_gain: f32,
    /// Stopped by a pause (0x430a00 stops every active buffer); a voice started while paused
    /// plays, as a new DirectSound buffer would.
    held: bool,
    /// [`Interpolation::Sinc`] only: the source frames `cursor − 15 ..= cursor + 16` as they
    /// play (across loop seams; zeros before the start and after the end).
    window: Window,
    /// The source index the window loads next (`None`: past the end, zeros follow).
    ahead: Option<usize>,
    /// After the data ended: source steps left until the last sample leaves the kernel.
    ring_out: u8,
}

impl Voice {
    const FREE: Self = Self {
        code: 0,
        source: Source::Effect(0),
        cursor: 0,
        frac: 0.0,
        streamed: 0,
        stop_at: None,
        pitch: 0,
        volume: 0,
        hz: 0,
        gain: 0.0,
        applied_gain: 0.0,
        held: false,
        window: Window::ZERO,
        ahead: None,
        ring_out: 0,
    };

    fn is_free(&self) -> bool {
        self.code == 0
    }

    fn half(&self) -> u64 {
        match self.source {
            Source::Effect(_) => SE_HALF,
            Source::Bgm(_) => BGM_HALF,
        }
    }

    /// Restarts streaming at `cursor` (play, or the release jump).
    fn restart_at(&mut self, cursor: usize, data_len: usize) {
        self.cursor = cursor;
        self.frac = 0.0;
        self.streamed = 0;
        self.stop_at = None;
        if cursor >= data_len {
            self.stop_at = Some(self.half());
        }
    }

    /// Marks the end of the data: the slot frees at the end of the current half-buffer.
    fn data_ended(&mut self) {
        let half = self.half();
        let halves = self.streamed.div_ceil(half).max(1);
        self.stop_at = Some(halves * half);
        self.ring_out = (HISTORY - 1) as u8;
        if self.streamed >= halves * half {
            *self = Self::FREE;
        }
    }

    /// Reloads the window at the cursor (play, the release jump, or switching to
    /// [`Interpolation::Sinc`]): the frames before it from what just played (`keep_history`,
    /// the release tail continues the loop seamlessly) or zeros, the rest read ahead.
    fn refill_window(&mut self, bank: &SoundBank, keep_history: bool) {
        let old = *self.window.frames();
        let mut new = [[0.0; 2]; TAPS];
        if keep_history {
            // The old window's cursor sat at HISTORY − 1; the frames up to it become the past.
            new[..HISTORY - 1].copy_from_slice(&old[1..HISTORY]);
        }
        let mut next = (self.stop_at.is_none()).then_some(self.cursor);
        for f in &mut new[HISTORY - 1..] {
            if let Some(i) = next {
                let (l, r) = frame_at(bank, self.source, i);
                *f = [l, r];
                next = next_index(bank, self.source, i);
            }
        }
        self.window.set(&new);
        self.ahead = next;
        self.ring_out = 0;
    }

    /// Moves the window one source sample on.
    #[inline]
    fn shift_window(&mut self, bank: &SoundBank) {
        let f = match self.ahead {
            Some(i) => {
                self.ahead = next_index(bank, self.source, i);
                let (l, r) = frame_at(bank, self.source, i);
                [l, r]
            }
            None => [0.0; 2],
        };
        self.window.push(f);
        if self.stop_at.is_some() {
            self.ring_out = self.ring_out.saturating_sub(1);
        }
    }
}

/// Where the source position goes after `cursor`: the next index, a loop wrap, or the end.
fn next_index(bank: &SoundBank, source: Source, cursor: usize) -> Option<usize> {
    match source {
        Source::Effect(i) => {
            let fx = &bank.effects()[i];
            let next = cursor + 1;
            let (ls, le) = (fx.loop_start as usize, fx.loop_end as usize);
            if le != 0 && cursor < le && next == le {
                Some(ls)
            } else if next < fx.len() {
                Some(next)
            } else {
                None
            }
        }
        Source::Bgm(i) => {
            let t = &bank.bgm_tracks()[i];
            let next = cursor + 1;
            if next < t.frames() {
                Some(next)
            } else if t.looping && t.frames() > 0 {
                Some(0)
            } else {
                None
            }
        }
    }
}

fn frame_at(bank: &SoundBank, source: Source, i: usize) -> (f32, f32) {
    match source {
        Source::Effect(e) => {
            let s = bank.effect_sample(&bank.effects()[e], i);
            (s, s)
        }
        Source::Bgm(t) => bank.bgm_frame(&bank.bgm_tracks()[t], i),
    }
}

fn data_len(bank: &SoundBank, source: Source) -> usize {
    match source {
        Source::Effect(i) => bank.effects()[i].len(),
        Source::Bgm(i) => bank.bgm_tracks()[i].frames(),
    }
}

/// The software mixer. Pure and deterministic: output depends only on the commands applied and
/// the number of frames rendered between them.
#[derive(Debug, Clone)]
pub struct Mixer {
    bank: Arc<SoundBank>,
    volumes: CodeVolumes,
    voices: [Voice; VOICES],
    paused: bool,
    out_rate: u32,
    config: MixerConfig,
    /// Per-frame gain step limit when smoothing (reaches any target in ~5 ms).
    smooth_step: f32,
}

impl Mixer {
    pub fn new(
        bank: Arc<SoundBank>,
        volumes: CodeVolumes,
        out_rate: u32,
        config: MixerConfig,
    ) -> Self {
        let out_rate = out_rate.max(1);
        if config.interpolation == Interpolation::Sinc {
            sinc::kernel();
        }
        Self {
            bank,
            volumes,
            voices: [Voice::FREE; VOICES],
            paused: false,
            out_rate,
            config,
            smooth_step: 1.0 / (out_rate as f32 * 0.005).max(1.0),
        }
    }

    pub fn out_rate(&self) -> u32 {
        self.out_rate
    }

    pub fn config(&self) -> MixerConfig {
        self.config
    }

    /// Changes the options; switching to [`Interpolation::Sinc`] loads the sounding voices'
    /// windows (their past as silence).
    pub fn set_config(&mut self, config: MixerConfig) {
        let load = config.interpolation == Interpolation::Sinc
            && self.config.interpolation != Interpolation::Sinc;
        self.config = config;
        if load {
            for v in self.voices.iter_mut().filter(|v| !v.is_free()) {
                v.refill_window(&self.bank, false);
            }
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// True if a voice holds `code` (including one draining its last half-buffer).
    pub fn is_playing(&self, code: u16) -> bool {
        code != 0 && self.find(code).is_some()
    }

    /// The occupied voices.
    pub fn voices(&self) -> impl Iterator<Item = VoiceInfo> + '_ {
        self.voices
            .iter()
            .enumerate()
            .filter(|(_, v)| !v.is_free())
            .map(|(slot, v)| VoiceInfo {
                slot,
                code: v.code,
                pitch: v.pitch,
                volume: v.volume,
                hz: v.hz,
                gain: v.gain,
                position: v.cursor,
                draining: v.stop_at.is_some(),
            })
    }

    fn find(&self, code: u16) -> Option<usize> {
        self.voices.iter().position(|v| v.code == code)
    }

    fn alloc(&self, code: u16) -> Option<usize> {
        let range = if crate::code::is_bgm(code) {
            SE_VOICES..VOICES
        } else {
            0..SE_VOICES
        };
        range.into_iter().find(|&i| self.voices[i].is_free())
    }

    fn native_rate(&self, source: Source) -> u32 {
        match source {
            Source::Effect(i) => self.bank.effects()[i].sample_rate,
            Source::Bgm(_) => 44_100,
        }
    }

    fn gain_for(&self, code: u16, v: i16) -> f32 {
        millibels_to_gain(volume_millibels(self.volumes.get(code), v))
    }

    pub fn apply(&mut self, cmd: Command) {
        match cmd {
            Command::Play(code) => self.play(code),
            Command::Release(code) => self.release(code),
            Command::StopAll => self.voices = [Voice::FREE; VOICES],
            Command::SetPitch(code, p) => self.set_pitch(code, p),
            Command::SetVolume(code, v) => self.set_volume(code, v),
            Command::Pause(on) => {
                // 0x430a00: Stop (or Play-looping again) on every active voice.
                self.paused = on;
                for v in self.voices.iter_mut().filter(|v| !v.is_free()) {
                    v.held = on;
                }
            }
        }
    }

    /// 0x430720: find the slot already playing `code`, else a free one of its kind.
    fn play(&mut self, code: u16) {
        let Some(source) = Source::of(code).filter(|_| self.bank.has(code)) else {
            return;
        };
        let Some(slot) = self.find(code).or_else(|| self.alloc(code)) else {
            return;
        };
        let hz = pitch_hz(code, 0, self.native_rate(source));
        let gain = self.gain_for(code, FULL_VOLUME);
        let len = data_len(&self.bank, source);
        let v = &mut self.voices[slot];
        let applied_gain = if v.code == code { v.applied_gain } else { gain };
        *v = Voice {
            code,
            source,
            pitch: 0,
            volume: FULL_VOLUME,
            hz: hz.clamp(FREQ_MIN as i64, FREQ_MAX as i64) as u32,
            gain,
            applied_gain: if self.config.smooth_volume {
                applied_gain
            } else {
                gain
            },
            ..Voice::FREE
        };
        v.restart_at(0, len);
        if self.config.interpolation == Interpolation::Sinc {
            v.refill_window(&self.bank, false);
        }
    }

    /// 0x430870: key off.
    fn release(&mut self, code: u16) {
        let Some(slot) = self.find(code).filter(|_| code != 0) else {
            return;
        };
        let v = &mut self.voices[slot];
        if let Source::Effect(i) = v.source {
            let fx: EffectInfo = self.bank.effects()[i];
            let le = fx.loop_end as usize;
            if le != 0 && le != fx.len() {
                // Still looping: jump to the release tail. Already in the tail: nothing.
                if v.cursor < le && v.stop_at.is_none() {
                    v.restart_at(le, fx.len());
                    if self.config.interpolation == Interpolation::Sinc {
                        v.refill_window(&self.bank, true);
                    }
                }
                return;
            }
        }
        *v = Voice::FREE;
    }

    /// 0x430960: only acts when the parameter changes; a frequency outside DirectSound's range
    /// is rejected (parameter stored, frequency kept).
    fn set_pitch(&mut self, code: u16, p: i16) {
        let Some(slot) = self.find(code).filter(|_| code != 0) else {
            return;
        };
        let rate = self.native_rate(self.voices[slot].source);
        let v = &mut self.voices[slot];
        if v.pitch == p {
            return;
        }
        v.pitch = p;
        let hz = pitch_hz(code, p, rate);
        if (FREQ_MIN as i64..=FREQ_MAX as i64).contains(&hz) {
            v.hz = hz as u32;
        }
    }

    /// 0x4309b0: only acts when the parameter changes.
    fn set_volume(&mut self, code: u16, vol: i16) {
        let Some(slot) = self.find(code).filter(|_| code != 0) else {
            return;
        };
        if self.voices[slot].volume == vol {
            return;
        }
        let gain = self.gain_for(code, vol);
        let v = &mut self.voices[slot];
        v.volume = vol;
        v.gain = gain;
        if !self.config.smooth_volume {
            v.applied_gain = gain;
        }
    }

    /// Mixes into interleaved stereo `out` (`out.len()` must be even; a trailing odd sample is
    /// left untouched).
    pub fn render(&mut self, out: &mut [f32]) {
        let bank = Arc::clone(&self.bank);
        let interp = self.config.interpolation;
        let kernel = (interp == Interpolation::Sinc).then(sinc::kernel);
        for frame in out.as_chunks_mut::<2>().0 {
            let (mut l, mut r) = (0.0f32, 0.0f32);
            {
                for v in self.voices.iter_mut().filter(|v| !v.is_free() && !v.held) {
                    if self.config.smooth_volume {
                        let d = v.gain - v.applied_gain;
                        v.applied_gain += d.clamp(-self.smooth_step, self.smooth_step);
                    }
                    let step = v.hz as f64 / self.out_rate as f64;
                    let (a, b) = match kernel {
                        Some(k) => Self::sample_sinc(k, v, step),
                        None => Self::sample(&bank, v, interp),
                    };
                    l += a * v.applied_gain;
                    r += b * v.applied_gain;
                    Self::advance(&bank, v, step, kernel.is_some());
                }
            }
            let m = self.config.master_volume;
            frame[0] = (l * m).clamp(-1.0, 1.0);
            frame[1] = (r * m).clamp(-1.0, 1.0);
        }
    }

    fn sample(bank: &SoundBank, v: &Voice, interp: Interpolation) -> (f32, f32) {
        if v.stop_at.is_some() {
            return (0.0, 0.0);
        }
        let a = frame_at(bank, v.source, v.cursor);
        if interp == Interpolation::Nearest || v.frac == 0.0 {
            return a;
        }
        let b = next_index(bank, v.source, v.cursor)
            .map(|n| frame_at(bank, v.source, n))
            .unwrap_or((0.0, 0.0));
        let t = v.frac as f32;
        (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
    }

    /// The band-limited sample at `cursor + frac` from the voice's window.
    fn sample_sinc(k: &sinc::Kernel, v: &Voice, step: f64) -> (f32, f32) {
        if v.stop_at.is_some() && v.ring_out == 0 {
            return (0.0, 0.0);
        }
        if step == 1.0 && v.frac == 0.0 {
            // No rate change: the samples themselves (the kernel would only low-pass them).
            let [l, r] = v.window.frames()[HISTORY - 1];
            return (l, r);
        }
        let c = k.coefficients(k.level_for(step), v.frac);
        match v.source {
            Source::Effect(_) => {
                let s = v.window.dot_mono(&c);
                (s, s)
            }
            Source::Bgm(_) => v.window.dot_stereo(&c),
        }
    }

    fn advance(bank: &SoundBank, v: &mut Voice, step: f64, window: bool) {
        v.frac += step;
        while v.frac >= 1.0 && !v.is_free() {
            v.frac -= 1.0;
            v.streamed += 1;
            if window {
                v.shift_window(bank);
            }
            match v.stop_at {
                Some(stop) => {
                    if v.streamed >= stop {
                        *v = Voice::FREE;
                    }
                }
                None => match next_index(bank, v.source, v.cursor) {
                    Some(n) => v.cursor = n,
                    None => v.data_ended(),
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bank::test_data::*;

    /// Effect codes: 1 = one-shot ramp (100 samples), 2 = looping (loop 10..20 of 30),
    /// 3 = loop to the end (loop_end == len), 4 = 0x1000-sample one-shot, 5..=0x31 = short,
    /// BGM 0x197 = 3 frames, 0x19e = looping 2 frames.
    fn bank() -> Arc<SoundBank> {
        let ramp: Vec<u8> = (0..100).map(|i| 128 + i as u8).collect();
        let mut fx = vec![
            effect(1000, (0, 0), &ramp),
            effect(1000, (10, 20), &[0xc0; 30]),
            effect(1000, (5, 8), &[0xc0; 8]),
            effect(1000, (0, 0), &vec![0xc0; 0x1000]),
        ];
        while fx.len() < 0x31 {
            fx.push(effect(22_050, (0, 0), &[0xc0; 4]));
        }
        let mut bgm = vec![bgm_frames(&[(16384, -16384); 3])];
        while bgm.len() < 8 {
            bgm.push(bgm_frames(&[(8192, 8192); 2]));
        }
        Arc::new(SoundBank::from_roms(rom2(&fx), rom2(&bgm)).unwrap())
    }

    /// Mixer at the effects' native rate (1000 Hz) so one output frame = one source sample.
    fn mixer() -> Mixer {
        Mixer::new(
            bank(),
            CodeVolumes::uniform(127),
            1000,
            MixerConfig::default(),
        )
    }

    fn render(m: &mut Mixer, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        m.render(&mut out);
        out
    }

    #[test]
    fn pitch_formula() {
        assert_eq!(pitch_hz(1, 0, 10_000), 10_000);
        assert_eq!(pitch_hz(1, 127, 10_000), 10_000 * 129 / 256);
        assert_eq!(pitch_hz(1, 64, 22_050), 16_537); // 22050·192/256 = 16537.5 → 16537
        // Code 0x31 is inverted.
        assert_eq!(pitch_hz(0x31, 127, 10_000), 10_000);
        assert_eq!(pitch_hz(0x31, 1, 10_000), 10_000 * 130 / 256);
        // BGM ignores the parameter.
        assert_eq!(pitch_hz(0x197, 100, 1), 44_100);
        // Negative products truncate toward zero like C (floor would give −1).
        assert_eq!(pitch_hz(1, 257, 255), 0);
        assert_eq!(pitch_hz(1, 257, 256), -1);
    }

    #[test]
    fn volume_curve() {
        assert_eq!(volume_millibels(127, 127), 0);
        assert_eq!(volume_millibels(127, 0), -10_000);
        assert_eq!(volume_millibels(127, 64), -4961); // 64/127 → −10000·(1 − 0.50394)
        assert_eq!(volume_millibels(0x74, 127), -867); // 116/127 → 9133.86 → 9133
        assert_eq!(volume_millibels(127, 200), 0); // clamped
        assert_eq!(millibels_to_gain(0), 1.0);
        assert_eq!(millibels_to_gain(-10_000), 0.0);
        assert!((millibels_to_gain(-2000) - 0.1).abs() < 1e-6);
    }

    #[test]
    fn one_shot_plays_and_frees_at_half_boundary() {
        let mut m = mixer();
        m.apply(Command::Play(1));
        let out = render(&mut m, 100);
        assert_eq!(out[0], 0.0);
        assert_eq!(out[2 * 50], 50.0 / 128.0);
        assert_eq!(out[2 * 50 + 1], out[2 * 50], "mono goes to both channels");
        // Data ended after 100 samples, but the slot drains to 0x1000.
        assert!(m.is_playing(1));
        assert!(m.voices().next().unwrap().draining);
        let tail = render(&mut m, 0x1000 - 101);
        assert!(tail.iter().all(|&s| s == 0.0));
        assert!(m.is_playing(1));
        render(&mut m, 1);
        assert!(!m.is_playing(1));
    }

    #[test]
    fn exact_half_length_frees_without_extra_half() {
        let mut m = mixer();
        m.apply(Command::Play(4));
        render(&mut m, 0x1000 - 1);
        assert!(m.is_playing(4));
        render(&mut m, 1);
        assert!(!m.is_playing(4));
    }

    #[test]
    fn loop_and_release() {
        let mut m = mixer();
        m.apply(Command::Play(2));
        render(&mut m, 25);
        // 0..20, then 10..: after 25 samples the cursor is at 15.
        assert_eq!(m.voices().next().unwrap().position, 15);
        render(&mut m, 1000);
        assert!(m.is_playing(2));
        let pos = m.voices().next().unwrap().position;
        assert!((10..20).contains(&pos));
        m.apply(Command::Release(2));
        assert_eq!(m.voices().next().unwrap().position, 20);
        // A second release in the tail does nothing.
        m.apply(Command::Release(2));
        assert_eq!(m.voices().next().unwrap().position, 20);
        // Tail of 10 samples, then drain to 0x1000 counted from the release restart.
        render(&mut m, 0x1000 - 1);
        assert!(m.is_playing(2));
        render(&mut m, 1);
        assert!(!m.is_playing(2));
    }

    #[test]
    fn release_of_one_shot_and_loop_to_end_stops() {
        let mut m = mixer();
        m.apply(Command::Play(1));
        m.apply(Command::Play(3));
        m.apply(Command::Release(1));
        m.apply(Command::Release(3));
        assert_eq!(m.voices().count(), 0);
    }

    #[test]
    fn retrigger_reuses_slot_and_resets_params() {
        let mut m = mixer();
        m.apply(Command::Play(1));
        m.apply(Command::SetPitch(1, 64));
        m.apply(Command::SetVolume(1, 10));
        render(&mut m, 30);
        m.apply(Command::Play(1));
        let v: Vec<_> = m.voices().collect();
        assert_eq!(v.len(), 1);
        assert_eq!(
            (v[0].position, v[0].pitch, v[0].volume, v[0].hz),
            (0, 0, 0x7f, 1000)
        );
    }

    #[test]
    fn slot_limits_without_stealing() {
        let mut m = mixer();
        for code in 5..15 {
            m.apply(Command::Play(code));
        }
        let codes: Vec<u16> = m.voices().map(|v| v.code).collect();
        assert_eq!(
            codes,
            (5..13).collect::<Vec<_>>(),
            "8 effect slots, rest dropped"
        );
        for code in [0x197, 0x198, 0x199] {
            m.apply(Command::Play(code));
        }
        let bgm: Vec<_> = m.voices().filter(|v| v.slot >= 8).map(|v| v.code).collect();
        assert_eq!(bgm, [0x197, 0x198]);
        m.apply(Command::StopAll);
        assert_eq!(m.voices().count(), 0);
    }

    #[test]
    fn invalid_codes_are_ignored() {
        let mut m = mixer();
        for c in [0, 0x1a9, 0x1000, 0x31 + 100] {
            m.apply(Command::Play(c));
        }
        // 0x95 (= 0x31 + 100) is a valid effect code but absent from the small test bank.
        assert_eq!(m.voices().count(), 0);
        m.apply(Command::Release(0));
        m.apply(Command::SetPitch(0, 3));
    }

    #[test]
    fn pitch_and_volume_apply_on_change() {
        let mut m = mixer();
        m.apply(Command::Play(0x31));
        assert_eq!(m.voices().next().unwrap().hz, 22_050 * 129 / 256);
        m.apply(Command::SetPitch(0x31, 127));
        assert_eq!(m.voices().next().unwrap().hz, 22_050);
        m.apply(Command::SetVolume(0x31, 0));
        let v = m.voices().next().unwrap();
        assert_eq!((v.volume, v.gain), (0, 0.0));
        // Out-of-range frequency: parameter stored, frequency kept.
        m.apply(Command::SetPitch(0x31, -30_000));
        let v = m.voices().next().unwrap();
        assert_eq!((v.pitch, v.hz), (-30_000, 22_050));
    }

    #[test]
    fn resampling_rate() {
        // Code 5 is 22050 Hz with 4 samples; at 44100 out it lasts 8 frames, then silence.
        let mut m = Mixer::new(
            bank(),
            CodeVolumes::uniform(127),
            44_100,
            MixerConfig::default(),
        );
        m.apply(Command::Play(5));
        let out = render(&mut m, 10);
        assert!(out[..14].iter().all(|&s| s == 0.5), "{out:?}");
        assert_eq!(out[16], 0.0);
        // Linear interpolation towards silence on the last sample.
        assert_eq!(out[14], 0.25);
    }

    #[test]
    fn bgm_stereo_loop_and_one_shot() {
        let mut m = Mixer::new(
            bank(),
            CodeVolumes::uniform(127),
            44_100,
            MixerConfig::default(),
        );
        m.apply(Command::Play(0x197));
        m.apply(Command::Play(0x19e));
        let out = render(&mut m, 2);
        assert_eq!((out[0], out[1]), (0.5 + 0.25, -0.5 + 0.25));
        render(&mut m, 0x2000);
        // 0x197 drained after one BGM half (0x2000 frames); 0x19e loops forever.
        assert!(!m.is_playing(0x197));
        assert!(m.is_playing(0x19e));
        assert_eq!(render(&mut m, 1)[0], 0.25);
        m.apply(Command::Release(0x19e));
        assert!(!m.is_playing(0x19e));
    }

    #[test]
    fn pause_holds_voices() {
        let mut m = mixer();
        m.apply(Command::Play(1));
        render(&mut m, 10);
        m.apply(Command::Pause(true));
        let out = render(&mut m, 50);
        assert!(out.iter().all(|&s| s == 0.0));
        assert_eq!(m.voices().next().unwrap().position, 10);
        m.apply(Command::Pause(false));
        assert_eq!(render(&mut m, 1)[0], 10.0 / 128.0);
        // A voice started while paused (the pause dialog's clicks) plays; the held ones don't.
        m.apply(Command::Pause(true));
        m.apply(Command::Play(2));
        assert!(render(&mut m, 5).iter().any(|&s| s != 0.0));
        assert_eq!(m.voices().find(|v| v.code == 1).unwrap().position, 11);
    }

    #[test]
    fn deterministic_and_clamped() {
        let run = || {
            let mut m = mixer();
            for c in [1, 2, 0x31, 0x197] {
                m.apply(Command::Play(c));
            }
            m.apply(Command::SetPitch(2, 50));
            render(&mut m, 500)
        };
        let a = run();
        assert_eq!(a, run());
        assert!(a.iter().all(|s| (-1.0..=1.0).contains(s)));
    }

    #[test]
    fn smoothing_ramps_volume() {
        let cfg = MixerConfig {
            smooth_volume: true,
            ..MixerConfig::default()
        };
        let mut m = Mixer::new(bank(), CodeVolumes::uniform(127), 1000, cfg);
        m.apply(Command::Play(2));
        render(&mut m, 1);
        m.apply(Command::SetVolume(2, 0));
        // 5 ms at 1 kHz = 5 frames to ramp from 1 to 0.
        let out = render(&mut m, 6);
        assert!(out[0] > 0.0 && out[0] < 0.5);
        assert!(out[10].abs() < 1e-6);
    }

    #[test]
    fn master_volume() {
        let cfg = MixerConfig {
            master_volume: 0.5,
            ..MixerConfig::default()
        };
        let mut m = Mixer::new(bank(), CodeVolumes::uniform(127), 1000, cfg);
        m.apply(Command::Play(2));
        assert_eq!(render(&mut m, 1)[0], 0.25);
    }

    // --- Interpolation::Sinc -------------------------------------------------------------

    fn sinc_cfg() -> MixerConfig {
        MixerConfig {
            interpolation: Interpolation::Sinc,
            ..MixerConfig::default()
        }
    }

    /// A bank whose effect `code` (1-based) is `pcm` at `rate`, looping `loop_`; every other
    /// effect is a short one-shot.
    fn bank_with(code: u16, rate: u32, loop_: (u32, u32), pcm: &[u8]) -> Arc<SoundBank> {
        let mut fx: Vec<Vec<u8>> = (1..code)
            .map(|_| effect(22_050, (0, 0), &[0x80; 4]))
            .collect();
        fx.push(effect(rate, loop_, pcm));
        let bgm = vec![bgm_frames(&[(0, 0); 2])];
        Arc::new(SoundBank::from_roms(rom2(&fx), rom2(&bgm)).unwrap())
    }

    /// `len` u8 samples of a sine of `period` samples (any real), amplitude `amp` / 128.
    fn sine_pcm(len: usize, period: f64, amp: f64) -> Vec<u8> {
        (0..len)
            .map(|i| {
                let s = (2.0 * std::f64::consts::PI * i as f64 / period).sin();
                (128.0 + amp * s).round() as u8
            })
            .collect()
    }

    /// Left channel of interleaved stereo.
    fn left(out: &[f32]) -> Vec<f32> {
        out.iter().step_by(2).copied().collect()
    }

    /// Amplitude of the component at `f` Hz (Hann-windowed DFT bin).
    fn tone(x: &[f32], fs: f64, f: f64) -> f64 {
        let n = x.len() as f64;
        let (mut re, mut im, mut wsum) = (0.0, 0.0, 0.0);
        for (i, &s) in x.iter().enumerate() {
            let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n).cos();
            let a = 2.0 * std::f64::consts::PI * f * i as f64 / fs;
            re += s as f64 * w * a.cos();
            im += s as f64 * w * a.sin();
            wsum += w;
        }
        2.0 * (re * re + im * im).sqrt() / wsum
    }

    fn render_mode(bank: Arc<SoundBank>, rate: u32, interp: Interpolation, code: u16) -> Mixer {
        let cfg = MixerConfig {
            interpolation: interp,
            ..MixerConfig::default()
        };
        let mut m = Mixer::new(bank, CodeVolumes::uniform(127), rate, cfg);
        m.apply(Command::Play(code));
        m
    }

    #[test]
    fn sinc_pitched_up_does_not_alias() {
        // 16 kHz source, 8 kHz output: step 2. A 7013 Hz tone is above the output's Nyquist
        // and must vanish instead of folding to 987 Hz; a 2003 Hz tone passes. (Frequencies
        // incommensurate with the rates, so the u8 rounding error is noise, not a line at the
        // alias frequency.)
        let run = |interp, f_in: f64, f_measure: f64| {
            let pcm = sine_pcm(16_000, 16_000.0 / f_in, 100.0);
            let bank = bank_with(1, 16_000, (0, 0), &pcm);
            let mut m = render_mode(bank, 8000, interp, 1);
            render(&mut m, 800);
            let x = left(&render(&mut m, 4000));
            tone(&x, 8000.0, f_measure)
        };
        let amp = 100.0 / 128.0;
        let sinc = run(Interpolation::Sinc, 7013.0, 987.0);
        let linear = run(Interpolation::Linear, 7013.0, 987.0);
        let nearest = run(Interpolation::Nearest, 7013.0, 987.0);
        let db = |a: f64| 20.0 * (a / amp).log10();
        println!(
            "alias at 987 Hz: sinc {:.1} dB, linear {:.1} dB, nearest {:.1} dB",
            db(sinc),
            db(linear),
            db(nearest)
        );
        assert!(db(sinc) < -60.0, "sinc alias {:.1} dB", db(sinc));
        assert!(
            db(linear) > -30.0 && db(nearest) > -30.0,
            "{linear} {nearest}"
        );
        let pass = db(run(Interpolation::Sinc, 2003.0, 2003.0));
        assert!(pass.abs() < 0.3, "passband {pass:.2} dB");
    }

    #[test]
    fn sinc_upsampling_has_no_images() {
        // A 10 kHz voice at 48 kHz: a 4.5 kHz tone images at 5.5 kHz unless filtered.
        let image = |interp| {
            let pcm = sine_pcm(10_000, 10.0 / 4.5, 100.0);
            let bank = bank_with(1, 10_000, (0, 10_000), &pcm);
            let mut m = render_mode(bank, 48_000, interp, 1);
            render(&mut m, 4800);
            let x = left(&render(&mut m, 48_000));
            (tone(&x, 48_000.0, 4500.0), tone(&x, 48_000.0, 5500.0))
        };
        let (t, i) = image(Interpolation::Sinc);
        let rel = 20.0 * (i / t).log10();
        assert!(rel < -60.0, "sinc image {rel:.1} dB");
        let (t, i) = image(Interpolation::Linear);
        let rel_linear = 20.0 * (i / t).log10();
        println!("image at 5.5 kHz: sinc {rel:.1} dB, linear {rel_linear:.1} dB");
        assert!(rel_linear > -10.0, "linear image {rel_linear:.1} dB");
    }

    #[test]
    fn sinc_dc_has_unity_gain() {
        // Code 2: a constant 0xc0 (0.5) looping 10..20. Upsampling, bypass and downsampling.
        for rate in [1370, 1000, 700, 260] {
            let mut m = Mixer::new(bank(), CodeVolumes::uniform(127), rate, sinc_cfg());
            m.apply(Command::Play(2));
            render(&mut m, 60);
            let out = render(&mut m, 500);
            for s in out {
                assert!((s - 0.5).abs() < 2e-4, "rate {rate}: {s}");
            }
        }
    }

    #[test]
    fn sinc_reads_across_the_loop_seam() {
        // A sine of period 20 looping 20..120 (5 periods): the output must be the unbroken
        // sine through every seam, up to the u8 quantisation.
        let pcm = sine_pcm(140, 20.0, 100.0);
        let bank = bank_with(1, 1000, (20, 120), &pcm);
        let mut m = render_mode(bank, 1370, Interpolation::Sinc, 1);
        let step = 1000.0 / 1370.0;
        let x = left(&render(&mut m, 700));
        let mut worst: f64 = 0.0;
        for (n, &y) in x.iter().enumerate().skip(30) {
            let pos = n as f64 * step;
            let want = 100.0 / 128.0 * (2.0 * std::f64::consts::PI * pos / 20.0).sin();
            worst = worst.max((y as f64 - want).abs());
        }
        // Positions 22..511: four seams crossed.
        assert!(worst < 0.008, "max error {worst}");
    }

    #[test]
    fn sinc_pitch_sweep_is_continuous() {
        // Code 0x31 (inverted pitch): an 800 Hz sine at 16 kHz into 12 kHz, swept from 1.33×
        // the output rate (downsampling, levels 1–2) to 0.68× and back.
        let pcm = sine_pcm(16_000, 20.0, 100.0);
        let bank = bank_with(0x31, 16_000, (0, 16_000), &pcm);
        let mut m = render_mode(bank, 12_000, Interpolation::Sinc, 0x31);
        m.apply(Command::SetPitch(0x31, 127));
        render(&mut m, 100);
        let mut x = Vec::new();
        for p in (1..=127).rev().chain(1..=127) {
            m.apply(Command::SetPitch(0x31, p));
            x.extend(left(&render(&mut m, 37)));
        }
        // The steepest slope of the fastest tone (800 Hz at 12 kHz), plus some noise.
        let amp = 100.0 / 128.0;
        let bound = 2.0 * std::f64::consts::PI * 800.0 / 12_000.0 * amp * 1.05 + 0.01;
        let worst = x
            .windows(2)
            .map(|w| (w[1] - w[0]).abs() as f64)
            .fold(0.0, f64::max);
        assert!(worst < bound, "largest step {worst:.4} > {bound:.4}");
        // The level stays full: no dropout at a level switch.
        let min_peak = x
            .chunks(400)
            .map(|c| c.iter().fold(0f32, |a, s| a.max(s.abs())))
            .fold(f32::MAX, f32::min);
        assert!(min_peak > 0.7, "{min_peak}");
    }

    #[test]
    fn sinc_one_shot_rings_out_and_drains() {
        let mut m = Mixer::new(bank(), CodeVolumes::uniform(127), 1000, sinc_cfg());
        m.apply(Command::Play(1));
        let out = left(&render(&mut m, 140));
        // Step 1: the samples themselves (no filtering without a rate change).
        assert_eq!(out[50], 50.0 / 128.0);
        assert!(out[110..].iter().all(|&s| s == 0.0));
        // At a fractional step the ramp's end rings for at most 15 source samples.
        let mut m = Mixer::new(bank(), CodeVolumes::uniform(127), 1370, sinc_cfg());
        m.apply(Command::Play(1));
        let out = left(&render(&mut m, 0x1400));
        let last = out.iter().rposition(|&s| s != 0.0).unwrap();
        let end = (100.0 * 1.37) as usize;
        assert!(last > end && last < end + 23, "{last}");
        assert!(out.iter().all(|s| s.is_finite()));
        assert!(m.is_playing(1), "still draining the half-buffer");
        render(&mut m, 0x800);
        assert!(!m.is_playing(1));
    }

    #[test]
    fn sinc_release_and_switching_modes() {
        // Release from the loop into the tail, and turning sinc on mid-sound.
        let mut m = Mixer::new(bank(), CodeVolumes::uniform(127), 1370, sinc_cfg());
        m.apply(Command::Play(2));
        render(&mut m, 100);
        m.apply(Command::Release(2));
        // The tail is 10 samples, so the end of the data already rings in the kernel.
        let out = left(&render(&mut m, 5));
        assert!(out.iter().all(|s| (s - 0.5).abs() < 0.02), "{out:?}");
        let mut m = mixer();
        m.apply(Command::Play(2));
        render(&mut m, 30);
        m.set_config(sinc_cfg());
        let out = render(&mut m, 100);
        assert!(
            out.iter()
                .all(|s| s.is_finite() && (-1.0..=1.0).contains(s))
        );
        assert!((out[198] - 0.5).abs() < 1e-3);
    }

    #[test]
    fn sinc_deterministic() {
        let run = || {
            let mut m = Mixer::new(bank(), CodeVolumes::uniform(127), 1370, sinc_cfg());
            for c in [1, 2, 0x31, 0x197] {
                m.apply(Command::Play(c));
            }
            m.apply(Command::SetPitch(2, 50));
            render(&mut m, 500)
        };
        assert_eq!(run(), run());
    }

    /// CPU cost: `cargo test --release -p ddg-audio cpu_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn cpu_cost() {
        let pcm = sine_pcm(40_000, 37.3, 100.0);
        let mut fx: Vec<Vec<u8>> = Vec::new();
        for i in 0..8u32 {
            fx.push(effect(10_000 + 4000 * i, (0, 40_000), &pcm));
        }
        let frames: Vec<(i16, i16)> = (0..44_100)
            .map(|i| ((i % 30_000) as i16, -((i % 30_000) as i16)))
            .collect();
        let bgm = vec![bgm_frames(&frames), bgm_frames(&frames)];
        let mut bgm_rom = bgm.clone();
        while bgm_rom.len() < 8 {
            bgm_rom.push(bgm_frames(&[(0, 0); 2]));
        }
        let bank = Arc::new(SoundBank::from_roms(rom2(&fx), rom2(&bgm_rom)).unwrap());
        let seconds = 20;
        for interp in Interpolation::ALL {
            let cfg = MixerConfig {
                interpolation: interp,
                ..MixerConfig::default()
            };
            let mut m = Mixer::new(bank.clone(), CodeVolumes::uniform(127), 48_000, cfg);
            for c in (1..=8).chain([0x197, 0x198]) {
                m.apply(Command::Play(c));
            }
            assert_eq!(m.voices().count(), 10);
            let mut buf = vec![0f32; 2 * 480];
            let t = std::time::Instant::now();
            for block in 0..100 * seconds {
                // Pitch moves every 10 ms, like the motor sounds.
                m.apply(Command::SetPitch(
                    1 + (block % 8) as u16,
                    (block % 120) as i16,
                ));
                m.render(&mut buf);
            }
            let dt = t.elapsed().as_secs_f64();
            println!(
                "{:>8}: {:.1} ms per s of audio ({:.2} % of one core), 10 voices at 48 kHz",
                interp.name(),
                dt * 1000.0 / seconds as f64,
                dt * 100.0 / seconds as f64
            );
        }
    }
}

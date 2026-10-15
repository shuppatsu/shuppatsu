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

/// Sample interpolation of the resampler.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Interpolation {
    /// Sample-and-hold.
    Nearest,
    /// Linear between neighbouring source samples (default).
    #[default]
    Linear,
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
        if self.streamed >= halves * half {
            *self = Self::FREE;
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

    pub fn set_config(&mut self, config: MixerConfig) {
        self.config = config;
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
            Command::Pause(on) => self.paused = on,
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
        for frame in out.as_chunks_mut::<2>().0 {
            let (mut l, mut r) = (0.0f32, 0.0f32);
            if !self.paused {
                for v in self.voices.iter_mut().filter(|v| !v.is_free()) {
                    if self.config.smooth_volume {
                        let d = v.gain - v.applied_gain;
                        v.applied_gain += d.clamp(-self.smooth_step, self.smooth_step);
                    }
                    let (a, b) = Self::sample(&bank, v, self.config.interpolation);
                    l += a * v.applied_gain;
                    r += b * v.applied_gain;
                    let step = v.hz as f64 / self.out_rate as f64;
                    Self::advance(&bank, v, step);
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

    fn advance(bank: &SoundBank, v: &mut Voice, step: f64) {
        v.frac += step;
        while v.frac >= 1.0 && !v.is_free() {
            v.frac -= 1.0;
            v.streamed += 1;
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
}

//! The Komachi movie (`movie_play_komachi` 0x429110, docs/re/movie.md): `DATA\Komachi.DAT`, an
//! MPEG-1 program stream, decoded by our own decoders ([`shuppatsu_formats::mpeg`]) and shown on its
//! own screen.
//!
//! The original played it through DirectShow (`AMMultiMediaStream`): the audio went straight
//! to the default renderer, and each video sample was stretch-blitted to a fixed 640×288
//! rectangle centred horizontally, 96 px from the top of the 640×480 screen (0x428f00: twice
//! the 320×144 picture, the sample aspect ignored), until the stream ended or the confirm edge
//! (0x1000, Enter) skipped it. Here the soundtrack goes to the mixer as a [`PcmStream`] (unity
//! gain, resampled by `audio_resampler`), whose played-frame count is the clock the pictures
//! follow; without a sound device the clock is the app's ticks.
//!
//! Where it goes on screen ([`movie_rect`]): with `hud_layout original` the original's
//! rectangle on the centred 640×480 design, scaled with it; with `enhanced` as large as the
//! window allows, always at the stream's display aspect (sample aspect 200:219, 2.03:1; the
//! original's 2:1 doubling showed 2.22:1, a deviation decided by the user).
//! `movie_scaling` picks the filter.

use anyhow::{Context, Result};
use shuppatsu_audio::PcmStream;
use shuppatsu_formats::mpeg::{Frame, Movie, Mp2Stream, VideoDecoder};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The original's movie rectangle on the 640×480 screen (0x428f00, resolution mode 1:
/// `(0, 0x60)` plus twice the 320×144 sample; modes 2 and 3 centre the same 640×288).
pub const ORIGINAL_RECT: [f32; 4] = [0.0, 96.0, 640.0, 288.0];

/// Audio queued ahead of the play position, in seconds.
const AUDIO_AHEAD: f64 = 0.5;

/// How the movie is scaled to its rectangle (setting `movie_scaling`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MovieScaling {
    /// Plain bilinear: suits the low-resolution live-action picture (the default).
    #[default]
    Smooth,
    /// Sharp bilinear, like the UI's `smooth`: crisp square pixels, blended edges.
    Sharp,
    /// Nearest neighbour.
    Nearest,
}

impl MovieScaling {
    pub const ALL: [Self; 3] = [Self::Smooth, Self::Sharp, Self::Nearest];

    pub fn name(self) -> &'static str {
        match self {
            Self::Smooth => "smooth",
            Self::Sharp => "sharp",
            Self::Nearest => "nearest",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        Self::ALL.into_iter().find(|m| m.name() == s)
    }

    /// Sets an overlay up for this filter.
    pub fn configure(self, o: &mut shuppatsu_render::overlay::Overlay) {
        use shuppatsu_render::overlay::UiScaling;
        o.bilinear = self == Self::Smooth;
        o.scaling = match self {
            Self::Nearest => UiScaling::Nearest,
            Self::Smooth | Self::Sharp => UiScaling::Smooth,
        };
    }
}

/// `DATA\Komachi.DAT` in the data directory, whatever the case of its name.
pub fn movie_path(data_dir: &Path) -> Option<PathBuf> {
    let direct = data_dir.join("komachi.dat");
    if direct.is_file() {
        return Some(direct);
    }
    std::fs::read_dir(data_dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("komachi.dat"))
        })
}

/// Where the movie goes in a `window`-sized target (x, y, w, h in pixels). `design` is the
/// presented 640×480 design rectangle (the original layout's reference), `dar` the picture's
/// display aspect.
pub fn movie_rect(window: (u32, u32), design: [f32; 4], enhanced: bool, dar: f32) -> [f32; 4] {
    if enhanced {
        let (ww, wh) = (window.0 as f32, window.1 as f32);
        let (w, h) = if ww / wh > dar {
            (wh * dar, wh)
        } else {
            (ww, ww / dar)
        };
        return [(ww - w) / 2.0, (wh - h) / 2.0, w, h];
    }
    let s = design[3] / 480.0;
    let h = ORIGINAL_RECT[3] * s;
    let w = h * dar;
    [
        design[0] + (design[2] - w) / 2.0,
        design[1] + ORIGINAL_RECT[1] * s,
        w,
        h,
    ]
}

/// The decoders, the clock and the picture on show.
pub struct MoviePlayer {
    video: VideoDecoder,
    audio: Option<Mp2Stream>,
    stream: Option<Arc<PcmStream>>,
    width: usize,
    height: usize,
    sample_aspect: f64,
    /// Seconds per frame.
    period: f64,
    /// Audio PTS minus video PTS of the first units, in seconds (0 for komachi.dat).
    offset: f64,
    /// The clock without a sound device (seconds since the start).
    ticks_clock: f64,
    /// The frame decoded next and its display index.
    next: Option<Frame>,
    next_index: u64,
    shown: Option<u64>,
    rgba: Vec<u8>,
    video_done: bool,
    started: std::time::Instant,
}

impl MoviePlayer {
    /// Opens the movie in `data_dir`.
    pub fn open(data_dir: &Path) -> Result<Self> {
        let path = movie_path(data_dir).context("Komachi.DAT not found in the data directory")?;
        let bytes = std::fs::read(&path).with_context(|| path.display().to_string())?;
        Self::from_bytes(&bytes).with_context(|| path.display().to_string())
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let Movie { mut video, audio } = Movie::open(bytes)?;
        let seq = video.read_sequence_header()?.clone();
        let (num, den) = seq.frame_rate();
        let mut player = Self {
            width: usize::from(seq.width),
            height: usize::from(seq.height),
            sample_aspect: seq.sample_aspect(),
            period: f64::from(den) / f64::from(num.max(1)),
            offset: 0.0,
            video,
            audio,
            stream: None,
            ticks_clock: 0.0,
            next: None,
            next_index: 0,
            shown: None,
            rgba: Vec::new(),
            video_done: false,
            started: std::time::Instant::now(),
        };
        player.decode_next();
        if let (Some(v), Some(a)) = (
            player.next.as_ref().and_then(|f| f.pts),
            player.audio.as_ref().and_then(Mp2Stream::first_pts),
        ) {
            player.offset = (a as f64 - v as f64) / shuppatsu_formats::mpeg::ps::CLOCK_HZ as f64;
        }
        Ok(player)
    }

    /// Plays the soundtrack on `stream` (made at the stream's rate, [`Self::audio_rate`]) and
    /// follows its clock.
    pub fn attach(&mut self, stream: Arc<PcmStream>) {
        self.stream = Some(stream);
        self.feed_audio();
    }

    /// The soundtrack's sample rate (44.1 kHz for komachi.dat).
    pub fn audio_rate(&self) -> Option<u32> {
        self.audio
            .as_ref()
            .and_then(|a| a.first_header().ok())
            .map(|h| h.sample_rate)
    }

    /// The picture's display aspect.
    pub fn display_aspect(&self) -> f32 {
        (self.width as f64 * self.sample_aspect / self.height.max(1) as f64) as f32
    }

    pub fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// Seconds since the start: the sound as played, or the ticks without a device.
    pub fn clock(&self) -> f64 {
        match &self.stream {
            Some(s) => s.played() as f64 / f64::from(s.rate()) + self.offset,
            None => self.ticks_clock,
        }
    }

    /// Advances the clock without a sound device (one app tick).
    pub fn advance(&mut self, seconds: f64) {
        if self.stream.is_none() {
            self.ticks_clock += seconds;
        }
    }

    /// Keeps about half a second of sound queued.
    pub fn feed_audio(&mut self) {
        let (Some(stream), Some(audio)) = (&self.stream, self.audio.as_mut()) else {
            return;
        };
        let want = (AUDIO_AHEAD * f64::from(stream.rate())) as usize;
        while stream.queued() < want {
            match audio.next_frame() {
                Ok(Some(f)) => stream.push(&f.samples, f.channels),
                Ok(None) => {
                    stream.finish();
                    self.audio = None;
                    return;
                }
                // A damaged frame is dropped; the stream carries on at the next one.
                Err(e) => eprintln!("movie: audio: {e}"),
            }
        }
    }

    fn decode_next(&mut self) {
        loop {
            match self.video.next_frame() {
                Ok(f) => {
                    self.video_done = f.is_none();
                    self.next = f;
                    return;
                }
                Err(e) => eprintln!("movie: video: {e}"),
            }
        }
    }

    /// Moves the picture to the clock; `true` when a new one is ready in [`Self::rgba`].
    pub fn update(&mut self) -> bool {
        self.feed_audio();
        let now = self.clock();
        let mut ready = None;
        while self.next.is_some() && self.next_index as f64 * self.period <= now {
            let f = self.next.take().expect("checked");
            if let Some(old) = ready.replace(f) {
                self.video.recycle(old);
            }
            self.shown = Some(self.next_index);
            self.next_index += 1;
            self.decode_next();
        }
        match ready {
            Some(f) => {
                f.to_rgba(&mut self.rgba);
                self.video.recycle(f);
                true
            }
            None => false,
        }
    }

    /// The picture on show, RGBA8 of [`Self::size`] (empty before the first).
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }

    /// The last picture has had its time.
    pub fn finished(&self) -> bool {
        self.video_done
            && self.next.is_none()
            && self.clock() >= self.next_index as f64 * self.period
    }

    /// The clock against the wall time since the start, for the log.
    pub fn timing(&self) -> String {
        format!(
            "clock {:.3} s after {:.3} s, picture {:?}",
            self.clock(),
            self.started.elapsed().as_secs_f64(),
            self.shown
        )
    }

    /// The display index of the picture on show.
    pub fn shown(&self) -> Option<u64> {
        self.shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_layout_is_the_blit_rectangle() {
        // 640×480 window, design = whole window: the original's 640×288 at y 96 at 2:1.
        let r = movie_rect((640, 480), [0.0, 0.0, 640.0, 480.0], false, 640.0 / 288.0);
        assert_eq!(r, [0.0, 96.0, 640.0, 288.0]);
        // The stream's aspect keeps the height and position, narrower.
        let dar = 320.0 * (200.0 / 219.0) / 144.0;
        let r = movie_rect((640, 480), [0.0, 0.0, 640.0, 480.0], false, dar);
        assert!((r[2] - 288.0 * dar).abs() < 1e-3 && r[1] == 96.0 && r[3] == 288.0);
        assert!((r[0] - (640.0 - r[2]) / 2.0).abs() < 1e-3);
        // 2560×1440: the design 1920×1440 centred, ×3.
        let r = movie_rect(
            (2560, 1440),
            [320.0, 0.0, 1920.0, 1440.0],
            false,
            640.0 / 288.0,
        );
        for (a, b) in r.iter().zip([320.0, 288.0, 1920.0, 864.0]) {
            assert!((a - b).abs() < 1e-3, "{r:?}");
        }
    }

    #[test]
    fn enhanced_layout_fills_the_window() {
        let dar = 2.03;
        let r = movie_rect((2560, 1440), [320.0, 0.0, 1920.0, 1440.0], true, dar);
        assert_eq!((r[0], r[2]), (0.0, 2560.0));
        assert!((r[3] - 2560.0 / dar).abs() < 1e-3);
        // A window wider than the picture: full height, centred.
        let r = movie_rect((3440, 1440), [0.0; 4], true, dar);
        assert_eq!((r[1], r[3]), (0.0, 1440.0));
        assert!((r[0] * 2.0 + r[2] - 3440.0).abs() < 1e-3);
    }

    #[test]
    fn scaling_names_round_trip() {
        for m in MovieScaling::ALL {
            assert_eq!(MovieScaling::parse(m.name()), Some(m));
        }
        assert_eq!(MovieScaling::default(), MovieScaling::Smooth);
        assert_eq!(MovieScaling::parse("bicubic"), None);
    }

    /// The real movie through the player without a device: frames follow the tick clock.
    /// Needs `SHUPPATSU_DATA_DIR`.
    #[test]
    fn plays_komachi_on_the_tick_clock() {
        let Ok(data) = crate::scene::GameData::from_env() else {
            eprintln!("skipped: SHUPPATSU_DATA_DIR not set");
            return;
        };
        let mut p = MoviePlayer::open(data.data_dir()).unwrap();
        assert_eq!(p.size(), (320, 144));
        assert!((p.display_aspect() - 2.029).abs() < 0.001);
        assert!(p.update(), "the first picture at once");
        assert_eq!(p.shown(), Some(0));
        assert_eq!(p.rgba().len(), 320 * 144 * 4);
        // One second: frame 29 (29 · 1001/30000 = 0.968 s ≤ 1 < 30 · …).
        p.advance(1.0);
        assert!(p.update());
        assert_eq!(p.shown(), Some(29));
        assert!(!p.finished());
    }
}

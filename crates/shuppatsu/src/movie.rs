//! The Komachi movie (`movie_play_komachi` 0x429110, docs/re/movie.md): `DATA\Komachi.DAT`, an
//! MPEG-1 program stream, decoded by our own decoders ([`shuppatsu_formats::mpeg`]) and shown on its
//! own screen.
//!
//! The original played it through DirectShow (`AMMultiMediaStream`): the audio went straight
//! to the default renderer, and each video sample was stretch-blitted to a fixed 640×288
//! rectangle centred horizontally, 96 px from the top of the 640×480 screen (0x428f00: twice
//! the 320×144 picture, the sample aspect ignored), until the stream ended or the confirm edge
//! (0x1000, Enter) skipped it. Here the soundtrack goes to the mixer as a [`PcmStream`] (unity
//! gain, resampled by `audio_resampler`), whose played-frame count, read smoothly between audio
//! callbacks, is the clock the pictures follow; without a sound device the clock is the app's
//! ticks. Each presented display frame shows the picture due at its refresh ([`FrameGrid`]),
//! uploaded once when it changes (docs/re/movie.md §6.1).
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
use std::time::Instant;

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

/// The display index of the picture due at `clock` seconds, `period` seconds per picture:
/// the last whose time (`n · period`) has come; `None` before the first.
pub fn picture_at(clock: f64, period: f64) -> Option<u64> {
    // The tolerance keeps `n · period` itself on picture n despite rounding.
    (clock >= 0.0 && period > 0.0).then(|| (clock / period + 1e-9).floor() as u64)
}

/// The display's refresh grid, from the times of the redraws: each present is shown on a
/// refresh, evenly spaced, but the redraw that renders it runs at a jittery moment before
/// (±2 ms measured at 60 Hz). [`FrameGrid::sample`] numbers the redraws by refresh and fits
/// a line through all of them (least squares, so the error shrinks as they add up); a redraw
/// gets its refresh's time on that line. One more than half a period off the line (a stall,
/// a resize, a new refresh rate) starts a new line there; skipped refreshes are only steps.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameGrid {
    /// The first redraw of the line, seconds; `None` before any.
    origin: Option<f64>,
    /// The refresh number of the last redraw.
    last: f64,
    /// Sums over the line's redraws of 1, n, n², t, n·t (t from `origin`).
    sums: [f64; 5],
}

impl FrameGrid {
    /// The line `(t at refresh 0 from origin, period)`, from two redraws on.
    fn line(&self) -> Option<(f64, f64)> {
        let [k, sn, snn, st, snt] = self.sums;
        let d = k * snn - sn * sn;
        (k >= 2.0 && d > 0.0).then(|| {
            let b = (k * snt - sn * st) / d;
            ((st - b * sn) / k, b)
        })
    }

    fn add(&mut self, n: f64, t: f64) {
        self.last = n;
        for (s, v) in self.sums.iter_mut().zip([1.0, n, n * n, t, n * t]) {
            *s += v;
        }
    }

    fn restart(&mut self, now: f64) {
        *self = Self {
            origin: Some(now),
            ..Self::default()
        };
        self.add(0.0, 0.0);
    }

    /// The grid time of a redraw at `now` seconds.
    pub fn sample(&mut self, now: f64) -> f64 {
        let Some(origin) = self.origin else {
            self.restart(now);
            return now;
        };
        let t = now - origin;
        let Some((a, b)) = self.line() else {
            // The second redraw: the next refresh.
            if t <= 0.0 {
                self.restart(now);
            } else {
                self.add(1.0, t);
            }
            return now;
        };
        let n = ((t - a) / b).round().max(self.last + 1.0);
        let on_line = a + n * b;
        if b <= 0.0 || (t - on_line).abs() > b / 2.0 || n - self.last > 4.0 {
            self.restart(now);
            return now;
        }
        self.add(n, t);
        let (a, b) = self.line().unwrap_or((a, b));
        origin + a + n * b
    }

    /// The refresh period on the line so far (seconds; 0 before two redraws).
    #[cfg(test)]
    pub fn period(&self) -> f64 {
        self.line().map_or(0.0, |(_, b)| b)
    }
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
    /// The clock at the last [`Self::update`] (it never goes back).
    last_clock: f64,
    /// A picture moved to and not yet taken by [`Self::take_new_picture`].
    new_picture: bool,
    /// A display presents the pictures ([`Self::update_display`]); ticks only feed the sound.
    display_driven: bool,
    /// The display's frame grid.
    grid: FrameGrid,
    rgba: Vec<u8>,
    video_done: bool,
    started: std::time::Instant,
    /// Debug trace of presented frames ([`Self::trace_present`]).
    trace: Option<std::io::BufWriter<std::fs::File>>,
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
            last_clock: 0.0,
            new_picture: false,
            display_driven: false,
            grid: FrameGrid::default(),
            rgba: Vec::new(),
            video_done: false,
            started: std::time::Instant::now(),
            trace: std::env::var_os("SHUPPATSU_MOVIE_TRACE")
                .and_then(|p| std::fs::File::create(p).ok())
                .map(std::io::BufWriter::new),
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

    /// Seconds since the start: the sound as heard (the audio clock read smoothly between
    /// device callbacks, `PcmStream::position_at`), or the ticks without a device.
    pub fn clock(&self) -> f64 {
        match &self.stream {
            Some(s) => s.position_at(Instant::now()) / f64::from(s.rate()) + self.offset,
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

    /// One app tick: keeps the sound fed and, until a display drives the pictures
    /// ([`Self::update_display`]; headless runs, before the first redraw), moves the picture
    /// to the clock. `true` when a new one is ready in [`Self::rgba`].
    pub fn update(&mut self) -> bool {
        self.feed_audio();
        if self.display_driven {
            return false;
        }
        let now = self.clock();
        self.show_at(now)
    }

    /// One presented display frame: the picture for the clock at this frame's place on the
    /// display's even grid ([`FrameGrid`]), not at the moment of the redraw, whose jitter
    /// (±2 ms measured) would otherwise show a picture 1 and its neighbour 3 frames whenever
    /// a picture boundary falls near a refresh. `true` when a new picture is ready.
    pub fn update_display(&mut self) -> bool {
        self.feed_audio();
        self.display_driven = true;
        let wall = self.started.elapsed().as_secs_f64();
        let at = self.clock() + (self.grid.sample(wall) - wall);
        self.show_at(at)
    }

    /// Moves the picture to the one due at `clock` (never back).
    fn show_at(&mut self, clock: f64) -> bool {
        let now = clock.max(self.last_clock);
        self.last_clock = now;
        let due = picture_at(now, self.period);
        let mut ready = None;
        while self.next.is_some() && due.is_some_and(|d| self.next_index <= d) {
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
                self.new_picture = true;
                true
            }
            None => false,
        }
    }

    /// A picture not yet uploaded is in [`Self::rgba`] (cleared by the call).
    pub fn take_new_picture(&mut self) -> bool {
        std::mem::take(&mut self.new_picture)
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

    /// Debug (env `SHUPPATSU_MOVIE_TRACE=<file>`): one line per presented display frame,
    /// `wall_s raw_clock_s clock_s picture uploaded redraw_ms` (the clock as the count of
    /// played frames and as read smoothly; the redraw's CPU time up to the present), for
    /// measuring the picture cadence.
    pub fn trace_present(&mut self, uploaded: bool, cost: std::time::Duration) {
        use std::io::Write;
        if self.trace.is_none() {
            return;
        }
        let raw = match &self.stream {
            Some(s) => s.played() as f64 / f64::from(s.rate()) + self.offset,
            None => self.ticks_clock,
        };
        let line = format!(
            "{:.6} {:.6} {:.6} {} {} {:.3}\n",
            self.started.elapsed().as_secs_f64(),
            raw,
            self.clock(),
            self.shown.map_or(-1, |n| n as i64),
            u8::from(uploaded),
            cost.as_secs_f64() * 1000.0
        );
        if let Some(w) = self.trace.as_mut() {
            let _ = w.write_all(line.as_bytes());
        }
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
    fn picture_at_is_the_last_due() {
        let p = 1001.0 / 30000.0;
        assert_eq!(picture_at(-0.001, p), None);
        assert_eq!(picture_at(0.0, p), Some(0));
        assert_eq!(picture_at(p * 0.999, p), Some(0));
        for n in [1u64, 29, 30, 4074] {
            assert_eq!(picture_at(n as f64 * p, p), Some(n), "{n}");
        }
        assert_eq!(picture_at(1.0, p), Some(29));
    }

    /// Redraws at `hz` with ±2 ms of jitter (a deterministic generator; the spread measured
    /// at 60 Hz on the real display), 60 s of a 29.97 Hz movie on an ideal audio clock: how
    /// many display frames each picture is shown, after the first second. `grid`: sampled on
    /// the [`FrameGrid`].
    fn holds(hz: f64, grid: bool) -> std::collections::BTreeMap<u32, u32> {
        let period = 1001.0 / 30000.0;
        let mut g = FrameGrid::default();
        let mut seed = 0x2545_f491_u32;
        let mut jitter = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (f64::from(seed >> 8) / f64::from(1u32 << 24) - 0.5) * 0.004
        };
        let mut out = std::collections::BTreeMap::new();
        let (mut cur, mut run) = (None, 0u32);
        for k in 0..(60.0 * hz) as u32 {
            // A redraw some ms before refresh k + 1, which shows it.
            let vsync = f64::from(k) / hz;
            let now = vsync + 0.003 + jitter();
            let at = if grid { g.sample(now) } else { now };
            let pic = picture_at(at, period);
            if pic == cur {
                run += 1;
            } else {
                // The first second learns the grid.
                if cur.is_some() && vsync > 1.0 {
                    *out.entry(run).or_default() += 1;
                }
                (cur, run) = (pic, 1);
            }
        }
        out
    }

    /// On the grid every picture is shown 2 refreshes at 60 Hz, 4 at 120 Hz, 6 at 180 Hz,
    /// but for the 29.97-against-30 beat: one picture a refresh longer every 1000 / (hz /
    /// 30) pictures. Sampled at the redraws themselves, pictures near a refresh flicker
    /// between 1 and 3 refreshes (at 60 Hz).
    #[test]
    fn display_cadence_is_even_on_the_grid() {
        for (hz, base) in [(60.0, 2), (120.0, 4), (180.0, 6)] {
            let h = holds(hz, true);
            eprintln!("{hz} Hz on the grid: {h:?}");
            let pictures: u32 = h.values().sum();
            let beat = f64::from(pictures) * (hz / 30.0) / 1000.0;
            let long = h.get(&(base + 1)).copied().unwrap_or(0);
            assert!(
                h.keys().all(|&k| k == base || k == base + 1),
                "{hz} Hz: {h:?}"
            );
            assert!(
                f64::from(long) <= beat.ceil() + 1.0,
                "{hz} Hz: {h:?}, beat {beat}"
            );
        }
        let raw = holds(60.0, false);
        eprintln!("60 Hz at the redraws: {raw:?}");
        assert!(
            raw.get(&1).copied().unwrap_or(0) > 5,
            "jittered sampling flickers: {raw:?}"
        );
    }

    /// The grid steps over skipped refreshes and restarts after a stall, learning the period.
    #[test]
    fn frame_grid_survives_skips_and_stalls() {
        let mut g = FrameGrid::default();
        let p = 1.0 / 60.0;
        for k in 0..100 {
            g.sample(f64::from(k) * p);
        }
        assert!((g.period() - p).abs() < 1e-9);
        // Two refreshes in one step: on the grid.
        assert!((g.sample(101.0 * p) - 101.0 * p).abs() < 1e-9);
        // A 300 ms stall: the grid restarts there and learns the period again.
        let t = 101.0 * p + 0.3;
        assert_eq!(g.sample(t), t);
        assert_eq!(g.sample(t + p), t + p);
        assert!((g.period() - p).abs() < 1e-9);
        assert!((g.sample(t + 2.0 * p) - (t + 2.0 * p)).abs() < 1e-9);
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
        // A picture moved to by a tick stays flagged for the next redraw's upload.
        assert!(p.take_new_picture());
        assert!(!p.take_new_picture());
        // Once a display presents the pictures, ticks only feed the sound.
        p.advance(0.1);
        assert!(p.update_display());
        assert_eq!(p.shown(), Some(32));
        p.advance(0.1);
        assert!(!p.update());
        assert_eq!(p.shown(), Some(32));
        assert!(p.update_display());
        assert_eq!(p.shown(), Some(35));
    }
}

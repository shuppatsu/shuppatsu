//! Subtitles for the Japanese voice lines (setting `subtitles`; docs/localisation.md,
//! "Subtitles"). A port addition: the original has none.
//!
//! The words of every clip were transcribed locally (Whisper, out/stt, not in the repo) and
//! cross-checked with where the game plays it. To show them, the sound
//! system records every voice line it starts together with the call that built it
//! ([`ddg_audio::Origin`]), and this module maps that call to a `[subtitle]` template of the
//! catalogue. A station call says the name of station-name id `n` (the pair table holds one
//! recording per id), so the subtitle uses the name the HUD's name sprite has in the
//! translation; a signal call is "kind + aspect" by its arguments; a speed call is "limit +
//! number" by its speed; a single sound has a line of its own (`0x97`), a table sequence a line per
//! sentence: a run of phrases that make one sentence has a key naming its first and last code
//! (`0x14e_0x150`), an in-train "next is <station>" is built like the announcer's call. Clips
//! without a key show nothing (the few left are listed in docs/localisation.md).
//!
//! Timing: a line is shown when its first spoken phrase starts (sequencer timing,
//! [`ddg_audio::voice::step_offsets`]) for the length of its phrases (sample lengths from
//! `effect.rom`), but at least a reading time; at most [`MAX_LINES`] lines are up at once,
//! the newest at the bottom; a line already up is not repeated, only kept longer. A stop-all
//! or the sequencer being switched off clears them.

use ddg_audio::code::{FIRST_BGM_CODE, SEQUENCE_FLAG};
use ddg_audio::system::{SIGNAL_ASPECTS, STATION_CALL_BLOCK, STATION_INTROS};
use ddg_audio::voice::step_offsets;
use ddg_audio::{Origin, SoundTables, Started, Step, VoiceEvent};
use ddg_formats::cg::Palette;
use ddg_render::sprites::{Anchor, Paint, SpriteLayer, TextStyle};
use ddg_render::text::{Align, FontRole};
use std::collections::HashMap;

/// Lines on screen at once (the newest at the bottom).
pub const MAX_LINES: usize = 2;
/// Game ticks (33 ms) per second.
const TICKS_PER_S: f32 = 1000.0 / 33.0;
/// Sequencer ticks (16.5 ms) per game tick (audio-text.md §5).
const SEQ_PER_TICK: u32 = 2;
/// Kept up after the audio ends, and the reading-time minimum: a base plus a time per
/// character (about 15 characters a second).
const LINGER_TICKS: u32 = 10;
const READ_BASE_TICKS: u32 = 30;
const READ_TICKS_PER_CHAR: u32 = 2;

/// Pair-table index of the call suffixes 通過 / 停車 used after a station name by the
/// approach call (0x40c4e0 passes 2 or 3 as `first`).
const PAIR_PASS: usize = 2;
const PAIR_STOP: usize = 3;
/// Table sequences of the numbered signal calls (0x486190): sequence `FIRST_BLOCK_SEQUENCE + k`
/// is "第k+1閉塞" (0x9b + k) + an aspect phrase for k 0..=6, sequence `FIRST_HOME_SEQUENCE + k`
/// "第k+1場内" (0xa2 + k) + an aspect phrase for k 0..=4. 0x40c010 starts 5..=0xb for blocks
/// 3..=9 counted down to the station; 0xc..=0x10 are not started in a stage.
const FIRST_BLOCK_SEQUENCE: usize = 5;
const FIRST_HOME_SEQUENCE: usize = 0xc;
const LAST_HOME_SEQUENCE: usize = 0x10;

/// Port-only palette slot of the subtitle colours (the plates use 0x7f).
pub const PAL_SUBTITLE: u8 = 0x7e;
const WHITE: u8 = 1;
const OUTLINE: u8 = 2;
/// Layout in 640×480 pixels: centred, above the speedometer (y ≥ 345), the time-up advice
/// balloon (y ≥ 344), the navigator's portrait and balloon (y ≥ 376) and the distance panel
/// (y ≥ 408), below the time messages' number rows; the newest line's box ends at
/// [`BOTTOM`], an older one sits directly above it.
const BOTTOM: f32 = 342.0;
const LINE_H: f32 = 23.0;
const LINE_W: f32 = 520.0;
const CENTRE_X: f32 = 320.0;
/// Capital height in original pixels (made smaller only to fit the width).
const CAP: f32 = 12.0;
/// Widget layer: with the captions, over the HUD art.
const LAYER: u8 = 3;

/// The aspect keys by aspect word index of the signal call (0x420320 words 0..=6).
const ASPECT_KEYS: [&str; 7] = [
    "aspect_clear",
    "aspect_reduce",
    "aspect_caution",
    "aspect_restricted",
    "aspect_stop",
    "aspect_high_speed",
    "aspect_limit",
];

/// Every key `[subtitle]` may have besides the per-code (`0x97`), per-run (`0x14e_0x150`) and
/// per-sequence (`seq_0x11`) lines.
pub const TEMPLATE_KEYS: &[&str] = &[
    "approach_pass",
    "approach_stop",
    "next_station",
    "arriving",
    "signal",
    "block_signal",
    "home_signal",
    "signal_home",
    "signal_starting",
    "repeater",
    "repeater_home",
    "repeater_starting",
    "main_home",
    "main_starting",
    "aspect_clear",
    "aspect_reduce",
    "aspect_caution",
    "aspect_restricted",
    "aspect_stop",
    "aspect_high_speed",
    "aspect_limit",
    "limit",
    "limit_speed",
    "limit_lifted",
];

/// Whether `key` is a valid `[subtitle]` key.
pub fn known_key(key: &str) -> bool {
    TEMPLATE_KEYS.contains(&key)
        || code_key(key).is_some()
        || run_key(key).is_some()
        || key
            .strip_prefix("seq_0x")
            .is_some_and(|h| usize::from_str_radix(h, 16).is_ok())
}

/// `0x97` → 0x97.
fn code_key(key: &str) -> Option<u16> {
    u16::from_str_radix(key.strip_prefix("0x")?, 16).ok()
}

/// `0x14e_0x150` → (0x14e, 0x150): a run of phrases from the first code to the last.
fn run_key(key: &str) -> Option<(u16, u16)> {
    let (a, b) = key.split_once('_')?;
    Some((code_key(a)?, code_key(b)?))
}

/// The length of every `effect.rom` sound in seconds, by code (index = code − 1).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VoiceLengths(pub Vec<f32>);

impl VoiceLengths {
    /// From `effect.rom` (sample count over sample rate).
    pub fn from_effect_rom(bytes: &[u8]) -> ddg_formats::Result<Self> {
        let fx = ddg_formats::audio::parse_effects(bytes)?;
        Ok(Self(
            fx.iter()
                .map(|e| e.pcm_u8.len() as f32 / e.sample_rate.max(1) as f32)
                .collect(),
        ))
    }

    /// Seconds of `code` (0 for BGM codes and unknown ones).
    fn seconds(&self, code: i16) -> f32 {
        usize::try_from(code)
            .ok()
            .and_then(|c| c.checked_sub(1))
            .and_then(|i| self.0.get(i))
            .copied()
            .unwrap_or(0.0)
    }
}

/// One subtitle line: shown `at` game ticks after the voice line started, for `ticks`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cue {
    pub at: u32,
    pub ticks: u32,
    pub text: String,
}

/// What maps a voice line to its subtitles.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SubtitleBook {
    templates: HashMap<String, String>,
    /// Station names by station-name id.
    names: HashMap<usize, String>,
    /// Station-name id of each announcer-voice station code (pair table entries 90 + id).
    announcer_names: HashMap<i16, usize>,
    sequences: Vec<Vec<Step>>,
    lengths: VoiceLengths,
}

impl SubtitleBook {
    /// `templates`: the catalogue's `[subtitle]`; `names`: station names by station-name id.
    pub fn new(
        templates: impl IntoIterator<Item = (String, String)>,
        names: HashMap<usize, String>,
        tables: &SoundTables,
        lengths: VoiceLengths,
    ) -> Self {
        let announcer_names = tables
            .call_pairs
            .iter()
            .enumerate()
            .skip(STATION_CALL_BLOCK)
            .map(|(i, s)| (s.code, i - STATION_CALL_BLOCK))
            .filter(|&(_, id)| names.contains_key(&id))
            .collect();
        Self {
            templates: templates.into_iter().collect(),
            names,
            announcer_names,
            sequences: tables.sequences.clone(),
            lengths,
        }
    }

    fn template(&self, key: &str) -> Option<&str> {
        self.templates.get(key).map(String::as_str)
    }

    fn station(&self, id: usize) -> Option<&str> {
        self.names.get(&id).map(String::as_str)
    }

    /// The subtitles of one voice line.
    pub fn cues(&self, s: &Started) -> Vec<Cue> {
        let steps = &s.steps;
        let whole = |text: Option<String>| -> Vec<Cue> {
            text.and_then(|t| self.cue(steps, first_voice(steps)?, steps.len() - 1, t))
                .into_iter()
                .collect()
        };
        match s.origin {
            Origin::Code(code) => whole(self.template(&format!("{code:#x}")).map(str::to_owned)),
            Origin::Pair { first, second } => {
                let key = match first {
                    PAIR_PASS => "approach_pass",
                    PAIR_STOP => "approach_stop",
                    _ => return Vec::new(),
                };
                whole(self.with_station(key, second))
            }
            Origin::Station { mode, name } => {
                let key = match mode {
                    0 | 2 => "next_station",
                    1 => "arriving",
                    _ => return Vec::new(),
                };
                whole(self.with_station(key, name))
            }
            Origin::Signal {
                prefix,
                mode,
                aspect,
            } => whole(self.signal_line(prefix, mode, aspect)),
            Origin::PrefixSpeed { prefix, kmh } => whole(self.limit_line(prefix, kmh)),
            Origin::Sequence(id) => self.sequence_cues(id, steps),
            Origin::List => Vec::new(),
        }
    }

    fn with_station(&self, key: &str, id: usize) -> Option<String> {
        Some(self.template(key)?.replace("{station}", self.station(id)?))
    }

    fn aspect(&self, word: i32) -> Option<&str> {
        self.template(ASPECT_KEYS.get(usize::try_from(word).ok()?)?)
    }

    /// 0x420320: kind `mode` 1 home, 2 starting, 0 repeater (only the repeater call 0x40c010
    /// passes 0), after an optional prefix 1 home / 2 starting (the repeater's own signal); prefix 3
    /// (0xd4 本線, from 0x40c010 on passing-station routes when the train does not stop) makes
    /// it the main-line signal.
    fn signal_line(&self, prefix: i32, mode: i32, aspect: i32) -> Option<String> {
        let kind = match (mode, prefix) {
            (1, 3) => "main_home",
            (2, 3) => "main_starting",
            (1, _) => "signal_home",
            (2, _) => "signal_starting",
            (0, 1) => "repeater_home",
            (0, 2) => "repeater_starting",
            (0, _) => "repeater",
            _ => return None,
        };
        Some(
            self.template("signal")?
                .replace("{signal}", self.template(kind)?)
                .replace("{aspect}", self.aspect(aspect)?),
        )
    }

    /// 0x420180 with prefix 0 (制限): the number phrase is `kmh / 5` (10..=125 km/h), 0..4
    /// km/h is "lifted", anything else the prefix alone.
    fn limit_line(&self, prefix: usize, kmh: i32) -> Option<String> {
        if prefix != 0 {
            return None;
        }
        let n = (kmh / 5).clamp(0, 26);
        Some(match n {
            0 => self.template("limit_lifted")?.to_owned(),
            2..=25 => self
                .template("limit_speed")?
                .replace("{kmh}", &(n * 5).to_string()),
            _ => self.template("limit")?.to_owned(),
        })
    }

    fn sequence_cues(&self, id: usize, steps: &[Step]) -> Vec<Cue> {
        // A line of its own for the whole sequence.
        if let Some(t) = self.template(&format!("seq_{id:#04x}")) {
            return first_voice(steps)
                .and_then(|i| self.cue(steps, i, steps.len() - 1, t.to_owned()))
                .into_iter()
                .collect();
        }
        // The numbered block / home signal calls: number phrase, aspect phrase.
        if (FIRST_BLOCK_SEQUENCE..=LAST_HOME_SEQUENCE).contains(&id) {
            let (key, n) = if id < FIRST_HOME_SEQUENCE {
                ("block_signal", id - FIRST_BLOCK_SEQUENCE + 1)
            } else {
                ("home_signal", id - FIRST_HOME_SEQUENCE + 1)
            };
            let word = steps
                .get(1)
                .and_then(|s| SIGNAL_ASPECTS.iter().position(|&w| w == s.code));
            let text = word.and_then(|w| {
                Some(
                    self.template(key)?
                        .replace("{n}", &n.to_string())
                        .replace("{aspect}", self.aspect(w as i32)?),
                )
            });
            return text
                .and_then(|t| self.cue(steps, 0, steps.len() - 1, t))
                .into_iter()
                .collect();
        }
        self.phrase_cues(steps)
    }

    /// A line per sentence of a phrase list (announcements, the navigator's sequences, the
    /// result voices): "next" + a station name of the announcer block (+ the closing
    /// phrase) as 0x420040 builds it, else the longest run `first_last` with a key, else the
    /// phrase's own key. Phrases without one (chimes, fragments) show nothing.
    fn phrase_cues(&self, steps: &[Step]) -> Vec<Cue> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < steps.len() {
            let (end, text) = self.sentence_at(steps, i);
            if let Some(c) = text.and_then(|t| self.cue(steps, i, end, t)) {
                out.push(c);
            }
            i = end + 1;
        }
        out
    }

    /// The sentence starting at step `i`: its last step and its line.
    fn sentence_at(&self, steps: &[Step], i: usize) -> (usize, Option<String>) {
        let code = steps[i].code;
        if let Some(&(_, suffix)) = STATION_INTROS.iter().find(|(intro, _)| *intro == code)
            && let Some(&name) = steps
                .get(i + 1)
                .and_then(|s| self.announcer_names.get(&s.code))
        {
            let end = if steps.get(i + 2).is_some_and(|s| s.code == suffix) {
                i + 2
            } else {
                i + 1
            };
            return (end, self.with_station("next_station", name));
        }
        for j in (i + 1..steps.len()).rev() {
            let key = format!("{:#x}_{:#x}", code, steps[j].code);
            if let Some(t) = self.template(&key) {
                return (j, Some(t.to_owned()));
            }
        }
        (i, self.template(&format!("{code:#x}")).map(str::to_owned))
    }

    /// The cue for steps `from..=to` of a line: from the start of step `from` to the end of
    /// step `to`, at least the reading time of `text`.
    fn cue(&self, steps: &[Step], from: usize, to: usize, text: String) -> Option<Cue> {
        let offsets = step_offsets(steps);
        let start = *offsets.get(from)?;
        let to = to.min(offsets.len() - 1);
        let end = offsets[to] as f32 + self.lengths.seconds(steps[to].code) * TICKS_PER_S * 2.0;
        // The sequencer runs after the tick's requests: a phrase at sequencer tick o ≥ 1
        // plays in game tick (o − 1) / 2 after the start.
        let at = start.saturating_sub(1) / SEQ_PER_TICK;
        let audio = ((end - start as f32) / SEQ_PER_TICK as f32).ceil().max(0.0) as u32;
        let read = READ_BASE_TICKS + READ_TICKS_PER_CHAR * text.chars().count() as u32;
        Some(Cue {
            at,
            ticks: (audio + LINGER_TICKS).max(read),
            text,
        })
    }
}

impl SubtitleBook {
    /// The book of a catalogue for the user's game: station names from the HUD's name
    /// sprites as translated, phrase lengths from `effect.rom`.
    pub fn for_game(
        cat: &crate::locale::Catalogue,
        exe: &ddg_formats::exe::Exe<'_>,
        data: &crate::scene::GameData,
        tables: &SoundTables,
    ) -> anyhow::Result<Self> {
        let tbl = cat.tbl_texts();
        let hud = ddg_formats::hud::HudTables::read(exe)?;
        let names = hud
            .station_names
            .iter()
            .enumerate()
            .filter_map(|(id, n)| Some((id, tbl.get(&n.plain)?.clone())))
            .collect();
        let lengths = VoiceLengths::from_effect_rom(&data.read("effect.rom")?)?;
        Ok(Self::new(cat.subtitles.clone(), names, tables, lengths))
    }
}

/// The first step that is a voice: not a BGM-slot chime and not a chained sequence.
fn first_voice(steps: &[Step]) -> Option<usize> {
    steps.iter().position(|s| {
        let c = s.code as u16;
        s.code > 0 && c < FIRST_BGM_CODE && c & SEQUENCE_FLAG == 0
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Shown {
    text: String,
    left: u32,
}

/// The subtitles of a run: cues waiting for their phrase and the lines on screen.
#[derive(Debug, Clone, Default)]
pub struct Subtitles {
    book: SubtitleBook,
    /// (game ticks to wait, cue).
    pending: Vec<(u32, Cue)>,
    /// Oldest first.
    shown: Vec<Shown>,
}

impl Subtitles {
    pub fn new(book: SubtitleBook) -> Self {
        Self {
            book,
            ..Self::default()
        }
    }

    /// The voice events of this tick (before [`Subtitles::tick`]).
    pub fn feed(&mut self, events: impl IntoIterator<Item = VoiceEvent>) {
        for e in events {
            match e {
                VoiceEvent::Started(s) => {
                    for c in self.book.cues(&s) {
                        self.pending.push((c.at, c));
                    }
                }
                VoiceEvent::Silenced => self.clear(),
            }
        }
    }

    /// Everything off the screen and forgotten.
    pub fn clear(&mut self) {
        self.pending.clear();
        self.shown.clear();
    }

    /// One game tick: lines age, cues whose phrase starts now come up. Returns the lines
    /// that came up (for the log).
    pub fn tick(&mut self) -> Vec<String> {
        self.shown.retain_mut(|s| {
            s.left = s.left.saturating_sub(1);
            s.left > 0
        });
        let mut due = Vec::new();
        self.pending.retain_mut(|(wait, c)| {
            if *wait == 0 {
                due.push(c.clone());
                false
            } else {
                *wait -= 1;
                true
            }
        });
        let mut new = Vec::new();
        for c in due {
            if self.show(c.clone()) {
                new.push(c.text);
            }
        }
        new
    }

    /// Puts `c` up; false when the line was up already (then only kept longer).
    fn show(&mut self, c: Cue) -> bool {
        if let Some(s) = self.shown.iter_mut().find(|s| s.text == c.text) {
            s.left = s.left.max(c.ticks);
            return false;
        }
        self.shown.push(Shown {
            text: c.text,
            left: c.ticks,
        });
        if self.shown.len() > MAX_LINES {
            self.shown.remove(0);
        }
        true
    }

    /// The lines on screen, oldest first.
    #[cfg(test)]
    pub fn lines(&self) -> impl Iterator<Item = &str> {
        self.shown.iter().map(|s| s.text.as_str())
    }

    /// Draws the lines as text widgets in the centre group (white, dark outline).
    pub fn draw(&self, layer: &mut SpriteLayer) {
        if self.shown.is_empty() {
            return;
        }
        let mut pal = Palette([0; 256]);
        // Palette entries are bytes B, R, G (cg::Palette).
        pal.0[usize::from(WHITE)] = u32::from_le_bytes([0xff, 0xff, 0xff, 0]);
        pal.0[usize::from(OUTLINE)] = u32::from_le_bytes([0x10, 0x10, 0x10, 0]);
        layer.palettes[usize::from(PAL_SUBTITLE)] = Some(pal);
        let paint = |index| Paint {
            palette: PAL_SUBTITLE,
            index,
        };
        let style = TextStyle {
            paint: paint(WHITE),
            cap: Some(CAP),
            outline: Some(paint(OUTLINE)),
            backdrop: None,
            align: Align::Center,
            vertical: false,
            font: FontRole::Body,
        };
        layer.anchored(Anchor::Centre, |layer| {
            for (i, s) in self.shown.iter().rev().enumerate() {
                let y = BOTTOM - LINE_H * (i + 1) as f32;
                let rect = [CENTRE_X - LINE_W / 2.0, y, LINE_W, LINE_H];
                layer.add_styled_text(LAYER, rect, &s.text, style);
            }
        });
    }
}

#[cfg(test)]
mod tests;

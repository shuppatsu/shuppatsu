//! Sound codes: the `u16` identifiers the game posts to its sound thread (audio-text.md §1, §3).
//!
//! * `1..=0x196`: `effect.rom` entry `code − 1` (u8 mono, per-sample rate, optional loop).
//! * `0x197..=0x1a8`: `bgm.rom` entry `code − 0x197` (s16 stereo 44.1 kHz).
//! * `0`: "no sound" (marks a free voice slot, never playable).
//! * `code | 0x1000` passed to `snd_request` (0x41fda0) starts sequence `code & 0xfff`.

pub use shuppatsu_formats::audio::{FIRST_BGM_CODE, LOOPING_BGM_CODE};

/// Highest effect code (0x430e00 compares against 0x196).
pub const LAST_EFFECT_CODE: u16 = 0x196;
/// Highest BGM code (18 `bgm.rom` entries).
pub const LAST_BGM_CODE: u16 = 0x1a8;
/// `snd_request` flag that selects a sequence instead of a sound (0x41fda0).
pub const SEQUENCE_FLAG: u16 = 0x1000;
/// The one code whose pitch parameter is inverted (`p = 127 − p`, 0x430e00). It is the motor
/// sound of every stage (stage table 0x459e38).
pub const INVERTED_PITCH_CODE: u16 = 0x31;
/// Continuous running/rail noise (started at segment start by 0x4080d0).
pub const RUNNING_NOISE: u16 = 9;

/// Where a code's sample data lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Index into `effect.rom`.
    Effect(usize),
    /// Index into `bgm.rom`.
    Bgm(usize),
}

impl Source {
    /// Maps a code to its ROM entry, or `None` for 0 and codes past the BGM range.
    pub fn of(code: u16) -> Option<Self> {
        match code {
            1..=LAST_EFFECT_CODE => Some(Self::Effect(code as usize - 1)),
            FIRST_BGM_CODE..=LAST_BGM_CODE => Some(Self::Bgm((code - FIRST_BGM_CODE) as usize)),
            _ => None,
        }
    }
}

/// True for codes played on the two BGM voices.
pub fn is_bgm(code: u16) -> bool {
    code >= FIRST_BGM_CODE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_ranges() {
        assert_eq!(Source::of(0), None);
        assert_eq!(Source::of(1), Some(Source::Effect(0)));
        assert_eq!(Source::of(0x196), Some(Source::Effect(0x195)));
        assert_eq!(Source::of(0x197), Some(Source::Bgm(0)));
        assert_eq!(Source::of(0x1a8), Some(Source::Bgm(17)));
        assert_eq!(Source::of(0x1a9), None);
        assert!(is_bgm(0x19e) && !is_bgm(0x31));
    }
}

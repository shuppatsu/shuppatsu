//! Sample data: `effect.rom` and `bgm.rom`, kept whole in memory and indexed by sound code.
//!
//! The original memory-maps `bgm.rom` (110 MB) and loads `effect.rom` whole (0x430230). Without a
//! memory-map dependency we read both files once at startup; voices then read straight from the
//! shared buffers, so playing a track never touches the disk.

use crate::code::{FIRST_BGM_CODE, LOOPING_BGM_CODE, Source};
use crate::{Error, Result};
use ddg_formats::audio::Effect;
use ddg_formats::rom2::Rom2;
use std::path::Path;

/// One `effect.rom` entry, as offsets into the bank's copy of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectInfo {
    /// Native sample rate in Hz (header field 0).
    pub sample_rate: u32,
    /// Loop region `[loop_start, loop_end)` in samples; `loop_end == 0` means one-shot.
    pub loop_start: u32,
    pub loop_end: u32,
    offset: usize,
    len: usize,
}

impl EffectInfo {
    /// Number of u8 samples.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// One `bgm.rom` track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BgmInfo {
    pub code: u16,
    /// Only code 0x19e wraps to the start (0x430a40).
    pub looping: bool,
    offset: usize,
    frames: usize,
}

impl BgmInfo {
    /// Number of stereo frames.
    pub fn frames(&self) -> usize {
        self.frames
    }
}

/// Both sound ROMs.
#[derive(Debug, Clone)]
pub struct SoundBank {
    effect_rom: Vec<u8>,
    effects: Vec<EffectInfo>,
    bgm_rom: Vec<u8>,
    bgm: Vec<BgmInfo>,
}

impl SoundBank {
    /// Parses the two ROM images (validating every entry).
    pub fn from_roms(effect_rom: Vec<u8>, bgm_rom: Vec<u8>) -> Result<Self> {
        let rom = Rom2::parse(&effect_rom)?;
        let mut effects = Vec::with_capacity(rom.len());
        for i in 0..rom.len() {
            let (offset, size) = rom.span(i).expect("index in range");
            let fx = Effect::parse(&effect_rom[offset..offset + size])?;
            let (loop_start, loop_end) = fx.loop_range.unwrap_or((0, 0));
            effects.push(EffectInfo {
                sample_rate: fx.sample_rate,
                loop_start,
                loop_end,
                offset: offset + 16,
                len: fx.pcm_u8.len(),
            });
        }
        // Validate with the format parser (whole stereo frames), then keep spans.
        let tracks = ddg_formats::audio::parse_bgm(&bgm_rom)?.len();
        let rom = Rom2::parse(&bgm_rom)?;
        let bgm = (0..tracks)
            .map(|i| {
                let (offset, size) = rom.span(i).expect("index in range");
                let code = FIRST_BGM_CODE + i as u16;
                BgmInfo {
                    code,
                    looping: code == LOOPING_BGM_CODE,
                    offset,
                    frames: size / 4,
                }
            })
            .collect();
        Ok(Self {
            effect_rom,
            effects,
            bgm_rom,
            bgm,
        })
    }

    /// Loads `effect.rom` and `bgm.rom` from the game's `DATA` directory (names matched
    /// case-insensitively).
    pub fn load(data_dir: &Path) -> Result<Self> {
        Self::from_roms(
            read_case_insensitive(data_dir, "effect.rom")?,
            read_case_insensitive(data_dir, "bgm.rom")?,
        )
    }

    pub fn effects(&self) -> &[EffectInfo] {
        &self.effects
    }

    pub fn bgm_tracks(&self) -> &[BgmInfo] {
        &self.bgm
    }

    /// The effect a code plays, if the code is an effect code present in the bank.
    pub fn effect(&self, code: u16) -> Option<&EffectInfo> {
        match Source::of(code)? {
            Source::Effect(i) => self.effects.get(i),
            Source::Bgm(_) => None,
        }
    }

    /// The track a code plays, if the code is a BGM code present in the bank.
    pub fn bgm(&self, code: u16) -> Option<&BgmInfo> {
        match Source::of(code)? {
            Source::Bgm(i) => self.bgm.get(i),
            Source::Effect(_) => None,
        }
    }

    /// True if `code` has sample data.
    pub fn has(&self, code: u16) -> bool {
        self.effect(code).is_some() || self.bgm(code).is_some()
    }

    /// Unsigned 8-bit samples of an effect.
    pub fn effect_pcm(&self, fx: &EffectInfo) -> &[u8] {
        &self.effect_rom[fx.offset..fx.offset + fx.len]
    }

    /// Effect sample `i` as `f32` in [−1, 1).
    pub(crate) fn effect_sample(&self, fx: &EffectInfo, i: usize) -> f32 {
        (self.effect_rom[fx.offset + i] as f32 - 128.0) / 128.0
    }

    /// BGM frame `i` as (left, right) `f32` in [−1, 1).
    pub(crate) fn bgm_frame(&self, t: &BgmInfo, i: usize) -> (f32, f32) {
        let o = t.offset + i * 4;
        let b = &self.bgm_rom[o..o + 4];
        let l = i16::from_le_bytes([b[0], b[1]]);
        let r = i16::from_le_bytes([b[2], b[3]]);
        (l as f32 / 32768.0, r as f32 / 32768.0)
    }
}

/// Reads `dir/name`, matching the file name case-insensitively.
pub(crate) fn read_case_insensitive(dir: &Path, name: &'static str) -> Result<Vec<u8>> {
    let io = |source| Error::Io {
        path: dir.display().to_string(),
        source,
    };
    let path = std::fs::read_dir(dir)
        .map_err(io)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name))
        })
        .ok_or_else(|| Error::Missing(name, dir.display().to_string()))?;
    std::fs::read(&path).map_err(|source| Error::Io {
        path: path.display().to_string(),
        source,
    })
}

#[cfg(test)]
pub(crate) mod test_data {
    //! Synthetic ROM images for unit tests.

    pub fn rom2(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut out = b"ROM2".to_vec();
        out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        let mut offset = 8 + 8 * entries.len();
        for e in entries {
            out.extend_from_slice(&(offset as u32).to_le_bytes());
            out.extend_from_slice(&(e.len() as u32).to_le_bytes());
            offset += e.len();
        }
        for e in entries {
            out.extend_from_slice(e);
        }
        out
    }

    pub fn effect(rate: u32, loop_: (u32, u32), pcm: &[u8]) -> Vec<u8> {
        let mut e = Vec::new();
        for v in [rate, pcm.len() as u32, loop_.0, loop_.1] {
            e.extend_from_slice(&v.to_le_bytes());
        }
        e.extend_from_slice(pcm);
        e
    }

    pub fn bgm_frames(frames: &[(i16, i16)]) -> Vec<u8> {
        frames
            .iter()
            .flat_map(|(l, r)| l.to_le_bytes().into_iter().chain(r.to_le_bytes()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::test_data::*;
    use super::*;

    #[test]
    fn indexes_by_code() {
        let fx = rom2(&[
            effect(10_000, (0, 0), &[0x80, 0xff]),
            effect(22_050, (1, 3), &[1, 2, 3, 4]),
        ]);
        let bgm = rom2(&[bgm_frames(&[(1, -1), (2, -2)])]);
        let bank = SoundBank::from_roms(fx, bgm).unwrap();
        let e = bank.effect(2).unwrap();
        assert_eq!((e.sample_rate, e.loop_start, e.loop_end), (22_050, 1, 3));
        assert_eq!(bank.effect_pcm(e), &[1, 2, 3, 4]);
        assert_eq!(bank.effect_sample(bank.effect(1).unwrap(), 0), 0.0);
        let t = bank.bgm(0x197).unwrap();
        assert_eq!((t.frames(), t.looping), (2, false));
        assert_eq!(bank.bgm_frame(t, 1), (2.0 / 32768.0, -2.0 / 32768.0));
        assert!(bank.effect(3).is_none() && bank.bgm(0x198).is_none() && !bank.has(0));
    }

    #[test]
    fn rejects_bad_entries() {
        let fx = rom2(&[effect(1, (0, 9), &[1])]);
        assert!(SoundBank::from_roms(fx, rom2(&[])).is_err());
        let fx = rom2(&[]);
        assert!(SoundBank::from_roms(fx, rom2(&[vec![0; 3]])).is_err());
    }
}

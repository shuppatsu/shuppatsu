//! Sound data: `effect.rom` and `bgm.rom` (docs/re/audio-text.md, loaders.md §6).
//!
//! - `effect.rom`: `ROM2` of 407 entries. Each is a 16-byte header (u32 sample rate,
//!   u32 data length, u32 loop start, u32 loop end; loop end 0 = one-shot) followed by unsigned
//!   8-bit mono PCM. Sound code *n* (1..=0x196) plays entry *n* − 1 (lookup at 0x430a40).
//! - `bgm.rom`: `ROM2` of 18 entries of raw signed 16-bit stereo PCM at 44100 Hz, no header.
//!   Sound code *n* (0x197..=0x1a8) plays entry *n* − 0x197; only code 0x19e loops.

use crate::error::{FormatError, Result};
use crate::reader::Bytes;
use crate::rom2::Rom2;

/// First sound code that refers to `bgm.rom` (0x430a40, 0x430e00).
pub const FIRST_BGM_CODE: u16 = 0x197;
/// The only BGM code that loops (whole entry), per the streaming code at 0x430a40.
pub const LOOPING_BGM_CODE: u16 = 0x19e;
pub const BGM_SAMPLE_RATE: u32 = 44_100;

/// One `effect.rom` sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effect<'a> {
    pub sample_rate: u32,
    /// Loop region in samples, if the sound loops while held.
    pub loop_range: Option<(u32, u32)>,
    /// Unsigned 8-bit mono PCM (0x80 = silence).
    pub pcm_u8: &'a [u8],
}

impl<'a> Effect<'a> {
    pub fn parse(entry: &'a [u8]) -> Result<Self> {
        let b = Bytes::new(entry, "effect.rom entry");
        let sample_rate = b.u32(0)?;
        let len = b.usize32(4)?;
        let loop_start = b.u32(8)?;
        let loop_end = b.u32(12)?;
        if len != entry.len().saturating_sub(16) {
            return Err(FormatError::invalid(
                "effect.rom entry",
                format!("data length {len} but entry has {} bytes", entry.len()),
            ));
        }
        if loop_end as usize > len || loop_start > loop_end {
            return Err(FormatError::invalid(
                "effect.rom entry",
                format!("loop {loop_start}..{loop_end} outside {len} samples"),
            ));
        }
        Ok(Self {
            sample_rate,
            loop_range: (loop_end != 0).then_some((loop_start, loop_end)),
            pcm_u8: b.slice(16, len)?,
        })
    }
}

/// All effects, index = sound code − 1.
pub fn parse_effects(data: &[u8]) -> Result<Vec<Effect<'_>>> {
    Rom2::parse(data)?.iter().map(Effect::parse).collect()
}

/// One `bgm.rom` track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BgmTrack<'a> {
    pub code: u16,
    pub looping: bool,
    /// Interleaved little-endian i16 stereo frames at 44100 Hz.
    pub pcm_s16le: &'a [u8],
}

pub fn parse_bgm(data: &[u8]) -> Result<Vec<BgmTrack<'_>>> {
    let rom = Rom2::parse(data)?;
    rom.iter()
        .enumerate()
        .map(|(i, pcm)| {
            if pcm.len() % 4 != 0 {
                return Err(FormatError::invalid(
                    "bgm.rom",
                    format!("entry {i} is not whole stereo frames ({} bytes)", pcm.len()),
                ));
            }
            let code = FIRST_BGM_CODE + i as u16;
            Ok(BgmTrack {
                code,
                looping: code == LOOPING_BGM_CODE,
                pcm_s16le: pcm,
            })
        })
        .collect()
}

/// Wraps PCM in a canonical 44-byte RIFF/WAVE header.
pub fn wav(pcm: &[u8], sample_rate: u32, channels: u16, bits: u16) -> Vec<u8> {
    let block_align = channels * bits / 8;
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * block_align as u32).to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn effect_entry(rate: u32, loop_: (u32, u32), pcm: &[u8]) -> Vec<u8> {
        let mut e = Vec::new();
        for v in [rate, pcm.len() as u32, loop_.0, loop_.1] {
            e.extend_from_slice(&v.to_le_bytes());
        }
        e.extend_from_slice(pcm);
        e
    }

    #[test]
    fn effect_header() {
        let e = effect_entry(10_000, (1, 3), &[0x80; 4]);
        let fx = Effect::parse(&e).unwrap();
        assert_eq!(fx.sample_rate, 10_000);
        assert_eq!(fx.loop_range, Some((1, 3)));
        assert_eq!(fx.pcm_u8.len(), 4);
        assert_eq!(
            Effect::parse(&effect_entry(1, (0, 0), &[1]))
                .unwrap()
                .loop_range,
            None
        );
        assert!(Effect::parse(&effect_entry(1, (0, 9), &[1])).is_err());
    }

    #[test]
    fn wav_header() {
        let w = wav(&[0; 8], 44_100, 2, 16);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(w.len(), 52);
        assert_eq!(u32::from_le_bytes(w[28..32].try_into().unwrap()), 176_400);
    }
}

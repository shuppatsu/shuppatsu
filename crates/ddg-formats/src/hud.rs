//! Driving-HUD tables in the exe's `.rdata`/`.data`, read from the user's `dengo3k.exe`.
//!
//! The in-cab HUD is drawn entirely with sprite slots (`tbl.rom` tilemaps, see [`crate::cg`]).
//! Where a slot sits on screen, and which tilemap shows a digit, handle position or station
//! name, comes from the tables below. Addresses, meanings and evidence: `docs/re/hud.md`.
//!
//! Only the table addresses and counts are hardcoded (3000-bandai exe); every value is read
//! from the exe.

use crate::error::{FormatError, Result};
use crate::exe::Exe;
use crate::reader::Bytes;

const WHAT: &str = "HUD tables";

/// Resolution modes (`0x816af4`): 512×384, 640×480, 800×600, 1024×768. Widths at 0x48c1e0,
/// heights at 0x48c1f0 (confirmed, read by the sprite blitter 0x430fb0).
pub const RES_MODES: usize = 4;
/// The 640×480 column, the one every sprite coordinate in the docs uses.
pub const MODE_640: usize = 1;

const MODE_WIDTHS: u32 = 0x48c1e0;
const MODE_HEIGHTS: u32 = 0x48c1f0;
/// Slot placement rows read by the HUD setup 0x40f7e0 (`0x464c90`, stride 4 × 10 bytes).
const SLOT_LAYOUT: u32 = 0x464c90;
/// Remaining-time digits 0–9 (`0x4653d4`, 0x40ff20).
const TIME_DIGITS: u32 = 0x4653d4;
/// Clock digits 0–9 (`0x465370`) and the same digits with a leading colon (`0x4653a8`), 0x417140.
const CLOCK_DIGITS: u32 = 0x465370;
const CLOCK_COLON_DIGITS: u32 = 0x4653a8;
/// Distance-to-stop digits 0–9 (`0x465540`, 0x4162c0).
const DISTANCE_DIGITS: u32 = 0x465540;
/// Brake handle sprites for brake 0..=10 (`0x46540c`) and mascon P0..P5 (`0x46543c`), 0x40ff20.
const BRAKE_SPRITES: u32 = 0x46540c;
const MASCON_SPRITES: u32 = 0x46543c;
/// Station names: `{u32 sprite, u32 width_px}` (`0x4665c4`) and the highlighted variant
/// (`0x4668ac`), indexed by station-name id < 0x5d (0x410450 / 0x410720).
const STATION_NAME_SPRITES: u32 = 0x4665c4;
const STATION_NAME_HIGHLIGHT: u32 = 0x4668ac;
/// Per-stage pointer → station-name ids, −1 terminated (`0x4860bc`, 0x4124f0).
const STAGE_STATION_IDS: u32 = 0x4860bc;
/// Seven-segment masks for the speedometer digits 0–9 (`0x4676a0`, u16 each, 0x416d10).
const SEGMENT_MASKS: u32 = 0x4676a0;

/// Station-name ids are checked `< 0x5d` before use (0x410450, 0x410720).
pub const STATION_NAME_COUNT: usize = 0x5d;
/// Stages with a station-name list (same 20 as [`crate::stages::STAGE_COUNT`]).
const STAGE_COUNT: usize = 20;
const MAX_LAYOUT_ROWS: usize = 256;
const MAX_STATION_LIST: usize = 64;

/// One slot's placement in one resolution mode: `{s16 x, s16 y, s16 layer, s16 palette,
/// s16 slot}` as passed to `Sprite_Place` (0x431850). Coordinates are native pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotPlacement {
    pub slot: i16,
    pub x: i16,
    pub y: i16,
    /// Blit pass 0..=3; higher layers are drawn first (0x430fb0).
    pub layer: i16,
    pub palette: i16,
}

/// A layout row: the same slot placed for each of the [`RES_MODES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotLayout(pub [SlotPlacement; RES_MODES]);

impl SlotLayout {
    /// The 640×480 placement.
    pub fn at_640(&self) -> SlotPlacement {
        self.0[MODE_640]
    }
}

/// Parses layout rows (40 bytes each) until a row whose first x is −1.
pub fn parse_slot_layout(bytes: &[u8]) -> Result<Vec<SlotLayout>> {
    let b = Bytes::new(bytes, "HUD slot layout");
    let mut rows = Vec::new();
    for row in 0..MAX_LAYOUT_ROWS {
        let base = row * 40;
        if b.i16(base)? == -1 {
            return Ok(rows);
        }
        let mut modes = [SlotPlacement {
            slot: 0,
            x: 0,
            y: 0,
            layer: 0,
            palette: 0,
        }; RES_MODES];
        for (m, p) in modes.iter_mut().enumerate() {
            let o = base + m * 10;
            *p = SlotPlacement {
                x: b.i16(o)?,
                y: b.i16(o + 2)?,
                layer: b.i16(o + 4)?,
                palette: b.i16(o + 6)?,
                slot: b.i16(o + 8)?,
            };
        }
        rows.push(SlotLayout(modes));
    }
    Err(FormatError::invalid("HUD slot layout", "no −1 terminator"))
}

/// `n` consecutive u32 sprite ids.
fn sprite_ids<const N: usize>(exe: &Exe<'_>, va: u32) -> Result<[u16; N]> {
    let b = exe.reader_at(va, N * 4, WHAT)?;
    let mut out = [0u16; N];
    for (i, id) in out.iter_mut().enumerate() {
        *id = u16::try_from(b.u32(i * 4)?)
            .map_err(|_| FormatError::invalid(WHAT, format!("sprite id at {va:#x}+{i}")))?;
    }
    Ok(out)
}

/// A station name as two tilemaps: plain (green) and highlighted (yellow on blue).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StationNameSprite {
    pub plain: u16,
    pub highlighted: u16,
    /// Width in pixels used to centre the banner and place the following sprite.
    pub width: u16,
}

/// The HUD tables of one exe.
#[derive(Debug, Clone)]
pub struct HudTables {
    /// Back-buffer size per resolution mode.
    pub mode_sizes: [(u16, u16); RES_MODES],
    /// Static slot placements set up when the HUD is created (0x40f7e0).
    pub layout: Vec<SlotLayout>,
    pub time_digits: [u16; 10],
    pub clock_digits: [u16; 10],
    pub clock_colon_digits: [u16; 10],
    pub distance_digits: [u16; 10],
    /// Brake 0 (released) ..= 8, 9 = EB, 10 = EB (handle removed); see input.md §4.1.
    pub brake_sprites: [u16; 11],
    /// N, P1..P5.
    pub mascon_sprites: [u16; 6],
    /// Indexed by station-name id.
    pub station_names: Vec<StationNameSprite>,
    /// Station-name id per station, per stage.
    pub stage_station_ids: Vec<Vec<u16>>,
    /// Bit `k` set = segment `k` lit; palette 4 entries 0xdb.. are rewritten from these.
    pub segment_masks: [u8; 10],
}

impl HudTables {
    pub fn read(exe: &Exe<'_>) -> Result<Self> {
        let w = exe.reader_at(MODE_WIDTHS, RES_MODES * 4, WHAT)?;
        let h = exe.reader_at(MODE_HEIGHTS, RES_MODES * 4, WHAT)?;
        let mut mode_sizes = [(0u16, 0u16); RES_MODES];
        for (i, s) in mode_sizes.iter_mut().enumerate() {
            let dim = |b: &Bytes<'_>| -> Result<u16> {
                u16::try_from(b.u32(i * 4)?)
                    .map_err(|_| FormatError::invalid(WHAT, "mode size out of range"))
            };
            *s = (dim(&w)?, dim(&h)?);
        }

        // The layout ends well before 0x465370 (the first sprite-id list after it).
        let layout = parse_slot_layout(
            exe.bytes_at(SLOT_LAYOUT, CLOCK_DIGITS as usize - SLOT_LAYOUT as usize)?,
        )?;

        let plain = exe.reader_at(STATION_NAME_SPRITES, STATION_NAME_COUNT * 8, WHAT)?;
        let hi = exe.reader_at(STATION_NAME_HIGHLIGHT, STATION_NAME_COUNT * 4, WHAT)?;
        let id16 = |v: u32| {
            u16::try_from(v).map_err(|_| FormatError::invalid(WHAT, format!("value {v:#x}")))
        };
        let station_names = (0..STATION_NAME_COUNT)
            .map(|i| {
                Ok(StationNameSprite {
                    plain: id16(plain.u32(i * 8)?)?,
                    width: id16(plain.u32(i * 8 + 4)?)?,
                    highlighted: id16(hi.u32(i * 4)?)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let mut stage_station_ids = Vec::with_capacity(STAGE_COUNT);
        for stage in 0..STAGE_COUNT {
            let ptr = exe.u32_at(STAGE_STATION_IDS + 4 * stage as u32)?;
            let mut ids = Vec::new();
            for i in 0..=MAX_STATION_LIST {
                if i == MAX_STATION_LIST {
                    return Err(FormatError::invalid(WHAT, "station-id list unterminated"));
                }
                let v = exe.reader_at(ptr + 4 * i as u32, 4, WHAT)?.i32(0)?;
                if v == -1 {
                    break;
                }
                let id = usize::try_from(v)
                    .ok()
                    .filter(|&id| id < STATION_NAME_COUNT)
                    .ok_or_else(|| FormatError::invalid(WHAT, format!("station-name id {v}")))?;
                ids.push(id as u16);
            }
            stage_station_ids.push(ids);
        }

        let seg = exe.reader_at(SEGMENT_MASKS, 20, WHAT)?;
        let mut segment_masks = [0u8; 10];
        for (i, m) in segment_masks.iter_mut().enumerate() {
            *m = u8::try_from(seg.u16(i * 2)?)
                .map_err(|_| FormatError::invalid(WHAT, "segment mask > 7 bits"))?;
        }

        Ok(Self {
            mode_sizes,
            layout,
            time_digits: sprite_ids(exe, TIME_DIGITS)?,
            clock_digits: sprite_ids(exe, CLOCK_DIGITS)?,
            clock_colon_digits: sprite_ids(exe, CLOCK_COLON_DIGITS)?,
            distance_digits: sprite_ids(exe, DISTANCE_DIGITS)?,
            brake_sprites: sprite_ids(exe, BRAKE_SPRITES)?,
            mascon_sprites: sprite_ids(exe, MASCON_SPRITES)?,
            station_names,
            stage_station_ids,
            segment_masks,
        })
    }

    /// The layout row for `slot`, if the static layout places it.
    pub fn slot(&self, slot: i16) -> Option<&SlotLayout> {
        self.layout.iter().find(|r| r.at_640().slot == slot)
    }
}

/// Splits `value` into decimal digits for a fixed-width readout, most significant first, with
/// leading zeros blanked (`None`) except the last digit, as the distance-to-stop readout does
/// (0x4162c0). The remaining-time and clock readouts keep their leading zeros.
pub fn digits_blank_leading<const N: usize>(value: u32) -> [Option<u8>; N] {
    let mut out = [None; N];
    let mut v = value;
    for d in out.iter_mut().rev() {
        *d = Some((v % 10) as u8);
        v /= 10;
    }
    for d in out.iter_mut().take(N.saturating_sub(1)) {
        if *d == Some(0) {
            *d = None;
        } else {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(entries: [[i16; 5]; RES_MODES]) -> Vec<u8> {
        entries
            .iter()
            .flat_map(|e| e.iter().flat_map(|v| v.to_le_bytes()))
            .collect()
    }

    #[test]
    fn layout_rows_until_terminator() {
        let mut bytes = row([
            [66, 56, 2, 4, 10],
            [66, 56, 2, 4, 10],
            [122, 68, 2, 4, 10],
            [389, 84, 2, 4, 10],
        ]);
        bytes.extend(row([[-1; 5]; RES_MODES]));
        let rows = parse_slot_layout(&bytes).unwrap();
        assert_eq!(rows.len(), 1);
        let p = rows[0].at_640();
        assert_eq!((p.slot, p.x, p.y, p.layer, p.palette), (10, 66, 56, 2, 4));
        assert_eq!(rows[0].0[2].x, 122);
    }

    #[test]
    fn layout_without_terminator_fails() {
        let bytes = row([[1, 2, 3, 4, 5]; RES_MODES]);
        assert!(parse_slot_layout(&bytes).is_err());
    }

    #[test]
    fn leading_zeros_blank() {
        assert_eq!(digits_blank_leading::<3>(7), [None, None, Some(7)]);
        assert_eq!(digits_blank_leading::<3>(0), [None, None, Some(0)]);
        assert_eq!(digits_blank_leading::<3>(105), [Some(1), Some(0), Some(5)]);
        assert_eq!(
            digits_blank_leading::<5>(40),
            [None, None, None, Some(4), Some(0)]
        );
    }
}

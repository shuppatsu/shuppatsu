//! Tables of the in-cab HUD elements beyond the static layout in [`crate::hud`]: station plates,
//! the pre-departure route strip and star marks, captions, banners, landmarks, the vehicle
//! plate, the speedometer variants, the ATC panel and the cab-signal lamp. Addresses and
//! meanings: `docs/re/hud.md` §3.5–§3.16 and §5.
//!
//! Only addresses and counts are hardcoded (3000-bandai exe); values are read from the exe. The
//! star-mark x offsets are not a data table: 0x4124f0 builds them on its stack with `mov`
//! instructions, so [`star_offsets`] reads the immediates out of that code.

use crate::error::{FormatError, Result};
use crate::exe::Exe;
use crate::hud::{MODE_640, RES_MODES, STATION_NAME_COUNT};

const WHAT: &str = "cab HUD tables";

/// Stages with HUD rows.
pub const STAGE_COUNT: usize = 20;
/// Vehicle rows: 20 stages plus the bonus rows `0x1000 | n` → 20 + n (0x410320): the two
/// bonus-section rows and the bonus game's EF63 (`0x1002`).
pub const VEHICLE_ROWS: usize = 23;
/// Landmark captions (course event 17). 0x410a70 accepts `id < 0x1f`, but the table holds 30
/// entries (0x466a20..0x466b10, where the strip colours start); id 30 would read colours.
pub const LANDMARK_COUNT: usize = 30;
/// Route strip entries read per stage: entry 0 is the progress strip, entry n the diagram with
/// n stations served (`0x48606c[stage][n]`, 0x4124f0). The lists are packed back to back, so
/// a fixed number is read; only indices up to the stage's stop count are used.
pub const ROUTE_STRIP_ENTRIES: usize = 32;
/// Star-mark slots per stage (0x3f..0x4e) and offset rows (row 0 = stage 10 before station 8,
/// row `s + 1` = stage `s`), 0x4124f0.
pub const STAR_SLOTS: usize = 16;
pub const STAR_ROWS: usize = STAGE_COUNT + 1;

/// Per-mode `{s16 x, y, layer, palette, slot}` rows of the station plates "from", arrow and
/// "next stop" (0x464c18 / 0x464c40 / 0x464c68, placed by 0x40f7e0).
const PLATE_ROWS: [u32; 3] = [0x464c18, 0x464c40, 0x464c68];
/// Vertical station-name plates by station-name id (u32, −1 = none), 0x40f780.
const STATION_PLATES: u32 = 0x46645c;
/// Per-stage `{持ち時間 label, vehicle name, vehicle picture, −1}` (u32 × 4), 0x40f7e0 / 0x410320.
const VEHICLE_ROWS_VA: u32 = 0x46594c;
/// Per-stage speedometer mode `0x856d34`: 0 none, 1 high-speed readout (0x40f7e0).
const SPEED_MODE: u32 = 0x466ca8;
/// ATC margin bar: `[0x4655b0 − 4 m]` for a margin `m` 0..=10 km/h (0x416d10).
const ATC_BAR_END: u32 = 0x4655b0;
/// Caption words (`0x465500[kind]`, plain) and its 駅 (0x465518); banner words (0x465520) and
/// its 駅 (0x465538). Kinds 0..=6.
const CAPTION_WORDS: u32 = 0x465500;
const CAPTION_STATION: u32 = 0x465518;
const BANNER_WORDS: u32 = 0x465520;
const BANNER_STATION: u32 = 0x465538;
/// Landmarks `{u32 sprite, u32 width}` (0x466a20, 0x410a70).
const LANDMARKS: u32 = 0x466a20;
/// Route strips: per-stage pointer to tbl ids (0x48606c), and the stage-10 second half
/// (0x465c8c), 0x40f7e0 / 0x4124f0.
const ROUTE_STRIPS: u32 = 0x48606c;
const ROUTE_STRIP_STAGE10: u32 = 0x465c8c;
/// Palette-4 colours of the strip (0x4124f0 / 0x413370): olive (not yet served), green
/// (served), and the per-stage recolours.
const STRIP_OLIVE: u32 = 0x466b10;
const STRIP_GREEN: u32 = 0x466b70;
const STRIP_EXTRA: u32 = 0x466bd0;
/// Colours read from each strip table (the most any call copies is 16, 0x40f7e0).
pub const STRIP_COLOURS: usize = 16;
/// Per-mode i32 positions (x table, y table).
const CAPTION_POS: (u32, u32) = (0x466ec8, 0x466ed8);
const TIME_UP_ADVICE_POS: (u32, u32) = (0x466ee8, 0x466ef8);
const DIAL_POS: (u32, u32) = (0x47c9e8, 0x47c9f8);
const DIGITAL_POS: (u32, u32) = (0x47ca28, 0x47ca38);
const LAMP_POS: (u32, u32) = (0x47ca48, 0x47ca58);
/// Per-mode origin of the 640×480 HUD (0x463d78 / 0x463d88).
const ORIGIN: (u32, u32) = (0x463d78, 0x463d88);
/// Segment HUD set-up 0x4124f0: the stack-table code starts after `sub esp, 0x540`.
const STAR_CODE: u32 = 0x4124f0;
const STAR_CODE_LEN: usize = 0x1000;

/// A vehicle row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VehicleRow {
    /// 持ち時間 label (slot 0x1c).
    pub label: i16,
    /// Line and rolling-stock name (slot 0x1d).
    pub name: i16,
    /// Train front picture (slot 0x1e).
    pub picture: i16,
}

/// A slot placement in the 640×480 column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    pub slot: i16,
    pub x: i16,
    pub y: i16,
    pub layer: i16,
    pub palette: i16,
}

/// A caption sprite and its width in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WideSprite {
    pub sprite: i16,
    pub width: i16,
}

#[derive(Debug, Clone)]
pub struct CabTables {
    /// "from" plate, arrow, "next stop" plate.
    pub plates: [Place; 3],
    /// By station-name id; −1 = none.
    pub station_plates: [i16; STATION_NAME_COUNT],
    pub vehicles: [VehicleRow; VEHICLE_ROWS],
    pub speed_mode: [i32; STAGE_COUNT],
    /// By margin 0..=10 km/h.
    pub atc_bar: [i16; 11],
    pub caption_words: [i16; 7],
    pub caption_station: i16,
    pub banner_words: [i16; 7],
    pub banner_station: i16,
    pub landmarks: [WideSprite; LANDMARK_COUNT],
    /// Per stage; see [`ROUTE_STRIP_ENTRIES`].
    pub route_strips: Vec<[i16; ROUTE_STRIP_ENTRIES]>,
    pub route_strip_stage10: [i16; ROUTE_STRIP_ENTRIES],
    pub strip_olive: [u32; STRIP_COLOURS],
    pub strip_green: [u32; STRIP_COLOURS],
    pub strip_extra: [u32; STRIP_COLOURS],
    /// x offsets of the star marks (add 0x90), see [`STAR_ROWS`].
    pub star_offsets: [[i32; STAR_SLOTS]; STAR_ROWS],
    pub caption_pos: (i16, i16),
    pub time_up_advice_pos: (i16, i16),
    pub dial_pos: (i16, i16),
    pub digital_pos: (i16, i16),
    pub lamp_pos: (i16, i16),
    pub origin: (i16, i16),
}

fn i16_of(v: i32) -> Result<i16> {
    i16::try_from(v).map_err(|_| FormatError::invalid(WHAT, format!("value {v:#x}")))
}

fn i32s<const N: usize>(exe: &Exe<'_>, va: u32) -> Result<[i32; N]> {
    let r = exe.reader_at(va, N * 4, WHAT)?;
    let mut out = [0; N];
    for (i, v) in out.iter_mut().enumerate() {
        *v = r.i32(i * 4)?;
    }
    Ok(out)
}

fn ids<const N: usize>(exe: &Exe<'_>, va: u32) -> Result<[i16; N]> {
    let v = i32s::<N>(exe, va)?;
    let mut out = [0; N];
    for (o, x) in out.iter_mut().zip(v) {
        *o = i16_of(x)?;
    }
    Ok(out)
}

fn u32s<const N: usize>(exe: &Exe<'_>, va: u32) -> Result<[u32; N]> {
    Ok(i32s::<N>(exe, va)?.map(|v| v as u32))
}

/// The 640×480 entry of a per-mode i32 position pair.
fn pos(exe: &Exe<'_>, (x, y): (u32, u32)) -> Result<(i16, i16)> {
    let xs = i32s::<RES_MODES>(exe, x)?;
    let ys = i32s::<RES_MODES>(exe, y)?;
    Ok((i16_of(xs[MODE_640])?, i16_of(ys[MODE_640])?))
}

fn place(exe: &Exe<'_>, va: u32) -> Result<Place> {
    let r = exe.reader_at(va + 10 * MODE_640 as u32, 10, WHAT)?;
    Ok(Place {
        x: r.i16(0)?,
        y: r.i16(2)?,
        layer: r.i16(4)?,
        palette: r.i16(6)?,
        slot: r.i16(8)?,
    })
}

impl CabTables {
    pub fn read(exe: &Exe<'_>) -> Result<Self> {
        let plates = [
            place(exe, PLATE_ROWS[0])?,
            place(exe, PLATE_ROWS[1])?,
            place(exe, PLATE_ROWS[2])?,
        ];
        let rows = i32s::<{ VEHICLE_ROWS * 4 }>(exe, VEHICLE_ROWS_VA)?;
        let mut vehicles = [VehicleRow {
            label: -1,
            name: -1,
            picture: -1,
        }; VEHICLE_ROWS];
        for (i, v) in vehicles.iter_mut().enumerate() {
            *v = VehicleRow {
                label: i16_of(rows[i * 4])?,
                name: i16_of(rows[i * 4 + 1])?,
                picture: i16_of(rows[i * 4 + 2])?,
            };
        }
        let bar = ids::<11>(exe, ATC_BAR_END - 40)?;
        let mut atc_bar = [0; 11];
        for (m, v) in atc_bar.iter_mut().enumerate() {
            *v = bar[10 - m];
        }
        let lm = i32s::<{ LANDMARK_COUNT * 2 }>(exe, LANDMARKS)?;
        let mut landmarks = [WideSprite {
            sprite: -1,
            width: 0,
        }; LANDMARK_COUNT];
        for (i, l) in landmarks.iter_mut().enumerate() {
            *l = WideSprite {
                sprite: i16_of(lm[i * 2])?,
                width: i16_of(lm[i * 2 + 1])?,
            };
        }
        let mut route_strips = Vec::with_capacity(STAGE_COUNT);
        for stage in 0..STAGE_COUNT {
            let ptr = exe.u32_at(ROUTE_STRIPS + 4 * stage as u32)?;
            route_strips.push(ids::<ROUTE_STRIP_ENTRIES>(exe, ptr)?);
        }
        Ok(Self {
            plates,
            station_plates: ids::<STATION_NAME_COUNT>(exe, STATION_PLATES)?,
            vehicles,
            speed_mode: i32s::<STAGE_COUNT>(exe, SPEED_MODE)?,
            atc_bar,
            caption_words: ids::<7>(exe, CAPTION_WORDS)?,
            caption_station: ids::<1>(exe, CAPTION_STATION)?[0],
            banner_words: ids::<7>(exe, BANNER_WORDS)?,
            banner_station: ids::<1>(exe, BANNER_STATION)?[0],
            landmarks,
            route_strips,
            route_strip_stage10: ids::<ROUTE_STRIP_ENTRIES>(exe, ROUTE_STRIP_STAGE10)?,
            strip_olive: u32s::<STRIP_COLOURS>(exe, STRIP_OLIVE)?,
            strip_green: u32s::<STRIP_COLOURS>(exe, STRIP_GREEN)?,
            strip_extra: u32s::<STRIP_COLOURS>(exe, STRIP_EXTRA)?,
            star_offsets: star_offsets(exe.bytes_at(STAR_CODE, STAR_CODE_LEN)?)?,
            caption_pos: pos(exe, CAPTION_POS)?,
            time_up_advice_pos: pos(exe, TIME_UP_ADVICE_POS)?,
            dial_pos: pos(exe, DIAL_POS)?,
            digital_pos: pos(exe, DIGITAL_POS)?,
            lamp_pos: pos(exe, LAMP_POS)?,
            origin: pos(exe, ORIGIN)?,
        })
    }
}

/// Reads the star-mark offset table that 0x4124f0 builds on its stack (`int local_540[336]`).
///
/// The prologue is `sub esp, imm32` and register pushes, followed by a run of `mov r32, imm32`
/// (0xb8 + r) and stores `mov [esp + disp], r32` (0x89) / `mov [esp + disp], imm32` (0xc7),
/// with 8- or 32-bit displacements (ModRM 0x44 / 0x84 with SIB 0x24). The array starts at
/// `[esp + 4 · pushes]`. The run ends at the first other instruction; every entry must have
/// been written by then.
pub fn star_offsets(code: &[u8]) -> Result<[[i32; STAR_SLOTS]; STAR_ROWS]> {
    let bad = |d: &str| FormatError::invalid("star offsets code", d.to_string());
    let byte = |i: usize| code.get(i).copied().ok_or_else(|| bad("truncated"));
    let imm = |i: usize| -> Result<i32> {
        let b = code.get(i..i + 4).ok_or_else(|| bad("truncated"))?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    // sub esp, imm32
    if code.get(..2) != Some(&[0x81, 0xec]) {
        return Err(bad("no sub esp prologue"));
    }
    let mut i = 6;
    let mut pushes = 0;
    while (0x50..=0x57).contains(&byte(i)?) {
        pushes += 1;
        i += 1;
    }
    // Arguments for a later call are pushed in the middle of the run (`push ecx`,
    // `push 0x84c1a4`), moving the array further from esp.
    let mut base = 4 * pushes;
    const N: usize = STAR_SLOTS * STAR_ROWS;
    let mut table: [Option<i32>; N] = [None; N];
    let mut regs: [Option<i32>; 8] = [None; 8];
    loop {
        let op = byte(i)?;
        match op {
            0x50..=0x57 => {
                base += 4;
                i += 1;
            }
            0x68 => {
                base += 4;
                i += 5;
            }
            0x6a => {
                base += 4;
                i += 2;
            }
            0xb8..=0xbf => {
                regs[usize::from(op - 0xb8)] = Some(imm(i + 1)?);
                i += 5;
            }
            0x89 | 0xc7 => {
                let modrm = byte(i + 1)?;
                if modrm & 7 != 4 || byte(i + 2)? != 0x24 {
                    break;
                }
                let (disp, len) = match modrm >> 6 {
                    1 => (i64::from(byte(i + 3)? as i8), 4),
                    2 => (i64::from(imm(i + 3)?), 7),
                    _ => break,
                };
                let value = if op == 0x89 {
                    regs[usize::from((modrm >> 3) & 7)].ok_or_else(|| bad("unknown register"))?
                } else {
                    if (modrm >> 3) & 7 != 0 {
                        break;
                    }
                    imm(i + len)?
                };
                let off = disp - base as i64;
                if off < 0 || off % 4 != 0 || off as usize / 4 >= N {
                    return Err(bad("store outside the table"));
                }
                table[off as usize / 4] = Some(value);
                i += len + if op == 0xc7 { 4 } else { 0 };
            }
            _ => break,
        }
    }
    let mut out = [[0; STAR_SLOTS]; STAR_ROWS];
    for (k, v) in table.iter().enumerate() {
        out[k / STAR_SLOTS][k % STAR_SLOTS] = v.ok_or_else(|| bad("entry not written"))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn star_offsets_follow_registers_and_immediates() {
        let mut code = vec![0x81, 0xec, 0x40, 0x05, 0, 0, 0x53, 0x56];
        // mov esi, 0x40
        code.extend([0xbe, 0x40, 0, 0, 0]);
        // Entry 0 via a register, disp8 = 8 (base 8 for two pushes).
        code.extend([0x89, 0x74, 0x24, 0x08]);
        // The other entries via imm32 with disp32.
        for k in 1..STAR_SLOTS * STAR_ROWS {
            let d = (8 + 4 * k) as u32;
            code.extend([0xc7, 0x84, 0x24]);
            code.extend(d.to_le_bytes());
            code.extend((k as u32).to_le_bytes());
        }
        code.push(0xc3);
        let t = star_offsets(&code).unwrap();
        assert_eq!(t[0][0], 0x40);
        assert_eq!(t[0][1], 1);
        assert_eq!(
            t[STAR_ROWS - 1][STAR_SLOTS - 1],
            (STAR_SLOTS * STAR_ROWS - 1) as i32
        );
    }

    #[test]
    fn star_offsets_need_every_entry() {
        let code = [0x81, 0xec, 0x40, 0x05, 0, 0, 0xc3];
        assert!(star_offsets(&code).is_err());
    }
}

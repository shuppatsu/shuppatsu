//! Scene assembly for the route renderer: which track models, scenery objects and background
//! layers the original draws for a camera position, and with which transforms.
//!
//! Spec, addresses and confidence levels: `docs/re/scene.md`. This module reproduces the
//! original's per-frame selection in world units; it holds no graphics code.
//!
//! Conventions (scene.md §1):
//! - World space is the `.COM` space: x/z horizontal, y up, 1 unit ≈ 10 m. It is right-handed:
//!   a camera looking along +z has +x on its *left*. A right-handed look-at (eye, target,
//!   up = +y) with no roll reproduces the original image without mirroring.
//! - [`Affine`] maps a part's local coordinates (raw/256) to world space. Its axes are **not**
//!   orthonormal in general: the original scales track pieces to the chord they span and
//!   applies an approximate pitch. Use the matrix as is.
//! - Engine index *i* is the camera's main-line slot minus the main line's second slot
//!   (0x41cb00 sets the origin to `seg0.start + 12`); main-line point *i + 1*.

use std::ops::RangeInclusive;

use crate::error::{FormatError, Result};
use crate::exe::Exe;
use crate::route::{HEIGHT_SCALE, Route, TrackPoint, TrackRun, UNITS_PER_POINT};

const WHAT: &str = "scene tables";

/// Number of areas (route sets R1..R5); `area` arguments are 0-based (`R-index − 1`).
pub const AREA_COUNT: usize = 5;
/// Track types per rail group (point flags bits 0–4).
pub const TRACK_TYPES: usize = 32;
/// Rail model groups per area: 0–2 single-point pieces, 3–7 the 2/4/8/16/32-point bands.
pub const RAIL_GROUPS: usize = 8;
/// Track type that draws no rail (0x41d890 / 0x41d9c0 skip it).
pub const NO_RAIL: u8 = 31;
/// Track types 28–30 mark tunnels (0x41de20).
pub const TUNNEL_TYPES: RangeInclusive<u8> = 28..=30;
/// Number of COM indices (0 = none, 1..=15), as in `stages::COM_TABLE_LEN`.
pub const COM_COUNT: usize = 16;

// --- Exe tables (read at runtime) -------------------------------------------------------------

/// ptr[5] → u32[8][32] rail part ids per area (0x41cb00 stores `[area]` in 0x850290).
const RAIL_TABLES: u32 = 0x486170;
/// ptr[5] → u32[5] per kind ≥ 9, indexed `kind × 5 − 45` (0x41dc90, 0x850200).
const KIND_TABLES: u32 = 0x486eb0;
/// Fixed part sets for kinds 3, 4, 7, 8 (0x41dc90 switch).
const FIXED_KIND_SETS: [(u16, u32); 4] =
    [(3, 0x47c2cc), (4, 0x47c2e0), (7, 0x47c2f4), (8, 0x47c308)];
/// i32[4]: rail group of single-point pieces by `(phase + k) & 3` (0x41d890).
const SINGLE_GROUP_PATTERN: u32 = 0x46a770;
/// i32[6]: points spanned by a rail piece of band 0..5 (0x41d9c0 reads `[band]`).
const RAIL_BAND_STEP: u32 = 0x469cbc;
/// f32[6]: 1/step per rail band (0x41d9c0).
const RAIL_BAND_SCALE: u32 = 0x469d84;
/// f32[8]: rotation-reuse dot threshold; rail band b uses entry b + 2 (0x41d9c0).
const RAIL_REUSE_DOT: u32 = 0x469da4;
/// i32[4]: points spanned per band of a non-rail run (0x41d7d0 / 0x41d5b0).
const KIND_BAND_STEP: u32 = 0x469cd4;
/// i32[4]: which of the kind's 5 parts each band draws (0x41d7d0).
const KIND_BAND_MODEL: u32 = 0x469ce4;
/// f32[4]: 1/step per kind band (0x41d5b0).
const KIND_BAND_SCALE: u32 = 0x469d9c;
/// {f32 eye height, f32 unused}[16] by COM index (0x41cb00 → 0x850140).
const EYE_HEIGHT_BY_COM: u32 = 0x469dc4;
/// {f32, f32}[5] by area, used when COM index < 1 (0x41cb00).
const EYE_HEIGHT_BY_AREA: u32 = 0x469e44;
/// u32[28] tunnel plug parts for background modes 1 and 3 (0x41dfc0 → 0x41de20).
const TUNNEL_PLUG_LISTS: [u32; 2] = [0x46a124, 0x46a194];
/// u32[45] part ids that become animated objects (0x401110 scans 0x44e820..0x44e8d4).
const ANIMATED_IDS: u32 = 0x44e820;
const ANIMATED_COUNT: usize = (0x44e8d4 - 0x44e820) / 4;
/// ptr[45] → {u32 n, u32 parts[n]} animation frames (0x401330).
const ANIMATED_FRAMES: u32 = 0x4841ec;
/// {i32 part, i32 width, i32 height, i32 repeats, i32 v_offset, i32 unused, u32 flags}[12]
/// sky panorama sprites (0x41c890 → 0x401420).
const SKY_SPRITES: u32 = 0x469e6c;
const SKY_SPRITE_COUNT: usize = 12;
/// {i32 count, i32 sprite[3], i32 switch, i32 switch2}[16] by COM index; rows 0–1 unused.
const COM_SKY: u32 = 0x469fa4;
/// i32[2]: switch points for COM < 2 in area 4 (0x41c890).
const KEIHIN_SKY_SWITCH: u32 = 0x46a0bc;
/// Sky/ground gradient endpoints, 12 bytes (4 × RGB) per R-index 0..5, normal / alternate
/// (0x40eed0, second argument 0 / nonzero).
const GRADIENT_COLOURS: [u32; 2] = [0x463b40, 0x463bac];
/// 2D quads in 496×400 virtual space: {i16 x, i16 y}[4] and u16 colours[4] (0x41dfc0).
const QUAD_SKY_XY: u32 = 0x469c68;
const QUAD_GROUND_XY: u32 = 0x469c90;
const QUAD_GROUND_TEX_XY: u32 = 0x469ca0;
const QUAD_MASK_XY: u32 = 0x486160;
const COLOURS_SKY: u32 = 0x469c60;
const COLOURS_GROUND_GRADIENT: u32 = 0x469c78;
const COLOURS_GROUND_TOP: u32 = 0x469c80;
const COLOURS_GROUND_BOTTOM: u32 = 0x469c88;
const COLOURS_MASK: u32 = 0x469858;
/// Per-point background switch tables {i32 from_engine_index, u32 bits}: (COM, VA, count),
/// immediates in 0x41cb00 at 0x41cddd..0x41ce2d.
const BACKGROUND_EVENTS: [(u32, u32, usize); 4] = [
    (7, 0x46a5b4, 30),
    (10, 0x46a69c, 25),
    (11, 0x46a204, 58),
    (12, 0x46a3dc, 58),
];

// --- Small verified constants (hardcoded, with source) ----------------------------------------

/// Main-line piece counts: 22 single points, then 4, 2, 1, 1, 2 pieces of 2/4/8/16/32 points
/// (immediates stored to 0x850120..0x850134 at 0x41cc18..0x41cc3d).
pub const MAIN_LINE_BANDS: [usize; 6] = [22, 4, 2, 1, 1, 2];
/// The main line is drawn from 4 points ahead of the camera slot (0x41db80 adds 0x30).
pub const MAIN_LINE_LEAD: usize = 4;
/// Chord → axis scale for rail pieces (double 8.0 at 0x458c48 = 1 / 0.125).
const CHORD_SCALE: f32 = 8.0;
/// Pitch sine per raw height step for rails (double 1/512 at 0x458c40 = 8/4096).
const RAIL_PITCH_SCALE: f32 = 1.0 / 512.0;
/// "Cosine" passed with the pitch: 1.1 for single pieces (0x3f8ccccd, 0x41d890), 1.01 for
/// banded pieces (0x3f8147ae, 0x41d9c0), 1.0 elsewhere (0x41d5b0, 0x41d400).
const PITCH_COS_SINGLE: f32 = 1.1;
const PITCH_COS_BAND: f32 = 1.01;
/// A single rail piece is drawn only if its view-space z (forward = −z) is ≤ 0.625
/// (double at 0x469740, compared with 0x856bd8 in 0x41d890).
pub const RAIL_BEHIND_LIMIT: f32 = 0.625;
/// Buffer stops are drawn only within XZ distance² 100 of the eye (double at 0x458b38).
pub const BUFFER_STOP_RADIUS_SQ: f32 = 100.0;
/// Tunnel plug: probe starts 110 points ahead (0x41de20: +0x528), steps 2 points forward
/// (+0x18) up to 27 times (argument 0x1b), walks back in 6-point steps (−0x48).
const TUNNEL_PROBE_AHEAD: usize = 110;
const TUNNEL_PROBE_MAX: usize = 27;
/// The camera target is the same interpolation one point further on (0x41d160: +0x40).
pub const TARGET_LEAD_POINTS: usize = 1;
/// Driving camera: vertical FOV 45.0° (float 450.0 tenths stored at 0x41cb33), projection aspect
/// 1.42 (int 142 at 0x47c3e4 × 0.01, set at init and restored at 0x41c3ac), near 0.05, far 32.0
/// (0x4254b0). renderer.md §4 for the 4.12 quantisation.
pub const DRIVING_FOV_Y_DEG: f32 = 45.0;
pub const DRIVING_ASPECT: f32 = 1.42;
pub const NEAR: f32 = 0.05;
pub const FAR: f32 = 32.0;
/// Camera clamps (0x41d160): the slot stays ≥ origin + 12 points (0x90) when moving back and
/// ≤ seg0.end − 23 slots (0xb4 + 0x60) when moving forward.
const CAMERA_MIN_ENGINE_INDEX: usize = 12;
const CAMERA_END_MARGIN_SLOTS: usize = 23;
/// At segment start the camera begins up to 300 points behind the stop (0x41cb00: 0x12d, 37.5).
pub const START_RUN_UP_POINTS: usize = 300;

/// Model transform: `world = l.x·x_axis + l.y·y_axis + l.z·z_axis + translation` for a local
/// point `l` (raw part coordinates / 256). The original's row-vector model matrix.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affine {
    pub x_axis: [f32; 3],
    pub y_axis: [f32; 3],
    pub z_axis: [f32; 3],
    pub translation: [f32; 3],
}

impl Affine {
    /// 0x4186a0 with arguments `(pos, s, c)`: local x → (c, 0, −s), local z → (s, 0, c).
    /// With `s = sin(yaw)`, `c = cos(yaw)` this is a yaw rotation; track pieces pass an
    /// unnormalised chord instead, which also scales x and z.
    pub fn yaw(translation: [f32; 3], s: f32, c: f32) -> Self {
        Self {
            x_axis: [c, 0.0, -s],
            y_axis: [0.0, 1.0, 0.0],
            z_axis: [s, 0.0, c],
            translation,
        }
    }

    /// 0x418000 applied to the model: `y' = c·y + s·z`, `z' = c·z − s·y`.
    fn pitch(&mut self, s: f32, c: f32) {
        let (y, z) = (self.y_axis, self.z_axis);
        for i in 0..3 {
            self.y_axis[i] = c * y[i] + s * z[i];
            self.z_axis[i] = c * z[i] - s * y[i];
        }
    }

    /// World position of a local point.
    pub fn transform_point(&self, l: [f32; 3]) -> [f32; 3] {
        let mut out = self.translation;
        for (i, o) in out.iter_mut().enumerate() {
            *o += l[0] * self.x_axis[i] + l[1] * self.y_axis[i] + l[2] * self.z_axis[i];
        }
        out
    }

    /// Column-major 4×4 (`glam::Mat4::from_cols_array` order).
    pub fn to_cols_array(&self) -> [f32; 16] {
        let [x, y, z, t] = [self.x_axis, self.y_axis, self.z_axis, self.translation];
        [
            x[0], x[1], x[2], 0.0, y[0], y[1], y[2], 0.0, z[0], z[1], z[2], 0.0, t[0], t[1], t[2],
            1.0,
        ]
    }
}

/// What produced a track instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PieceSource {
    /// Main line (0x41db80); band 0 = single points, 1..=5 = 2..32-point pieces.
    MainLine { band: u8 },
    /// Kind-1 run of the current bucket (0x41dbf0); bands as for the main line.
    Rail { band: u8 },
    /// Run of kind 3/4/7/8/≥9 (0x41d7d0); band 0..=3, or 4 for the coarse remainder.
    Kind { kind: u16, band: u8 },
    /// Buffer stop at the end of a run (0x41d400), drawn `n + 1` times along the last chord.
    BufferStop { kind: u16 },
    /// Tunnel plug 110 points ahead (0x41de20).
    TunnelPlug,
}

/// One part to draw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackInstance {
    pub part_id: u32,
    pub transform: Affine,
    /// First slot of the piece (index into [`Route::slots`]).
    pub slot: usize,
    pub source: PieceSource,
}

/// Per-area model tables.
#[derive(Debug, Clone, PartialEq)]
pub struct AreaTables {
    /// `rail[group][track_type]` part ids.
    pub rail: [[u32; TRACK_TYPES]; RAIL_GROUPS],
    /// Parts for kinds 9.. (`kinds[kind − 9]`), as many as requested when reading.
    pub kinds: Vec<[u32; 5]>,
}

/// Background sky sprite (one row of 0x469e6c).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SkySprite {
    /// Part whose first group's rect and UVs give the texture (0x432d30).
    pub part_id: u32,
    /// Tile size in 496×400 virtual units (minus one at draw time, 0x401600).
    pub width: i32,
    pub height: i32,
    /// Tiles per 360° of camera heading.
    pub repeats: i32,
    /// Vertical placement relative to the projected horizon point, in texels (see scene.md).
    pub v_offset: i32,
    /// Field +0x14: always 127, not read by 0x41c890.
    pub unused: i32,
    /// Bits 0–3: corner rotation/flip (0x401420); bits 8+: heading phase in 1/256 tile.
    pub flags: u32,
}

/// Per-COM sky selection (0x469fa4 row, COM ≥ 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComSky {
    /// 1–3 sprites loaded (0x8500b8/bc/c0; with 3 also the fixed sprite 9 as the 4th).
    pub count: i32,
    pub sprites: [i32; 3],
    /// Engine index from which areas 1/2/4 switch from the first to the second sprite.
    pub switch_index: i32,
    /// Stored in 0x85029c; no reader found.
    pub switch_index2: i32,
}

/// A 2D background quad in 496×400 virtual units, drawn as a fan; colours are palette
/// references (`hi byte < 0x80`: palette `hi`, entry `lo`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quad2d {
    pub xy: [[i16; 2]; 4],
    pub colours: [u16; 4],
}

/// Gradient endpoints for one R-index: sky top/bottom, ground top/bottom (RGB).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GradientColours {
    pub sky: [[u8; 3]; 2],
    pub ground: [[u8; 3]; 2],
}

impl GradientColours {
    /// The 256-entry palette 3 that 0x40eed0 builds: entries 0..128 sky, 128..256 ground,
    /// each `start + k·(end − start)/128` truncated and clamped to 0..=255 (RGB).
    pub fn palette(&self) -> [[u8; 3]; 256] {
        let mut out = [[0u8; 3]; 256];
        for (half, ends) in [self.sky, self.ground].iter().enumerate() {
            for k in 0..128 {
                for c in 0..3 {
                    let start = f32::from(ends[0][c]);
                    // The step is stored as f32 (0x40eed0: fmul 1/128 then fstp dword).
                    let step = (f32::from(ends[1][c]) - start) * (1.0 / 128.0);
                    let v = (k as f32 * step + start) as i32; // __ftol truncates
                    out[half * 128 + k][c] = v.clamp(0, 255) as u8;
                }
            }
        }
        out
    }
}

/// Background tables (0x41c890, 0x41dfc0, 0x40eed0).
#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundTables {
    pub sky_sprites: Vec<SkySprite>,
    /// By COM index; `None` for 0 and 1 (those use the area's sprite).
    pub com_sky: Vec<Option<ComSky>>,
    /// Switch points used for COM < 2 in area 4.
    pub keihin_switch: [i32; 2],
    /// `[variant][r_index]`, variant 0 = normal, 1 = alternate; r_index 0..=5 (0 unused).
    pub gradients: [Vec<GradientColours>; 2],
    pub sky_quad: Quad2d,
    /// Flat ground gradient (palette 3 entries 0x80 → 0xff top to bottom).
    pub ground_quad: Quad2d,
    /// Ground in one flat colour: palette 3 entry 0x80 (ground top) …
    pub ground_top_quad: Quad2d,
    /// … or entry 0xff (ground bottom).
    pub ground_bottom_quad: Quad2d,
    /// Ground area of the textured variant 2 in area 3 (y from 251).
    pub ground_tex_xy: [[i16; 2]; 4],
    /// Drawn at depth 0 before the track every frame (0x41e460): masks virtual rows 0..94.
    pub mask_quad: Quad2d,
    /// Per-point switch tables by COM index (only 7, 10, 11, 12 have one).
    pub events: Vec<Vec<BackgroundEvent>>,
}

/// One entry of a per-point background table (0x41d0e0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackgroundEvent {
    /// Applies from this engine index on.
    pub from: i32,
    pub bits: u32,
}

impl BackgroundEvent {
    /// Bits 0–2: sky sprite (1 = first, 2 = second, 3 = third, 4 = second + fourth, 0 = none).
    pub fn sky(&self) -> u32 {
        self.bits & 7
    }
    /// Bits 3–5: ground (area 1: nonzero = flat top colour; area 3: 1 flat top, 2 textured,
    /// 3 flat bottom).
    pub fn ground(&self) -> u32 {
        (self.bits >> 3) & 7
    }
    /// Bits 6–7: draw the sky gradient quad.
    pub fn sky_gradient(&self) -> bool {
        (self.bits >> 6) & 3 != 0
    }
    /// Bit 8: run the tunnel-plug probe (its "inside" result is ignored here).
    pub fn tunnel_probe(&self) -> bool {
        (self.bits >> 8) & 1 != 0
    }
    /// Bits 9–10: extra CPU-projected world quad (0x41df00), 1 or 2.
    pub fn extra_quad(&self) -> u32 {
        (self.bits >> 9) & 3
    }
}

/// An animated scenery part: drawn by its own task instead of the scenery list (0x401330).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnimatedPart {
    pub part_id: u32,
    /// Behaviour class = position in the id table: < 6 frame from a global (0x401140),
    /// 6..13 frame cycles every tick (0x401160), ≥ 13 proximity-triggered (0x4011b0).
    pub class: usize,
    pub frames: Vec<u32>,
}

/// All exe tables the scene builder needs.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneTables {
    pub areas: Vec<AreaTables>,
    /// Parts of kinds 3, 4, 7, 8.
    pub fixed_kinds: [(u16, [u32; 5]); 4],
    pub single_group_pattern: [usize; 4],
    pub rail_band_step: [usize; 6],
    pub rail_band_scale: [f32; 6],
    pub rail_reuse_dot: [f32; 8],
    pub kind_band_step: [usize; 4],
    pub kind_band_model: [usize; 4],
    pub kind_band_scale: [f32; 4],
    /// Eye height above the track by COM index (index 0 is never used by the game).
    pub eye_height_by_com: [f32; COM_COUNT],
    pub eye_height_by_area: [f32; AREA_COUNT],
    /// Tunnel plug parts `[list][1..=27]` (index 0 unused); list 0 for area 1, 1 for area 3.
    pub tunnel_plugs: [[u32; 28]; 2],
    pub animated: Vec<AnimatedPart>,
    pub background: BackgroundTables,
}

fn i32s<const N: usize>(exe: &Exe<'_>, va: u32) -> Result<[i32; N]> {
    let r = exe.reader_at(va, 4 * N, WHAT)?;
    let mut out = [0i32; N];
    for (i, v) in out.iter_mut().enumerate() {
        *v = r.i32(4 * i)?;
    }
    Ok(out)
}

fn u32s<const N: usize>(exe: &Exe<'_>, va: u32) -> Result<[u32; N]> {
    Ok(i32s::<N>(exe, va)?.map(|v| v as u32))
}

fn f32s<const N: usize>(exe: &Exe<'_>, va: u32) -> Result<[f32; N]> {
    let r = exe.reader_at(va, 4 * N, WHAT)?;
    let mut out = [0f32; N];
    for (i, v) in out.iter_mut().enumerate() {
        *v = r.f32(4 * i)?;
    }
    Ok(out)
}

fn usizes<const N: usize>(exe: &Exe<'_>, va: u32) -> Result<[usize; N]> {
    let v = i32s::<N>(exe, va)?;
    let mut out = [0usize; N];
    for (o, x) in out.iter_mut().zip(v) {
        *o = usize::try_from(x)
            .map_err(|_| FormatError::invalid(WHAT, format!("negative entry at {va:#x}")))?;
    }
    Ok(out)
}

fn quad(exe: &Exe<'_>, xy: u32, colours: u32) -> Result<Quad2d> {
    Ok(Quad2d {
        xy: quad_xy(exe, xy)?,
        colours: {
            let r = exe.reader_at(colours, 8, WHAT)?;
            [r.u16(0)?, r.u16(2)?, r.u16(4)?, r.u16(6)?]
        },
    })
}

fn quad_xy(exe: &Exe<'_>, va: u32) -> Result<[[i16; 2]; 4]> {
    let r = exe.reader_at(va, 16, WHAT)?;
    let mut out = [[0i16; 2]; 4];
    for (i, p) in out.iter_mut().enumerate() {
        *p = [r.i16(4 * i)?, r.i16(4 * i + 2)?];
    }
    Ok(out)
}

impl SceneTables {
    /// Reads every table from the exe. The per-area kind tables have no stored length; pass
    /// the highest run kind the routes use ([`max_run_kind`]); kinds up to it are read.
    pub fn read(exe: &Exe<'_>, max_kind: u16) -> Result<Self> {
        let kind_rows = usize::from(max_kind.saturating_sub(8));
        let mut areas = Vec::with_capacity(AREA_COUNT);
        for a in 0..AREA_COUNT as u32 {
            let base = exe.u32_at(RAIL_TABLES + 4 * a)?;
            let mut rail = [[0u32; TRACK_TYPES]; RAIL_GROUPS];
            for (g, row) in rail.iter_mut().enumerate() {
                *row = u32s::<TRACK_TYPES>(exe, base + (g * TRACK_TYPES * 4) as u32)?;
            }
            let kbase = exe.u32_at(KIND_TABLES + 4 * a)?;
            let kinds = (0..kind_rows)
                .map(|k| u32s::<5>(exe, kbase + (k * 20) as u32))
                .collect::<Result<Vec<_>>>()?;
            areas.push(AreaTables { rail, kinds });
        }
        let mut fixed_kinds = [(0u16, [0u32; 5]); 4];
        for (out, (kind, va)) in fixed_kinds.iter_mut().zip(FIXED_KIND_SETS) {
            *out = (kind, u32s::<5>(exe, va)?);
        }
        let eye_com = f32s::<{ 2 * COM_COUNT }>(exe, EYE_HEIGHT_BY_COM)?;
        let eye_area = f32s::<{ 2 * AREA_COUNT }>(exe, EYE_HEIGHT_BY_AREA)?;
        let mut animated = Vec::with_capacity(ANIMATED_COUNT);
        for (class, part_id) in u32s::<ANIMATED_COUNT>(exe, ANIMATED_IDS)?
            .into_iter()
            .enumerate()
        {
            let p = exe.u32_at(ANIMATED_FRAMES + 4 * class as u32)?;
            let n = exe.u32_at(p)?;
            if n > 64 {
                return Err(FormatError::invalid(
                    WHAT,
                    format!("animation {class}: {n} frames"),
                ));
            }
            let frames = (0..n)
                .map(|i| exe.u32_at(p + 4 + 4 * i))
                .collect::<Result<Vec<_>>>()?;
            animated.push(AnimatedPart {
                part_id,
                class,
                frames,
            });
        }
        Ok(Self {
            areas,
            fixed_kinds,
            single_group_pattern: usizes::<4>(exe, SINGLE_GROUP_PATTERN)?,
            rail_band_step: usizes::<6>(exe, RAIL_BAND_STEP)?,
            rail_band_scale: f32s::<6>(exe, RAIL_BAND_SCALE)?,
            rail_reuse_dot: f32s::<8>(exe, RAIL_REUSE_DOT)?,
            kind_band_step: usizes::<4>(exe, KIND_BAND_STEP)?,
            kind_band_model: usizes::<4>(exe, KIND_BAND_MODEL)?,
            kind_band_scale: f32s::<4>(exe, KIND_BAND_SCALE)?,
            eye_height_by_com: std::array::from_fn(|i| eye_com[2 * i]),
            eye_height_by_area: std::array::from_fn(|i| eye_area[2 * i]),
            tunnel_plugs: [
                u32s::<28>(exe, TUNNEL_PLUG_LISTS[0])?,
                u32s::<28>(exe, TUNNEL_PLUG_LISTS[1])?,
            ],
            animated,
            background: read_background(exe)?,
        })
    }

    /// The five parts of a non-rail run kind, if the kind draws anything.
    pub fn kind_parts(&self, area: usize, kind: u16) -> Option<[u32; 5]> {
        match kind {
            9.. => self
                .areas
                .get(area)?
                .kinds
                .get(usize::from(kind - 9))
                .copied(),
            _ => self
                .fixed_kinds
                .iter()
                .find(|(k, _)| *k == kind)
                .map(|(_, p)| *p),
        }
    }

    /// Eye height above the track for a COM (0x41cb00).
    pub fn eye_height(&self, area: usize, com: u32) -> f32 {
        if com < 1 {
            self.eye_height_by_area[area]
        } else {
            self.eye_height_by_com[com as usize]
        }
    }

    /// The animation record of a part id, if it is animated.
    pub fn animated(&self, part_id: u32) -> Option<&AnimatedPart> {
        self.animated.iter().find(|a| a.part_id == part_id)
    }
}

fn read_background(exe: &Exe<'_>) -> Result<BackgroundTables> {
    let sky_sprites = (0..SKY_SPRITE_COUNT)
        .map(|i| {
            let v = i32s::<7>(exe, SKY_SPRITES + 0x1c * i as u32)?;
            Ok(SkySprite {
                part_id: v[0] as u32,
                width: v[1],
                height: v[2],
                repeats: v[3],
                v_offset: v[4],
                unused: v[5],
                flags: v[6] as u32,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let com_sky = (0..COM_COUNT)
        .map(|c| {
            if c < 2 {
                return Ok(None);
            }
            let v = i32s::<6>(exe, COM_SKY + 0x18 * c as u32)?;
            Ok(Some(ComSky {
                count: v[0],
                sprites: [v[1], v[2], v[3]],
                switch_index: v[4],
                switch_index2: v[5],
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    let gradients = GRADIENT_COLOURS.map(|va| {
        (0..6)
            .map(|r| {
                let b = exe.bytes_at(va + 12 * r, 12)?;
                let rgb = |o: usize| [b[o], b[o + 1], b[o + 2]];
                Ok(GradientColours {
                    sky: [rgb(0), rgb(3)],
                    ground: [rgb(6), rgb(9)],
                })
            })
            .collect::<Result<Vec<_>>>()
    });
    let [g0, g1] = gradients;
    let mut events = vec![Vec::new(); COM_COUNT];
    for (com, va, count) in BACKGROUND_EVENTS {
        events[com as usize] = (0..count)
            .map(|i| {
                let v = i32s::<2>(exe, va + 8 * i as u32)?;
                Ok(BackgroundEvent {
                    from: v[0],
                    bits: v[1] as u32,
                })
            })
            .collect::<Result<Vec<_>>>()?;
    }
    Ok(BackgroundTables {
        sky_sprites,
        com_sky,
        keihin_switch: i32s::<2>(exe, KEIHIN_SKY_SWITCH)?,
        gradients: [g0?, g1?],
        sky_quad: quad(exe, QUAD_SKY_XY, COLOURS_SKY)?,
        ground_quad: quad(exe, QUAD_GROUND_XY, COLOURS_GROUND_GRADIENT)?,
        ground_top_quad: quad(exe, QUAD_GROUND_XY, COLOURS_GROUND_TOP)?,
        ground_bottom_quad: quad(exe, QUAD_GROUND_XY, COLOURS_GROUND_BOTTOM)?,
        ground_tex_xy: quad_xy(exe, QUAD_GROUND_TEX_XY)?,
        mask_quad: quad(exe, QUAD_MASK_XY, COLOURS_MASK)?,
        events,
    })
}

/// Highest run kind used by a route (to size [`SceneTables::read`]).
pub fn max_run_kind(route: &Route) -> u16 {
    route
        .buckets
        .iter()
        .flatten()
        .map(|r| r.kind)
        .max()
        .unwrap_or(0)
}

// --- Camera -----------------------------------------------------------------------------------

/// The game state the scene depends on besides the camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SceneContext {
    /// 0-based area (`stages` R-index − 1): selects the model and background tables.
    pub area: usize,
    /// COM index (1..=15) of the loaded route.
    pub com: u32,
    /// 1-based station number within this COM (0x850214: stop index + 1 at segment start,
    /// +1 on each arrival). Only a few hard-coded exceptions read it.
    pub station: u32,
}

/// Camera position on the main line: engine index plus the fraction `t` in track units
/// (`0 ≤ t ≤ 0.125` after a step; one point = 0.125 units).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackPos {
    pub engine_index: usize,
    pub t: f32,
}

impl TrackPos {
    /// Slot index of the point the camera is on (`seg0.start + 1 + engine_index`).
    pub fn slot(&self, route: &Route) -> usize {
        route.segments[0].points.start + 1 + self.engine_index
    }

    /// Distance from the main line's second point in track units.
    pub fn units(&self) -> f32 {
        self.engine_index as f32 * UNITS_PER_POINT + self.t
    }
}

/// Engine-index limits that 0x41d160 clamps to: `(min when moving back, max when moving on)`.
pub fn camera_limits(route: &Route) -> (usize, usize) {
    let seg = &route.segments[0];
    let end = seg.points.end + usize::from(seg.tail_marker);
    let max = (end - CAMERA_END_MARGIN_SLOTS).saturating_sub(seg.points.start + 1);
    (CAMERA_MIN_ENGINE_INDEX, max)
}

/// Eye and look-at target of the in-cab camera (0x41d160), world units. Roll is 0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CabCamera {
    pub eye: [f32; 3],
    pub target: [f32; 3],
}

impl CabCamera {
    /// Unit view direction.
    pub fn forward(&self) -> [f32; 3] {
        let d = sub(self.target, self.eye);
        let len = dot(d, d).sqrt();
        if len == 0.0 {
            [0.0, 0.0, 1.0]
        } else {
            d.map(|v| v / len)
        }
    }

    /// View-space z of a world point in the original's camera frame (forward = −z): the value
    /// 0x41d890 compares with [`RAIL_BEHIND_LIMIT`].
    pub fn view_z(&self, p: [f32; 3]) -> f32 {
        -dot(sub(p, self.eye), self.forward())
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Interpolated point between slots `i` and `i + 1` at factor `f`, with `lift` added to y.
fn lerp_point(slots: &[TrackPoint], i: usize, f: f32, lift: f32) -> [f32; 3] {
    let (a, b) = (slots[i], slots[(i + 1).min(slots.len() - 1)]);
    let (ha, hb) = (a.y(), b.y());
    [
        (b.x - a.x) * f + a.x,
        (hb - ha) * f + ha + lift,
        (b.z - a.z) * f + a.z,
    ]
}

/// The in-cab camera at `pos` (0x41d160). Eye = linear interpolation between the camera's
/// point and the next at `t × 8`, raised by `eye_height`; target = the same one point further
/// on. `jolt` is the camera-jolt amplitude (0x40e3d0; 0 when idle): it lowers the eye by
/// `jolt` and the target by `2·jolt`.
pub fn cab_camera(route: &Route, pos: TrackPos, eye_height: f32, jolt: f32) -> CabCamera {
    let slot = pos.slot(route);
    let f = pos.t * CHORD_SCALE; // 0x46a7c8: 8.0
    CabCamera {
        eye: lerp_point(&route.slots, slot, f, eye_height - jolt),
        target: lerp_point(
            &route.slots,
            slot + TARGET_LEAD_POINTS,
            f,
            eye_height - 2.0 * jolt,
        ),
    }
}

// --- Scenery activation -----------------------------------------------------------------------

/// The live scenery list and camera position, stepped like 0x41d160 (camera walk) with
/// 0x41cfe0 / 0x41d060 (activation lists).
#[derive(Debug, Clone, PartialEq)]
pub struct CabWalker {
    pos: TrackPos,
    /// Live placement indices in insertion order (the original's linked list, 0x8502a8).
    live: Vec<u16>,
}

impl CabWalker {
    /// State right after 0x41cb00 for main-line stop `stop` (an index into `route.stops`):
    /// the camera sits up to 300 points behind the stop with an empty live list, and the
    /// fraction holds the distance back to the stop, so the first [`CabWalker::advance`]
    /// walks forward to the stop and replays every activation list on the way.
    pub fn at_segment_start(route: &Route, stop: usize) -> Self {
        let stop_engine = route.stops[stop].saturating_sub(1);
        let back = stop_engine.min(START_RUN_UP_POINTS);
        let t = if stop_engine <= START_RUN_UP_POINTS {
            stop_engine as f32 * UNITS_PER_POINT
        } else {
            37.5 // double at 0x46a7b8
        };
        Self {
            pos: TrackPos {
                engine_index: stop_engine - back,
                t,
            },
            live: Vec::new(),
        }
    }

    /// A walker at an arbitrary position whose live list is rebuilt by replaying the lists
    /// from `max(0, engine_index − 300)` (an approximation of a segment start there).
    pub fn at(route: &Route, pos: TrackPos) -> Self {
        let from = pos.engine_index.saturating_sub(START_RUN_UP_POINTS);
        let mut w = Self {
            pos: TrackPos {
                engine_index: from,
                t: 0.0,
            },
            live: Vec::new(),
        };
        for i in from + 1..=pos.engine_index {
            w.pos.engine_index = i;
            w.apply(route, i, false);
        }
        w.pos = pos;
        w
    }

    pub fn pos(&self) -> TrackPos {
        self.pos
    }

    pub fn live(&self) -> &[u16] {
        &self.live
    }

    /// Advances by `delta` track units (one tick's `speed × 0.000925`, negative to go back),
    /// stepping point by point with the original's clamps and activation lists.
    pub fn advance(&mut self, route: &Route, delta: f32) {
        let (min, max) = camera_limits(route);
        self.pos.t += delta;
        while self.pos.t > UNITS_PER_POINT {
            self.pos.t -= UNITS_PER_POINT;
            self.pos.engine_index = (self.pos.engine_index + 1).min(max);
            self.apply(route, self.pos.engine_index, false);
        }
        while self.pos.t < 0.0 {
            self.pos.t += UNITS_PER_POINT;
            self.pos.engine_index = self.pos.engine_index.saturating_sub(1).max(min);
            self.apply(route, self.pos.engine_index, true);
        }
    }

    fn apply(&mut self, route: &Route, engine_index: usize, backward: bool) {
        let list = route.activations(engine_index + usize::from(backward));
        for &v in list {
            let add = (v > 0) != backward;
            let n = v.unsigned_abs();
            if add {
                if !self.live.contains(&n) {
                    self.live.push(n);
                }
            } else {
                self.live.retain(|&x| x != n);
            }
        }
    }
}

/// A scenery object to draw (0x41dd40).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneryInstance {
    pub placement: u16,
    pub part_id: u32,
    pub transform: Affine,
    /// Drawn by an animation task instead (0x401330); `part_id` is then the base id, see
    /// [`SceneTables::animated`].
    pub animated: bool,
}

/// The live scenery with transforms, after the one hard-coded exception in 0x41dd40 (part
/// 0x132f hidden on COM 10, station 1, engine index 0x579..=1999). No distance or frustum test.
pub fn scenery(
    route: &Route,
    tables: &SceneTables,
    ctx: SceneContext,
    walker: &CabWalker,
) -> Vec<SceneryInstance> {
    scenery_from(
        route,
        tables,
        ctx,
        walker.pos.engine_index,
        walker.live.iter().copied(),
    )
}

/// [`scenery`] for an explicit live set (e.g. from [`live_at`] over widened intervals) at
/// engine index `engine`; applies the same exception.
pub fn scenery_from(
    route: &Route,
    tables: &SceneTables,
    ctx: SceneContext,
    engine: usize,
    live: impl IntoIterator<Item = u16>,
) -> Vec<SceneryInstance> {
    live.into_iter()
        .filter_map(|n| {
            let p = route.placements.get(usize::from(n))?;
            if p.part_id == 0x132f
                && ctx.com == 10
                && ctx.station == 1
                && (0x579..=1999).contains(&engine)
            {
                return None;
            }
            let (s, c) = p.yaw.sin_cos();
            Some(SceneryInstance {
                placement: n,
                part_id: p.part_id,
                transform: Affine::yaw([p.x, p.y, p.z], s, c),
                animated: tables.animated(p.part_id).is_some(),
            })
        })
        .collect()
}

// --- Track pieces -----------------------------------------------------------------------------

struct TrackBuilder<'a> {
    slots: &'a [TrackPoint],
    /// Slots at or beyond this are not read (the end of the region being drawn).
    limit: usize,
    tables: &'a SceneTables,
    area: &'a AreaTables,
    camera: &'a CabCamera,
    out: Vec<TrackInstance>,
}

fn h(p: &TrackPoint) -> i32 {
    i32::from(p.height_raw)
}

impl TrackBuilder<'_> {
    fn pair(&self, i: usize, step: usize) -> Option<(TrackPoint, TrackPoint)> {
        (i + step < self.limit).then(|| (self.slots[i], self.slots[i + step]))
    }

    /// 0x41d890: `count` single-point rail pieces from slot `start`.
    fn singles(&mut self, start: usize, count: usize, phase: usize, source: PieceSource) {
        for k in 0..count {
            let i = start + k;
            let Some((p, q)) = self.pair(i, 1) else {
                return;
            };
            let ty = p.flags as u8 & 0x1f;
            if ty == NO_RAIL {
                continue;
            }
            let pos = [
                (q.x - p.x) * 0.5 + p.x,
                (h(&p) + h(&q)) as f32 * HEIGHT_SCALE * 0.5,
                (q.z - p.z) * 0.5 + p.z,
            ];
            let mut m = Affine::yaw(pos, -(q.x - p.x) * CHORD_SCALE, -(q.z - p.z) * CHORD_SCALE);
            let dh = h(&q) - h(&p);
            if dh != 0 {
                m.pitch(dh as f32 * RAIL_PITCH_SCALE, PITCH_COS_SINGLE);
            }
            if self.camera.view_z(pos) > RAIL_BEHIND_LIMIT {
                continue;
            }
            let group = self.tables.single_group_pattern[(phase + k) & 3];
            self.out.push(TrackInstance {
                part_id: self.area.rail[group][usize::from(ty)],
                transform: m,
                slot: i,
                source,
            });
        }
    }

    /// 0x41d9c0: `count` rail pieces of band `band` (1..=5) from slot `start`. Returns the
    /// slot after the last piece.
    fn band(&mut self, start: usize, count: usize, band: usize, source: PieceSource) -> usize {
        let step = self.tables.rail_band_step[band];
        let scale = self.tables.rail_band_scale[band];
        let threshold = self.tables.rail_reuse_dot[band + 2];
        let group = band + 2;
        // Last freshly computed piece: (dh, dx, dz, transform).
        let mut prev: Option<(i32, f32, f32, Affine)> = None;
        let mut i = start;
        for _ in 0..count {
            let Some((p, q)) = self.pair(i, step) else {
                break;
            };
            let ty = p.flags as u8 & 0x1f;
            if ty != NO_RAIL {
                let (dx, dz, dh) = (q.x - p.x, q.z - p.z, h(&q) - h(&p));
                let mid = self.slots[i + step / 2];
                let pos = [mid.x, mid.y(), mid.z];
                let transform = match prev {
                    Some((pdh, pdx, pdz, m)) if pdh == dh && pdx * dx + pdz * dz > threshold => {
                        // Reuse the previous rotation (0x418800 + 0x432030).
                        Affine {
                            translation: pos,
                            ..m
                        }
                    }
                    _ => {
                        let mut m =
                            Affine::yaw(pos, -dx * scale * CHORD_SCALE, -dz * scale * CHORD_SCALE);
                        if dh != 0 {
                            m.pitch(dh as f32 * scale * RAIL_PITCH_SCALE, PITCH_COS_BAND);
                        }
                        prev = Some((dh, dx, dz, m));
                        m
                    }
                };
                self.out.push(TrackInstance {
                    part_id: self.area.rail[group][usize::from(ty)],
                    transform,
                    slot: i,
                    source,
                });
            }
            i += step;
        }
        start + step * count
    }

    /// 0x41db80 / 0x41dbf0: singles then bands 1..=5.
    fn rail_run(&mut self, start: usize, counts: [usize; 6], phase: usize, main: bool) {
        let src = |band: u8| {
            if main {
                PieceSource::MainLine { band }
            } else {
                PieceSource::Rail { band }
            }
        };
        self.singles(start, counts[0], phase, src(0));
        let mut i = start + counts[0];
        for (band, &count) in counts.iter().enumerate().skip(1) {
            i = self.band(i, count, band, src(band as u8));
        }
    }

    /// 0x41d5b0: `count` non-rail pieces of band `band` from slot `start`.
    #[allow(clippy::too_many_arguments)]
    fn kind_band(
        &mut self,
        start: usize,
        count: usize,
        band: usize,
        part_id: u32,
        kind: u16,
        out_band: u8,
        ctx: SceneContext,
        engine: usize,
    ) {
        let step = self.tables.kind_band_step[band];
        let scale = self.tables.kind_band_scale[band];
        for k in 0..count {
            let i = start + k * step;
            let Some((p, q)) = self.pair(i, step) else {
                return;
            };
            let (bx, bz) = (p.x - q.x, p.z - q.z);
            if kind_piece_hidden(ctx, engine, part_id, bx, bz) {
                continue;
            }
            let pos = [
                (p.x + q.x) * 0.5,
                (h(&q) + h(&p)) as f32 * (HEIGHT_SCALE * 0.5), // 0x46a7e0: 1/8192
                (p.z + q.z) * 0.5,
            ];
            let mut m = Affine::yaw(pos, bx * scale, bz * scale);
            if h(&q) != h(&p) {
                m.pitch((h(&q) - h(&p)) as f32 * scale * HEIGHT_SCALE, 1.0);
            }
            self.out.push(TrackInstance {
                part_id,
                transform: m,
                slot: i,
                source: PieceSource::Kind {
                    kind,
                    band: out_band,
                },
            });
        }
    }

    /// 0x41d7d0: bands 0..=3, the coarse remainder, then an optional buffer stop.
    fn kind_run(&mut self, run: &TrackRun, parts: [u32; 5], ctx: SceneContext, engine: usize) {
        let mut i = run.slots.start;
        for band in 0..4 {
            let count = usize::from(run.lod[band]);
            let part = parts[self.tables.kind_band_model[band]];
            self.kind_band(i, count, band, part, run.kind, band as u8, ctx, engine);
            i += self.tables.kind_band_step[band] * count;
        }
        let end = run.slots.end;
        let rest = (end as isize - i as isize) - 1;
        if rest > 0 {
            self.kind_band(i, rest as usize, 1, parts[0], run.kind, 4, ctx, engine);
        }
        let Some(&e) = self.slots.get(end) else {
            return;
        };
        if run.lod[2] == 0 && e.flags & 0xf000 == 0xe000 && e.flags & 7 != 0 && end > 0 {
            self.buffer_stop(end, parts[1], run.kind);
        }
    }

    /// 0x41d400: `n + 1` copies along the chord from slot `end − 1` to `end`, where `n` =
    /// flags & 7 of slot `end`, if the previous point is within 10 units of the eye.
    fn buffer_stop(&mut self, end: usize, part_id: u32, kind: u16) {
        let (b, e) = (self.slots[end - 1], self.slots[end]);
        let (ex, ez) = (self.camera.eye[0], self.camera.eye[2]);
        if (b.x - ex) * (b.x - ex) + (b.z - ez) * (b.z - ez) > BUFFER_STOP_RADIUS_SQ {
            return;
        }
        let n = u32::from(e.flags & 7);
        let inv = 1.0 / n as f32;
        let d = [
            (e.x - b.x) * inv,
            (h(&e) - h(&b)) as f32 * HEIGHT_SCALE * inv,
            (e.z - b.z) * inv,
        ];
        let pos = [b.x - d[0] * 0.5, b.y(), b.z - d[2] * 0.5];
        let mut m = Affine::yaw(pos, d[0] * -CHORD_SCALE, d[2] * -CHORD_SCALE);
        if e.height_raw != b.height_raw {
            m.pitch(d[1], 1.0);
        }
        for k in 0..=n {
            let k = k as f32;
            self.out.push(TrackInstance {
                part_id,
                transform: Affine {
                    translation: [pos[0] + k * d[0], pos[1] + k * d[1], pos[2] + k * d[2]],
                    ..m
                },
                slot: end - 1,
                source: PieceSource::BufferStop { kind },
            });
        }
    }
}

/// Hard-coded exceptions in 0x41d5b0 (hide pieces bridging segment gaps and two parts).
fn kind_piece_hidden(ctx: SceneContext, engine: usize, part_id: u32, bx: f32, bz: f32) -> bool {
    let long = (bx * bx + bz * bz).sqrt() > 3.5; // double at 0x46a7d8
    let gap = if ctx.station == 3 {
        ctx.com == 1 && (0xe4c..=0xe56).contains(&engine) && long
    } else {
        ctx.station == 4 && ctx.com == 2 && (0x129b..=0x129f).contains(&engine) && long
    };
    gap || (part_id == 0xe15 && ctx.station == 6 && ctx.com == 7)
        || (part_id == 0x2616 && ctx.station == 6 && ctx.com == 3 && bz.abs() > 5.0)
}

/// Every track piece the original draws for this camera position: the main line from 4
/// points ahead (0x41db80), then the runs of bucket `engine_index / 4` (0x41dc90).
pub fn track_instances(
    route: &Route,
    tables: &SceneTables,
    ctx: SceneContext,
    pos: TrackPos,
    camera: &CabCamera,
) -> Vec<TrackInstance> {
    let mut b = TrackBuilder::new(route, tables, ctx, camera);
    let phase = pos.engine_index & 3;
    b.rail_run(
        pos.slot(route) + MAIN_LINE_LEAD,
        MAIN_LINE_BANDS,
        phase,
        true,
    );
    b.limit = route.slots.len();
    if let Some(bucket) = route.buckets.get(pos.engine_index / 4) {
        for run in bucket {
            b.run(run, ctx, pos.engine_index);
        }
    }
    b.out
}

impl<'a> TrackBuilder<'a> {
    fn new(
        route: &'a Route,
        tables: &'a SceneTables,
        ctx: SceneContext,
        camera: &'a CabCamera,
    ) -> Self {
        Self {
            slots: &route.slots,
            limit: route.segments[0].points.end,
            tables,
            area: &tables.areas[ctx.area],
            camera,
            out: Vec::new(),
        }
    }

    /// 0x41dc90 for one run.
    fn run(&mut self, run: &TrackRun, ctx: SceneContext, engine: usize) {
        match run.kind {
            1 => {
                let [b4, b5, b6, b7] = run.lod.map(usize::from);
                let counts = [b4, b5 >> 4, b5 & 15, b6 >> 4, b6 & 15, b7];
                self.rail_run(run.slots.start, counts, engine & 3, false);
            }
            k => {
                if let Some(parts) = self.tables.kind_parts(ctx.area, k) {
                    self.kind_run(run, parts, ctx, engine);
                }
            }
        }
    }
}

// --- Widescreen support (enhancement; scene.md §5) ---------------------------------------------

/// Horizontal half-angle tangent of the view the original's visibility lists were built for:
/// the 4:3 screen's 3D band (640×418 px at 45° vertical, aspect ≈ 1.53, edge ≈ 32.4°).
/// Measured: placements leave the live list from 32–33° off the view axis (scene.md §5).
pub const ORIGINAL_TAN_HALF_FOV_X: f32 = 0.634;

/// [`track_instances`] plus, for a wider view, the side-track pieces of the previous
/// `extra_buckets` buckets (nearest first) at slots the nearer buckets do not cover. The
/// original's buckets only list runs inside its 4:3 view, so without this, side tracks vanish
/// at the edges of a wider view as the camera moves on. Everything the original draws is
/// unchanged; only pieces on otherwise uncovered slots are added.
pub fn track_instances_wide(
    route: &Route,
    tables: &SceneTables,
    ctx: SceneContext,
    pos: TrackPos,
    camera: &CabCamera,
    extra_buckets: usize,
) -> Vec<TrackInstance> {
    let mut out = track_instances(route, tables, ctx, pos, camera);
    let current = pos.engine_index / 4;
    let mut covered = std::collections::HashSet::new();
    let cover = |covered: &mut std::collections::HashSet<(bool, usize)>, run: &TrackRun| {
        for s in run.slots.clone() {
            covered.insert((run.kind == 1, s));
        }
    };
    if let Some(bucket) = route.buckets.get(current) {
        bucket.iter().for_each(|r| cover(&mut covered, r));
    }
    for b in (current.saturating_sub(extra_buckets)..current).rev() {
        for run in &route.buckets[b] {
            let mut builder = TrackBuilder::new(route, tables, ctx, camera);
            builder.limit = route.slots.len();
            builder.run(run, ctx, pos.engine_index);
            out.extend(
                builder
                    .out
                    .into_iter()
                    .filter(|i| !covered.contains(&(run.kind == 1, i.slot))),
            );
            cover(&mut covered, run);
        }
    }
    out
}

/// A stretch of engine indices `from..to` during which a placement is in the live list
/// (`to = usize::MAX`: never removed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiveInterval {
    pub placement: u16,
    pub from: usize,
    pub to: usize,
}

/// Every live interval of a route, from a forward walk over all activation lists. The live
/// set at engine index `e` is the placements with `from ≤ e < to`; it equals what the
/// original holds after a segment start (checked at every stop by the data tests).
pub fn live_intervals(route: &Route) -> Vec<LiveInterval> {
    let mut open: Vec<(u16, usize)> = Vec::new();
    let mut out = Vec::new();
    for e in 1..route.activation_index.len() {
        for &v in route.activations(e) {
            let n = v.unsigned_abs();
            if v > 0 {
                if !open.iter().any(|&(p, _)| p == n) {
                    open.push((n, e));
                }
            } else if let Some(k) = open.iter().position(|&(p, _)| p == n) {
                let (placement, from) = open.remove(k);
                out.push(LiveInterval {
                    placement,
                    from,
                    to: e,
                });
            }
        }
    }
    out.extend(open.into_iter().map(|(placement, from)| LiveInterval {
        placement,
        from,
        to: usize::MAX,
    }));
    out.sort_by_key(|i| (i.from, i.placement));
    out
}

/// Horizontal view parameters for [`widen_intervals`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WideView {
    /// tan of the wide view's horizontal half-angle.
    pub tan_half_fov_x: f32,
    /// tan of the original's half-angle ([`ORIGINAL_TAN_HALF_FOV_X`]); removals of objects
    /// still well inside it (centre angle below 90 % of it) are kept as they are.
    pub original_tan_half_fov_x: f32,
    /// Upper bound on how far a removal is postponed, in points.
    pub max_extra_points: usize,
}

/// Postpones each removal that happened because the object left the original's 4:3 view
/// until its bounding circle (`radius(part_id)`, world units) leaves the wider view or falls
/// behind the eye, measured from the camera at each later point. Fidelity in front: an
/// object removed while still well inside the original view keeps its original removal.
pub fn widen_intervals(
    route: &Route,
    intervals: &[LiveInterval],
    view: WideView,
    radius: impl Fn(u32) -> f32,
) -> Vec<LiveInterval> {
    let (_, max_engine) = camera_limits(route);
    let horizontal = |e: usize| {
        let cam = cab_camera(
            route,
            TrackPos {
                engine_index: e,
                t: 0.0,
            },
            0.0,
            0.0,
        );
        let (fx, fz) = (cam.target[0] - cam.eye[0], cam.target[2] - cam.eye[2]);
        let len = (fx * fx + fz * fz).sqrt().max(f32::MIN_POSITIVE);
        (cam.eye, [fx / len, fz / len])
    };
    // (along, |lateral|) of a point relative to the camera at engine index e.
    let local = |e: usize, x: f32, z: f32| {
        let (eye, f) = horizontal(e);
        let (dx, dz) = (x - eye[0], z - eye[2]);
        (dx * f[0] + dz * f[1], (dx * f[1] - dz * f[0]).abs())
    };
    let sec = (1.0 + view.tan_half_fov_x * view.tan_half_fov_x).sqrt();
    intervals
        .iter()
        .map(|&iv| {
            let Some(p) = route.placements.get(usize::from(iv.placement)) else {
                return iv;
            };
            if iv.to == usize::MAX || iv.to > max_engine {
                return iv;
            }
            let (along, lat) = local(iv.to, p.x, p.z);
            if along > 0.0 && lat < along * view.original_tan_half_fov_x * 0.9 {
                return iv;
            }
            let r = radius(p.part_id);
            let visible = |e: usize| {
                let (along, lat) = local(e, p.x, p.z);
                along > -r && lat <= along * view.tan_half_fov_x + r * sec
            };
            let limit = (iv.to + view.max_extra_points).min(max_engine);
            let to = (iv.to..limit).find(|&e| !visible(e)).unwrap_or(limit);
            LiveInterval { to, ..iv }
        })
        .collect()
}

/// Placements live at engine index `e` under a set of intervals.
pub fn live_at(intervals: &[LiveInterval], e: usize) -> impl Iterator<Item = u16> + '_ {
    intervals
        .iter()
        .filter(move |i| i.from <= e && e < i.to)
        .map(|i| i.placement)
}

/// Result of the tunnel probe (0x41de20).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TunnelProbe {
    pub plug: Option<TrackInstance>,
    /// Every 6th point from 110 ahead back to the camera is a tunnel type: in area 1 (outside
    /// COM 7/10) the whole background is then skipped.
    pub inside: bool,
}

/// 0x41de20 with plug list `list` (0 for area 1, 1 for area 3).
pub fn tunnel_probe(
    route: &Route,
    tables: &SceneTables,
    pos: TrackPos,
    list: usize,
) -> TunnelProbe {
    let slots = &route.slots;
    let limit = route.segments[0].points.end;
    let cur = pos.slot(route);
    let probe = cur + TUNNEL_PROBE_AHEAD;
    let is_tunnel = |i: usize| i < limit && TUNNEL_TYPES.contains(&(slots[i].flags as u8 & 0x1f));
    let n = (0..TUNNEL_PROBE_MAX)
        .take_while(|&k| is_tunnel(probe + 2 * k))
        .count();
    if n == 0 || probe + 1 >= limit {
        return TunnelProbe {
            plug: None,
            inside: false,
        };
    }
    let (p, q) = (slots[probe], slots[probe + 1]);
    let plug = TrackInstance {
        part_id: tables.tunnel_plugs[list][n],
        transform: Affine::yaw(
            [p.x, p.y(), p.z],
            -(q.x - p.x) * CHORD_SCALE,
            -(q.z - p.z) * CHORD_SCALE,
        ),
        slot: probe,
        source: PieceSource::TunnelPlug,
    };
    let mut i = probe as isize;
    let inside = loop {
        if i < cur as isize {
            break true;
        }
        if !is_tunnel(i as usize) {
            break false;
        }
        i -= 6;
    };
    TunnelProbe {
        plug: Some(plug),
        inside,
    }
}

// --- Background -------------------------------------------------------------------------------

/// Ground layer of the background.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ground {
    None,
    /// Flat quad with palette references (see [`BackgroundTables`]).
    Flat(Quad2d),
    /// Textured quad: `part_id`'s first group selects the page. For 0x76f and 0x283b the UVs
    /// are the sky-quad corners × 16 (tiled); for 0x2dd2 the part's own UVs.
    Textured {
        xy: [[i16; 2]; 4],
        part_id: u32,
    },
}

/// The background layers for one frame (0x41dfc0), back to front.
#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundPlan {
    /// Sky panorama sprites (indices into [`BackgroundTables::sky_sprites`]).
    pub sky_sprites: Vec<usize>,
    /// Draw [`BackgroundTables::sky_quad`] (palette 3 gradient).
    pub sky_gradient: bool,
    pub ground: Ground,
    /// Extra CPU-projected world quad of 0x41df00 (area 3 only), 1 or 2.
    pub extra_quad: Option<u32>,
    pub tunnel: Option<TunnelProbe>,
}

/// Sprite indices loaded for a COM (0x41c890): first, second, third, fourth.
pub fn sky_sprite_slots(tables: &SceneTables, area: usize, com: u32) -> [Option<usize>; 4] {
    let bg = &tables.background;
    match bg.com_sky.get(com as usize).copied().flatten() {
        None => [Some(area), None, None, None],
        Some(c) => {
            let idx = |v: i32| usize::try_from(v).ok();
            let mut out = [idx(c.sprites[0]), None, None, None];
            if c.count != 1 {
                // COM 15 copies the first sprite over the second (0x41c890).
                out[1] = if com == 15 { out[0] } else { idx(c.sprites[1]) };
                if c.count != 2 {
                    out[2] = idx(c.sprites[2]);
                    out[3] = Some(9); // fixed row at 0x469f68
                }
            }
            out
        }
    }
}

fn switch_index(tables: &SceneTables, area: usize, com: u32) -> i32 {
    match tables
        .background
        .com_sky
        .get(com as usize)
        .copied()
        .flatten()
    {
        Some(c) => c.switch_index,
        None if area == 4 => tables.background.keihin_switch[0],
        None => i32::MAX,
    }
}

/// The background for this frame (0x41dfc0 with mode = area).
pub fn background(
    route: &Route,
    tables: &SceneTables,
    ctx: SceneContext,
    pos: TrackPos,
) -> BackgroundPlan {
    let bg = &tables.background;
    let slots = sky_sprite_slots(tables, ctx.area, ctx.com);
    let engine = pos.engine_index as i32;
    let first_or_second = if engine < switch_index(tables, ctx.area, ctx.com) {
        slots[0]
    } else {
        slots[1]
    };
    let event = || {
        let list = &bg.events[ctx.com as usize];
        let i = list
            .iter()
            .position(|e| engine < e.from)
            .unwrap_or(list.len());
        list.get(i.wrapping_sub(1))
            .copied()
            .unwrap_or(BackgroundEvent { from: 0, bits: 0 })
    };
    let sky_from_event = |e: BackgroundEvent| -> Vec<usize> {
        match e.sky() {
            1 => slots[0].into_iter().collect(),
            2 => slots[1].into_iter().collect(),
            3 => slots[2].into_iter().collect(),
            4 => [slots[1], slots[3]].into_iter().flatten().collect(),
            _ => Vec::new(),
        }
    };
    let plan = |sky: Vec<usize>, sky_gradient, ground| BackgroundPlan {
        sky_sprites: sky,
        sky_gradient,
        ground,
        extra_quad: None,
        tunnel: None,
    };
    match ctx.area {
        0 => {
            // Ground drawn nearer (depth 0x7fee) on COM 4 at 1501..1589: ordering only.
            plan(
                slots[0].into_iter().collect(),
                true,
                Ground::Textured {
                    xy: bg.ground_quad.xy,
                    part_id: 0x76f,
                },
            )
        }
        1 if ctx.com != 7 && ctx.com != 10 => {
            let t = tunnel_probe(route, tables, pos, 0);
            if t.inside {
                let mut p = plan(Vec::new(), false, Ground::None);
                p.tunnel = Some(t);
                return p;
            }
            let mut p = plan(
                first_or_second.into_iter().collect(),
                true,
                Ground::Flat(bg.ground_quad),
            );
            p.tunnel = Some(t);
            p
        }
        1 => {
            let e = event();
            let mut p = plan(
                sky_from_event(e),
                e.sky_gradient(),
                if e.ground() != 0 {
                    Ground::Flat(bg.ground_top_quad)
                } else {
                    Ground::None
                },
            );
            p.tunnel = e
                .tunnel_probe()
                .then(|| tunnel_probe(route, tables, pos, 0));
            p
        }
        3 => {
            let e = event();
            let ground = match e.ground() {
                1 => Ground::Flat(bg.ground_top_quad),
                2 => Ground::Textured {
                    xy: bg.ground_tex_xy,
                    part_id: 0x2dd2,
                },
                3 => Ground::Flat(bg.ground_bottom_quad),
                _ => Ground::None,
            };
            let mut p = plan(sky_from_event(e), e.sky_gradient(), ground);
            p.tunnel = e
                .tunnel_probe()
                .then(|| tunnel_probe(route, tables, pos, 1));
            p.extra_quad = (e.extra_quad() != 0).then_some(e.extra_quad());
            p
        }
        4 => plan(
            first_or_second.into_iter().collect(),
            true,
            Ground::Textured {
                xy: bg.ground_quad.xy,
                part_id: 0x283b,
            },
        ),
        _ => plan(
            first_or_second.into_iter().collect(),
            true,
            Ground::Flat(bg.ground_quad),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    #[test]
    fn yaw_matches_sin_cos_convention() {
        let yaw = 0.3f32;
        let m = Affine::yaw([1.0, 2.0, 3.0], yaw.sin(), yaw.cos());
        // Local +z goes to (sin, 0, cos), local +x to (cos, 0, −sin).
        assert!(close(
            m.transform_point([0.0, 0.0, 1.0]),
            [1.0 + yaw.sin(), 2.0, 3.0 + yaw.cos()]
        ));
        assert!(close(
            m.transform_point([1.0, 0.0, 0.0]),
            [1.0 + yaw.cos(), 2.0, 3.0 - yaw.sin()]
        ));
        let c = m.to_cols_array();
        assert_eq!(&c[12..15], &[1.0, 2.0, 3.0]);
    }

    #[test]
    fn pitch_mixes_y_and_z() {
        let mut m = Affine::yaw([0.0; 3], 0.0, 1.0);
        m.pitch(0.5, 1.0);
        assert!(close(m.y_axis, [0.0, 1.0, 0.5]));
        assert!(close(m.z_axis, [0.0, -0.5, 1.0]));
    }

    fn point(x: f32, z: f32, h: i16, flags: u16) -> TrackPoint {
        TrackPoint {
            x,
            z,
            height_raw: h,
            flags,
        }
    }

    /// A straight main line along +z, one point per 0.125 units, rising 1/4096 per point.
    fn straight_route(n: usize) -> Route {
        let slots: Vec<_> = (0..n)
            .map(|i| point(0.0, i as f32 * 0.125, i as i16, 0))
            .collect();
        Route {
            header: [0; 8],
            slots,
            segments: vec![crate::route::Segment {
                points: 0..n,
                head_marker: false,
                tail_marker: false,
                link_prev: None,
                link_next: None,
                reserved: [0; 2],
            }],
            traffic: vec![],
            placements: vec![
                crate::route::Placement {
                    part_id: 0,
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    yaw: 0.0,
                },
                crate::route::Placement {
                    part_id: 77,
                    x: 1.0,
                    y: 0.0,
                    z: 5.0,
                    yaw: 0.0,
                },
            ],
            buckets: vec![vec![]; n / 4],
            stops: vec![50, 400],
            activation_index: {
                let mut v = vec![0u16; n];
                v[40] = 1; // add placement 1 at engine index 40
                v[60] = 3; // remove it at 60
                v
            },
            activation_pool: vec![0, 1, 0, -1, 0],
            sections: vec![],
        }
    }

    fn tables() -> SceneTables {
        let mut rail = [[0u32; TRACK_TYPES]; RAIL_GROUPS];
        for (g, row) in rail.iter_mut().enumerate() {
            for (t, v) in row.iter_mut().enumerate() {
                *v = (g * 100 + t) as u32;
            }
        }
        let quad = Quad2d {
            xy: [[0; 2]; 4],
            colours: [0; 4],
        };
        SceneTables {
            areas: vec![
                AreaTables {
                    rail,
                    kinds: vec![]
                };
                AREA_COUNT
            ],
            fixed_kinds: [(3, [1, 2, 3, 4, 5]), (4, [0; 5]), (7, [0; 5]), (8, [0; 5])],
            single_group_pattern: [0, 1, 2, 1],
            rail_band_step: [1, 2, 4, 8, 16, 32],
            rail_band_scale: [1.0, 0.5, 0.25, 0.125, 0.0625, 0.03125],
            rail_reuse_dot: [0.5, 0.25, 0.0155, 0.062, 0.248, 0.9924, 3.97, 15.88],
            kind_band_step: [1, 1, 2, 4],
            kind_band_model: [2, 0, 3, 4],
            kind_band_scale: [1.0, 1.0, 0.5, 0.25],
            eye_height_by_com: [0.26; COM_COUNT],
            eye_height_by_area: [0.26; AREA_COUNT],
            tunnel_plugs: [[0; 28]; 2],
            animated: vec![],
            background: BackgroundTables {
                sky_sprites: vec![],
                com_sky: vec![None; COM_COUNT],
                keihin_switch: [0; 2],
                gradients: [vec![], vec![]],
                sky_quad: quad,
                ground_quad: quad,
                ground_top_quad: quad,
                ground_bottom_quad: quad,
                ground_tex_xy: [[0; 2]; 4],
                mask_quad: quad,
                events: vec![vec![]; COM_COUNT],
            },
        }
    }

    #[test]
    fn camera_interpolates_and_looks_one_point_ahead() {
        let r = straight_route(300);
        let pos = TrackPos {
            engine_index: 20,
            t: 0.0625,
        };
        let cam = cab_camera(&r, pos, 0.3, 0.0);
        // Slot 21 at z = 2.625, halfway to slot 22.
        assert!(close(cam.eye, [0.0, 21.5 / 4096.0 + 0.3, 2.6875]));
        assert!(close(cam.target, [0.0, 22.5 / 4096.0 + 0.3, 2.8125]));
        assert!(cam.view_z([0.0, 0.3, 10.0]) < 0.0);
    }

    #[test]
    fn walker_replays_activation_to_the_stop() {
        let r = straight_route(300);
        let mut w = CabWalker::at_segment_start(&r, 0);
        assert_eq!(w.pos().engine_index, 0);
        w.advance(&r, 0.0);
        // The catch-up loop stops when t is no longer > 0.125: one point short, t = 0.125.
        assert_eq!(w.pos().engine_index, 48);
        assert_eq!(w.pos().t, 0.125);
        assert_eq!(w.live(), &[1]);
        w.advance(&r, 11.0 * 0.125 + 0.01);
        assert_eq!(w.pos().engine_index, 60);
        assert!(w.live().is_empty());
        w.advance(&r, -0.125);
        assert_eq!(w.live(), &[1]);
        assert_eq!(CabWalker::at(&r, w.pos()).live(), &[1]);
    }

    #[test]
    fn main_line_pieces_follow_band_layout() {
        let r = straight_route(400);
        let t = tables();
        let ctx = SceneContext {
            area: 0,
            com: 1,
            station: 1,
        };
        let pos = TrackPos {
            engine_index: 100,
            t: 0.0,
        };
        let cam = cab_camera(&r, pos, 0.3, 0.0);
        let inst = track_instances(&r, &t, ctx, pos, &cam);
        // 22 singles + 4 + 2 + 1 + 1 + 2 band pieces, all ahead of the camera.
        assert_eq!(inst.len(), 32);
        let first = inst[0];
        assert_eq!(first.slot, pos.slot(&r) + MAIN_LINE_LEAD);
        // Phase (100 & 3 = 0) → group 0, track type 0.
        assert_eq!(first.part_id, 0);
        assert_eq!(inst[1].part_id, 100);
        // Single piece: local z spans the chord backwards, scaled to unit length.
        let z = first.transform.z_axis;
        assert!((z[2] + 1.1).abs() < 1e-4, "{z:?}"); // 1.1 × (−1) after the pitch
        // Band 1 pieces use group 3 and sit on the mid point of a 2-point span.
        let b1 = inst[22];
        assert_eq!(b1.source, PieceSource::MainLine { band: 1 });
        assert_eq!(b1.part_id, 300);
        assert!((b1.transform.translation[2] - r.slots[b1.slot + 1].z).abs() < 1e-6);
        // The second band-1 piece reuses the rotation (straight line, same slope).
        assert_eq!(inst[23].transform.z_axis, b1.transform.z_axis);
    }

    #[test]
    fn live_intervals_and_widening() {
        let mut r = straight_route(300);
        let iv = live_intervals(&r);
        assert_eq!(
            iv,
            [LiveInterval {
                placement: 1,
                from: 40,
                to: 60
            }]
        );
        assert_eq!(live_at(&iv, 59).collect::<Vec<_>>(), [1]);
        assert_eq!(live_at(&iv, 60).count(), 0);
        // 3 units to the side, 1.375 ahead of the eye at the removal point (slot 61, z 7.625).
        r.placements[1].x = 3.0;
        r.placements[1].z = 9.0;
        let view = WideView {
            tan_half_fov_x: 3.0,
            original_tan_half_fov_x: ORIGINAL_TAN_HALF_FOV_X,
            max_extra_points: 100,
        };
        let wide = widen_intervals(&r, &iv, view, |_| 0.1);
        assert_eq!(wide[0].to, 64);
        // Removed straight ahead: kept as is.
        r.placements[1].x = 0.0;
        assert_eq!(widen_intervals(&r, &iv, view, |_| 0.1)[0].to, 60);
    }

    #[test]
    fn kind_run_with_buffer_stop() {
        let mut r = straight_route(400);
        // A side run of 6 slots whose end slot carries 0xe000 | 2.
        r.slots[206].flags = 0xe002;
        r.buckets[25] = vec![TrackRun {
            kind: 3,
            unknown: 0,
            lod: [2, 0, 0, 0],
            slots: 200..206,
            dist_sq: 0.0,
        }];
        let t = tables();
        let ctx = SceneContext {
            area: 0,
            com: 1,
            station: 1,
        };
        let pos = TrackPos {
            engine_index: 100,
            t: 0.0,
        };
        // Eye near slot 205 so the buffer stop passes the radius test.
        let cam = CabCamera {
            eye: [0.0, 0.3, 25.0],
            target: [0.0, 0.3, 26.0],
        };
        let inst = track_instances(&r, &t, ctx, pos, &cam);
        let kinds: Vec<_> = inst
            .iter()
            .filter(|i| !matches!(i.source, PieceSource::MainLine { .. }))
            .collect();
        // 2 band-0 pieces (part list[2] = 3), remainder 6 − 2 − 1 = 3 pieces (list[0] = 1),
        // then 3 buffer-stop copies (list[1] = 2).
        let ids: Vec<_> = kinds.iter().map(|i| i.part_id).collect();
        assert_eq!(ids, [3, 3, 1, 1, 1, 2, 2, 2]);
        let x = kinds[0].transform.z_axis;
        assert!(
            (x[2] + 0.125).abs() < 1e-6,
            "kind pieces are not scaled by 8: {x:?}"
        );
    }
}

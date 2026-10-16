//! Weather and cab effects of the driving view (docs/re/renderer.md §13): the overlay that
//! 0x424740 creates when the timed weather event runs (a wiper, raindrops on the windscreen or
//! three scrolling snow layers with fog), the sky-palette fades of that event and the camera
//! jolt of a buffer-stop crash.
//!
//! The state is stepped once per simulation tick from the rules' weather state (`0x84c3a0`
//! active, `0x857230` level, `0x84c2e0` wiper flag) and the drive phase, and turned into 2D
//! quads in the original's 496×400 virtual space for the renderer's effect pass. Positions stay
//! in the original's 4:3 area (x 0..496) under Hor+.

use crate::scene::{RouteScene, ScreenMap};
use anyhow::Result;
use ddg_engine::drive::{Drive, DrivePhase};
use ddg_engine::rules::{AfterHold, Phase};
use ddg_formats::exe::Exe;
use ddg_formats::reader::Bytes;
use ddg_formats::scene::{PALETTE_FADE_TICKS, SpriteTexture};
use ddg_render::Camera;
use ddg_render::effects::EffectGeometry;

// --- Constants from code immediates (data-section values are read in `EffectTables::read`) ---

/// Drops alive at once (loop bound 10 at 0x42488e).
const DROP_COUNT: u32 = 10;
/// A drop task dies when its counter reaches 8 (0x42500d) and spawns its successor.
const DROP_LIFE: u32 = 8;
/// Drop placement: `x = rand·432 >> 15` (×0x1b0 at 0x424f60), `y = (rand·284 >> 15) + 52`
/// (×0x11c at 0x424f82, +0x34 at 0x424f9f); the quad is 63 units square (+0x3f at 0x424f99).
const DROP_X_RANGE: u32 = 0x1b0;
const DROP_Y_RANGE: u32 = 0x11c;
const DROP_TOP: i32 = 0x34;
const DROP_SIZE: i32 = 0x3f;
/// Depth words: wiper 0 (push 0 at 0x4249a2), drops 2 (0x42504e), snow layers 0x10 / 0x100 /
/// 0x200 (0x425179 / 0x4251b3 / 0x4251f6).
const WIPER_DEPTH: u16 = 0;
const DROP_DEPTH: u16 = 2;
const SNOW_DEPTHS: [u16; 3] = [0x10, 0x100, 0x200];
/// Snow texture: part 0x8de (push at 0x42508e), whose cached UVs 0x425070 overrides with
/// these page units (0x4250ba / 0x4250cb).
const SNOW_PART: u32 = 0x8de;
const SNOW_V_SPAN: i16 = 0x570;
const SNOW_U_SPAN: i16 = 0x7c0;
/// The snow task's counter step per tick (0x425113).
const SNOW_STEP: i32 = 8;
/// Bottom and right edges of the virtual screen used by the wiper shapes (0x18f at 0x424a56,
/// 0x424c8a, 0x424d8a; 0x1f0 at 0x424e27 / 0x424e59; 0x190 at 0x424a14; −399 at 0x424bc9).
const BOTTOM: i32 = 0x18f;
const RIGHT: i32 = 0x1f0;
const SHAPE0_LINE_Y: i32 = 0x190;
/// Camera point types that hide the drops and snow (0x4246f0: 0x1b..=0x1e, the tunnel types),
/// and the extra type hidden on stage 16 (0x424720, stage test at 0x424fd3).
const HIDDEN_TYPES: std::ops::RangeInclusive<u8> = 0x1b..=0x1e;
const STAGE16_HIDDEN_TYPE: u8 = 10;
const STAGE16: usize = 0x10;
/// Camera jolt of the buffer-stop crash: amplitude −0.018 (immediate 0xbc9374bc pushed at
/// 0x40a5ab when `0x8570b4 == 1`).
const CRASH_JOLT: f32 = f32::from_bits(0xbc93_74bc);
/// The jolt task 0x40e3d0 runs while |a| ≥ 1e-4 (f64 at 0x459480) and multiplies `a` by −0.8
/// (f64 at 0x4594e8) on even values of the global tick counter.
const JOLT_MIN: f64 = 1e-4;
const JOLT_DAMP: f64 = -0.8;

/// Weather type `0x857210`, a switch on the stage in 0x40d040.
fn weather_type(stage: usize) -> i16 {
    match stage {
        0..=2 | 17..=19 => 5,
        3 | 4 => 4,
        7 => 2,
        8..=11 => 1,
        _ => 0,
    }
}

/// The shape and swing table each weather type uses (0x424740's first switch) and its extras
/// (second switch): drops for types 0–2, the snow layers and fog for 4–5, nothing for 3.
fn overlay_kind(kind: i16) -> (usize, Extra) {
    match kind {
        0 => (0, Extra::Drops),
        1 => (1, Extra::Drops),
        2 => (2, Extra::Drops),
        3 => (1, Extra::None),
        4 => (0, Extra::Snow),
        _ => (2, Extra::Snow),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Extra {
    None,
    Drops,
    Snow,
}

/// `__ftol`: truncation toward zero, kept as the 16-bit word the callers store.
fn ftol(v: f64) -> i32 {
    i32::from(v as i32 as i16)
}

// --- Tables -----------------------------------------------------------------------------------

/// Swing of one wiper shape: angle limits and step per tick, radians.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Swing {
    min: f32,
    max: f32,
    speed: f32,
}

/// The overlay's constants in the exe's data section (0x47c31c..0x47c3b8, 0x4697b0).
#[derive(Debug, Clone, PartialEq)]
pub struct EffectTables {
    blade_colour: u16,
    arm_colour: u16,
    /// Corners of the snow layers (0x47c32c): the 3D band.
    band: [[i32; 2]; 4],
    swings: [Swing; 3],
    // Shape 0 (0x424950): an arm from a pivot far below the screen and a vertical blade.
    s0_pivot: [i32; 2],
    s0_radius: i32,
    half_width: i32,
    s0_blade_up: i32,
    s0_blade_down: i32,
    half_pi: f64,
    // Shape 1 (0x424aa0): a tapered blade around a pivot below the left corner, with an arm
    // from a second foot point.
    s1_pivot: [i32; 2],
    s1_tip: i32,
    s1_root: i32,
    s1_tip_half: f64,
    s1_root_half: f64,
    s1_root_offset: f32,
    s1_arm_angle: f32,
    s1_arm_radius: i32,
    s1_foot: [i32; 2],
    // Shape 2 (0x424cb0): a long arm from a pivot below the right edge; only its outer part
    // between the tip circle and the screen edges is drawn.
    s2_centre: [f32; 2],
    s2_radius: f32,
    s2_half: f32,
    s2_from: f32,
    s2_corner: [f32; 2],
    drop_parts: [u32; 4],
}

impl EffectTables {
    pub fn read(exe: &Exe<'_>) -> Result<Self> {
        let b = exe.reader_at(0x47c31c, 0x47c3b8 - 0x47c31c, "weather overlay tables")?;
        let o = |va: u32| (va - 0x47c31c) as usize;
        let i16_ = |b: &Bytes<'_>, va: u32| -> Result<i32> { Ok(i32::from(b.i16(o(va))?)) };
        let f32_ = |b: &Bytes<'_>, va: u32| -> Result<f32> { Ok(f32::from_bits(b.u32(o(va))?)) };
        let f64_ = |va: u32| -> Result<f64> {
            let r = exe.reader_at(va, 8, "weather overlay constant")?;
            Ok(f64::from_bits(
                u64::from(r.u32(0)?) | u64::from(r.u32(4)?) << 32,
            ))
        };
        let swing = |va: [u32; 3]| -> Result<Swing> {
            Ok(Swing {
                min: f32_(&b, va[0])?,
                max: f32_(&b, va[1])?,
                speed: f32_(&b, va[2])?,
            })
        };
        let mut band = [[0; 2]; 4];
        for (k, c) in band.iter_mut().enumerate() {
            let va = 0x47c32c + 4 * k as u32;
            *c = [i16_(&b, va)?, i16_(&b, va + 2)?];
        }
        let mut drop_parts = [0; 4];
        for (k, p) in drop_parts.iter_mut().enumerate() {
            *p = exe.u32_at(0x47c3a8 + 4 * k as u32)?;
        }
        Ok(Self {
            blade_colour: b.u16(o(0x47c31c))?,
            arm_colour: b.u16(o(0x47c324))?,
            band,
            swings: [
                swing([0x47c340, 0x47c344, 0x47c350])?,
                swing([0x47c358, 0x47c35c, 0x47c360])?,
                swing([0x47c380, 0x47c384, 0x47c388])?,
            ],
            s0_pivot: [i16_(&b, 0x47c33c)?, i16_(&b, 0x47c33e)?],
            s0_radius: i16_(&b, 0x47c348)?,
            half_width: i16_(&b, 0x47c34a)?,
            s0_blade_up: i16_(&b, 0x47c34c)?,
            s0_blade_down: i16_(&b, 0x47c34e)?,
            half_pi: f64_(0x4697b0)?,
            s1_pivot: [i16_(&b, 0x47c354)?, i16_(&b, 0x47c356)?],
            s1_tip: i16_(&b, 0x47c36c)?,
            s1_root: i16_(&b, 0x47c36e)?,
            s1_tip_half: f64_(0x47c3d8)?,
            s1_root_half: f64_(0x47c3d0)?,
            s1_root_offset: f32_(&b, 0x47c364)?,
            s1_arm_angle: f32_(&b, 0x47c368)?,
            s1_arm_radius: i16_(&b, 0x47c370)?,
            s1_foot: [i16_(&b, 0x47c372)?, exe.u32_at(0x47c374)? as i32],
            s2_centre: [f32_(&b, 0x47c378)?, f32_(&b, 0x47c37c)?],
            s2_radius: f32_(&b, 0x47c394)?,
            s2_half: f32_(&b, 0x47c38c)?,
            s2_from: f32_(&b, 0x47c39c)?,
            s2_corner: [f32_(&b, 0x47c3a0)?, f32_(&b, 0x47c3a4)?],
            drop_parts,
        })
    }

    /// The wiper's flat quads at angle `a` for `shape`, in virtual units, each with its colour
    /// word: `(corners, colour)`.
    fn wiper_quads(&self, shape: usize, a: f32) -> Vec<([[i32; 2]; 4], u16)> {
        let a64 = f64::from(a);
        let hw = self.half_width;
        match shape {
            0 => {
                // 0x424950: blade = vertical bar at the tip, arm = tip down to the bottom edge.
                let [px, py] = self.s0_pivot;
                let r = f64::from(self.s0_radius);
                let x = ftol(r * a64.cos()) + px;
                let y = py - ftol(r * a64.sin());
                let (up, down) = (y - self.s0_blade_up, y + self.s0_blade_down);
                let foot = ftol(
                    f64::from(px) - f64::from(py - SHAPE0_LINE_Y) * (a64 - self.half_pi).tan(),
                );
                vec![
                    (
                        [[x - hw, up], [x + hw, up], [x + hw, down], [x - hw, down]],
                        self.blade_colour,
                    ),
                    (
                        [
                            [x - hw, y],
                            [x + hw, y],
                            [foot + hw, BOTTOM],
                            [foot - hw, BOTTOM],
                        ],
                        self.arm_colour,
                    ),
                ]
            }
            1 => {
                // 0x424aa0: blade from the root circle (offset by 32°) to the tip circle.
                let [px, py] = self.s1_pivot;
                let polar = |r: i32, t: f64| {
                    let r = f64::from(r);
                    [ftol(r * t.cos()) + px, py - ftol(r * t.sin())]
                };
                let root = f64::from(self.s1_root_offset);
                let blade = [
                    polar(self.s1_root, root + a64 + self.s1_root_half),
                    polar(self.s1_tip, a64 + self.s1_tip_half),
                    polar(self.s1_tip, a64 - self.s1_tip_half),
                    polar(self.s1_root, a64 - self.s1_root_half + root),
                ];
                let arm_angle = f64::from(self.s1_arm_angle) + a64;
                let [fx, fy] = self.s1_foot;
                let foot = ftol(f64::from(fx) + f64::from(fy - BOTTOM) / arm_angle.tan());
                let [ax, ay] = polar(self.s1_arm_radius, arm_angle + self.s1_root_half);
                vec![
                    (blade, self.blade_colour),
                    (
                        [
                            [ax - hw, ay],
                            [ax + hw, ay],
                            [foot + hw, BOTTOM],
                            [foot - hw, BOTTOM],
                        ],
                        self.arm_colour,
                    ),
                ]
            }
            _ => {
                // 0x424cb0: the arm's outer end, from the tip circle to the right / bottom edge.
                if a < self.s2_from {
                    return Vec::new();
                }
                let [cx, cy] = self.s2_centre.map(f64::from);
                let (r, d) = (f64::from(self.s2_radius), f64::from(self.s2_half));
                let point = |t: f64| [ftol(t.cos() * r + cx), ftol(cy - t.sin() * r)];
                let p0 = point(d + a64);
                let p3 = point(a64 - d);
                let tan = a64.tan();
                let to_right =
                    |p: [i32; 2]| [RIGHT, ftol(f64::from(p[1]) - f64::from(RIGHT - p[0]) * tan)];
                let to_bottom = |p: [i32; 2]| {
                    [
                        ftol(f64::from(p[0]) - f64::from(BOTTOM - p[1]) / tan),
                        BOTTOM,
                    ]
                };
                let (p1, p2) = if a <= self.s2_corner[0] {
                    (to_right(p0), to_right(p3))
                } else if a <= self.s2_corner[1] {
                    (to_bottom(p0), to_right(p3))
                } else {
                    (to_bottom(p0), to_bottom(p3))
                };
                vec![([p0, p1, p2, p3], self.blade_colour)]
            }
        }
    }
}

/// Linear RGBA of a 2D colour word (0x432b50): ≥ 0x8000 is inline ARGB4444, otherwise palette
/// `hi` entry `lo`. The overlay only uses palette 0, which nothing in the exe writes (it stays
/// zero, so black and opaque); other palettes are not needed here and also give black.
fn colour(word: u16) -> [f32; 4] {
    if word >= 0x8000 {
        let n = |s: u16| (f32::from((word >> s) & 0xf) / 15.0).powf(2.2);
        [n(8), n(4), n(0), 1.0]
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

// --- State ------------------------------------------------------------------------------------

/// The C runtime's `rand` (0x437d10): `seed = seed·214013 + 2531011`, result bits 16–30.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CrtRand(u32);

impl CrtRand {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(0x343fd).wrapping_add(0x269ec3);
        (self.0 >> 16) & 0x7fff
    }
}

/// The wiper task (0x4248d0): angle +0x3c, direction +0x44.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Wiper {
    shape: usize,
    angle: f32,
    dir: i32,
}

impl Wiper {
    fn step(&mut self, swing: Swing, wipe: bool) {
        self.angle += self.dir as f32 * swing.speed;
        if self.dir > 0 {
            if self.angle > swing.max {
                self.dir = -self.dir;
            }
        } else if self.dir == 0 || self.angle < swing.min {
            self.dir = i32::from(wipe);
        }
    }
}

/// A drop task (0x424f20 / 0x425000): counter +0x30, quad origin, visibility +0x5c.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Drop {
    counter: u32,
    x: i32,
    y: i32,
    visible: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct Overlay {
    /// `None` once the wiper task ended (the drops may still run out).
    wiper: Option<Wiper>,
    drops: Vec<Drop>,
    /// The snow task's counter (+0x30); `None` without snow or after the task ended.
    snow: Option<i32>,
}

/// A palette-3 fade of the sky gradient (0x40f320 → task 0x436a60).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SkyFade {
    from: usize,
    frame: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Weather {
    tables: EffectTables,
    stage: usize,
    kind: i16,
    rng: CrtRand,
    overlay: Option<Overlay>,
    /// Created by a segment start while the rules have the event off (0x424740 sets
    /// `0x84c3a0` again): alive until the next driving tick.
    revived: bool,
    /// The wiper flag `0x84c2e0` as the overlay sees it.
    wipe: bool,
    sky_variant: usize,
    sky_fade: Option<SkyFade>,
    jolt: Option<f32>,
    /// The jolt the camera uses this tick (the value written to `0x850140` before the decay).
    jolt_now: f32,
    prev_on: bool,
    prev_level: i32,
    prev_phase: Phase,
}

impl Weather {
    /// The run's effect state. The CRT seed starts at 1; the original reseeds it from the
    /// global tick counter when a traffic train spawns (0x404f10), which the port can't
    /// reproduce, so drop positions follow the same law but not the same sequence.
    pub fn new(exe: &Exe<'_>, stage: usize) -> Result<Self> {
        Ok(Self {
            tables: EffectTables::read(exe)?,
            stage,
            kind: weather_type(stage),
            rng: CrtRand(1),
            overlay: None,
            revived: false,
            wipe: false,
            sky_variant: 0,
            sky_fade: None,
            jolt: None,
            jolt_now: 0.0,
            prev_on: false,
            prev_level: 0,
            prev_phase: Phase::PreDeparture,
        })
    }

    /// One simulation tick, after [`Drive::tick`]. `segment_started` is the tick's segment
    /// (re)start flag (0x4080d0's restart path).
    pub fn tick(&mut self, drive: &Drive, segment_started: bool) {
        let r = drive.rules();
        let (on, level) = (r.weather_on, r.weather_level);
        let driving = drive.phase() == DrivePhase::Driving;
        if level == 0 && !on {
            // A fresh run (restart from the first station).
            self.overlay = None;
            self.sky_fade = None;
            self.sky_variant = 0;
        }

        // The station result clears task list 5 (0x4080d0: 0x425380(0x84c1bc)).
        if matches!(drive.phase(), DrivePhase::Arrived | DrivePhase::Cleared) {
            self.overlay = None;
        }

        let camera_type = camera_point_type(drive);
        if segment_started {
            // 0x4080d0: the palette is rebuilt for the level (no fade) and the overlay comes
            // back while the level is above 0.
            if self.sky_fade.is_none() {
                self.sky_variant = usize::from(level >= 1);
            }
            if level > 0 {
                self.create(camera_type);
                self.revived = !on;
                // 0x4080d0 sets 0x84c2e0 above level 199.
                self.wipe = level > 199;
            }
        } else if on && !self.prev_on {
            // Course event 0x15 (0x40acd0) or the station-reached start (0x40d620): the
            // overlay and a 150-tick fade to the darker gradient set.
            self.create(camera_type);
            self.revived = false;
            self.wipe = false;
            self.sky_fade = Some(SkyFade { from: 0, frame: 0 });
            self.sky_variant = 1;
        } else if self.overlay.is_some() && driving {
            self.revived = false;
            self.wipe = r.wet_rails;
        }
        if level == -1 && self.prev_level > 0 {
            // 0x40c4e0: the level reached 1; fade back to the normal set.
            self.sky_fade = Some(SkyFade { from: 1, frame: 0 });
            self.sky_variant = 0;
        } else if let Some(f) = self.sky_fade.as_mut() {
            f.frame += 1;
            if f.frame >= PALETTE_FADE_TICKS {
                self.sky_fade = None;
            }
        }

        self.step_overlay(on || self.revived, camera_type);
        self.step_jolt(drive);
        self.prev_on = on;
        self.prev_level = level;
        self.prev_phase = r.phase;
    }

    /// 0x424740: the wiper at rest, 10 drops (counters `i & 7`) or the snow task.
    fn create(&mut self, camera_type: u8) {
        let (shape, extra) = overlay_kind(self.kind);
        let mut overlay = Overlay {
            wiper: Some(Wiper {
                shape,
                angle: self.tables.swings[shape].min,
                dir: 0,
            }),
            drops: Vec::new(),
            snow: (extra == Extra::Snow).then_some(0),
        };
        if extra == Extra::Drops {
            for i in 0..DROP_COUNT {
                let d = self.new_drop(i & 7, camera_type);
                overlay.drops.push(d);
            }
        }
        self.overlay = Some(overlay);
    }

    /// 0x424f20: a drop at a random place; its visibility is fixed at creation.
    fn new_drop(&mut self, counter: u32, camera_type: u8) -> Drop {
        let x = (self.rng.next() * DROP_X_RANGE) >> 15;
        let y = (self.rng.next() * DROP_Y_RANGE) >> 15;
        Drop {
            counter,
            x: x as i32,
            y: y as i32 + DROP_TOP,
            visible: self.visible(camera_type),
        }
    }

    /// 0x4246f0 / 0x424720: drops and snow are hidden at tunnel points (and type 10 on
    /// stage 16).
    fn visible(&self, camera_type: u8) -> bool {
        !(HIDDEN_TYPES.contains(&camera_type)
            || (self.stage == STAGE16 && camera_type == STAGE16_HIDDEN_TYPE))
    }

    /// The overlay tasks' updates (list 5, in creation order).
    fn step_overlay(&mut self, active: bool, camera_type: u8) {
        let Some(mut o) = self.overlay.take() else {
            return;
        };
        // Wiper (0x4248d0): returns 0, ending the task, once 0x84c3a0 is clear; its kill
        // callback 0x424940 clears the wiper flag.
        if let Some(w) = o.wiper.as_mut() {
            w.step(self.tables.swings[w.shape], self.wipe);
        }
        if !active && o.wiper.take().is_some() {
            self.wipe = false;
        }
        // Drops (0x425000): the successor is appended to the list, so it runs in the same pass.
        let mut drops = Vec::with_capacity(o.drops.len());
        for mut d in std::mem::take(&mut o.drops) {
            d.counter += 1;
            if d.counter < DROP_LIFE {
                drops.push(d);
            } else if active {
                let mut n = self.new_drop(0, camera_type);
                n.counter = 1;
                drops.push(n);
            }
        }
        o.drops = drops;
        // Snow (0x425100): +8 per tick; ends with the event (its kill callback turns the fog
        // off and lets the panorama draw again).
        o.snow = o.snow.filter(|_| active).map(|n| n + SNOW_STEP);
        // The drop tasks outlive the wiper until their own counters run out.
        if o.wiper.is_some() || !o.drops.is_empty() {
            self.overlay = Some(o);
        }
    }

    /// Starts a jolt task 0x40e3d0 with amplitude `a` (0x40e370; the coupling minigame).
    pub fn start_jolt(&mut self, a: f32) {
        self.jolt = Some(a);
    }

    /// The jolt task 0x40e3d0, started by a forced-stop arrival (0x40a0a0, `0x8570b4 == 1`).
    fn step_jolt(&mut self, drive: &Drive) {
        let r = drive.rules();
        let crashed = r.forced_stop
            && self.prev_phase == Phase::Driving
            && matches!(
                r.phase,
                Phase::Holding {
                    then: AfterHold::Drain,
                    ..
                }
            );
        if crashed {
            self.jolt = Some(CRASH_JOLT);
        }
        self.jolt_now = 0.0;
        if let Some(a) = self.jolt {
            if f64::from(a).abs() < JOLT_MIN {
                self.jolt = None;
            } else {
                self.jolt_now = a;
                // Parity of the global tick counter 0x84c494 (modelled by `Drive::global_tick`).
                if drive.global_tick().is_multiple_of(2) {
                    self.jolt = Some((f64::from(a) * JOLT_DAMP) as f32);
                }
            }
        }
    }

    /// Fog is on while the snow task lives (0x425070 → `Gfx_SetFog(1)`, its kill callback
    /// 0x425220 → `Gfx_SetFog(0)`), and only if DengoSetting's FOGENABLE is set.
    pub fn fog(&self) -> bool {
        self.overlay.as_ref().is_some_and(|o| o.snow.is_some())
    }

    /// Sets the scene's sky gradient (variant and fade) and hides the area-1 panorama while it
    /// snows (`0x8502ac = 0` from 0x425070 until 0x425220; read by 0x41dfc0 for area 1 when
    /// the COM is not 7 or 10).
    pub fn apply_to_scene(&self, scene: &mut RouteScene) {
        scene.gradient_variant = self.sky_variant;
        scene.gradient_fade = self.sky_fade.map(|f| (f.from, f.frame));
        scene.hide_panorama = self.fog() && scene.ctx.area == 1 && !matches!(scene.ctx.com, 7 | 10);
    }

    /// Lowers the eye by the jolt and the target by twice it, like `scene::cab_camera`'s
    /// `jolt` (0x40e3d0 writes the eye height 0x850140 and 0x850294).
    pub fn shake(&self, camera: &mut Camera) {
        camera.eye.y -= self.jolt_now;
        camera.target.y -= 2.0 * self.jolt_now;
    }

    /// Appends the overlay's quads for a frame, mapped into the 3D band of a target of band
    /// aspect `aspect` (width / band height). The caller clears `out` first; the drive's top
    /// mask goes in before these quads (`Play::cab_mask`).
    pub fn compose(
        &self,
        scene: &RouteScene,
        drive: &Drive,
        aspect: f32,
        out: &mut EffectGeometry,
    ) {
        let Some(o) = &self.overlay else {
            return;
        };
        let map = ScreenMap { aspect };
        let ndc = |c: [[i32; 2]; 4]| c.map(|[x, y]| map.ndc(x as f32, y as f32));
        // Draw order of list 5: wiper, then the drops, then (snow types) the snow task.
        if let Some(w) = &o.wiper {
            for (corners, word) in self.tables.wiper_quads(w.shape, w.angle) {
                out.add_flat_quad(ndc(corners), colour(word), WIPER_DEPTH);
            }
        }
        let sprite = |part: u32| -> Option<(usize, SpriteTexture)> {
            let group = scene.library.part(part)?.groups.first()?;
            let (page, rect) = scene.library.rect(group.rect_id)?;
            Some((page, SpriteTexture::from_group(group, &rect)?))
        };
        let uv = |p: [i16; 2]| p.map(|v| f32::from(v) / 256.0);
        for d in o.drops.iter().filter(|d| d.visible) {
            let frame = ((d.counter >> 1) & 3) as usize;
            let Some((page, tex)) = sprite(self.tables.drop_parts[frame]) else {
                continue;
            };
            let (x0, y0) = (d.x, d.y);
            let (x1, y1) = (x0 + DROP_SIZE, y0 + DROP_SIZE);
            out.add_tex_quad(
                page,
                ndc([[x0, y0], [x1, y0], [x1, y1], [x0, y1]]),
                tex.uv.map(uv),
                DROP_DEPTH,
            );
        }
        if let Some(n) = o.snow
            && self.visible(camera_point_type(drive))
            && let Some((page, tex)) = sprite(SNOW_PART)
        {
            // 0x425070's cached UVs: (u, v) per corner, v across the screen and u down it.
            let mut base = tex.uv;
            base[0] = [SNOW_U_SPAN, 0];
            base[1] = [SNOW_U_SPAN, SNOW_V_SPAN];
            base[2][1] = SNOW_V_SPAN;
            base[3][1] = 0;
            // 0x425120: three layers, each shifting u and v by a counter-derived offset.
            let offsets = [(-3 * n) & 0xff, (n + 2) & 0xff, (n / 2 + 4) & 0xff];
            for (off, depth) in offsets.into_iter().zip(SNOW_DEPTHS) {
                let uvs = base.map(|p| p.map(|v| v.wrapping_add(off as i16)));
                out.add_tex_quad(page, ndc(self.tables.band), uvs.map(uv), depth);
            }
        }
    }
}

/// Track type (`flags & 0x1f`) of the camera's point, `0x850114` (0x41d160).
fn camera_point_type(drive: &Drive) -> u8 {
    let route = drive.route();
    route
        .slots
        .get(drive.pos().slot(route))
        .map_or(0, |p| p.track_type())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crt_rand_sequence() {
        // MSVC rand() after srand(1): 41, 18467, 6334, 26500.
        let mut r = CrtRand(1);
        assert_eq!(
            [r.next(), r.next(), r.next(), r.next()],
            [41, 18467, 6334, 26500]
        );
    }

    #[test]
    fn weather_types_by_stage() {
        assert_eq!(overlay_kind(weather_type(0)), (2, Extra::Snow));
        assert_eq!(overlay_kind(weather_type(3)), (0, Extra::Snow));
        assert_eq!(overlay_kind(weather_type(7)), (2, Extra::Drops));
        assert_eq!(overlay_kind(weather_type(9)), (1, Extra::Drops));
        assert_eq!(overlay_kind(weather_type(16)), (0, Extra::Drops));
    }

    #[test]
    fn wiper_swings_only_while_wet_and_parks_below_min() {
        let s = Swing {
            min: 1.0,
            max: 1.1,
            speed: 0.04,
        };
        let mut w = Wiper {
            shape: 0,
            angle: s.min,
            dir: 0,
        };
        w.step(s, false);
        assert_eq!((w.angle, w.dir), (1.0, 0));
        w.step(s, true);
        assert_eq!(w.dir, 1);
        let mut top: f32 = 0.0;
        for _ in 0..3 {
            w.step(s, true);
            top = top.max(w.angle);
        }
        // Up past max (1.12 > 1.1), then down.
        assert!(top > s.max && w.dir == -1);
        for _ in 0..4 {
            w.step(s, false);
        }
        // Below min with the flag off: parked.
        assert!(w.angle < s.min && w.dir == 0);
    }

    /// The tables from the user's exe (skips unless `DDG_DATA_DIR` is set), and the shapes at
    /// the rest angle.
    #[test]
    fn tables_and_rest_shapes_from_the_exe() {
        let Some(root) = std::env::var_os("DDG_DATA_DIR") else {
            eprintln!("skipped: DDG_DATA_DIR not set");
            return;
        };
        let path =
            std::path::PathBuf::from(root).join("install/Program_Executable_Files/BIN/dengo3k.exe");
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("skipped: {} unreadable", path.display());
            return;
        };
        let t = EffectTables::read(&Exe::parse(&bytes).unwrap()).unwrap();
        assert_eq!((t.blade_colour, t.arm_colour), (0x39, 0x3c));
        assert_eq!(t.band, [[0, 52], [496, 52], [496, 400], [0, 400]]);
        assert_eq!(t.s0_pivot, [399, 1167]);
        assert_eq!(t.drop_parts, [0x299e, 0x299f, 0x29a0, 0x29a1]);
        // Swing limits in degrees: 86–112 by 1.5, 15–90 by 5, 40–140 by 2.
        let deg = |s: Swing| [s.min, s.max, s.speed].map(|v| (v.to_degrees() * 10.0).round());
        assert_eq!(deg(t.swings[0]), [860.0, 1120.0, 15.0]);
        assert_eq!(deg(t.swings[1]), [150.0, 900.0, 50.0]);
        assert_eq!(deg(t.swings[2]), [400.0, 1400.0, 20.0]);
        // Shape 0 at rest: blade at the tip (399 + ⌊952·cos 86°⌋, 1167 − ⌊952·sin 86°⌋) =
        // (465, 218), 3 units wide, from 112 above to 80 below.
        let q = t.wiper_quads(0, t.swings[0].min);
        assert_eq!(q[0].0[0], [465 - 3, 218 - 112]);
        assert_eq!(q[1].0[2][1], BOTTOM);
        // Shape 2 is off screen below 94.3°, so at rest.
        assert!(t.wiper_quads(2, t.swings[2].min).is_empty());
        assert_eq!(t.wiper_quads(2, t.swings[2].max).len(), 1);
    }

    #[test]
    fn ftol_truncates_to_a_word() {
        assert_eq!(ftol(-2.7), -2);
        assert_eq!(ftol(2.7), 2);
        assert_eq!(ftol(65536.0 + 5.0), 5);
    }
}

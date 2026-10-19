//! The attract cold-start opening (item 2, 0x402e80 → 0x402fb0 … 0x403750, docs/re/menus.md
//! §2.1), played before the title picture when the walk started at item 0. One task with seven
//! phases:
//!
//! * A (0x402fb0, 145 updates) and C (0x4031d0, 94): render mode 2, a fixed camera on a
//!   track-bed strip (0x2f83) with the Komachi consist (model list `*0x4573dc`) coming head-on,
//!   a platform photo (tbl 0x198) over it, all in the 496×400 window at (96, 56) that
//!   `Viewport_Apply` gives mode 2 (black around it);
//! * B (0x403080) and D (0x4032a0), 57 each: black 3D, a 480×208 picture with white flashes
//!   (palette 0x14) and the 3000番台 badge;
//! * E (0x403420, 89): the real route from Morioka (stage 1 station 24, AKITA1C) at 30°, with
//!   an E3 set passing and the speed readout climbing to 275;
//! * F (0x403540, 91): a white studio with the EF63 pair and the 489 car in three camera shots
//!   and the 3000番台 scroller;
//! * G (0x403750, 33): the navigator girl, then the title picture.
//!
//! BGM 0x1a1 starts at the creation and plays on through the title picture.

use super::Play;
use crate::slots;
use anyhow::{Context, Result};
use glam::{Mat4, Vec3};
use shuppatsu_engine::traffic::SpawnRequest;
use shuppatsu_formats::exe::Exe;
use shuppatsu_formats::{part::Part, rom2::Rom2};
use shuppatsu_render::effects::EffectGeometry;
use shuppatsu_render::sprites::Fan;
use shuppatsu_render::world::{BgVertex, bg_depth};
use shuppatsu_render::{Camera, FrameGeometry, PartLibrary, Projection, SubView};

/// The BGM (0x42ff20(0x1a1)), released by the title picture's destroy 0x403d20.
pub const MUSIC: u16 = 0x1a1;
/// TR sets of the attract assets (`stage_assets_load(0x15)`: 0x463c74 row 8 = {1, 8}).
const TR_SETS: [usize; 2] = [1, 8];
/// Picture frames (0x457a20 i32[15]): B 0..5, D 5..10, G 10..15.
const FRAMES: u32 = 0x457a20;
/// The Komachi consist: pointer at 0x4573dc to 28-byte records `{part[5], offset, spacing}`.
const CARS_PTR: u32 = 0x4573dc;
/// Slots: picture 0x10, badge 0x26 (tbl 0x123 at (0x170, 0x154)), speed readout 0x75.
const SLOT_PICTURE: usize = 0x10;
const SLOT_BADGE: usize = 0x26;
const SLOT_SPEED: usize = 0x75;
const TBL_PHOTO: i16 = 0x198;
const TBL_BADGE: i16 = 0x123;
const TBL_SPEED: i16 = 0x53;
const TBL_SCROLLER: i16 = 0x443;
const BADGE_POS: (i16, i16) = (0x170, 0x154);
const PHOTO_POS: (i16, i16) = (0x60, 0x38);
const PICTURE_POS: (i16, i16) = (0x50, 0x7c);
const SPEED_POS: (i16, i16) = (64, 112);
const PAL_PHOTO: u8 = 0x11;
const PAL_PICTURE: u8 = 0xe;
const PAL_FLASH: u8 = 0x14;
/// Phase lengths: an update with the counter at this value switches (0x90, 0x38, 0x5d, 0x38,
/// 0x58, 0x5a, 0x20).
const ENDS: [i32; 7] = [0x90, 0x38, 0x5d, 0x38, 0x58, 0x5a, 0x20];
/// Studio train: z from 10.0, −0.04 per update in A (f64 0x458ac0); from 7.0, −0.1953125 in
/// C (0x458ac8); cars at (0.15, −0.2, t + 0.5·L − offset).
const T_A: f32 = 10.0;
const STEP_A: f64 = 0.039_999_996;
const T_C: f32 = 7.0;
const STEP_C: f64 = 0.195_312_5;
const TRAIN_X: f32 = 0.15;
const TRAIN_Y: f32 = -0.2;
/// Render mode 2's camera (0x425680 case 2): 45°, the whole 496×400 screen.
const STUDIO_EYE: [f32; 3] = [0.5, 0.13, -2.488];
const STUDIO_TARGET: [f32; 3] = [-1.041, 0.08, 2.551];
const STUDIO_FOV: u16 = 450;
/// The backdrop gradient (0x4692e0, virtual y 64..384): palette 3 of area 2, sky
/// (213, 213, 238) at the top to ground (200, 200, 200) at the bottom (0x463b58).
const SKY: [u8; 3] = [213, 213, 238];
const GROUND: [u8; 3] = [200, 200, 200];
/// Phase E: Morioka (stage 1 station 24), the camera 150 units on, FOV 30°; spawn 2, flags
/// 0x11, 270 km/h, model 7; v from 0.1852 (f32 0x3e3da512) +0.0038892 per update up to 0.51,
/// readout `trunc(v · 0.5 · 1079.913607)`.
const ROUTE_STAGE_STATION: usize = 24;
const ROUTE_JUMP: f32 = 150.0;
const ROUTE_FOV: u16 = 300;
const ROUTE_V0: f32 = f32::from_bits(0x3e3d_a512);
const ROUTE_ACCEL: f64 = 0.003_889_2;
const ROUTE_TOP: f64 = 0.51;
const READOUT: f64 = 0.5 * 1_079.913_607;
/// Phase F (immediates of 0x403540): parts, keys `(from c, z20, z40, eye, target, fov)`, the
/// slide −0.15 per update from c 41 (0x458ae8) and the scroller at (20c − 1376, 140).
const PART_SET: u32 = 0x2df3;
const PART_CAR: u32 = 0x327a;
const PART_LOCO: u32 = 0x2df2;
const PART_BED: u32 = 0x2f83;
const STUDIO_X: f32 = -1.5;
const LOCO_SPACING: f32 = 3.9;
type Key = (i32, f32, f32, [f32; 3], [f32; 3], u16);
const KEYS: [Key; 3] = [
    (
        0,
        4.28,
        -5.28,
        [-2.8661, 0.93, -9.4129],
        [-0.9659, -0.03, 1.442],
        160,
    ),
    (
        20,
        4.28,
        -6.13,
        [-1.9393, 0.95, -7.3119],
        [-1.4893, 0.21, 1.4766],
        120,
    ),
    (
        40,
        -1.11,
        -4.89,
        [-7.89, 0.95, -4.3306],
        [0.88, -0.02, -4.3198],
        220,
    ),
];
const SLIDE: f32 = 0.15;
const SLIDE_FROM: i32 = 41;
/// Render mode 2's 3D window in original 640×480 pixels. From 640×480 up (`0x816afc != 0`)
/// `Viewport_Apply` (0x436e14) draws the full 496×400 virtual frame of mode 2 unscaled at
/// ((W − 480)/2 + 16, (H − 400)/2 + 16) (16.0 at 0x47cbec), which is the rectangle of the
/// platform photo (tbl 0x198 at (96, 56)); the rest of the screen stays black.
const MODE2_WINDOW: [f32; 4] = [96.0, 56.0, 496.0, 400.0];
/// Render mode 2's projection aspect (f32 1.25 for 0x4183b0 in 0x425680 case 2).
const MODE2_ASPECT: f32 = 1.25;
/// The black bars of phases E and F (draw callbacks 0x403c70 / 0x403a60): x 0..64 and
/// 576..640.
const INSET_LEFT: i32 = 64;
const INSET_RIGHT: i32 = 576;
/// Mask quads in virtual y: the top band 0..94 and the bottom band 268..400.
const VIEW_TOP: f32 = 52.0;
const VIEW_H: f32 = 348.0;

/// What the 3D layer shows in a phase.
pub(super) struct CustomView {
    pub camera: Camera,
    pub fov_tenths: u16,
    /// Virtual y where the 3D viewport starts (52 normally, 0 in render mode 2).
    pub full_screen: bool,
    /// The 3D drawn only in this rectangle of the 640×480 design (original pixels) with the
    /// rest black: render mode 2's 1:1 window ([`MODE2_WINDOW`]).
    pub window: Option<[f32; 4]>,
}

#[derive(Debug, Clone)]
pub(super) struct Opening {
    phase: usize,
    c: i32,
    /// The studio train's z (task+0x20) and phase F's z20 / z40.
    t: f32,
    z20: f32,
    z40: f32,
    /// Phase E's camera speed, units per tick (`0x850208`).
    v: f32,
    frames: [i16; 15],
    cars: Vec<(u32, f32)>,
    lead: f32,
    /// The 3D is drawn (a draw callback is set).
    draw: bool,
    pub(super) started: bool,
}

impl Play {
    /// Starts the opening on this run of stage 1 (0x402e80, cold path).
    pub fn start_opening(&mut self) -> Result<()> {
        let exe_bytes = self.data.exe()?;
        let exe = Exe::parse(&exe_bytes)?;
        let r = exe.reader_at(FRAMES, 60, "opening frames")?;
        let mut frames = [0; 15];
        for (i, f) in frames.iter_mut().enumerate() {
            *f = r.i32(4 * i)? as i16;
        }
        let mut cars = Vec::new();
        let mut va = exe.u32_at(CARS_PTR)?;
        for _ in 0..8 {
            let r = exe.reader_at(va, 0x1c, "opening cars")?;
            let part = r.u32(0)?;
            if part == 0 {
                break;
            }
            cars.push((part, r.f32(0x14)?));
            va += 0x1c;
        }
        let lead = exe
            .reader_at(exe.u32_at(CARS_PTR)? + 0x1c + 0x18, 4, "opening cars")?
            .f32(0)?;
        for k in TR_SETS {
            self.load_set(k)?;
        }
        self.layer
            .slots
            .fill(shuppatsu_render::sprites::Slot::CLEAR);
        self.layer.widgets.clear();
        // `0x436c20(0x14, 0xffffff)`: the flash palette is all white.
        self.layer.palettes[usize::from(PAL_FLASH)] =
            Some(shuppatsu_formats::cg::Palette([0x00ff_ffff; 256]));
        slots::place(&mut self.layer, SLOT_BADGE, BADGE_POS.0, BADGE_POS.1, 3, 4);
        if let Some(s) = self.sound.as_mut() {
            s.request_ungated(MUSIC);
        }
        self.world = crate::screens::flow::WorldView::Hidden;
        self.opening = Some(Opening {
            phase: 0,
            c: 0,
            t: T_A,
            z20: 0.0,
            z40: 0.0,
            v: 0.0,
            frames,
            cars,
            lead,
            draw: false,
            started: false,
        });
        Ok(())
    }

    /// This run is the attract opening.
    pub fn in_opening(&self) -> bool {
        self.opening.is_some()
    }

    /// The opening ended by START.
    pub fn opening_ended_by_start(&self) -> bool {
        self.opening.as_ref().is_some_and(|o| o.started)
    }

    fn load_set(&mut self, k: usize) -> Result<()> {
        let tex = format!("texTR{}.rom", k + 1);
        self.data.add_textures(&mut self.scene.library, &tex)?;
        let parts = format!("partsTR{}.rom", k + 1);
        let bytes = self.data.read(&parts)?;
        let entries = Rom2::parse(&bytes)?
            .iter()
            .map(Part::parse_entry)
            .collect::<shuppatsu_formats::Result<Vec<_>>>()
            .with_context(|| parts.clone())?;
        self.scene.library.add_parts(entries);
        if let Some(g) = self.gpu.as_mut() {
            g.renderer
                .upload_pages(&g.device, &g.queue, &self.scene.library.pages);
        }
        Ok(())
    }

    /// One update of the opening task, then list 5 in phase E.
    pub(super) fn opening_tick(&mut self) -> Result<()> {
        self.ticks += 1;
        let Some(mut o) = self.opening.take() else {
            return Ok(());
        };
        if self.menu_input.confirm {
            // START: 0x855ff8 & 4 cleared, the task's destroy runs (BGM released).
            o.started = true;
            if let Some(s) = self.sound.as_mut() {
                s.release(MUSIC);
            }
            self.leave = true;
            self.opening = Some(o);
            return Ok(());
        }
        let mut route_units = None;
        let l = &mut self.layer;
        l.fans.clear();
        l.widgets.clear();
        // On a wide canvas (enhancement `wide_hud`) the bars reach its edges, like the 4:3
        // screen edges they end at in the original.
        let room = l.side_room().ceil() as i32;
        match o.phase {
            0 | 2 => {
                if o.phase == 2 && o.c == 0 {
                    o.t = T_C;
                }
                if o.c == if o.phase == 0 { 2 } else { 0 } {
                    o.draw = true;
                    slots::set(l, SLOT_PICTURE, TBL_PHOTO, PHOTO_POS, PAL_PHOTO, 3);
                } else {
                    let step = if o.phase == 0 { STEP_A } else { STEP_C };
                    o.t = (f64::from(o.t) - step) as f32;
                }
                // Draw 0x403920's rect-layer bars (top and bottom 40 px).
                if o.draw {
                    l.fans.push(bar(-room, 0, 640 + room, 40));
                    l.fans.push(bar(-room, 440, 640 + room, 480));
                }
            }
            1 | 3 => {
                if o.c == 0 {
                    slots::image(l, SLOT_BADGE, TBL_BADGE);
                    o.draw = false;
                }
                let base = if o.phase == 1 { 0 } else { 5 };
                let (tbl, pal) = flash_frame(&o.frames[base..base + 5], o.c);
                if o.c < ENDS[o.phase] {
                    slots::set(l, SLOT_PICTURE, tbl, PICTURE_POS, pal, 3);
                }
            }
            4 => {
                if o.c == 0 {
                    let req = SpawnRequest {
                        spawn: 2,
                        flags: 0x11,
                        speed_kmh: 270,
                        param: 0,
                        model: 7,
                    };
                    self.drive.ending_spawn(&req);
                    o.v = ROUTE_V0;
                } else {
                    let v = f64::from(o.v) + ROUTE_ACCEL;
                    o.v = if v >= ROUTE_TOP {
                        ROUTE_TOP as f32
                    } else {
                        v as f32
                    };
                }
                let kmh = (f64::from(o.v) * READOUT) as i32;
                if kmh > 0 {
                    slots::set(l, SLOT_SPEED, TBL_SPEED, SPEED_POS, 4, 3);
                } else {
                    slots::clear(l, SLOT_SPEED);
                }
                self.cab.hud.segments(kmh, l);
                side_bars(l, room);
                route_units = Some(o.v);
            }
            5 => {
                if o.c == 0 {
                    slots::image(l, SLOT_BADGE, TBL_BADGE);
                    slots::clear(l, SLOT_SPEED);
                    o.draw = true;
                }
                let key = KEYS
                    .iter()
                    .rev()
                    .find(|k| o.c >= k.0)
                    .copied()
                    .unwrap_or(KEYS[0]);
                if o.c == key.0 {
                    (o.z20, o.z40) = (key.1, key.2);
                }
                if o.c >= SLIDE_FROM && o.c < ENDS[5] {
                    o.z20 -= SLIDE;
                    o.z40 -= SLIDE;
                    let x = (20 * o.c - 1376) as i16;
                    slots::set(l, SLOT_PICTURE, TBL_SCROLLER, (x, 140), PAL_PICTURE, 3);
                }
                if o.c < ENDS[5] {
                    side_bars(l, room);
                }
            }
            _ => {
                o.draw = false;
                let i = match o.c {
                    0..=2 => 10 + o.c as usize,
                    3..=23 => 13,
                    _ => 14,
                };
                if o.c < ENDS[6] {
                    slots::set(l, SLOT_PICTURE, o.frames[i], PICTURE_POS, PAL_PICTURE, 3);
                }
            }
        }
        if o.c >= ENDS[o.phase] {
            match o.phase {
                1 => {
                    slots::clear(l, SLOT_PICTURE);
                    slots::image(l, SLOT_BADGE, -1);
                    o.draw = true;
                    o.t = T_C;
                }
                3 => {
                    // The hand-off to the drive: 0x408080(1, 24), 30°, 150 units on.
                    slots::clear(l, SLOT_PICTURE);
                    self.drive.ending_place(ROUTE_STAGE_STATION)?;
                    route_units = Some(ROUTE_JUMP);
                }
                4 => self.drive.ending_kill(),
                5 => {
                    o.draw = false;
                    slots::clear(&mut self.layer, SLOT_PICTURE);
                }
                6 => {
                    slots::clear(&mut self.layer, SLOT_PICTURE);
                    slots::image(&mut self.layer, SLOT_BADGE, -1);
                    self.leave = true;
                }
                _ => {}
            }
            o.phase += 1;
            o.c = 0;
        } else {
            o.c += 1;
        }
        match route_units {
            Some(u) => {
                self.drive.ending_tick(u);
                self.scene.traffic = self.drive.traffic_poses(1.0);
            }
            None => self.drive.idle_global_tick(),
        }
        if self.drive.com() != self.scene_com {
            self.reload_scene()?;
            for k in TR_SETS {
                self.load_set(k)?;
            }
        }
        self.world = if o.phase == 4 {
            crate::screens::flow::WorldView::Cab
        } else {
            crate::screens::flow::WorldView::Hidden
        };
        if let Some(s) = self.sound.as_mut() {
            s.traffic(self.drive.traffic_sounds());
            s.menu_frame();
        }
        self.interpolate = o.phase == 4;
        self.poll();
        self.layer.compose(&self.bank, &mut self.canvas);
        self.canvas_dirty = true;
        self.opening = Some(o);
        Ok(())
    }

    /// The studio views (render mode 2 in A and C, the white studio of F) and their effects;
    /// `None` when the drive's camera is used or nothing is drawn.
    pub(super) fn opening_compose(
        opening: Option<&Opening>,
        library: &PartLibrary,
        frame: &mut FrameGeometry,
        fx: &mut EffectGeometry,
        aspect: f32,
    ) -> Option<CustomView> {
        let o = opening?;
        if o.phase == 4 {
            // Phase E: the drive's view under the bottom band (0x486ec4, `0x84c0f8 = 1`).
            return None;
        }
        frame.clear(library);
        fx.clear();
        let black = [0.0, 0.0, 0.0, 1.0];
        let quad = |a: f32, b: f32| [[-1.0, a], [1.0, a], [1.0, b], [-1.0, b]];
        if !o.draw {
            return Some(CustomView {
                camera: studio_camera(),
                fov_tenths: STUDIO_FOV,
                full_screen: true,
                window: None,
            });
        }
        if o.phase == 5 {
            let ndc = |vy: f32| 1.0 - 2.0 * ((vy - VIEW_TOP) / VIEW_H);
            fx.add_flat_quad(quad(1.0, ndc(94.0)), black, 0);
            fx.add_flat_quad(quad(ndc(268.0), -1.0), black, 0);
            fx.add_flat_quad(quad(ndc(64.0), ndc(384.0)), [1.0; 4], 0x7ffe);
            inset_masks(fx, aspect);
            let at = |z: f32| Mat4::from_translation(Vec3::new(STUDIO_X, 0.0, z));
            frame.add_instance(library, PART_SET, &Mat4::IDENTITY);
            frame.add_instance(library, PART_CAR, &at(o.z40));
            frame.add_instance(library, PART_LOCO, &at(o.z20));
            frame.add_instance(library, PART_LOCO, &at(o.z20 + LOCO_SPACING));
            let key = KEYS
                .iter()
                .rev()
                .find(|k| o.c >= k.0)
                .copied()
                .unwrap_or(KEYS[0]);
            return Some(CustomView {
                camera: Camera {
                    eye: Vec3::from(key.3),
                    target: Vec3::from(key.4),
                },
                fov_tenths: key.5,
                full_screen: false,
                window: None,
            });
        }
        // Render mode 2: the 496×400 window; the backdrop from virtual y 64 to 384.
        let ndc = |vy: f32| 1.0 - 2.0 * (vy / 400.0);
        gradient(fx, ndc(64.0), ndc(384.0));
        frame.add_instance(library, PART_BED, &Mat4::IDENTITY);
        let z = o.t + 0.5 * o.lead;
        for &(part, offset) in &o.cars {
            let m = Mat4::from_translation(Vec3::new(TRAIN_X, TRAIN_Y, z - offset));
            frame.add_instance(library, part, &m);
        }
        Some(CustomView {
            camera: studio_camera(),
            fov_tenths: STUDIO_FOV,
            full_screen: true,
            window: Some(MODE2_WINDOW),
        })
    }

    /// Phase E's top and bottom bands and inset bars over the drive's view; `aspect` is the target's
    /// width / height.
    pub(super) fn opening_masks(opening: Option<&Opening>, fx: &mut EffectGeometry, aspect: f32) {
        if opening.is_some_and(|o| o.phase == 4) {
            let ndc = |vy: f32| 1.0 - 2.0 * ((vy - VIEW_TOP) / VIEW_H);
            fx.add_flat_quad(
                [
                    [-1.0, ndc(268.0)],
                    [1.0, ndc(268.0)],
                    [1.0, -1.0],
                    [-1.0, -1.0],
                ],
                [0.0, 0.0, 0.0, 1.0],
                0,
            );
            // The drive's own callback 0x41e460 masks virtual rows 0..94 (0x486160,
            // scene.md §2.1), which the cab HUD hides in a run but the opening shows black.
            fx.add_flat_quad(
                [[-1.0, 1.0], [1.0, 1.0], [1.0, ndc(94.0)], [-1.0, ndc(94.0)]],
                [0.0, 0.0, 0.0, 1.0],
                0,
            );
            inset_masks(fx, aspect);
        }
    }

    /// The FOV of phase E (30°).
    pub(super) fn opening_route_fov(opening: Option<&Opening>) -> Option<u16> {
        opening.filter(|o| o.phase == 4).map(|_| ROUTE_FOV)
    }
}

fn studio_camera() -> Camera {
    Camera {
        eye: Vec3::from(STUDIO_EYE),
        target: Vec3::from(STUDIO_TARGET),
    }
}

/// Phases E and F: the black bars left and right of the inset, out to the canvas edges.
fn side_bars(l: &mut shuppatsu_render::sprites::SpriteLayer, room: i32) {
    l.fans.push(bar(-room, 0, INSET_LEFT, 480));
    l.fans.push(bar(INSET_RIGHT, 0, 640 + room, 480));
}

/// NDC x of original canvas x in a 3D view spanning a window of `aspect` (width / height):
/// the 640×480 design is centred at full height.
fn canvas_ndc_x(x: i32, aspect: f32) -> f32 {
    (x as f32 / 320.0 - 1.0) * (4.0 / 3.0) / aspect
}

/// Phases E and F: the 3D outside the inset x 64..576 is black on any window shape, as
/// the 4:3 screen shows it (the canvas bars only cover the canvas).
fn inset_masks(fx: &mut EffectGeometry, aspect: f32) {
    let black = [0.0, 0.0, 0.0, 1.0];
    let (l, r) = (
        canvas_ndc_x(INSET_LEFT, aspect),
        canvas_ndc_x(INSET_RIGHT, aspect),
    );
    fx.add_flat_quad([[-1.0, 1.0], [l, 1.0], [l, -1.0], [-1.0, -1.0]], black, 0);
    fx.add_flat_quad([[r, 1.0], [1.0, 1.0], [1.0, -1.0], [r, -1.0]], black, 0);
}

/// Where render mode 2's window ([`CustomView::window`]) lands in a `size` target: the
/// rectangle scaled with the centred 640×480 design, so it stays on the platform photo, and
/// the projection aspect 1.25 after the 4.12 round trip of 0x4183b0 (or square pixels).
pub(super) fn window_view(
    window: [f32; 4],
    fov_tenths: u16,
    (width, height): (u32, u32),
    square_pixels: bool,
) -> SubView {
    let [cx, cy, cw, _] = shuppatsu_render::overlay::canvas_rect(width, height);
    let s = cw / 640.0;
    let rect = [
        cx + window[0] * s,
        cy + window[1] * s,
        window[2] * s,
        window[3] * s,
    ];
    let quantised = 4096.0 / (4096.0 / MODE2_ASPECT).trunc();
    let stretch = if square_pixels {
        1.0
    } else {
        (rect[2] / rect[3]) / quantised
    };
    SubView {
        rect,
        projection: Projection {
            fov_tenths,
            stretch,
            ..Default::default()
        },
        clear: true,
    }
}

/// A black rect-layer bar in canvas pixels.
fn bar(x0: i32, y0: i32, x1: i32, y1: i32) -> Fan {
    Fan {
        points: [[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
        count: 4,
        argb: 0xff00_0000,
        anchor: Default::default(),
    }
}

/// The backdrop quad: sky colour at the top, ground colour at the bottom, far depth.
fn gradient(fx: &mut EffectGeometry, top: f32, bottom: f32) {
    let lin = |c: [u8; 3]| {
        let f = |v: u8| (f32::from(v) / 255.0).powf(2.2);
        [f(c[0]), f(c[1]), f(c[2]), 1.0]
    };
    let (s, g) = (lin(SKY), lin(GROUND));
    let z = bg_depth(0x7ffe);
    let v = |x: f32, y: f32, colour: [f32; 4]| BgVertex {
        pos: [x, y, z],
        colour,
    };
    let corners = [
        v(-1.0, top, s),
        v(1.0, top, s),
        v(1.0, bottom, g),
        v(-1.0, bottom, g),
    ];
    for i in [0, 1, 2, 0, 2, 3] {
        fx.flat.push(corners[i]);
    }
}

/// Phases B and D: the first frame for 20 updates, then frames 1–4 each starting with a white
/// flash (palette 0x14) at counts 20, 26, 32, 40 (stack table of 0x403080).
fn flash_frame(f: &[i16], c: i32) -> (i16, u8) {
    match c {
        0..=19 => (f[0], PAL_PICTURE),
        20 => (f[1], PAL_FLASH),
        21..=25 => (f[1], PAL_PICTURE),
        26 => (f[2], PAL_FLASH),
        27..=31 => (f[2], PAL_PICTURE),
        32 => (f[3], PAL_FLASH),
        33..=39 => (f[3], PAL_PICTURE),
        40 => (f[4], PAL_FLASH),
        _ => (f[4], PAL_PICTURE),
    }
}

impl Play {
    /// A 3D view of its own (the bonus game's set, the opening's studio), if one is up.
    pub(super) fn custom_view(
        drive: &shuppatsu_engine::drive::Drive,
        opening: Option<&Opening>,
        library: &PartLibrary,
        frame: &mut FrameGeometry,
        fx: &mut EffectGeometry,
        aspect: f32,
    ) -> Option<CustomView> {
        if let Some(camera) = Self::bonus_compose(drive, library, frame, fx) {
            return Some(CustomView {
                camera,
                fov_tenths: super::bonus::FOV_TENTHS,
                full_screen: false,
                window: None,
            });
        }
        Self::opening_compose(opening, library, frame, fx, aspect)
    }

    /// Headless frame of the bonus game or the opening.
    pub(super) fn custom_shot(&self, out: &std::path::Path) -> Result<()> {
        let (w, h) = self.opts.size;
        let filtering = self.opts.settings.texture_filtering;
        let square_pixels = self.opts.settings.square_pixels;
        crate::shot::render_to_ppm_with(
            (w, h),
            |device, queue, view, format| {
                let mut r = shuppatsu_render::Renderer::new(device, format);
                r.texture_filtering = filtering;
                r.shading = self.opts.settings.shading();
                r.fog.enabled = false;
                r.upload_pages(device, queue, &self.scene.library.pages);
                let mut frame = FrameGeometry::default();
                let mut fx = EffectGeometry::default();
                let custom = Self::custom_view(
                    &self.drive,
                    self.opening.as_ref(),
                    &self.scene.library,
                    &mut frame,
                    &mut fx,
                    w as f32 / h as f32,
                );
                let sub = custom.as_ref().and_then(|v| {
                    let win = v.window?;
                    Some(window_view(win, v.fov_tenths, (w, h), square_pixels))
                });
                let camera = match custom {
                    Some(v) => {
                        r.band_top = if v.full_screen {
                            0.0
                        } else {
                            super::HUD_BAND_TOP
                        };
                        r.projection.fov_tenths = v.fov_tenths;
                        v.camera
                    }
                    None if self.world == crate::screens::flow::WorldView::Cab => {
                        r.band_top = super::HUD_BAND_TOP;
                        r.projection.fov_tenths =
                            Self::opening_route_fov(self.opening.as_ref()).unwrap_or(450);
                        let pos = self.drive.pos();
                        let aspect = w as f32 / (h as f32 * (1.0 - super::HUD_BAND_TOP));
                        self.scene.compose(&mut frame, pos, false, aspect);
                        Self::opening_masks(self.opening.as_ref(), &mut fx, w as f32 / h as f32);
                        self.scene.cab_camera(pos)
                    }
                    None => {
                        crate::shot::clear_black(device, queue, view);
                        return;
                    }
                };
                r.effects = fx;
                r.set_geometry(device, queue, &frame);
                match &sub {
                    Some(sub) => r.render_view(device, queue, view, (w, h), &camera, sub),
                    None => r.render(device, queue, view, (w, h), &camera),
                }
            },
            Some((&self.canvas, self.layer.canvas_size())),
            out,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// At 640×480 the mode-2 window is the photo's rectangle 1:1 (0x436e14); on a 16:10 window
    /// it scales and centres with the 640×480 design.
    #[test]
    fn mode2_window_sits_on_the_photo() {
        let s = window_view(MODE2_WINDOW, STUDIO_FOV, (640, 480), false);
        assert_eq!(s.rect, [96.0, 56.0, 496.0, 400.0]);
        // 496 / 400 = 1.24 px against the projection's 1.25031 (4096 / 3276).
        assert!((s.projection.stretch - 1.24 / (4096.0 / 3276.0)).abs() < 1e-6);
        let s = window_view(MODE2_WINDOW, STUDIO_FOV, (1280, 800), false);
        let k = 800.0 / 480.0;
        let x0 = (1280.0 - 640.0 * k) / 2.0;
        assert!((s.rect[0] - (x0 + 96.0 * k)).abs() < 1e-3);
        assert!((s.rect[2] - 496.0 * k).abs() < 1e-3);
    }

    /// The inset edges x 64 / 576 in NDC: ±0.8 at 4:3, nearer the centre on a wider window.
    #[test]
    fn inset_edges_follow_the_centred_design() {
        assert!((canvas_ndc_x(INSET_LEFT, 4.0 / 3.0) + 0.8).abs() < 1e-6);
        assert!((canvas_ndc_x(INSET_RIGHT, 4.0 / 3.0) - 0.8).abs() < 1e-6);
        assert!((canvas_ndc_x(INSET_RIGHT, 16.0 / 9.0) - 0.6).abs() < 1e-6);
    }
}

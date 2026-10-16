//! The bonus game's presentation (0x41b540 / 0x41b810 / draw 0x41c3c0, docs/re/menus.md
//! §12.1): its 3D set (partsTR9: the platform 0x2df3, the 489-series car 0x327a and the EF63
//! pair 0x2df2, loaded by `0x40ee50(0x14)`) under its own orbit camera at 45°, the white
//! backdrop and the top mask, and the sprites: captions, the shock meter and its needle, the
//! distance readout, the prize picture and the timer. The state is
//! [`ddg_engine::bonus::BonusGame`].

use super::Play;
use crate::slots;
use anyhow::{Context, Result};
use ddg_engine::bonus::{BonusGame, PAIR_SPACING, PAIR_X};
use ddg_engine::drive::Drive;
use ddg_formats::exe::Exe;
use ddg_formats::hud::{MODE_640, RES_MODES};
use ddg_formats::{part::Part, rom2::Rom2, tex::TexFile};
use ddg_render::effects::EffectGeometry;
use ddg_render::{Camera, FrameGeometry, PartLibrary};
use glam::{Mat4, Vec3};

/// TR set 8 = partsTR9 / texTR9 (`0x463c74` row 8 via `0x463d14[0x14]`).
const TR_SET: usize = 8;
/// The set's parts.
const PART_SET: u32 = 0x2df3;
const PART_CAR: u32 = 0x327a;
const PART_LOCO: u32 = 0x2df2;
/// FOV 45° (`0x84c150 = 450`) while the bonus runs.
pub const FOV_TENTHS: u16 = 450;
/// Sprite slots of 0x41b810.
const SLOT_NEEDLE: usize = 0x36;
const SLOT_METER: usize = 0x37;
const SLOT_BANNER: usize = 0x38;
const SLOT_INSTRUCTION: usize = 0x39;
const SLOT_TIME_UP: usize = 0x3a;
const SLOT_PRIZE: usize = 0x3e;
const SLOT_DIST_HEAD: usize = 0x69;
const SLOT_DIST: usize = 0x64;
const DIST_DIGITS: usize = 5;
const TBL_NEEDLE: i16 = 0x2db;
const TBL_METER: i16 = 0x2da;
const TBL_DIST_HEAD: i16 = 0x58;
const TBL_TIME_UP: i16 = 0x252;
/// Positions of 0x41b810 for res mode 1 (origin (0, 0)): code immediates.
const BANNER_TITLE: (i16, i16) = (160, 172);
const BANNER_RESULT: (i16, i16) = (240, 164);
const BANNER_TIME_UP: (i16, i16) = (240, 200);
const INSTRUCTION_LONG: (i16, i16) = (136, 172);
const INSTRUCTION_SHORT: (i16, i16) = (272, 172);
const TIME_UP: (i16, i16) = (232, 240);
const DIST_DIGIT_X: i16 = 112;
const DIST_DIGIT_Y: i16 = 70;
/// Per-resolution tables (i32[4]): needle x base / y, meter x / y, prize x / y, distance
/// header x / y; distance digits 0x465540.
const POS_TABLES: [u32; 8] = [
    0x4696ac, 0x4696bc, 0x46968c, 0x46969c, 0x46966c, 0x46967c, 0x466f28, 0x466f38,
];
const DIGITS: u32 = 0x465540;
/// Timer slots of 0x417040.
const SLOT_TIMER: usize = 0x18;
const TBL_TIMER_MASKS: [i16; 2] = [0x40b, 0x40c];
const TBL_TIMER_TAIL: i16 = 0x40e;
const TIME_DIGITS: u32 = 0x4653d4;
const PAL_PRIZE: u8 = 15;
/// The backdrop quad 0x4692e0 (virtual y 64..384, palette 3 = white for area 7) and the top
/// mask 0x486160 (0..94).
const VIEW_TOP: f32 = 52.0;
const VIEW_H: f32 = 348.0;

#[derive(Debug, Clone)]
pub(super) struct BonusView {
    pos: [(i16, i16); 4],
    digits: [i16; 10],
    time_digits: [i16; 10],
}

impl Play {
    /// The sprites of one bonus tick (after the HUD), and the set's assets at its start and
    /// end.
    pub(super) fn bonus_present(&mut self) -> Result<()> {
        let Some(b) = self.drive.bonus().cloned() else {
            if self.bonus_view.take().is_some() {
                self.bonus_end()?;
            }
            return Ok(());
        };
        if self.bonus_view.is_none() {
            self.bonus_start()?;
        }
        let Some(v) = self.bonus_view.clone() else {
            return Ok(());
        };
        let s = &b.screen;
        let l = &mut self.layer;
        if s.started {
            let [needle, meter, ..] = v.pos;
            slots::set(
                l,
                SLOT_NEEDLE,
                TBL_NEEDLE,
                (needle.0 + s.needle as i16, needle.1),
                4,
                3,
            );
            slots::set(l, SLOT_METER, TBL_METER, meter, 4, 3);
        }
        match s.banner {
            Some(tbl) => {
                let at = match tbl {
                    0x2d8 => BANNER_TITLE,
                    _ if s.time_up => BANNER_TIME_UP,
                    _ => BANNER_RESULT,
                };
                slots::set(l, SLOT_BANNER, tbl, at, 4, 3);
            }
            None => slots::clear(l, SLOT_BANNER),
        }
        match s.instruction {
            Some(tbl) => {
                let at = if tbl == 0x2d9 {
                    INSTRUCTION_LONG
                } else {
                    INSTRUCTION_SHORT
                };
                slots::set(l, SLOT_INSTRUCTION, tbl, at, 4, 3);
            }
            None => slots::clear(l, SLOT_INSTRUCTION),
        }
        if s.time_up {
            slots::set(l, SLOT_TIME_UP, TBL_TIME_UP, TIME_UP, 4, 3);
        }
        if let Some(tbl) = s.prize {
            slots::set(l, SLOT_PRIZE, tbl, v.pos[2], PAL_PRIZE, 3);
        }
        // 0x416ab0: header and up to five digits, leading zeros hidden.
        match s.distance_m {
            Some(m) => {
                slots::set(l, SLOT_DIST_HEAD, TBL_DIST_HEAD, v.pos[3], 4, 3);
                let text = m.max(0).to_string();
                let pad = DIST_DIGITS.saturating_sub(text.len());
                for k in 0..DIST_DIGITS {
                    let x = DIST_DIGIT_X + 16 * k as i16;
                    match text
                        .as_bytes()
                        .get(k.wrapping_sub(pad))
                        .filter(|_| k >= pad)
                    {
                        Some(&c) => {
                            let d = usize::from(c - b'0');
                            slots::set(l, SLOT_DIST + k, v.digits[d], (x, DIST_DIGIT_Y), 4, 3);
                        }
                        None => slots::clear(l, SLOT_DIST + k),
                    }
                }
            }
            None => {
                for s in (SLOT_DIST..SLOT_DIST + DIST_DIGITS).chain([SLOT_DIST_HEAD]) {
                    slots::clear(l, s);
                }
            }
        }
        // 0x417040 on the remaining-time slots.
        let t = s.timer.max(0);
        let tbls = match t % 30 {
            26 | 27 => [TBL_TIMER_MASKS[0]; 3],
            28 | 29 => [TBL_TIMER_MASKS[1]; 3],
            _ => {
                let sec = (t / 30).min(999) as usize;
                [sec / 100, sec / 10 % 10, sec % 10].map(|d| v.time_digits[d])
            }
        };
        for (i, tbl) in tbls.into_iter().enumerate() {
            slots::image(l, SLOT_TIMER + i, tbl);
        }
        slots::image(l, SLOT_TIMER + 3, TBL_TIMER_TAIL);
        self.layer.compose(&self.bank, &mut self.canvas);
        self.canvas_dirty = true;
        Ok(())
    }

    /// `0x40ee50(0x14)`: the bonus set's textures and parts.
    fn bonus_start(&mut self) -> Result<()> {
        let exe_bytes = self.data.exe()?;
        let exe = Exe::parse(&exe_bytes)?;
        let pos = |i: usize| -> Result<(i16, i16)> {
            let x = exe.reader_at(POS_TABLES[2 * i], 4 * RES_MODES, "bonus layout")?;
            let y = exe.reader_at(POS_TABLES[2 * i + 1], 4 * RES_MODES, "bonus layout")?;
            Ok((x.i32(4 * MODE_640)? as i16, y.i32(4 * MODE_640)? as i16))
        };
        let ids = |va: u32| -> Result<[i16; 10]> {
            let r = exe.reader_at(va, 40, "bonus digits")?;
            let mut out = [0; 10];
            for (i, v) in out.iter_mut().enumerate() {
                *v = r.i32(4 * i)? as i16;
            }
            Ok(out)
        };
        let view = BonusView {
            pos: [pos(0)?, pos(1)?, pos(2)?, pos(3)?],
            digits: ids(DIGITS)?,
            time_digits: ids(TIME_DIGITS)?,
        };
        self.load_train_set(TR_SET)?;
        self.bonus_view = Some(view);
        Ok(())
    }

    /// Destroy 0x41c2e0 and `0x40ee50(stage)`: the sprites go, the stage's train sets again.
    fn bonus_end(&mut self) -> Result<()> {
        for s in [
            SLOT_NEEDLE,
            SLOT_METER,
            SLOT_BANNER,
            SLOT_INSTRUCTION,
            SLOT_TIME_UP,
            SLOT_PRIZE,
        ] {
            slots::clear(&mut self.layer, s);
        }
        crate::traffic::load_stage_trains(&mut self.scene, &self.data, self.opts.stage)?;
        self.upload_pages();
        Ok(())
    }

    fn load_train_set(&mut self, k: usize) -> Result<()> {
        let tex = format!("texTR{}.rom", k + 1);
        self.scene
            .library
            .add_textures(&TexFile::parse(&self.data.read(&tex)?).with_context(|| tex.clone())?);
        let parts = format!("partsTR{}.rom", k + 1);
        let bytes = self.data.read(&parts)?;
        let entries = Rom2::parse(&bytes)?
            .iter()
            .map(Part::parse_entry)
            .collect::<ddg_formats::Result<Vec<_>>>()
            .with_context(|| parts.clone())?;
        self.scene.library.add_parts(entries);
        self.upload_pages();
        Ok(())
    }

    fn upload_pages(&mut self) {
        if let Some(g) = self.gpu.as_mut() {
            g.renderer
                .upload_pages(&g.device, &g.queue, &self.scene.library.pages);
        }
    }

    /// The draw callback 0x41c3c0 into `frame` / `fx` and its camera; `None` outside the bonus.
    /// Before the callback is set (the 60-tick delay) the 3D is empty.
    pub(super) fn bonus_compose(
        drive: &Drive,
        library: &PartLibrary,
        frame: &mut FrameGeometry,
        fx: &mut EffectGeometry,
    ) -> Option<Camera> {
        let b: &BonusGame = drive.bonus()?;
        frame.clear(library);
        fx.clear();
        let (eye, target) = b.eye_target();
        let camera = Camera {
            eye: Vec3::from(eye),
            target: Vec3::from(target),
        };
        if !b.screen.started {
            return Some(camera);
        }
        let ndc = |vy: f32| 1.0 - 2.0 * ((vy - VIEW_TOP) / VIEW_H);
        let quad = |a: f32, b: f32| [[-1.0, a], [1.0, a], [1.0, b], [-1.0, b]];
        fx.add_flat_quad(quad(1.0, ndc(94.0)), [0.0, 0.0, 0.0, 1.0], 0);
        fx.add_flat_quad(quad(ndc(64.0), ndc(384.0)), [1.0, 1.0, 1.0, 1.0], 0x7ffe);
        let at = |x: f32, z: f32| Mat4::from_translation(Vec3::new(x, 0.0, z));
        frame.add_instance(library, PART_SET, &Mat4::IDENTITY);
        frame.add_instance(library, PART_CAR, &at(PAIR_X, b.car_z));
        frame.add_instance(library, PART_LOCO, &at(PAIR_X, b.pair_z));
        frame.add_instance(library, PART_LOCO, &at(PAIR_X, b.pair_z + PAIR_SPACING));
        Some(camera)
    }
}

impl Play {
    /// Headless frame of the bonus game.
    pub(super) fn bonus_shot(&self, out: &std::path::Path) -> Result<()> {
        let (w, h) = self.opts.size;
        let filtering = self.opts.texture_filtering;
        crate::shot::render_to_ppm_with(
            (w, h),
            |device, queue, view, format| {
                let mut r = ddg_render::Renderer::new(device, format);
                r.texture_filtering = filtering;
                r.fog.enabled = false;
                r.band_top = super::HUD_BAND_TOP;
                r.projection.fov_tenths = FOV_TENTHS;
                r.upload_pages(device, queue, &self.scene.library.pages);
                let mut frame = FrameGeometry::default();
                let mut fx = EffectGeometry::default();
                let camera =
                    Self::bonus_compose(&self.drive, &self.scene.library, &mut frame, &mut fx);
                r.effects = fx;
                r.set_geometry(device, queue, &frame);
                if let Some(c) = camera {
                    r.render(device, queue, view, (w, h), &c);
                }
            },
            Some(&self.canvas),
            out,
        )
    }
}

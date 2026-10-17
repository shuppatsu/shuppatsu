//! The stage intro fly-by (0x4091a0, called through 0x408ec0 by `Stage_Run` between the
//! loading wait and the departure board; docs/re/scene.md §9). Before stages 1, 5–8, 11–15 and
//! 18 the cab camera jumps a stretch down the line from one of the stage's stations and runs
//! backwards while an AI train of the stage's line comes towards it and passes; a caption card
//! about the train (slot 0x1d) sits at the bottom left and BGM 0x197 plays. Horn or A skips.
//!
//! It runs as its own [`Play`] (like the attract opening): `App` makes one for the chosen
//! stage, calls [`Play::start_stage_intro`], and goes on to the departure board when the run
//! leaves. The camera is the drive's ([`Drive::ending_place`] = 0x408010, [`Drive::ending_tick`]
//! with `0x850208`, [`Drive::ending_kill`]); the view is the driving view with its top panel.
//!
//! [`Drive::ending_place`]: ddg_engine::drive::Drive::ending_place
//! [`Drive::ending_tick`]: ddg_engine::drive::Drive::ending_tick
//! [`Drive::ending_kill`]: ddg_engine::drive::Drive::ending_kill

use super::Play;
use crate::screens::flow::WorldView;
use crate::slots;
use anyhow::{Context, Result};
use ddg_engine::traffic::SpawnRequest;
use ddg_formats::exe::Exe;
use ddg_formats::hud::{MODE_640, RES_MODES};
use ddg_render::sprites::Anchor;

const WHAT: &str = "stage intro tables";
/// The stages 0x408ec0 sends to 0x4091a0 (its stack table of 20 flags, code immediates at
/// 0x408ec6..; the others get 0x408f40, which is not ported).
const FLYBY_STAGES: [usize; 11] = [1, 5, 6, 7, 8, 11, 12, 13, 14, 15, 18];
/// Per stage, 9 × i32 at 0x458c80 + 0x24·stage: caption tbl, station, jump (m), camera km/h,
/// spawn, train km/h, spawn parameter, camera deceleration (km/h per tick), sound tick.
const ROWS: u32 = 0x458c80;
const ROW_LEN: u32 = 0x24;
/// The passing sound per stage (i16 at 0x459e88 + 2·stage; 4 also plays 1).
const SOUNDS: u32 = 0x459e88;
/// Caption position per resolution mode (i32[4] at 0x458c60 / 0x458c70).
const CAPTION_X: u32 = 0x458c60;
const CAPTION_Y: u32 = 0x458c70;
/// `0x4316e0(0x1d, tbl, x, y, 4, 3, 0)`.
const SLOT_CAPTION: usize = 0x1d;
const PAL_CAPTION: u8 = 4;
const LAYER_CAPTION: u8 = 3;
/// BGM started with the caption (0x42ff20, ungated); released only on a skip (0x420630).
pub const MUSIC: u16 = 0x197;
/// The jump: `0x850208 = metres × 0.1` (f64 0x44e8e0) for one tick.
const JUMP_SCALE: f64 = 0.1;
/// km/h → track units per tick: −0.000925 for the run back (f64 0x459438), +0.000925 for the
/// deceleration (f64 0x458af0).
const KMH_TO_UNITS: f64 = 0.000925;
/// Loop counts (immediates of 0x4091a0): the deceleration from count 0x6e, the end at 0x8c.
const DECEL_AFTER: u32 = 0x6d;
const END_AT: u32 = 0x8c;
/// Spawn flags (mode 5: the parameter is points to skip) and model (−1: the spawn's type).
const SPAWN_FLAGS: i32 = 5;
/// A passing sound that also plays sound 1, and stops it (0x4091a0).
const SOUND_WITH_ONE: i16 = 4;

/// Whether 0x408ec0 gives `stage` the fly-by.
pub fn has_stage_intro(stage: usize) -> bool {
    FLYBY_STAGES.contains(&stage)
}

/// One stage's row of 0x458c80.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct IntroRow {
    pub caption: i16,
    pub station: usize,
    pub jump_m: i32,
    pub camera_kmh: i32,
    pub spawn: i16,
    pub train_kmh: i32,
    pub param: i32,
    pub decel_kmh: i32,
    pub sound_at: i32,
    pub sound: i16,
    pub caption_at: (i16, i16),
}

impl IntroRow {
    /// The fly-by of `stage`, or `None` when 0x408ec0 picks the other intro.
    pub fn read(exe: &Exe<'_>, stage: usize) -> Result<Option<Self>> {
        if !has_stage_intro(stage) {
            return Ok(None);
        }
        let s = u32::try_from(stage)?;
        let r = exe.reader_at(ROWS + ROW_LEN * s, ROW_LEN as usize, WHAT)?;
        let f = |i: usize| r.i32(4 * i);
        let at = |va: u32| -> Result<i16> {
            let v = exe.reader_at(va, 4 * RES_MODES, WHAT)?.i32(4 * MODE_640)?;
            Ok(i16::try_from(v)?)
        };
        Ok(Some(Self {
            caption: i16::try_from(f(0)?)?,
            station: usize::try_from(f(1)?).context("intro station")?,
            jump_m: f(2)?,
            camera_kmh: f(3)?,
            spawn: i16::try_from(f(4)?)?,
            train_kmh: f(5)?,
            param: f(6)?,
            decel_kmh: f(7)?,
            sound_at: f(8)?,
            sound: exe.reader_at(SOUNDS + 2 * s, 2, WHAT)?.i16(0)?,
            caption_at: (at(CAPTION_X)?, at(CAPTION_Y)?),
        }))
    }
}

/// Where the intro is. Every variant is one tick (`Task_Tick`) of 0x4091a0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// The `Task_Tick`s of 0x40eed0 and after `0x855ff8 |= 0x20000`: nothing drawn yet.
    Lead(u32),
    /// 0x408010 placed the camera; it jumps `jump_m` in this tick.
    Jump,
    /// Before loop count `i + 1` (1..=0x8c): one tick each, except the closing count.
    Run(u32),
    /// The second of the two closing ticks after list 5 is killed (black); the first is the
    /// tick of the closing loop count.
    Tail,
    Done,
}

/// What one tick of the intro does, in order.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Plan {
    /// 0x408010: the camera placed at the row's station.
    place: bool,
    /// After the jump tick: the train spawned, BGM 0x197 started, the card shown.
    start: bool,
    /// `0x41fda0(sound)` this tick.
    sound: bool,
    /// The end of 0x4091a0 (`Some(true)` when skipped: 0x197 released first).
    close: Option<bool>,
    /// List 5 runs with `0x850208` = this; `None`: an idle `Task_Tick`.
    camera: Option<f32>,
    /// The intro is over after this tick.
    leave: bool,
}

#[derive(Debug, Clone)]
pub(super) struct Intro {
    row: IntroRow,
    step: Step,
    /// `0x850208`, track units per tick.
    v: f32,
}

impl Intro {
    fn new(row: IntroRow) -> Self {
        Self {
            row,
            step: Step::Lead(0),
            v: 0.0,
        }
    }

    /// One tick; `skip` is the horn or A edge of the last poll (`0x48c42c`).
    fn advance(&mut self, skip: bool) -> Plan {
        let mut p = Plan::default();
        match self.step {
            Step::Lead(n) => self.step = if n == 0 { Step::Lead(1) } else { Step::Jump },
            Step::Jump => {
                p.place = true;
                self.v = (f64::from(self.row.jump_m) * JUMP_SCALE) as f32;
                p.camera = Some(self.v);
                self.step = Step::Run(0);
            }
            Step::Run(i) => {
                if i == 0 {
                    p.start = true;
                    self.v = (-f64::from(self.row.camera_kmh) * KMH_TO_UNITS) as f32;
                }
                let i = i + 1;
                if i > DECEL_AFTER {
                    self.v =
                        (f64::from(self.row.decel_kmh) * KMH_TO_UNITS + f64::from(self.v)) as f32;
                }
                p.sound = i64::from(i) == i64::from(self.row.sound_at);
                if i == END_AT || skip {
                    p.close = Some(i != END_AT);
                    self.step = Step::Tail;
                } else {
                    p.camera = Some(self.v);
                    self.step = Step::Run(i);
                }
            }
            Step::Tail | Step::Done => {
                p.leave = true;
                self.step = Step::Done;
            }
        }
        p
    }
}

impl Play {
    /// Turns this run, fresh at station 0, into the stage intro of 0x4091a0. Returns false (and
    /// changes nothing) for the stages without the fly-by.
    pub fn start_stage_intro(&mut self) -> Result<bool> {
        let exe_bytes = self.data.exe()?;
        let exe = Exe::parse(&exe_bytes)?;
        let Some(row) = IntroRow::read(&exe, self.opts.stage)? else {
            return Ok(false);
        };
        // Nothing of the driving HUD exists yet (0x40f7e0 comes after the intro).
        self.layer.slots.fill(ddg_render::sprites::Slot::CLEAR);
        self.layer.widgets.clear();
        self.layer.fans.clear();
        self.world = WorldView::Hidden;
        eprintln!(
            "[play tick {}] stage intro: station {}, {} m on, camera {} km/h back, spawn {} at {} km/h",
            self.ticks, row.station, row.jump_m, row.camera_kmh, row.spawn, row.train_kmh
        );
        self.intro = Some(Intro::new(row));
        Ok(true)
    }

    /// One tick of 0x4091a0.
    pub(super) fn intro_tick(&mut self) -> Result<()> {
        self.ticks += 1;
        let Some(mut o) = self.intro.take() else {
            return Ok(());
        };
        let row = o.row;
        let p = o.advance(self.latched.horn_pressed || self.latched.confirm_pressed);
        if p.place {
            // 0x408010(stage, station) → 0x41cb00: the camera task of that station's segment.
            self.drive.ending_place(row.station)?;
            self.interpolate = false;
        }
        if p.start {
            self.drive.ending_spawn(&SpawnRequest {
                spawn: row.spawn,
                flags: SPAWN_FLAGS,
                speed_kmh: row.train_kmh,
                param: row.param,
                model: -1,
            });
            if let Some(s) = self.sound.as_mut() {
                s.request_ungated(MUSIC);
            }
            slots::set(
                &mut self.layer,
                SLOT_CAPTION,
                row.caption,
                row.caption_at,
                PAL_CAPTION,
                LAYER_CAPTION,
            );
        }
        if p.sound {
            self.intro_sound(row.sound, true);
        }
        if let Some(skipped) = p.close {
            if skipped {
                eprintln!("[play tick {}] stage intro skipped", self.ticks);
                if let Some(s) = self.sound.as_mut() {
                    s.release(MUSIC);
                }
            }
            // The card hidden, the passing sound stopped, list 5 killed.
            slots::image(&mut self.layer, SLOT_CAPTION, -1);
            self.intro_sound(row.sound, false);
            self.drive.ending_kill();
        }
        match p.camera {
            Some(v) => {
                self.drive.ending_tick(v);
                self.scene.traffic = self.drive.traffic_poses(1.0);
                self.world = WorldView::Cab;
                self.interpolate = !p.place;
            }
            None => {
                self.drive.idle_global_tick();
                self.scene.traffic.clear();
                self.world = WorldView::Hidden;
            }
        }
        self.leave |= p.leave;
        if self.drive.com() != self.scene_com {
            self.reload_scene()?;
        }
        if let Some(s) = self.sound.as_mut() {
            s.traffic(self.drive.traffic_sounds());
            s.menu_frame();
        }
        // Enhancement `wide_hud`: the card keeps to the left edge like the driving HUD's
        // bottom-left group; the original layout keeps the centred 4:3 design.
        if super::wide_hud(&self.opts) {
            self.layer.anchors[SLOT_CAPTION] = Anchor::Left;
        }
        self.poll();
        self.layer.compose(&self.bank, &mut self.canvas);
        self.canvas_dirty = true;
        self.intro = Some(o);
        Ok(())
    }

    /// `0x41fda0(sound)` (and sound 1 with sound 4), or their release (0x420650).
    fn intro_sound(&mut self, code: i16, play: bool) {
        let (Some(s), Ok(code)) = (self.sound.as_mut(), u16::try_from(code)) else {
            return;
        };
        let also_one = i16::try_from(code) == Ok(SOUND_WITH_ONE);
        for c in std::iter::once(code).chain(also_one.then_some(1)) {
            if play {
                s.request(c);
            } else {
                s.release(c);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> IntroRow {
        IntroRow {
            caption: 934,
            station: 24,
            jump_m: 1915,
            camera_kmh: 260,
            spawn: 2,
            train_kmh: 270,
            param: 0,
            decel_kmh: 6,
            sound_at: 110,
            sound: 8,
            caption_at: (24, 408),
        }
    }

    /// Runs the intro to its end, skipping at tick `skip_at`; the plans by tick (1-based).
    fn run(skip_at: Option<usize>) -> Vec<Plan> {
        let mut o = Intro::new(row());
        let mut plans = Vec::new();
        loop {
            // The edge seen in tick t was polled at the end of tick t − 1.
            let p = o.advance(skip_at == Some(plans.len() + 1));
            plans.push(p);
            if p.leave {
                return plans;
            }
        }
    }

    /// 144 ticks (`0x855ff8 & 0x20000` on ticks 2..=142, 141 in the oracle trace): two idle,
    /// the jump of 191.5 units, 139 ticks back at 260 km/h less 6 km/h a tick from loop count
    /// 110 (tick 113, also the sound's), then two black closing ticks.
    #[test]
    fn unskipped_intro_runs_144_ticks() {
        let p = run(None);
        assert_eq!(p.len(), 144);
        assert_eq!(p[0], Plan::default());
        assert_eq!(p[1], Plan::default());
        assert!(p[2].place && p[2].camera == Some(191.5));
        assert!(p[3].start);
        assert_eq!(p[3].camera, Some((-260.0 * KMH_TO_UNITS) as f32));
        assert_eq!(p[111].camera, p[3].camera);
        assert!(p[112].sound && p.iter().filter(|p| p.sound).count() == 1);
        // The trace logs −0.19055 at the ninth decelerated tick (−0.2405 + 9 × 0.00555).
        assert!((p[120].camera.unwrap() - -0.190_55).abs() < 1e-6);
        assert!(p[141].camera.is_some() && p[142].camera.is_none());
        assert_eq!(p[142].close, Some(false));
        assert!(p[143].leave);
    }

    /// A skip edge seen on tick t closes the intro in tick t (0x197 released) and leaves one
    /// tick later; on the first loop tick the train, music and card were started first.
    #[test]
    fn skip_closes_at_once() {
        let p = run(Some(50));
        assert_eq!(p.len(), 51);
        assert_eq!(p[49].close, Some(true));
        assert!(p[49].camera.is_none());
        let p = run(Some(4));
        assert!(p[3].start && p[3].close == Some(true) && p.len() == 5);
        // Edges before the loop are not read.
        assert_eq!(run(Some(2)).len(), 144);
    }

    /// The decoded rows of the stages the exe sends to the fly-by.
    #[test]
    fn rows_from_the_exe() {
        let Ok(data) = crate::scene::GameData::from_env() else {
            return;
        };
        let Ok(bytes) = data.exe() else { return };
        let exe = Exe::parse(&bytes).unwrap();
        assert_eq!(IntroRow::read(&exe, 0).unwrap(), None);
        assert_eq!(IntroRow::read(&exe, 1).unwrap(), Some(row()));
        let r = IntroRow::read(&exe, 7).unwrap().unwrap();
        assert_eq!((r.sound, r.sound_at), (0x1a3, 70));
    }
}

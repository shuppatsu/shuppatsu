//! The stage intro (0x408ec0, called by `Stage_Run` between the loading wait and the departure
//! board; docs/re/scene.md §9). Every stage has one of two kinds, both with a caption card
//! about the train (slot 0x1d) at the bottom left and BGM 0x197; horn or A skips.
//!
//! - The fly-by (0x4091a0; stages 1, 5–8, 11–15 and 18): the cab camera jumps a stretch down
//!   the line from one of the stage's stations and runs backwards while an AI train of the
//!   stage's line comes towards it and passes.
//! - The chase (0x408f40; stages 0, 2–4, 9, 10, 16, 17 and 19): the player's own train stands
//!   a little ahead of the camera at the stage's first station (0x406960); the camera follows
//!   it at 100 km/h, then slows and the train pulls away.
//!
//! It runs as its own [`Play`] (like the attract opening): `App` makes one for the chosen
//! stage, calls [`Play::start_stage_intro`], and goes on to the departure board when the run
//! leaves. The camera is the drive's ([`Drive::ending_place`] = 0x408010, [`Drive::ending_tick`]
//! with `0x850208`, [`Drive::ending_kill`]); the view is the driving view with its top panel.
//!
//! [`Drive::ending_place`]: shuppatsu_engine::drive::Drive::ending_place
//! [`Drive::ending_tick`]: shuppatsu_engine::drive::Drive::ending_tick
//! [`Drive::ending_kill`]: shuppatsu_engine::drive::Drive::ending_kill

use super::Play;
use crate::screens::flow::WorldView;
use crate::slots;
use anyhow::{Context, Result};
use shuppatsu_engine::traffic::{PlacedTrain, SpawnRequest};
use shuppatsu_formats::code::stack_table;
use shuppatsu_formats::exe::Exe;
use shuppatsu_formats::hud::{MODE_640, RES_MODES};
use shuppatsu_render::sprites::Anchor;

const WHAT: &str = "stage intro tables";
/// Stages with an intro (0x408ec0's stack table of 20 flags).
const STAGES: usize = 20;
/// The stages 0x408ec0 sends to 0x4091a0 (flag 1, code immediates at 0x408ec6..); the others
/// (flag 0) get 0x408f40.
const FLYBY_STAGES: [usize; 11] = [1, 5, 6, 7, 8, 11, 12, 13, 14, 15, 18];
/// Per stage, 9 × i32 at 0x458c80 + 0x24·stage: caption tbl, station, jump (m), camera km/h,
/// spawn, train km/h, spawn parameter, camera deceleration (km/h per tick), sound tick. The
/// chase reads only the caption.
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
/// A passing sound that also plays sound 1, and stops it (0x4091a0, 0x408f40).
const SOUND_WITH_ONE: i16 = 4;

// The fly-by, 0x4091a0.
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

// The chase, 0x408f40.
/// The function and the length of the prologue that stores its per-stage point offsets
/// (`int local_50[20]`, 0x408f40..0x408fe0, ending at the call of 0x40eed0).
const CHASE_CODE: u32 = 0x408f40;
const CHASE_CODE_LEN: usize = 0xa5;
/// The train's head is `0x8502a4 + 6 + offset[stage]` points (`add` immediate in 0x408f40).
const CHASE_EXTRA_POINTS: i64 = 6;
/// Model row of the player's train per stage (i32 at 0x4596e0 + 4·stage).
const CHASE_ROWS: u32 = 0x4596e0;
/// `0x408010(stage, 1)`: the camera at the stop of station 1.
const CHASE_STATION: usize = 1;
/// `0x406960(…, 1, 100, 0, row | 0x1000, 1)`: mode 1 with no acceleration at 100 km/h, the
/// second car list (forward), the cars placed at once.
const CHASE_MODE: i32 = 1;
const CHASE_TRAIN_KMH: i32 = 100;
/// `0x850208 = 0.0925` (f32 immediate 0x3dbd70a4, 100 km/h) for counts < 0x6f, then
/// `−= (float)0.002775` (f64 0x459430, 3 km/h) a tick; the loop ticks through count 0x8c.
const CHASE_SPEED: f32 = 0.0925;
const CHASE_SLOW_FROM: u32 = 0x6f;
const CHASE_DECEL: f64 = 0.002775;
const CHASE_LAST: u32 = 0x8c;
/// The passing sound plays at count 0x78.
const CHASE_SOUND_AT: u32 = 0x78;

/// Whether `stage` has a stage intro (0x408ec0 gives every stage one of the two kinds).
pub fn has_stage_intro(stage: usize) -> bool {
    stage < STAGES
}

/// What 0x4091a0 reads for one stage (its row of 0x458c80).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FlyBy {
    pub station: usize,
    pub jump_m: i32,
    pub camera_kmh: i32,
    pub spawn: i16,
    pub train_kmh: i32,
    pub param: i32,
    pub decel_kmh: i32,
    pub sound_at: i32,
}

/// What 0x408f40 places for one stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Chase {
    /// Model row (0x4596e0).
    pub row: usize,
    /// Points from the camera's engine index to the head car (6 + the stack table).
    pub ahead_points: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    FlyBy(FlyBy),
    Chase(Chase),
}

/// One stage's intro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct IntroRow {
    pub caption: i16,
    pub caption_at: (i16, i16),
    pub sound: i16,
    pub kind: Kind,
}

impl IntroRow {
    /// The intro of `stage`, or `None` past the 20 stages.
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
        let kind = if FLYBY_STAGES.contains(&stage) {
            Kind::FlyBy(FlyBy {
                station: usize::try_from(f(1)?).context("intro station")?,
                jump_m: f(2)?,
                camera_kmh: f(3)?,
                spawn: i16::try_from(f(4)?)?,
                train_kmh: f(5)?,
                param: f(6)?,
                decel_kmh: f(7)?,
                sound_at: f(8)?,
            })
        } else {
            let offsets = stack_table(exe.bytes_at(CHASE_CODE, CHASE_CODE_LEN)?, STAGES)?;
            let row = exe.reader_at(CHASE_ROWS + 4 * s, 4, WHAT)?.i32(0)?;
            Kind::Chase(Chase {
                row: usize::try_from(row).context("chase model row")?,
                ahead_points: CHASE_EXTRA_POINTS + i64::from(offsets[stage]),
            })
        };
        Ok(Some(Self {
            caption: i16::try_from(f(0)?)?,
            caption_at: (at(CAPTION_X)?, at(CAPTION_Y)?),
            sound: exe.reader_at(SOUNDS + 2 * s, 2, WHAT)?.i16(0)?,
            kind,
        }))
    }
}

/// Where the intro is. Every variant is one tick (`Task_Tick`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// The `Task_Tick`s of 0x40eed0 and after `0x855ff8 |= 0x20000`: nothing drawn yet.
    Lead(u32),
    /// 0x408010 placed the camera; it moves by `0x850208` in this tick (the fly-by's jump; 0
    /// in the chase).
    Place,
    /// Before loop count `i + 1`: one tick each, except the closing count.
    Run(u32),
    /// The second of the two closing ticks after list 5 is killed (black); the first is the
    /// tick of the closing loop count.
    Tail,
    Done,
}

/// What one tick of the intro does, in order.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Plan {
    /// 0x408010: the camera placed at this station.
    place: Option<usize>,
    /// Before the first loop tick: the train spawned or placed, BGM 0x197 started, the card
    /// shown.
    start: bool,
    /// `0x41fda0(sound)` this tick.
    sound: bool,
    /// The end of the intro (`Some(true)` when skipped: 0x197 released first).
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
            Step::Lead(n) => self.step = if n == 0 { Step::Lead(1) } else { Step::Place },
            Step::Place => {
                match self.row.kind {
                    Kind::FlyBy(f) => {
                        p.place = Some(f.station);
                        self.v = (f64::from(f.jump_m) * JUMP_SCALE) as f32;
                    }
                    // 0x850208 is not set before this tick: it holds what the last camera
                    // left, 0 in the oracle trace (the drive's camera ends at a stop).
                    Kind::Chase(_) => p.place = Some(CHASE_STATION),
                }
                p.camera = Some(self.v);
                self.step = Step::Run(0);
            }
            Step::Run(i) => {
                p.start = i == 0;
                let i = i + 1;
                let end = match self.row.kind {
                    Kind::FlyBy(f) => {
                        if i == 1 {
                            self.v = (-f64::from(f.camera_kmh) * KMH_TO_UNITS) as f32;
                        }
                        if i > DECEL_AFTER {
                            self.v =
                                (f64::from(f.decel_kmh) * KMH_TO_UNITS + f64::from(self.v)) as f32;
                        }
                        p.sound = i64::from(i) == i64::from(f.sound_at);
                        i == END_AT
                    }
                    Kind::Chase(_) if i > CHASE_LAST => true,
                    Kind::Chase(_) => {
                        self.v = if i < CHASE_SLOW_FROM {
                            CHASE_SPEED
                        } else {
                            self.v - CHASE_DECEL as f32
                        };
                        p.sound = i == CHASE_SOUND_AT;
                        false
                    }
                };
                if end || skip {
                    p.close = Some(!end);
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
    /// Turns this run, fresh at station 0, into the stage intro of 0x408ec0 (the fly-by
    /// 0x4091a0 or the chase 0x408f40). Returns false (and changes nothing) for a stage
    /// without one.
    pub fn start_stage_intro(&mut self) -> Result<bool> {
        let exe_bytes = self.data.exe()?;
        let exe = Exe::parse(&exe_bytes)?;
        let Some(row) = IntroRow::read(&exe, self.opts.stage)? else {
            return Ok(false);
        };
        // Nothing of the driving HUD exists yet (0x40f7e0 comes after the intro).
        self.layer
            .slots
            .fill(shuppatsu_render::sprites::Slot::CLEAR);
        self.layer.widgets.clear();
        self.layer.fans.clear();
        self.world = WorldView::Hidden;
        match row.kind {
            Kind::FlyBy(f) => eprintln!(
                "[play tick {}] stage intro fly-by: station {}, {} m on, camera {} km/h back, spawn {} at {} km/h",
                self.ticks, f.station, f.jump_m, f.camera_kmh, f.spawn, f.train_kmh
            ),
            Kind::Chase(c) => eprintln!(
                "[play tick {}] stage intro chase: station {CHASE_STATION}, train row {} {} points ahead",
                self.ticks, c.row, c.ahead_points
            ),
        }
        self.intro = Some(Intro::new(row));
        Ok(true)
    }

    /// One tick of 0x4091a0 / 0x408f40.
    pub(super) fn intro_tick(&mut self) -> Result<()> {
        self.ticks += 1;
        let Some(mut o) = self.intro.take() else {
            return Ok(());
        };
        let row = o.row;
        let p = o.advance(self.latched.horn_pressed || self.latched.confirm_pressed);
        if let Some(station) = p.place {
            // 0x408010(stage, station) → 0x41cb00: the camera task of that station's segment.
            self.drive.ending_place(station)?;
            self.interpolate = false;
        }
        if p.start {
            match row.kind {
                Kind::FlyBy(f) => self.drive.ending_spawn(&SpawnRequest {
                    spawn: f.spawn,
                    flags: SPAWN_FLAGS,
                    speed_kmh: f.train_kmh,
                    param: f.param,
                    model: -1,
                }),
                // 0x406960 from the engine index the placement tick left.
                Kind::Chase(c) => self.drive.ending_place_train(&PlacedTrain {
                    row: c.row,
                    forward: true,
                    ahead_points: c.ahead_points,
                    mode: CHASE_MODE,
                    speed_kmh: CHASE_TRAIN_KMH,
                    param: 0,
                    spaced: true,
                }),
            }
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
                self.interpolate = p.place.is_none();
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
        self.canvas.compose(&self.layer, &self.bank, None);
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
            caption_at: (24, 408),
            sound: 8,
            kind: Kind::FlyBy(FlyBy {
                station: 24,
                jump_m: 1915,
                camera_kmh: 260,
                spawn: 2,
                train_kmh: 270,
                param: 0,
                decel_kmh: 6,
                sound_at: 110,
            }),
        }
    }

    fn chase_row() -> IntroRow {
        IntroRow {
            caption: 934,
            caption_at: (24, 408),
            sound: 8,
            kind: Kind::Chase(Chase {
                row: 11,
                ahead_points: 0x55 + 6,
            }),
        }
    }

    /// Runs the intro to its end, skipping at tick `skip_at`; the plans by tick (1-based).
    fn run_row(row: IntroRow, skip_at: Option<usize>) -> Vec<Plan> {
        let mut o = Intro::new(row);
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

    fn run(skip_at: Option<usize>) -> Vec<Plan> {
        run_row(row(), skip_at)
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
        assert!(p[2].place == Some(24) && p[2].camera == Some(191.5));
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

    /// The chase against the oracle trace of stage 0 (`out/captures/intro0.csv`, global ticks
    /// 1777..1921): 145 ticks, `0x20000` on ticks 2..=143 (142); `0x850208` = 0 on the
    /// placement tick, 0.0925 for 110 loop ticks, then 0.089725 down to 0.00925003 on the
    /// last of the 140 loop ticks; the sound at loop count 120.
    #[test]
    fn unskipped_chase_runs_145_ticks() {
        let p = run_row(chase_row(), None);
        assert_eq!(p.len(), 145);
        assert_eq!(p[0], Plan::default());
        assert_eq!(p[1], Plan::default());
        assert!(p[2].place == Some(CHASE_STATION) && p[2].camera == Some(0.0));
        assert!(p[3].start && p[3].camera == Some(0.0925));
        assert_eq!(p[112].camera, Some(0.0925));
        assert_eq!(p[113].camera.map(f32::to_bits), Some(0x3db7_c1be));
        assert_eq!(p[142].camera.map(f32::to_bits), Some(0x3c17_8d70));
        assert!(p[122].sound && p.iter().filter(|p| p.sound).count() == 1);
        assert_eq!(p[143].close, Some(false));
        assert!(p[143].camera.is_none() && p[144].leave);
        // A skip on the last loop tick still closes as a skip; past it the end wins.
        let p = run_row(chase_row(), Some(143));
        assert_eq!((p.len(), p[142].close), (144, Some(true)));
        assert_eq!(run_row(chase_row(), Some(144))[143].close, Some(false));
    }

    /// The decoded rows of both kinds, and the chase's point offsets from 0x408f40's code.
    #[test]
    fn rows_from_the_exe() {
        let Ok(data) = crate::scene::GameData::from_env() else {
            return;
        };
        let Ok(bytes) = data.exe() else { return };
        let exe = Exe::parse(&bytes).unwrap();
        assert_eq!(IntroRow::read(&exe, 20).unwrap(), None);
        assert_eq!(IntroRow::read(&exe, 1).unwrap(), Some(row()));
        let r = IntroRow::read(&exe, 7).unwrap().unwrap();
        assert_eq!(r.sound, 0x1a3);
        assert!(matches!(r.kind, Kind::FlyBy(FlyBy { sound_at: 70, .. })));
        assert_eq!(IntroRow::read(&exe, 0).unwrap(), Some(chase_row()));
        let offsets = stack_table(exe.bytes_at(CHASE_CODE, CHASE_CODE_LEN).unwrap(), STAGES);
        assert_eq!(
            offsets.unwrap(),
            [
                0x55, 0x55, 0x55, 0x20, 0x20, 0x10, 0x10, 0x90, 0x86, 0x96, 0x9a, 0x9a, 0x6e, 0x6b,
                0x7b, 0x7b, 0xa7, 0x55, 0x55, 0x55
            ]
        );
        let Kind::Chase(c) = IntroRow::read(&exe, 9).unwrap().unwrap().kind else {
            panic!("stage 9 has the chase");
        };
        assert_eq!((c.row, c.ahead_points), (42, 0x96 + 6));
    }
}

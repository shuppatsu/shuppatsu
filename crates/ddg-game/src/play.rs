//! `ddg-game play`: drive a stage. The simulation is `ddg_engine::drive` at the original's
//! fixed 33 ms tick; rendering interpolates the camera along the track between the last two
//! ticks. The stage flow (pause dialogs, station result, continue, report card, ranking and
//! name entry, run-end banner) is [`crate::screens::flow::StageFlow`]; quitting or ending the run
//! leaves play. Standalone (`ddg-game play <stage>`) that closes the window; under the front end
//! ([`crate::app`]) the run is a sub-state built from [`Shared`] resources and the title comes
//! back. The save (`eeprom.dat`, rankings and clear marks) is written when the program exits,
//! like the original at shutdown.

use crate::cab::{CabHud, Snap, TickInput};
use crate::gamepad::{Controls, PadOptions};
use crate::hud::{DistanceReadout, Hud, Speedometer};
use crate::input::ButtonState;
use crate::result_view::{ResultView, ResultViewInput};
use crate::save::Save;
use crate::scene::{GameData, RouteScene, Visibility};
use crate::screens::flow::{Action, FlowDefs, Observation, StageFlow, WorldView};
use crate::screens::pause::PauseResult;
use crate::screens::ranking::RankingRequest;
use crate::screens::result::ResultInput;
use crate::screens::tables::ScreenTables;
use crate::screens::{MenuInput, Sfx};
use crate::window::Gpu;
use anyhow::{Context, Result};
use ddg_engine::coupling::CouplingEvent;
use ddg_engine::drive::{Drive, DrivePhase, DriverInput, LoadError};
use ddg_engine::physics::TrainTables;
use ddg_engine::rules::{RuleEvent, RuleTables};
use ddg_formats::exe::Exe;
use ddg_formats::result_camera::ResultTables;
use ddg_formats::route::UNITS_PER_POINT;
use ddg_formats::scene::{DRIVING_FOV_Y_DEG, TrackPos};
use ddg_formats::stages::StageTables;
use ddg_render::FrameGeometry;
use ddg_render::camera::ORIGINAL_STRETCH;
use ddg_render::sprites::{CANVAS_H, CANVAS_W, Slot, SpriteBank, SpriteLayer};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::WindowId;

/// The original's fixed tick (timeSetEvent(33, …) at 0x436f30).
const TICK: Duration = Duration::from_millis(33);
/// The original's 3D viewport starts at virtual y 52 of 400 (renderer.md §5).
pub const HUD_BAND_TOP: f32 = 52.0 / 400.0;
/// Visibility is widened for at least this aspect, so resizing up to 21:9 needs no rebuild.
const MIN_WIDE_ASPECT: f32 = 21.0 / 9.0;
/// Pixels per `tbl.rom` tile.
const TILE_PX: i16 = 16;
/// `Hud_Distance` 0x4162c0 adds speed (m/s) × this (f64 at 0x4676f0, ≈ 1/30, used as f32) to
/// `0x8571f0` each tick the distance meter is shown: metres driven with the meter.
const METER_SCALE: f64 = f64::from_bits(0x3fa1_1113_4dba_9df6);
/// The run's PRO flag `0x85723c` is set when `0x8571f0 ≤ 30.0` (f64 at 0x458b90, 0x4080d0).
const PRO_LIMIT_M: f32 = 30.0;

pub struct PlayOptions {
    pub stage: usize,
    pub difficulty: usize,
    pub station: usize,
    /// Control mode 0..=6 (0–3 keyboard, 4–6 joystick; input.md §4–§5).
    pub control_mode: usize,
    pub pad: PadOptions,
    pub size: (u32, u32),
    pub uncapped: bool,
    pub square_pixels: bool,
    pub original_visibility: bool,
    pub stable_track_variants: bool,
    pub texture_filtering: bool,
    pub fog: bool,
    pub mute: bool,
    /// Scripted inputs: `tick notch brake` lines (the oracle's format, drive ticks), held from
    /// each tick on, and `key <tick>[-<last>] <enter|esc|space|up|down|left|right>` lines: the
    /// key is down for the poll at the end of play tick `tick` (through `last`; play ticks also
    /// count paused ticks).
    pub inputs: Option<std::path::PathBuf>,
    /// Headless: run this many play ticks, render one frame with the HUD to a PPM and exit.
    pub shot: Option<(u64, std::path::PathBuf)>,
    /// Overrides the save's speedometer option (0 none, 1 analog, 2 digital).
    pub speedometer: Option<u8>,
    /// Select stage 20: route 10 with `0x48ddf0 = 1`, whose clear mark is the 21st byte.
    pub variant: bool,
    /// Language and font of the text sprites (docs/localisation.md).
    pub locale: crate::locale::LocaleConfig,
}

/// The exe tables a (re)start needs.
struct Tables {
    train: TrainTables,
    rules: RuleTables,
    stages: StageTables,
    result: ResultTables,
}

fn start_drive(tables: &Tables, opts: &PlayOptions, station: usize) -> Result<Drive> {
    let loader_data = GameData::from_env()?;
    let loader = move |name: &str| -> Result<Vec<u8>, LoadError> {
        loader_data
            .read(name)
            .map_err(|e| LoadError::from(format!("{e:#}")))
    };
    let drive = Drive::new(
        &tables.train,
        &tables.rules,
        &tables.stages,
        opts.stage,
        opts.difficulty,
        station,
        loader,
    )
    .context("starting the stage")?;
    eprintln!(
        "stage {} difficulty {} from station {station} ({})",
        opts.stage,
        opts.difficulty,
        drive.station_name(station).unwrap_or("?")
    );
    Ok(drive)
}

/// What a run borrows from its owner: moved into [`Play::new`] and handed back by
/// [`Play::into_shared`] (the front end keeps one window, sprite bank, save, input and sound
/// device for the whole session).
pub struct Shared {
    pub data: GameData,
    pub bank: SpriteBank,
    pub layer: SpriteLayer,
    pub save: Save,
    pub controls: Controls,
    /// The sound device, without a stage (`Sound::open_device`).
    pub sound: Option<crate::sound::Sound>,
    pub gpu: Option<Gpu>,
}

/// Loads the sprite bank (`cg.rom` + `tbl.rom`) with the locale's text overrides.
pub fn load_bank(
    data: &GameData,
    exe: &Exe<'_>,
    locale: &crate::locale::LocaleConfig,
) -> Result<SpriteBank> {
    let cg_bytes = data.read("cg.rom")?;
    let mut bank = SpriteBank::new(
        &ddg_formats::cg::CgFile::parse(&cg_bytes)?,
        ddg_formats::cg::parse_tbl(&data.read("tbl.rom")?)?,
    );
    crate::locale::install(locale, exe, &mut bank)?;
    Ok(bank)
}

/// `ddg-game play <stage>`: one run in its own window (or headless with `--shot`).
pub fn run(opts: PlayOptions) -> Result<()> {
    let data = GameData::from_env()?;
    let exe_bytes = data.exe()?;
    let exe = Exe::parse(&exe_bytes)?;
    // Headless shots use the exe's default save and never write one.
    let save_path = if opts.shot.is_some() {
        None
    } else {
        crate::save::default_path()
    };
    let save = Save::open(&exe, save_path)?;
    if let Some(p) = save.path() {
        eprintln!("save: {}", p.display());
    }
    let controls = Controls::new(&exe, opts.control_mode, &opts.pad)?;
    let bank = load_bank(&data, &exe, &opts.locale)?;
    let sound = if opts.shot.is_some() || opts.mute {
        None
    } else {
        crate::sound::Sound::open_device(data.data_dir(), &exe)?
    };
    let shared = Shared {
        data,
        bank,
        layer: SpriteLayer::default(),
        save,
        controls,
        sound,
        gpu: None,
    };
    let mut app = Play::new(shared, opts)?;
    if let Some((ticks, out)) = app.opts.shot.clone() {
        while app.ticks < ticks && !app.leave {
            app.tick()?;
        }
        app.shot(&out)?;
        eprintln!("wrote {} at play tick {}", out.display(), app.ticks);
        return Ok(());
    }
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    event_loop.run_app(&mut app)?;
    let saved = app.save.write().context("writing the save");
    match app.error {
        Some(e) => Err(e),
        None => saved,
    }
}

impl Play {
    /// Starts a run (`Stage_Run` after the pre-run screens) on the owner's resources. With a
    /// window, the renderer is set up for the cab view here.
    pub fn new(shared: Shared, opts: PlayOptions) -> Result<Self> {
        let Shared {
            data,
            bank,
            mut layer,
            save,
            controls,
            mut sound,
            gpu,
        } = shared;
        let exe_bytes = data.exe()?;
        let exe = Exe::parse(&exe_bytes)?;
        let tables = Tables {
            train: TrainTables::read(&exe)?,
            rules: RuleTables::read(&exe)?,
            stages: StageTables::read(&exe)?,
            result: ResultTables::read(&exe)?,
        };
        let tolerance_m = tables
            .rules
            .stop_tolerance_m
            .get(opts.stage, opts.difficulty);
        let drive = start_drive(&tables, &opts, opts.station)?;

        layer.slots.fill(Slot::CLEAR);
        layer.fans.clear();
        Hud::install_palettes(&exe, &mut layer)?;
        // The horn hint names the B button with a joystick in modes 5–6 (`FUN_00433270`).
        let mut cab = CabHud::new(&exe, opts.stage, matches!(opts.control_mode, 5 | 6))?;
        let o = save.eeprom.options();
        cab.hud.speedometer = match opts.speedometer.unwrap_or(o.speed_meter) {
            0 => Speedometer::None,
            2 => Speedometer::Digital,
            _ => Speedometer::Analog,
        };
        cab.hud.distance = match o.distance_meter {
            0 => DistanceReadout::Off,
            1 => DistanceReadout::Centimetres,
            _ => DistanceReadout::Metres,
        };
        cab.create(&mut layer);
        let screen_tables = ScreenTables::read(&exe)?;
        let flow = StageFlow::new(FlowDefs::new(&screen_tables, |tbl| {
            usize::try_from(tbl)
                .ok()
                .and_then(|i| bank.tilemaps.get(i))
                .map_or((0, 0), |m| {
                    (i16::from(m.width) * TILE_PX, i16::from(m.height) * TILE_PX)
                })
        }));

        if let Some(s) = sound.as_mut() {
            s.begin_stage(&exe, &drive)?;
        }
        let visibility = visibility(&opts);
        let scene = load_scene(&data, drive.com_name(), visibility, &opts)?;
        let script = match &opts.inputs {
            Some(p) => {
                parse_script(&std::fs::read_to_string(p).with_context(|| p.display().to_string())?)?
            }
            None => Script::default(),
        };
        let now = Instant::now();
        let mut play = Play {
            scene_com: drive.com(),
            drive,
            tables,
            data,
            visibility,
            scene,
            controls,
            latched: DriverInput::default(),
            menu_input: MenuInput::NONE,
            saved_levers: (0, 0),
            down: [false; 256],
            cab,
            flow,
            layer,
            bank,
            canvas: vec![0; CANVAS_W * CANVAS_H * 4],
            canvas_dirty: true,
            tolerance_m,
            last_events: Vec::new(),
            ticks: 0,
            leave: false,
            interpolate: false,
            frame: FrameGeometry::default(),
            gpu,
            accumulator: Duration::ZERO,
            last_frame: now,
            title_at: now,
            error: None,
            sound,
            script,
            save,
            meter_m: 0.0,
            weather: crate::weather::Weather::new(&exe, opts.stage)?,
            result_view: None,
            world: WorldView::Cab,
            opts,
        };
        play.configure_gpu();
        Ok(play)
    }

    /// Ends the run and hands the shared resources back (sound back to the menus, the HUD's
    /// sprites cleared).
    pub fn into_shared(self) -> Shared {
        let mut sound = self.sound;
        if let Some(s) = sound.as_mut() {
            s.end_stage();
        }
        let mut layer = self.layer;
        layer.slots.fill(Slot::CLEAR);
        layer.fans.clear();
        Shared {
            data: self.data,
            bank: self.bank,
            layer,
            save: self.save,
            controls: self.controls,
            sound,
            gpu: self.gpu,
        }
    }

    /// Quit or run over: the owner goes back to the title.
    pub fn left(&self) -> bool {
        self.leave
    }

    pub fn gpu(&self) -> Option<&Gpu> {
        self.gpu.as_ref()
    }

    pub fn save(&self) -> &Save {
        &self.save
    }

    pub fn take_error(&mut self) -> Option<anyhow::Error> {
        self.error.take()
    }

    /// Keys down for the next poll, by DirectInput code (the front end's headless script).
    pub fn set_down(&mut self, down: [bool; 256]) {
        self.down = down;
    }

    /// The global tick counter `0x84c494` at the stage start (the ticks the game ran before,
    /// menus included); the drive advances it from there.
    pub fn set_global_tick(&mut self, tick: u32) {
        self.drive.set_global_tick(tick);
    }

    /// One tick, for headless drivers.
    pub fn step(&mut self) -> Result<()> {
        self.tick()
    }

    /// Renders the current frame (cab view and HUD) to a PPM, headless.
    pub fn shot(&self, out: &std::path::Path) -> Result<()> {
        let (texture_filtering, fog, square) = (
            self.opts.texture_filtering,
            self.opts.fog,
            self.opts.square_pixels,
        );
        let (w, h) = self.opts.size;
        if self.world != WorldView::Cab {
            // Station result (scene.md §8): the replay views, or nothing while hidden.
            return crate::shot::render_to_ppm_with(
                self.opts.size,
                |device, queue, view, format| {
                    let mut r = ddg_render::Renderer::new(device, format);
                    r.texture_filtering = texture_filtering;
                    r.fog.enabled = false;
                    r.upload_pages(device, queue, &self.scene.library.pages);
                    let mut frame = FrameGeometry::default();
                    match &self.result_view {
                        Some(v) => v.compose(&mut frame, &self.scene.library, 1.0),
                        None => frame.clear(&self.scene.library),
                    }
                    eprintln!(
                        "result replay: {} triangles, {} missing parts",
                        frame.triangle_count(),
                        frame.missing_parts
                    );
                    r.set_geometry(device, queue, &frame);
                    match &self.result_view {
                        Some(v) => v.render(
                            &mut r,
                            device,
                            queue,
                            view,
                            (w, h),
                            ddg_render::overlay::canvas_rect(w, h),
                            square,
                            1.0,
                        ),
                        None => crate::shot::clear_black(device, queue, view),
                    }
                },
                Some(&self.canvas),
                out,
            );
        }
        let mut effects = ddg_render::effects::EffectGeometry::default();
        let band_aspect = w as f32 / (h as f32 * (1.0 - HUD_BAND_TOP)).max(1.0);
        self.weather
            .compose(&self.scene, &self.drive, band_aspect, &mut effects);
        let fog = fog && self.weather.fog();
        crate::shot::render_to_ppm(
            Some((&self.scene, self.drive.pos())),
            self.opts.size,
            |r| {
                r.texture_filtering = texture_filtering;
                r.fog.enabled = fog;
                r.projection.fov_tenths = (DRIVING_FOV_Y_DEG * 10.0) as u16;
                r.projection.stretch = if square { 1.0 } else { ORIGINAL_STRETCH };
                r.band_top = HUD_BAND_TOP;
                r.effects = effects;
            },
            Some(&self.canvas),
            out,
        )
    }

    /// Window events other than redraws: keys, focus, resize.
    pub fn window_input(&mut self, event: &WindowEvent) {
        match event {
            WindowEvent::Resized(size) => {
                if let Some(g) = self.gpu.as_mut() {
                    g.resize(size.width, size.height);
                }
            }
            WindowEvent::Focused(false) => self.down = [false; 256],
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state,
                        ..
                    },
                ..
            } => {
                // Esc is pause and cancel in the button word (input.md §6.1).
                if let Some(d) = crate::input::dik(*code) {
                    self.down[usize::from(d)] = *state == ElementState::Pressed;
                }
            }
            _ => {}
        }
    }
}

fn visibility(opts: &PlayOptions) -> Visibility {
    if opts.original_visibility {
        return Visibility::Original;
    }
    let mut p = ddg_render::Projection {
        fov_tenths: (DRIVING_FOV_Y_DEG * 10.0) as u16,
        ..Default::default()
    };
    if opts.square_pixels {
        p.stretch = 1.0;
    }
    let band_aspect = opts.size.0 as f32 / (opts.size.1 as f32 * (1.0 - HUD_BAND_TOP));
    Visibility::Wide {
        tan_half_fov_x: p.tan_half_horizontal(band_aspect.max(MIN_WIDE_ASPECT)),
    }
}

fn load_scene(
    data: &GameData,
    com: &str,
    visibility: Visibility,
    opts: &PlayOptions,
) -> Result<RouteScene> {
    let t = Instant::now();
    let mut scene = RouteScene::load(data, com, visibility)?;
    scene.stable_track_variants = opts.stable_track_variants;
    crate::traffic::load_stage_trains(&mut scene, data, opts.stage)?;
    eprintln!("{com}: loaded in {:.2?}", t.elapsed());
    Ok(scene)
}

/// Scripted inputs (see [`PlayOptions::inputs`]).
#[derive(Debug, Default)]
pub struct Script {
    /// `(drive tick, notch, brake)`, sorted.
    pub notches: Vec<(u64, u8, u8)>,
    /// `(play tick, DIK code)`.
    pub keys: Vec<(u64, u8)>,
}

/// DirectInput codes of the script's key names.
fn script_key(name: &str) -> Option<u8> {
    Some(match name {
        "enter" => 0x1c,
        "esc" => 0x01,
        "space" => 0x39,
        "up" => 0xc8,
        "down" => 0xd0,
        "left" => 0xcb,
        "right" => 0xcd,
        "r" => 0x13,
        _ => return None,
    })
}

/// Parses the input script (`#` comments).
pub fn parse_script(text: &str) -> Result<Script> {
    let mut s = Script::default();
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let v: Vec<&str> = line.split_whitespace().collect();
        match v[..] {
            ["key", t, k] => {
                let code =
                    script_key(k).with_context(|| format!("line {}: unknown key {k}", n + 1))?;
                // `key <from>-<to> name` holds the key over a range of ticks.
                let (from, to) = t.split_once('-').unwrap_or((t, t));
                for tick in from.parse::<u64>()?..=to.parse()? {
                    s.keys.push((tick, code));
                }
            }
            [t, p, b] => s.notches.push((t.parse()?, p.parse()?, b.parse()?)),
            _ => anyhow::bail!(
                "line {}: expected `tick notch brake` or `key tick name`",
                n + 1
            ),
        }
    }
    s.notches.sort_by_key(|s| s.0);
    Ok(s)
}

/// Track units from the main-line origin.
fn units(p: TrackPos) -> f32 {
    p.engine_index as f32 * UNITS_PER_POINT + p.t
}

fn track_pos(units: f32) -> TrackPos {
    let engine_index = (units / UNITS_PER_POINT).floor().max(0.0) as usize;
    TrackPos {
        engine_index,
        t: units - engine_index as f32 * UNITS_PER_POINT,
    }
}

pub struct Play {
    opts: PlayOptions,
    tables: Tables,
    data: GameData,
    drive: Drive,
    visibility: Visibility,
    scene: RouteScene,
    scene_com: u32,
    controls: Controls,
    /// Controls polled at the end of the last tick (the physics reads them this tick).
    latched: DriverInput,
    /// Button edges of that poll, as the screens' tasks see them this tick.
    menu_input: MenuInput,
    /// Notches saved when the pause opened (0x42b7f0 saves 0x829d00 / 0x829d04).
    saved_levers: (u8, u8),
    /// Keys down now, by DirectInput scan code.
    down: [bool; 256],
    cab: CabHud,
    flow: StageFlow,
    layer: SpriteLayer,
    bank: SpriteBank,
    canvas: Vec<u8>,
    canvas_dirty: bool,
    tolerance_m: i32,
    /// Rule events of the previous tick, to keep repeats out of the log.
    last_events: Vec<ddg_engine::rules::RuleEvent>,
    /// Play ticks (simulation and paused).
    ticks: u64,
    /// Quit or run over: leave play.
    leave: bool,
    /// False right after a segment change: the position jumped, don't interpolate.
    interpolate: bool,
    frame: FrameGeometry,
    gpu: Option<Gpu>,
    accumulator: Duration,
    last_frame: Instant,
    title_at: Instant,
    error: Option<anyhow::Error>,
    sound: Option<crate::sound::Sound>,
    script: Script,
    save: Save,
    /// Metres driven with the distance meter shown (`0x8571f0`; reset by 0x40d040 at a run
    /// start, kept over continues).
    meter_m: f32,
    weather: crate::weather::Weather,
    /// The station result's replay (scene.md §8) and what the 3D layer shows.
    result_view: Option<ResultView>,
    world: WorldView,
}

impl Play {
    /// One 33 ms tick: a frozen pause tick, or a simulation tick with the stage flow.
    fn tick(&mut self) -> Result<()> {
        self.ticks += 1;
        let input = self.menu_input;
        let mut sfx = Vec::new();
        if self.flow.paused() {
            let result = self.flow.frozen_tick(&input, &mut sfx);
            self.poll();
            if let Some(r) = result {
                self.end_pause(r)?;
            }
            self.play(&sfx);
            self.draw_frozen();
            return Ok(());
        }
        if self.flow.try_pause(&input, self.drive.phase(), &mut sfx) {
            self.saved_levers = (self.controls.power(), self.controls.brake());
            if let Some(s) = self.sound.as_mut() {
                s.set_paused(true);
            }
            self.poll();
            self.play(&sfx);
            self.draw_frozen();
            return Ok(());
        }

        let before = crate::sound::Before::of(&self.drive);
        let out = self.drive.tick(self.latched);
        self.scene.traffic = self.drive.traffic_poses(1.0);
        self.scene.signal_model = usize::from(self.drive.rules().signal_model);
        if let Some(s) = self.sound.as_mut() {
            s.events(&out.events);
            s.after_tick(
                &self.drive,
                before,
                self.latched.horn_held,
                out.segment_started,
            );
        }
        self.interpolate = !out.segment_started;
        if out.segment_started {
            // Digital modes force P0 / EB at each station start (0x4332c0, input.md §4.1).
            self.controls.reset_levers();
        }
        // Some messages are re-raised every tick while their condition holds (the overrun
        // message, 0x40dad0); log only what changed.
        for e in out.events.iter().filter(|e| !self.last_events.contains(e)) {
            eprintln!("[{}] {e:?}", self.drive.tick_count());
        }
        let hud_input = TickInput {
            segment_started: out.segment_started,
            horn_held: self.latched.horn_held,
            confirm_pressed: self.latched.confirm_pressed,
            remaining_override: None,
            tolerance_m: self.tolerance_m,
        };
        self.last_events = out.events;

        // The input poll at the end of `Task_Tick`: next tick's controls.
        self.poll();
        let obs = self.observation();
        match self.flow.after_tick(&obs, &input, &mut sfx) {
            Some(Action::Ranking { cleared }) => self.start_ranking(cleared, &mut sfx),
            Some(action) => self.act(action)?,
            None => {}
        }
        self.play(&sfx);

        if self.drive.com() != self.scene_com {
            self.reload_scene()?;
        }
        self.update_world();
        for e in &self.last_events {
            if let RuleEvent::Coupling(CouplingEvent::Jolt(a)) = *e {
                self.weather.start_jolt(a);
            }
        }
        self.weather.tick(&self.drive, hud_input.segment_started);
        self.weather.apply_to_scene(&mut self.scene);
        self.update_hud(hud_input);
        self.track_meter();
        Ok(())
    }

    /// The distance-meter odometer of `Hud_Distance` 0x4162c0: runs while the HUD's distance
    /// panel is active and shown (not the "Professional" plate) and the distance option is on.
    fn track_meter(&mut self) {
        let shown = self.flow.hud_visible()
            && self.cab.panel_shown()
            && self.save.eeprom.options().distance_meter != 0;
        if shown {
            self.meter_m += self.drive.train().speed * METER_SCALE as f32;
        }
    }

    /// The station result's replay camera: created at result countdown 94 (0x419730), one
    /// update per tick after that (0x41a010), gone when the result closes (scene.md §8).
    fn update_world(&mut self) {
        self.world = self.flow.world_view();
        if self.world != WorldView::ResultReplay {
            self.result_view = None;
            return;
        }
        if let Some(v) = self.result_view.as_mut() {
            v.tick();
            return;
        }
        let r = self.drive.rules();
        let field7 = self.drive.next_station().1.map_or(0, |row| row.params[3]);
        self.result_view = Some(ResultView::new(
            &self.tables.result,
            &self.tables.stages.train_models,
            ResultViewInput {
                stage: r.stage().0,
                station: r.station,
                field7,
                train_type: self.drive.train().train_type,
                record: &r.stop_record,
            },
        ));
    }

    /// The end of `Stage_Run` after the report card (0x4080d0, menus.md §11.4): the clear mark,
    /// then the ranking insert with the run's value and PRO flag.
    fn start_ranking(&mut self, cleared: bool, sfx: &mut Vec<Sfx>) {
        let r = self.drive.rules();
        let (stage, _) = r.stage();
        if cleared {
            // Select stage 20 keeps its own mark (`Stage_Run` sets `0x8570a4 = 0x14` around it).
            let mark = if self.opts.variant { 20 } else { stage };
            self.save.eeprom.raise_clear_mark(mark, r.continues);
        }
        let score = r.ranking_score(cleared);
        let pro = self.meter_m <= PRO_LIMIT_M;
        eprintln!(
            "[play tick {}] ranking value {score}, PRO {pro} ({:.1} m with the meter shown)",
            self.ticks, self.meter_m
        );
        let req = RankingRequest {
            route: stage,
            score,
            pro,
            power: self.controls.power(),
            rankings: &self.save.rankings,
        };
        self.flow.start_ranking(Some(&req), sfx);
    }

    /// Polls the controls and latches the next tick's driver input and menu edges.
    fn poll(&mut self) -> ButtonState {
        let down = self.down;
        let tick = self.ticks;
        let keys = &self.script.keys;
        // The Shift override is allowed outside driving or while paused (input.md §6).
        let shift_allowed = self.drive.phase() != DrivePhase::Driving || self.flow.paused();
        let buttons = self.controls.poll(
            |c| down[usize::from(c)] || keys.iter().any(|&(t, k)| t == tick && k == c),
            shift_allowed,
        );
        // Analog-lever modes: dialog cursors follow the power notch (menus.md §1.4), the
        // name-entry wheel the brake notch.
        let levers = self
            .controls
            .is_analog()
            .then(|| (self.controls.power(), self.controls.brake()));
        self.menu_input = MenuInput::from_word(&buttons.word, levers);
        self.latched = DriverInput {
            power: self.controls.power(),
            brake: self.controls.brake(),
            horn_held: buttons.horn_held,
            horn_pressed: buttons.horn_pressed,
            confirm_pressed: buttons.confirm_pressed,
        };
        let next = self.drive.tick_count();
        if let Some(&(_, p, b)) = self.script.notches.iter().rev().find(|s| s.0 <= next) {
            self.latched.power = p;
            self.latched.brake = b;
        }
        buttons
    }

    /// The end of 0x42b7f0: sound resumes, the notches come back; then quit or restart.
    fn end_pause(&mut self, r: PauseResult) -> Result<()> {
        if let Some(s) = self.sound.as_mut() {
            s.set_paused(false);
        }
        let (p, b) = self.saved_levers;
        self.controls.set_levers(p, b);
        if self.script.notches.is_empty() {
            (self.latched.power, self.latched.brake) = self.saved_levers;
        }
        eprintln!("pause: {r:?}");
        match r {
            PauseResult::Resume => {}
            PauseResult::Quit => self.act(Action::Quit)?,
            PauseResult::Retry => self.act(Action::Restart)?,
        }
        Ok(())
    }

    fn observation(&self) -> Observation {
        let r = self.drive.rules();
        let (stage, difficulty) = r.stage();
        let t = &self.tables.rules;
        Observation {
            phase: self.drive.phase(),
            can_continue: self.drive.can_continue(),
            remaining_s: r.remaining_s,
            result: ResultInput {
                stage,
                dist_round_m: r.dist_round_m,
                tolerance_m: t.stop_tolerance_m.get(stage, difficulty),
                dist_m: r.dist_m,
                repowered: r.repowered > 0,
                repower_s: t.repower_s.get(stage, difficulty),
                emergency_brake: r.eb_approach_ticks > 0,
                lesson: r.stage_rank() == 1,
                clean_mark: r.marks.get(r.station).is_some_and(|&m| m > 0),
                overrun_factor: t.overrun_factor.get(stage, difficulty),
                exact_bonus_s: t.exact_stop_bonus_s.get(stage, difficulty),
            },
            grades: r.grades().parts,
            continues: r.continues,
            stage,
            stage_kind: r.stage_rank(),
            time_up_running: self.cab.time_up_running(),
        }
    }

    fn act(&mut self, action: Action) -> Result<()> {
        match action {
            Action::FinishStation => {
                let r = self.drive.finish_station()?;
                eprintln!("station result: {r:?}");
                let can_continue = self.drive.can_continue();
                self.flow.station_finished(r.outcome, can_continue);
            }
            Action::ContinueRun => {
                eprintln!("continue");
                self.drive.continue_run()?;
            }
            Action::Restart => {
                // 始発駅に戻る: the route again from station 0, no departure board.
                let global_tick = self.drive.global_tick();
                self.drive = start_drive(&self.tables, &self.opts, 0)?;
                self.drive.set_global_tick(global_tick);
                self.controls.reset_levers();
                self.meter_m = 0.0;
                self.interpolate = false;
                self.flow.reset();
                self.layer.slots.fill(Slot::CLEAR);
                self.cab.create(&mut self.layer);
                if self.drive.com() != self.scene_com {
                    self.reload_scene()?;
                }
            }
            // Answered in `tick`, which has the sound list.
            Action::Ranking { .. } => {}
            Action::RankingDone {
                table,
                rows,
                commit,
            } => {
                if let Some(t) = self.save.rankings.get_mut(table) {
                    *t = rows;
                }
                if commit {
                    self.save.commit_table(table);
                }
                eprintln!("ranking table {table} updated (packed into the save: {commit})");
            }
            Action::Quit | Action::RunOver => {
                if let Some(s) = self.sound.as_mut() {
                    s.stop_all();
                }
                eprintln!("{action:?}: leaving play");
                self.leave = true;
            }
        }
        Ok(())
    }

    fn reload_scene(&mut self) -> Result<()> {
        self.scene = load_scene(
            &self.data,
            self.drive.com_name(),
            self.visibility,
            &self.opts,
        )?;
        self.scene_com = self.drive.com();
        self.interpolate = false;
        if let Some(g) = self.gpu.as_mut() {
            g.renderer
                .upload_pages(&g.device, &g.queue, &self.scene.library.pages);
        }
        Ok(())
    }

    fn play(&mut self, sfx: &[Sfx]) {
        let paused = self.flow.paused();
        let Some(s) = self.sound.as_mut() else {
            return;
        };
        for &c in sfx {
            match c {
                // Dialog clicks bypass the pause gate (menus.md §1.4).
                Sfx::Play(code) if paused => s.request_ungated(code),
                Sfx::Play(code) => s.request(code),
                Sfx::Stop(code) => s.release(code),
            }
        }
    }

    /// A paused tick: only the render list runs (`0x855ffc & 4`); the dialog changes.
    fn draw_frozen(&mut self) {
        self.flow.clear_drawn(&mut self.layer);
        self.flow.draw(&mut self.layer);
        self.layer.compose(&self.bank, &mut self.canvas);
        self.canvas_dirty = true;
    }

    fn update_hud(&mut self, input: TickInput) {
        self.layer.reset_transient();
        self.flow.clear_drawn(&mut self.layer);
        if !self.flow.hud_visible() {
            // The HUD task is killed before the report card and its slots cleared.
            self.layer.slots.fill(Slot::CLEAR);
            self.flow.draw(&mut self.layer);
            self.layer.compose(&self.bank, &mut self.canvas);
            self.canvas_dirty = true;
            return;
        }
        let input = TickInput {
            remaining_override: self.flow.remaining_override(),
            ..input
        };
        let snap = Snap::of(&self.drive);
        self.cab
            .tick(&snap, &input, &self.last_events, &mut self.layer);
        self.flow.draw(&mut self.layer);
        self.layer.compose(&self.bank, &mut self.canvas);
        self.canvas_dirty = true;
    }

    pub fn redraw(&mut self) -> Result<()> {
        let now = Instant::now();
        let dt = now - self.last_frame;
        self.last_frame = now;
        self.accumulator += dt.min(Duration::from_millis(250));
        while self.accumulator >= TICK && !self.leave {
            self.accumulator -= TICK;
            self.tick()?;
        }
        // Interpolate the track distance between the last two ticks (along the track, so the
        // camera follows curves), then evaluate the original's cab camera there. Frozen while
        // paused.
        let (prev, cur) = (units(self.drive.prev_pos()), units(self.drive.pos()));
        let alpha = self.accumulator.as_secs_f32() / TICK.as_secs_f32();
        let pos = if self.interpolate && !self.flow.paused() {
            track_pos(prev + (cur - prev) * alpha)
        } else {
            self.drive.pos()
        };
        if self.interpolate && !self.flow.paused() {
            self.scene.traffic = self.drive.traffic_poses(alpha);
        }

        let Some(gpu) = self.gpu.as_mut() else {
            return Ok(());
        };
        let (width, height) = gpu.size();
        let band_aspect = width as f32 / (height as f32 * (1.0 - HUD_BAND_TOP)).max(1.0);
        let mut camera = self.scene.cab_camera(pos);
        if self.world == WorldView::Cab {
            self.scene.compose(&mut self.frame, pos, false, band_aspect);
            self.weather.shake(&mut camera);
            let fx = &mut gpu.renderer.effects;
            self.weather
                .compose(&self.scene, &self.drive, band_aspect, fx);
            gpu.renderer.fog.enabled = self.opts.fog && self.weather.fog();
        } else {
            // The drive's task list (scenery, traffic, weather, fog) is gone at a station result.
            match &self.result_view {
                Some(v) => v.compose(&mut self.frame, &self.scene.library, alpha),
                None => self.frame.clear(&self.scene.library),
            }
            gpu.renderer.effects = Default::default();
            gpu.renderer.fog.enabled = false;
        }
        let Some(frame) = gpu.acquire() else {
            return Ok(());
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        gpu.renderer
            .set_geometry(&gpu.device, &gpu.queue, &self.frame);
        match &self.result_view {
            Some(v) if self.world == WorldView::ResultReplay => v.render(
                &mut gpu.renderer,
                &gpu.device,
                &gpu.queue,
                &view,
                (width, height),
                ddg_render::overlay::canvas_rect(width, height),
                self.opts.square_pixels,
                alpha,
            ),
            _ => gpu
                .renderer
                .render(&gpu.device, &gpu.queue, &view, (width, height), &camera),
        }
        if std::mem::take(&mut self.canvas_dirty) {
            gpu.overlay.upload(&gpu.queue, &self.canvas);
        }
        gpu.overlay.render(
            &gpu.device,
            &gpu.queue,
            &view,
            ddg_render::overlay::canvas_rect(width, height),
        );
        gpu.window.pre_present_notify();
        gpu.queue.present(frame);

        if now - self.title_at >= Duration::from_millis(500) {
            self.title_at = now;
            let (station, _) = self.drive.next_station();
            gpu.window.set_title(&format!(
                "ddg-game play | {:?}{} | {:.1} km/h | next {} | {} m",
                self.drive.phase(),
                if self.flow.paused() { " (paused)" } else { "" },
                self.drive.speed_kmh(),
                self.drive.station_name(station).unwrap_or("?"),
                self.drive.rules().dist_round_m,
            ));
        }
        Ok(())
    }

    fn init_gpu(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        self.gpu = Some(Gpu::create(
            event_loop,
            "ddg-game play",
            self.opts.size,
            self.opts.uncapped,
        )?);
        self.configure_gpu();
        Ok(())
    }

    /// The cab view's renderer settings and the route's textures.
    fn configure_gpu(&mut self) {
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        self.last_frame = Instant::now();
        self.accumulator = Duration::ZERO;
        let r = &mut gpu.renderer;
        r.texture_filtering = self.opts.texture_filtering;
        r.fog.enabled = self.opts.fog;
        r.projection.fov_tenths = (DRIVING_FOV_Y_DEG * 10.0) as u16;
        r.projection.stretch = if self.opts.square_pixels {
            1.0
        } else {
            ORIGINAL_STRETCH
        };
        r.band_top = HUD_BAND_TOP;
        gpu.renderer
            .upload_pages(&gpu.device, &gpu.queue, &self.scene.library.pages);
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, e: anyhow::Error) {
        self.error = Some(e);
        event_loop.exit();
    }
}

impl ApplicationHandler for Play {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        match self.init_gpu(event_loop) {
            Ok(()) => {
                self.last_frame = Instant::now();
                if let Some(g) = &self.gpu {
                    g.window.request_redraw();
                }
            }
            Err(e) => self.fail(event_loop, e.context("GPU init")),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                if let Err(e) = self.redraw() {
                    self.fail(event_loop, e);
                    return;
                }
                if self.leave {
                    event_loop.exit();
                    return;
                }
                if let Some(g) = &self.gpu {
                    g.window.request_redraw();
                }
            }
            other => self.window_input(&other),
        }
    }
}

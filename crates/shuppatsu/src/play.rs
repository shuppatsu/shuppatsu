//! `shuppatsu play`: drive a stage. The simulation is `shuppatsu_engine::drive` at the original's
//! fixed 33 ms tick; rendering interpolates the camera along the track between the last two
//! ticks. The stage flow (pause dialogs, station result, continue, report card, ranking and
//! name entry, run-end banner) is [`crate::screens::flow::StageFlow`]; quitting or ending the run
//! leaves play. Standalone (`shuppatsu play <stage>`) that closes the window; under the front end
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
use crate::script::{InputLog, Script, parse_script};
use crate::window::Gpu;
use anyhow::{Context, Result};
use shuppatsu_engine::coupling::CouplingEvent;
use shuppatsu_engine::drive::{Drive, DrivePhase, DriverInput, LoadError};
use shuppatsu_engine::physics::TrainTables;
use shuppatsu_engine::rules::{RuleEvent, RuleTables};
use shuppatsu_formats::exe::Exe;
use shuppatsu_formats::result_camera::ResultTables;
use shuppatsu_formats::route::UNITS_PER_POINT;
use shuppatsu_formats::scene::{DRIVING_FOV_Y_DEG, TrackPos};
use shuppatsu_formats::stages::StageTables;
use shuppatsu_render::FrameGeometry;
use shuppatsu_render::camera::ORIGINAL_STRETCH;
use shuppatsu_render::sprites::{Anchor, SLOT_COUNT, Slot, SpriteBank, SpriteLayer};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

mod bonus;
mod cab_mask;
mod demo;
mod ending;
mod intro;
pub(crate) mod mark;
mod opening;
mod staff_roll;
pub use intro::has_stage_intro;
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
    /// No `--difficulty` given: use the save's (a fresh install's default image says 1, Normal,
    /// like the original).
    pub difficulty_from_save: bool,
    pub station: usize,
    /// Control mode 0..=6 (0–3 keyboard, 4–6 joystick; input.md §4–§5).
    pub control_mode: usize,
    pub pad: PadOptions,
    pub size: (u32, u32),
    /// The port's switches (docs/settings.md).
    pub settings: crate::settings::Settings,
    pub mute: bool,
    /// Scripted inputs ([`crate::script`]): `tick notch brake` lines (the oracle's format,
    /// drive ticks), held from each tick on, `key <tick>[-<last>] <name>` lines (the key is
    /// down for the poll at the end of play tick `tick`, through `last`; play ticks also count
    /// paused ticks) and `pad` lines.
    pub inputs: Option<std::path::PathBuf>,
    /// Check aid (issue marks): the save to start from, read only (`--save`).
    pub save_file: Option<std::path::PathBuf>,
    /// Check aid (issue marks): the global tick counter at the start (`--global-tick`).
    pub global_tick: Option<u32>,
    /// Headless: run this many play ticks, render one frame with the HUD to a PPM and exit.
    pub shot: Option<(u64, std::path::PathBuf)>,
    /// Overrides the save's speedometer option (0 none, 1 analog, 2 digital).
    pub speedometer: Option<u8>,
    /// Select stage 20: route 10 with `0x48ddf0 = 1`, whose clear mark is the 21st byte.
    pub variant: bool,
    /// Check aid: start with the ending (`full` or `plain`) instead of driving.
    pub ending: Option<bool>,
    /// Check aid: play the stage intro (0x4091a0 or 0x408f40) and leave.
    pub intro: bool,
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
    let drive = new_drive(tables, (opts.stage, opts.difficulty, station))?;
    eprintln!(
        "stage {} difficulty {} from station {station} ({})",
        opts.stage,
        opts.difficulty,
        drive.station_name(station).unwrap_or("?")
    );
    Ok(drive)
}

/// The drive of `stage` at `difficulty` from `station`.
fn new_drive(
    tables: &Tables,
    (stage, difficulty, station): (usize, usize, usize),
) -> Result<Drive> {
    let loader_data = GameData::from_env()?;
    let loader = move |name: &str| -> Result<Vec<u8>, LoadError> {
        loader_data
            .read(name)
            .map_err(|e| LoadError::from(format!("{e:#}")))
    };
    Drive::new(
        &tables.train,
        &tables.rules,
        &tables.stages,
        stage,
        difficulty,
        station,
        loader,
    )
    .context("starting the stage")
}

/// What a run's route scene is loaded from ([`load_scene_for`]): the COM file, visibility,
/// settings, geometry, stage and texture pack.
type SceneKey = (
    String,
    Visibility,
    crate::settings::Settings,
    crate::scene::Geometry,
    usize,
    Option<PathBuf>,
);

/// A scene loading on a thread ahead of its run ([`load_scene_ahead`]), with its key.
type SceneAhead = std::thread::JoinHandle<Result<(SceneKey, RouteScene)>>;

static SCENE_AHEAD: std::sync::Mutex<Option<SceneAhead>> = std::sync::Mutex::new(None);

/// The key of the scene a run with `opts` starting on `com` loads.
fn scene_key(com: &str, opts: &PlayOptions) -> SceneKey {
    (
        com.to_owned(),
        visibility(opts),
        opts.settings,
        geometry(opts),
        opts.stage,
        opts.locale.texture_pack.clone(),
    )
}

/// Starts loading on a thread the route scene of the run `opts` will start (the departure
/// board shows for seconds before it); [`Play::new`] takes it if its key matches. The
/// stage-start tick waited for that load (about 20 ms, its longest part).
pub fn load_scene_ahead(data: &GameData, opts: &PlayOptions) {
    let mut data = data.clone();
    data.texture_pack = opts.locale.texture_pack.clone();
    let key = scene_key("", opts);
    let (stage, difficulty, station) = (opts.stage, opts.difficulty, opts.station);
    let job = std::thread::spawn(move || {
        let exe_bytes = data.exe()?;
        let exe = Exe::parse(&exe_bytes)?;
        let tables = Tables {
            train: TrainTables::read(&exe)?,
            rules: RuleTables::read(&exe)?,
            stages: StageTables::read(&exe)?,
            result: ResultTables::read(&exe)?,
        };
        let drive = new_drive(&tables, (stage, difficulty, station))?;
        let (_, vis, settings, geometry, stage, pack) = key;
        let key = (
            drive.com_name().to_owned(),
            vis,
            settings,
            geometry,
            stage,
            pack,
        );
        let scene = load_scene_for(&data, &key.0, key.1, (key.2, key.3, key.4))?;
        Ok((key, scene))
    });
    *SCENE_AHEAD.lock().unwrap_or_else(|e| e.into_inner()) = Some(job);
}

/// The scene loaded ahead for `key`, if there is one ([`load_scene_ahead`]).
fn scene_ahead(key: &SceneKey) -> Option<RouteScene> {
    let job = SCENE_AHEAD
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()?;
    match job.join() {
        Ok(Ok((k, scene))) if k == *key => Some(scene),
        _ => None,
    }
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
    /// What every poll of the session read, for issue marks ([`crate::marks`]).
    pub log: InputLog,
}

/// The input script of `opts.inputs` (none: empty).
pub fn load_script(inputs: Option<&std::path::Path>) -> Result<Script> {
    match inputs {
        Some(p) => {
            parse_script(&std::fs::read_to_string(p).with_context(|| p.display().to_string())?)
        }
        None => Ok(Script::default()),
    }
}

/// The save of a run or session: the user's (`path`), or with a check aid's `--save` that
/// file, read and never written.
pub fn open_save(exe: &Exe<'_>, path: Option<PathBuf>, file: Option<&Path>) -> Result<Save> {
    let Some(file) = file else {
        return Save::open(exe, path);
    };
    let bytes = std::fs::read(file).with_context(|| file.display().to_string())?;
    let defaults = crate::save::Eeprom::defaults(exe)?;
    Ok(Save::new(
        crate::save::Eeprom::load(&bytes, &defaults),
        None,
    ))
}

/// Loads the sprite bank (`cg.rom` + `tbl.rom`) with the locale's text overrides, and the
/// front-end tables pointing at the port's own cells in it ([`crate::front::add_port_art`]).
pub fn load_bank(
    data: &GameData,
    exe: &Exe<'_>,
    locale: &crate::locale::LocaleConfig,
) -> Result<(SpriteBank, crate::front::tables::FrontTables)> {
    let cg_bytes = data.read("cg.rom")?;
    let mut bank = SpriteBank::new(
        &shuppatsu_formats::cg::CgFile::parse(&cg_bytes)?,
        shuppatsu_formats::cg::parse_tbl(&data.read("tbl.rom")?)?,
    );
    // Port: the movie label numbered 5 and the options' extra cells, before the translations
    // name them.
    let mut front = crate::front::tables::FrontTables::read(exe)?;
    crate::front::add_port_art(&mut bank, &mut front);
    crate::locale::install(locale, exe, &mut bank)?;
    // Controller hints (`hybrid_input`) in every language.
    bank.set_legend_font(Some(shuppatsu_render::label::LabelFont::new(Box::new(
        crate::locale::bundled_font()?,
    ))));
    Ok((bank, front))
}

/// `shuppatsu play <stage>`: one run in its own window (or headless with `--shot`).
pub fn run(mut opts: PlayOptions) -> Result<()> {
    let data = GameData::from_env()?;
    let exe_bytes = data.exe()?;
    let exe = Exe::parse(&exe_bytes)?;
    // Headless shots use the exe's default save and never write one.
    let save_path = if opts.shot.is_some() {
        None
    } else {
        crate::save::default_path()
    };
    let mut save = open_save(&exe, save_path, opts.save_file.as_deref())?;
    if opts.difficulty_from_save {
        opts.difficulty = usize::from(save.eeprom.difficulty());
    }
    save.save_on_change = opts.settings.save_on_change;
    if let Some(p) = save.path() {
        eprintln!("save: {}", p.display());
    }
    // A script with `pad` lines is the gamepad (a recorded session, docs/marks.md).
    let script = load_script(opts.inputs.as_deref())?;
    if script.has_pad() {
        opts.pad.scripted_pad = Some(script.pad_at(0).is_some());
    }
    let controls = Controls::new(&exe, opts.control_mode, &opts.pad)?;
    let mut log = InputLog::default();
    log.start(controls.pad_connected());
    let (bank, _) = load_bank(&data, &exe, &opts.locale)?;
    let sound = if opts.shot.is_some() || opts.mute {
        None
    } else {
        crate::sound::Sound::open_device(data.data_dir(), &exe, opts.locale.audio_resampler)?
    };
    let shared = Shared {
        data,
        bank,
        layer: SpriteLayer::default(),
        save,
        controls,
        sound,
        gpu: None,
        log,
    };
    let global_tick = opts.global_tick;
    let mut app = Play::new(shared, opts)?;
    if let Some(t) = global_tick {
        app.set_global_tick(t);
    }
    if let Some(full) = app.opts.ending {
        app.start_ending(Some(full))?;
    }
    if app.opts.intro && !app.start_stage_intro()? {
        eprintln!("stage {} has no stage intro", app.opts.stage);
    }
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
    shuppatsu_render::perf::flush();
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
        let mut opts = opts;
        let Shared {
            mut data,
            mut bank,
            mut layer,
            save,
            controls,
            mut sound,
            gpu,
            log,
        } = shared;
        let start = RunStart {
            eeprom: save.eeprom.bytes().to_vec(),
            global_tick: 0,
            hint: controls.hint_device(),
            locale: opts.locale.clone(),
            settings: opts.settings,
        };
        // Enhancement `texture_pack` (docs/texture-packs.md): read when the stage loads.
        data.texture_pack = opts.locale.texture_pack.clone();
        bank.set_upscale(opts.settings.hud_upscale);
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
        layer.widgets.clear();
        layer.centre_all();
        let window = gpu.as_ref().map_or(opts.size, Gpu::size);
        let wide = wide_hud(&opts);
        crate::presentation::fit_layer(
            &mut opts.locale,
            &data,
            &mut bank,
            &mut layer,
            window,
            wide,
        )?;
        let canvas = shuppatsu_render::sprites::Composer::blank(layer.canvas_size());
        // The route scene loads on its own thread while the cab is set up (both take tens of ms
        // at a stage start).
        // Loaded ahead while the departure board showed ([`load_scene_ahead`]), or here.
        let ahead = scene_ahead(&scene_key(drive.com_name(), &opts));
        let scene_job = ahead.is_none().then(|| {
            let (data, com) = (data.clone(), drive.com_name().to_owned());
            let (vis, what) = (
                visibility(&opts),
                (opts.settings, geometry(&opts), opts.stage),
            );
            std::thread::spawn(move || load_scene_for(&data, &com, vis, what))
        });
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
        // Enhancement `wide_plates`: horizontal plates (kanji + romaji) with a translation drawn
        // and the enhanced HUD layout; Japanese and the original layout keep the vertical plates.
        if opts.locale.enhanced_hud() && opts.locale.active() && bank.has_label_font() {
            let names = crate::locale::station_plate_names(&opts.locale)?;
            cab.hud.use_horizontal_plates(&bank, &names);
        }
        // Enhancement `landmark_names`: with a translation drawn, the name under the kanji.
        if opts.settings.landmark_names && bank.has_label_font() {
            let names = crate::locale::landmark_names(&opts.locale)?;
            if !names.is_empty() {
                cab.use_landmark_names(&bank, &names);
            }
        }
        // Enhancements `one_line_captions` and `board_names` (docs/localisation.md): with a
        // translation drawn.
        let s = opts.settings;
        let mut staff_parts = None;
        if (s.one_line_captions || s.board_names)
            && bank.has_label_font()
            && let Some(p) = crate::locale::caption_phrases(&opts.locale)?
        {
            if s.one_line_captions {
                cab.use_one_line_captions(&bank, &p);
                let kanji = s.staff_kanji.then(|| p.staff_kanji.clone());
                staff_parts = Some((p.tbl_parts.clone(), p.fonts[0], kanji));
            }
            if s.board_names {
                cab.use_board_names(&bank, &p.tbl_texts);
            }
        }
        cab.create(&mut layer);
        let screen_tables = ScreenTables::read(&exe)?;
        let flow = StageFlow::new(FlowDefs::new(&screen_tables, |tbl| {
            usize::try_from(tbl)
                .ok()
                .and_then(|i| bank.tilemaps().get(i))
                .map_or((0, 0), |m| {
                    (i16::from(m.width) * TILE_PX, i16::from(m.height) * TILE_PX)
                })
        }));

        let ui = crate::front::port_settings::UiText::load(
            &opts.locale.lang,
            opts.locale.dir.as_deref(),
        );
        let pad_words = crate::screens::PadHints {
            select: ui.get("hint_select").to_owned(),
            confirm: ui.get("hint_confirm").to_owned(),
            resume: ui.get("hint_resume").to_owned(),
        };
        let subtitles = start_subtitles(&opts, &bank, &data, &exe, &mut sound)?;
        if let Some(s) = sound.as_mut() {
            s.begin_stage(&exe, &drive)?;
        }
        let visibility = visibility(&opts);
        let scene = match (ahead, scene_job) {
            (Some(scene), _) => scene,
            (None, Some(job)) => job
                .join()
                .map_err(|_| anyhow::anyhow!("the scene loader panicked"))??,
            (None, None) => unreachable!("a scene job without a scene ahead"),
        };
        let script = load_script(opts.inputs.as_deref())?;
        let mut controls = controls;
        if let Some(h) = script.hint {
            controls.set_hint_device(h);
        }
        let mark_text = ui.get("mark_saved").to_owned();
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
            pad_words,
            layer,
            bank,
            canvas,
            tolerance_m,
            last_events: Vec::new(),
            ticks: 0,
            leave: false,
            quit: false,
            interpolate: false,
            frame: FrameGeometry::default(),
            gpu,
            accumulator: Duration::ZERO,
            last_frame: now,
            clock: crate::pacing::FrameClock::new(opts.settings.vsync),
            idle: crate::pacing::IdleWait::from_env(),
            refresh: None,
            title_at: now,
            title_frames: (0, Duration::ZERO),
            error: None,
            sound,
            script,
            save,
            meter_m: 0.0,
            weather: crate::weather::Weather::new(&exe, opts.stage, opts.settings.wide_weather)?,
            needle: Default::default(),
            result_view: None,
            world: WorldView::Cab,
            demo: None,
            ending: None,
            ending_tables: None,
            staff_parts,
            bonus_view: None,
            opening: None,
            intro: None,
            subtitles,
            log,
            tick_base: None,
            start,
            mark: None,
            mark_text,
            opts,
        };
        play.configure_gpu();
        Ok(play)
    }

    /// Ends the run and hands the shared resources back (sound back to the menus, the HUD's
    /// sprites cleared).
    pub fn into_shared(self) -> Shared {
        // A silent sound system (subtitles without a device) is not handed on.
        let mut sound = self.sound.filter(crate::sound::Sound::has_device);
        if let Some(s) = sound.as_mut() {
            s.set_voice_log(false);
            // The intro's music runs on into the departure board (both kinds leave it).
            if self.intro.is_some() {
                s.leave_stage();
            } else {
                s.end_stage();
            }
        }
        let mut layer = self.layer;
        layer.wide = 0;
        layer.centre_all();
        layer.slots.fill(Slot::CLEAR);
        layer.fans.clear();
        layer.widgets.clear();
        Shared {
            data: self.data,
            bank: self.bank,
            layer,
            save: self.save,
            controls: self.controls,
            sound,
            gpu: self.gpu,
            log: self.log,
        }
    }

    /// Quit or run over: the owner goes back to the title.
    pub fn left(&self) -> bool {
        self.leave
    }

    /// The run was left by quitting from the pause (`0x855ffc |= 0x10`: attract then skips its
    /// opening); after a game over or a clear it starts from its first item.
    pub fn quit(&self) -> bool {
        self.quit
    }

    pub fn gpu(&self) -> Option<&Gpu> {
        self.gpu.as_ref()
    }

    pub fn gpu_mut(&mut self) -> Option<&mut Gpu> {
        self.gpu.as_mut()
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
        self.start.global_tick = tick;
    }

    /// Under the front end: the app tick before this run's first, so its polls are logged in
    /// app ticks ([`InputLog`]).
    pub fn set_tick_base(&mut self, tick: u64) {
        self.tick_base = Some(tick);
    }

    /// Play ticks so far (every tick of the run, paused, demo and ending ones included).
    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    /// The pad sample the next poll reads, from the front end's script (headless).
    pub fn feed_pad(&mut self, sample: Option<crate::gamepad::PadSample>) {
        self.controls.feed_pad(sample);
    }

    /// The session's input log.
    pub fn log(&self) -> &InputLog {
        &self.log
    }

    /// One tick, for headless drivers.
    pub fn step(&mut self) -> Result<()> {
        self.tick()
    }

    /// Renders the current frame (cab view and HUD) to a PPM, headless.
    pub fn shot(&self, out: &std::path::Path) -> Result<()> {
        if self.drive.bonus().is_some() || self.opening.is_some() {
            return self.custom_shot(out);
        }
        let (texture_filtering, fog, square) = (
            self.opts.settings.texture_filtering,
            self.opts.settings.fog,
            self.opts.settings.square_pixels,
        );
        let (w, h) = self.opts.size;
        let ui_rect =
            shuppatsu_render::overlay::design_rect(shuppatsu_render::overlay::present_rect(
                w,
                h,
                self.layer.canvas_size(),
                self.opts.locale.ui_scaling,
            ));
        if self.world != WorldView::Cab {
            // Station result (scene.md §8): the replay views, or nothing while hidden.
            return crate::shot::render_to_ppm_with(
                self.opts.size,
                |device, queue, view, format| {
                    let mut r = shuppatsu_render::Renderer::new(device, format);
                    r.texture_filtering = texture_filtering;
                    r.shading = self.opts.settings.shading();
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
                        Some(v) => {
                            v.render(&mut r, device, queue, view, (w, h), ui_rect, square, 1.0)
                        }
                        None => crate::shot::clear_black(device, queue, view),
                    }
                },
                Some(&self.canvas),
                out,
            );
        }
        let mut effects = shuppatsu_render::effects::EffectGeometry::default();
        let band = self.band_top((w, h));
        let band_aspect = w as f32 / (h as f32 * (1.0 - band)).max(1.0);
        Self::cab_mask(&self.scene, self.ending.as_ref(), band_aspect, &mut effects);
        Self::bottom_band(
            &self.scene,
            self.cab.bottom_band(),
            self.opts.settings.wide_coupling_band,
            band_aspect,
            &mut effects,
        );
        self.weather
            .compose(&self.scene, &self.drive, band_aspect, 1.0, &mut effects);
        Self::ending_masks(self.ending.as_ref(), &mut effects);
        let fog = fog && self.weather.fog();
        let render = |hud: Option<crate::shot::Canvas<'_>>, path: &std::path::Path| {
            crate::shot::render_to_ppm(
                Some((&self.scene, self.drive.pos())),
                self.opts.size,
                |r| {
                    r.texture_filtering = texture_filtering;
                    r.shading = self.opts.settings.shading();
                    r.fog.enabled = fog;
                    r.projection.fov_tenths = (DRIVING_FOV_Y_DEG * 10.0) as u16;
                    r.projection.stretch = if square { 1.0 } else { ORIGINAL_STRETCH };
                    self.scene.geometry().configure(r);
                    r.band_top = band;
                    r.effects = effects.clone();
                },
                hud,
                path,
            )
        };
        if crate::shot::layer_dump_requested() {
            // Debug aid: the frame without the 2D layer and the layer itself, for offline
            // recomposition (`shuppatsu-tools upscale-sprites --frame`).
            render(None, &out.with_extension("world.ppm"))?;
            crate::shot::write_layer_dump(&self.layer, &out.with_extension("layer.txt"))?;
        }
        render(Some(&self.canvas), out)
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

/// Enhancement `wide_hud` (docs/re/hud.md §6.4): in the enhanced HUD layout the HUD canvas
/// spans the window and the driving HUD's groups keep to its edges. The original layout is
/// the 4:3 canvas, centred.
fn wide_hud(opts: &PlayOptions) -> bool {
    opts.locale.enhanced_hud()
}

/// Setting `subtitles`, with a translation drawn: the subtitle book, and the voice log of the
/// sound (a silent one when there is no device, so headless runs and muted play have them).
fn start_subtitles(
    opts: &PlayOptions,
    bank: &SpriteBank,
    data: &GameData,
    exe: &Exe<'_>,
    sound: &mut Option<crate::sound::Sound>,
) -> Result<Option<crate::subtitles::Subtitles>> {
    if !(opts.settings.subtitles && opts.locale.active() && bank.has_label_font()) {
        if let Some(s) = sound.as_mut() {
            s.set_voice_log(false);
        }
        return Ok(None);
    }
    if sound.is_none() {
        *sound = Some(crate::sound::Sound::silent(exe)?);
    }
    let Some(s) = sound.as_mut() else {
        return Ok(None);
    };
    s.set_voice_log(true);
    let cat = crate::locale::Catalogue::load(&opts.locale.lang, opts.locale.dir.as_deref())?;
    let book = crate::subtitles::SubtitleBook::for_game(&cat, exe, data, s.tables())?;
    Ok(Some(crate::subtitles::Subtitles::new(book)))
}

fn visibility(opts: &PlayOptions) -> Visibility {
    if !opts.settings.wide_visibility {
        return Visibility::Original;
    }
    let mut p = shuppatsu_render::Projection {
        fov_tenths: (DRIVING_FOV_Y_DEG * 10.0) as u16,
        ..Default::default()
    };
    if opts.settings.square_pixels {
        p.stretch = 1.0;
    }
    let band_aspect = opts.size.0 as f32 / (opts.size.1 as f32 * (1.0 - HUD_BAND_TOP));
    Visibility::Wide {
        tan_half_fov_x: p.tan_half_horizontal(band_aspect.max(MIN_WIDE_ASPECT)),
    }
}

/// The geometry settings of a run.
pub fn geometry(opts: &PlayOptions) -> crate::scene::Geometry {
    crate::scene::Geometry {
        max_lod: opts.settings.max_lod,
        even_track_width: opts.settings.even_track_width,
        early_activation: opts.settings.early_activation,
        draw_distance: opts.locale.draw_distance,
        distance_fade: opts.settings.distance_fade,
    }
}

fn load_scene(
    data: &GameData,
    com: &str,
    visibility: Visibility,
    opts: &PlayOptions,
) -> Result<RouteScene> {
    load_scene_for(
        data,
        com,
        visibility,
        (opts.settings, geometry(opts), opts.stage),
    )
}

/// [`load_scene`] from what it needs of the options (settings, geometry, stage): owned, so a
/// run can load its scene on another thread while it sets up the cab.
fn load_scene_for(
    data: &GameData,
    com: &str,
    visibility: Visibility,
    (settings, geometry, stage): (crate::settings::Settings, crate::scene::Geometry, usize),
) -> Result<RouteScene> {
    let t = Instant::now();
    let mut scene = RouteScene::load(data, com, visibility)?;
    scene.stable_track_variants = settings.stable_track_variants;
    scene.set_geometry(geometry);
    scene.traffic_reach = settings.far_traffic.then_some(f64::from(geometry.far()));
    crate::traffic::load_stage_trains(&mut scene, data, stage)?;
    eprintln!("{com}: loaded in {:.2?}", t.elapsed());
    Ok(scene)
}

/// What a run started from, for replaying it alone from an issue mark ([`crate::marks`]).
struct RunStart {
    /// The save as it was.
    eeprom: Vec<u8>,
    /// The global tick counter `0x84c494`.
    global_tick: u32,
    /// The device the hints followed.
    hint: Option<crate::hybrid::HintDevice>,
    locale: crate::locale::LocaleConfig,
    settings: crate::settings::Settings,
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

/// The catalogue's parts by sprite tilemap ([`crate::locale::CaptionPhrases::tbl_parts`]).
type StaffParts = std::collections::HashMap<u16, Vec<shuppatsu_render::text::TextPart>>;

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
    /// The words of the stage flow's glyph legends (`hybrid_input` with a gamepad).
    pad_words: crate::screens::PadHints,
    layer: SpriteLayer,
    bank: SpriteBank,
    canvas: shuppatsu_render::sprites::Composer,
    tolerance_m: i32,
    /// Rule events of the previous tick, to keep repeats out of the log.
    last_events: Vec<shuppatsu_engine::rules::RuleEvent>,
    /// Play ticks (simulation and paused).
    ticks: u64,
    /// Quit or run over: leave play.
    leave: bool,
    /// Left by quitting from the pause (not by the end of the run).
    quit: bool,
    /// False right after a segment change: the position jumped, don't interpolate.
    interpolate: bool,
    frame: FrameGeometry,
    gpu: Option<Gpu>,
    accumulator: Duration,
    last_frame: Instant,
    /// How far each redraw moves the simulation ([`crate::pacing`]).
    clock: crate::pacing::FrameClock,
    /// How the event loop waits between redraws (`shuppatsu play`).
    idle: crate::pacing::IdleWait,
    /// The display's refresh period ([`Play::refresh_period`]), `None` before the first look.
    refresh: Option<Option<Duration>>,
    title_at: Instant,
    /// Frames since `title_at` and the slowest of them (`--debug-title`).
    title_frames: (u32, Duration),
    error: Option<anyhow::Error>,
    sound: Option<crate::sound::Sound>,
    script: Script,
    save: Save,
    /// Metres driven with the distance meter shown (`0x8571f0`; reset by 0x40d040 at a run
    /// start, kept over continues).
    meter_m: f32,
    weather: crate::weather::Weather,
    /// The speedometer needle between ticks (presentation only).
    needle: crate::smooth::Needle,
    /// The station result's replay (scene.md §8) and what the 3D layer shows.
    result_view: Option<ResultView>,
    world: WorldView,
    /// An attract demo drive (`play/demo.rs`).
    demo: Option<demo::DemoView>,
    /// The ending (`play/ending.rs`) and its tables.
    ending: Option<ending::Ending>,
    ending_tables: Option<ending::EndingTables>,
    /// Enhancement `one_line_captions`: the catalogue's sprite parts and the caption font, from
    /// which the ending draws its staff credits as text (`play/staff_roll.rs`), and with
    /// `staff_kanji` how the names' kanji follow them.
    staff_parts: Option<(StaffParts, shuppatsu_render::text::FontRole, Option<String>)>,
    /// The bonus game's screen (`play/bonus.rs`) while it runs.
    bonus_view: Option<bonus::BonusView>,
    /// The attract opening (`play/opening.rs`).
    opening: Option<opening::Opening>,
    /// The stage intro (`play/intro.rs`).
    intro: Option<intro::Intro>,
    /// Setting `subtitles`: the voice lines in the translation (`subtitles.rs`).
    subtitles: Option<crate::subtitles::Subtitles>,
    /// What every poll read (issue marks).
    log: InputLog,
    /// [`Play::set_tick_base`]: the log counts app ticks under the front end.
    tick_base: Option<u64>,
    start: RunStart,
    /// `shuppatsu play`: an issue mark waiting for its frame.
    mark: Option<crate::marks::Pending>,
    /// The toast of a mark, `{n}` for its number.
    mark_text: String,
}

impl Play {
    /// One 33 ms tick: a frozen pause tick, or a simulation tick with the stage flow.
    fn tick(&mut self) -> Result<()> {
        // The driving HUD anchors its slots again in `update_hud`; everything else is centred.
        // A pause tick runs no HUD, so it keeps the anchors of the frame it froze.
        let frozen = self.layer.anchors;
        self.layer.centre_all();
        if self.demo.is_some() {
            return self.demo_tick();
        }
        if self.ending.is_some() {
            return self.ending_tick();
        }
        if self.opening.is_some() {
            return self.opening_tick();
        }
        if self.intro.is_some() {
            return self.intro_tick();
        }
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
            self.draw_frozen(frozen);
            return Ok(());
        }
        // The pause test sits at the top of the loop body (0x4084f5); the tick that finishes a
        // body split by a sky fade's `Task_Tick` (0x40f320) does not pass it.
        let mid_body = self.drive.rules().fade_pending().is_some();
        if !mid_body && self.flow.try_pause(&input, self.drive.phase(), &mut sfx) {
            self.saved_levers = (self.controls.power(), self.controls.brake());
            if let Some(s) = self.sound.as_mut() {
                s.set_paused(true);
            }
            self.poll();
            self.play(&sfx);
            self.draw_frozen(frozen);
            return Ok(());
        }

        let kmh_before = self.drive.speed_kmh();
        let before = crate::sound::Before::of(&self.drive);
        self.drive.set_analog_levers(self.controls.is_analog());
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
        if out.segment_started || out.notches_forced {
            // Digital modes force P0 / EB at each station start and in the red-signal / ATS
            // recovery (0x4332c0, input.md §4.1).
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
        if let (Some(subs), Some(s)) = (self.subtitles.as_mut(), self.sound.as_mut()) {
            subs.feed(s.take_voice_events());
            for t in subs.tick() {
                eprintln!(
                    "[{}] subtitle (play tick {}): {t}",
                    self.drive.tick_count(),
                    self.ticks
                );
            }
        }
        self.update_hud(hud_input);
        self.needle
            .ticked(kmh_before, self.drive.speed_kmh(), &self.layer);
        self.track_meter();
        self.bonus_present()?;
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
        // The ending (0x4040a0) builds its own world after the run's lists were killed; it
        // sets the view itself (ending.rs).
        if self.ending.is_some() {
            return;
        }
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
        let mut view = ResultView::new(
            &self.tables.result,
            &self.tables.stages.train_models,
            ResultViewInput {
                stage: r.stage().0,
                station: r.station,
                field7,
                train_type: self.drive.train().train_type,
                record: &r.stop_record,
                coupling: r.result_coupling_flags(),
            },
        );
        view.max_lod = self.opts.settings.max_lod;
        self.result_view = Some(view);
    }

    /// The end of `Stage_Run` after the report card (0x4080d0, menus.md §11.4): the clear mark,
    /// then the ranking insert with the run's value and PRO flag.
    fn start_ranking(&mut self, cleared: bool, sfx: &mut Vec<Sfx>) {
        let r = self.drive.rules();
        let (stage, _) = r.stage();
        if cleared {
            // Select stage 20 keeps its own mark (`Stage_Run` sets `0x8570a4 = 0x14` around it).
            let mark = if self.opts.variant { 20 } else { stage };
            // Written at once (decided port behaviour; the original saves only at exit).
            self.save.mark_clear(mark, r.continues);
        }
        if cleared && self.has_ending() {
            // Stage_Run: the ending (0x4040a0) between the clear mark and the ranking.
            match self.start_ending(None) {
                Ok(()) => return,
                Err(e) => eprintln!("ending: {e:#}"),
            }
        }
        self.start_ranking_now(cleared, sfx);
    }

    /// The ranking insert with the run's value and PRO flag (0x41e8d0).
    fn start_ranking_now(&mut self, cleared: bool, sfx: &mut Vec<Sfx>) {
        let r = self.drive.rules();
        let (stage, _) = r.stage();
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
            save_timed_out: self.opts.settings.save_timed_out_names,
        };
        self.flow.start_ranking(Some(&req), sfx);
    }

    /// Polls the controls and latches the next tick's driver input and menu edges.
    fn poll(&mut self) -> ButtonState {
        let tick = self.ticks;
        let mut down = self.script.down_at(tick);
        for (d, &held) in down.iter_mut().zip(&self.down) {
            *d |= held;
        }
        if self.script.has_pad() {
            self.controls.feed_pad(self.script.pad_at(tick));
        }
        // The Shift override is allowed outside driving or while paused (input.md §6).
        let shift_allowed = self.drive.phase() != DrivePhase::Driving || self.flow.paused();
        let buttons = self.controls.poll(|c| down[usize::from(c)], shift_allowed);
        // Issue marks: what this poll read, in the session's ticks.
        let label = self.tick_base.unwrap_or(0) + tick;
        if self.log.record(label, &down, self.controls.last_pad()) {
            crate::marks::request();
        }
        // Analog-lever modes: dialog cursors follow the power notch (menus.md §1.4), the
        // name-entry wheel the brake notch.
        let levers = self
            .controls
            .is_analog()
            .then(|| (self.controls.power(), self.controls.brake()));
        let hint = crate::glyphs::Hint::new(
            u8::try_from(self.opts.control_mode).unwrap_or(0),
            self.controls.hint_device(),
        );
        self.flow.pad_hints = (hint == crate::glyphs::Hint::Glyphs).then(|| self.pad_words.clone());
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
                if let Some(s) = self.subtitles.as_mut() {
                    s.clear();
                }
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
                // Packed and written at once (decided port behaviour, menus.md §14).
                self.save.finish_ranking(table, rows, commit);
                eprintln!("ranking table {table} updated (packed into the save: {commit})");
            }
            Action::Quit | Action::RunOver => {
                if let Some(s) = self.sound.as_mut() {
                    s.stop_all();
                }
                eprintln!("{action:?}: leaving play");
                self.quit = matches!(action, Action::Quit);
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

    /// A paused tick: only the render list runs (`0x855ffc & 4`); the dialog changes. The HUD
    /// keeps `anchors`, those of the last driving frame, so pausing moves nothing on a wide
    /// canvas (`wide_hud`).
    fn draw_frozen(&mut self, anchors: [Anchor; SLOT_COUNT]) {
        self.layer.anchors = anchors;
        self.flow.clear_drawn(&mut self.layer);
        self.flow.draw(&mut self.layer);
        self.centre_screen();
        self.canvas.compose(&self.layer, &self.bank, None);
    }

    fn update_hud(&mut self, input: TickInput) {
        self.layer.reset_transient();
        self.flow.clear_drawn(&mut self.layer);
        if !self.flow.hud_visible() {
            // The HUD task is killed before the report card and its slots cleared.
            self.layer.slots.fill(Slot::CLEAR);
            self.flow.draw(&mut self.layer);
            self.canvas
                .compose(&self.layer, &self.bank, self.flow.backdrop());
            return;
        }
        let input = TickInput {
            remaining_override: self.flow.remaining_override(),
            ..input
        };
        let snap = Snap::of(&self.drive);
        self.cab
            .tick(&snap, &input, &self.last_events, &mut self.layer);
        if let Some(s) = &self.subtitles {
            // The pre-departure route board (slot 0x7b, departure_hud.rs) sits where the
            // subtitles go; while it is up they move under it.
            let board = self.layer.slots[crate::departure_hud::SLOT_DIAGRAM];
            let board_bottom = u16::try_from(board.tilemap)
                .ok()
                .and_then(|t| self.bank.tilemaps().get(usize::from(t)))
                .map(|m| f32::from(board.y) + f32::from(m.height) * 16.0);
            // The coupling minigame's readout panel covers their usual place: above it.
            let panel_top = crate::coupling_hud::panel_top(&self.layer);
            s.draw_below(&mut self.layer, board_bottom, panel_top);
        }
        self.flow.draw(&mut self.layer);
        self.centre_screen();
        self.canvas.compose(&self.layer, &self.bank, None);
    }

    /// The stage flow's screens (pause, result, continue) stay a centred 4:3 over the HUD.
    fn centre_screen(&mut self) {
        for &i in self.flow.drawn() {
            if let Some(a) = self.layer.anchors.get_mut(i) {
                *a = Anchor::Centre;
            }
        }
    }

    /// The canvas multiple follows the window's height and, with `wide_hud`, the canvas its
    /// shape (presentation.rs); a new size recomposes.
    fn fit_canvas(&mut self) -> Result<()> {
        let Some(window) = self.gpu.as_ref().map(Gpu::size) else {
            return Ok(());
        };
        let wide = wide_hud(&self.opts);
        let resized = crate::presentation::fit_layer(
            &mut self.opts.locale,
            &self.data,
            &mut self.bank,
            &mut self.layer,
            window,
            wide,
        )?;
        if resized {
            self.canvas.compose(&self.layer, &self.bank, None);
        }
        Ok(())
    }

    /// The 3D view's top edge (fraction of the window) under the HUD canvas as presented in
    /// a `window`: integer scaling can leave bars above and below the canvas.
    fn band_top(&self, (w, h): (u32, u32)) -> f32 {
        let rect = shuppatsu_render::overlay::present_rect(
            w,
            h,
            self.layer.canvas_size(),
            self.opts.locale.ui_scaling,
        );
        crate::presentation::band_top(rect, h, HUD_BAND_TOP)
    }

    /// The display's refresh period, looked up every 120 redraws (it changes only when the
    /// window moves to another monitor).
    fn refresh_period(&mut self) -> Option<Duration> {
        if self.title_frames.0 % 120 == 1 || self.refresh.is_none() {
            let before = self.refresh.flatten();
            self.refresh = Some(
                self.gpu
                    .as_ref()
                    .and_then(|g| g.window.current_monitor())
                    .and_then(|m| m.refresh_rate_millihertz())
                    .filter(|&mhz| mhz > 0)
                    .map(|mhz| Duration::from_secs_f64(1000.0 / f64::from(mhz))),
            );
            let now = self.refresh.flatten();
            if now != before {
                let hz = now.map_or(0.0, |p| 1.0 / p.as_secs_f64());
                eprintln!("display refresh {hz:.2} Hz (pacing on its grid with vsync)");
            }
        }
        self.refresh.flatten()
    }

    /// What the run shows, for the frame trace ([`shuppatsu_render::perf`]).
    fn screen_name(&self) -> &'static str {
        if self.demo.is_some() {
            "demo"
        } else if self.ending.is_some() {
            "ending"
        } else if self.opening.is_some() {
            "opening"
        } else if self.intro.is_some() {
            "intro"
        } else if self.flow.paused() {
            "pause"
        } else if self.world == WorldView::ResultReplay {
            "result"
        } else if !self.flow.hud_visible() {
            "report"
        } else if self.drive.rules().phase == shuppatsu_engine::rules::Phase::Coupling {
            "coupling"
        } else {
            "drive"
        }
    }

    pub fn redraw(&mut self) -> Result<()> {
        self.fit_canvas()?;
        let now = Instant::now();
        let dt = now - self.last_frame;
        self.last_frame = now;
        self.title_frames = (self.title_frames.0 + 1, self.title_frames.1.max(dt));
        // The simulation advances by the refresh grid's time, not the jittery redraw's.
        let period = self.refresh_period();
        let step = self.clock.advance(now, period);
        self.accumulator += step.min(Duration::from_millis(250));
        while self.accumulator >= TICK && !self.leave {
            self.accumulator -= TICK;
            shuppatsu_render::perf::count(shuppatsu_render::perf::Counter::Ticks, 1);
            let perf = shuppatsu_render::perf::scope(shuppatsu_render::perf::Section::Tick);
            self.tick()?;
            drop(perf);
        }
        // Interpolate the track distance between the last two ticks (along the track, so the
        // camera follows curves), then evaluate the original's cab camera there. Frozen while
        // paused. Setting `interpolate` off: every frame shows the last tick (alpha 1).
        let (prev, cur) = (units(self.drive.prev_pos()), units(self.drive.pos()));
        let alpha = if self.opts.settings.interpolate {
            self.accumulator.as_secs_f32() / TICK.as_secs_f32()
        } else {
            1.0
        };
        let smooth = self.opts.settings.interpolate && self.interpolate && !self.flow.paused();
        let pos = if smooth {
            track_pos(prev + (cur - prev) * alpha)
        } else {
            self.drive.pos()
        };
        if smooth {
            self.scene.traffic = self.drive.traffic_poses(alpha);
        }
        // The wiper, snow scroll and needle follow the camera (1 = the last tick).
        let fx_alpha = if smooth { alpha } else { 1.0 };
        if self.world == WorldView::Cab && self.needle.frame(fx_alpha, &mut self.layer) {
            self.canvas.compose(&self.layer, &self.bank, None);
        }

        let screen = self.screen_name();
        let band = self
            .gpu
            .as_ref()
            .map_or(HUD_BAND_TOP, |g| self.band_top(g.size()));
        let Some(gpu) = self.gpu.as_mut() else {
            return Ok(());
        };
        let (width, height) = gpu.size();
        let band_aspect = width as f32 / (height as f32 * (1.0 - band)).max(1.0);
        let build = shuppatsu_render::perf::scope(shuppatsu_render::perf::Section::Build3d);
        let mut camera = self.scene.cab_camera(pos);
        let custom = Self::custom_view(
            &self.drive,
            self.opening.as_ref(),
            &self.scene.library,
            &mut self.frame,
            &mut gpu.renderer.effects,
            width as f32 / height.max(1) as f32,
        );
        gpu.renderer.projection.fov_tenths = match &custom {
            Some(v) => v.fov_tenths,
            None => Self::opening_route_fov(self.opening.as_ref())
                .unwrap_or((DRIVING_FOV_Y_DEG * 10.0) as u16),
        };
        let window = custom.as_ref().and_then(|v| {
            let win = v.window?;
            Some(opening::window_view(
                win,
                v.fov_tenths,
                (width, height),
                self.opts.settings.square_pixels,
            ))
        });
        gpu.renderer.band_top = if custom.as_ref().is_some_and(|v| v.full_screen) {
            0.0
        } else {
            band
        };
        if let Some(v) = custom {
            camera = v.camera;
            gpu.renderer.fog.enabled = false;
        } else if self.world == WorldView::Cab {
            self.scene.compose(&mut self.frame, pos, false, band_aspect);
            self.weather.shake(&mut camera);
            let fx = &mut gpu.renderer.effects;
            fx.clear();
            Self::cab_mask(&self.scene, self.ending.as_ref(), band_aspect, fx);
            Self::bottom_band(
                &self.scene,
                self.cab.bottom_band(),
                self.opts.settings.wide_coupling_band,
                band_aspect,
                fx,
            );
            self.weather
                .compose(&self.scene, &self.drive, band_aspect, fx_alpha, fx);
            Self::ending_masks(self.ending.as_ref(), fx);
            Self::opening_masks(
                self.opening.as_ref(),
                fx,
                width as f32 / height.max(1) as f32,
            );
            Self::ending_camera(self.ending.as_ref(), &mut camera);
            gpu.renderer.fog.enabled = self.opts.settings.fog && self.weather.fog();
        } else {
            // The drive's task list (scenery, traffic, weather, fog) is gone at a station result.
            match &self.result_view {
                Some(v) => v.compose(&mut self.frame, &self.scene.library, alpha),
                None => self.frame.clear(&self.scene.library),
            }
            gpu.renderer.effects = Default::default();
            gpu.renderer.fog.enabled = false;
        }
        drop(build);
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
                shuppatsu_render::overlay::design_rect(shuppatsu_render::overlay::present_rect(
                    width,
                    height,
                    self.layer.canvas_size(),
                    self.opts.locale.ui_scaling,
                )),
                self.opts.settings.square_pixels,
                alpha,
            ),
            _ => match &window {
                Some(sub) => gpu.renderer.render_view(
                    &gpu.device,
                    &gpu.queue,
                    &view,
                    (width, height),
                    &camera,
                    sub,
                ),
                None => {
                    gpu.renderer
                        .render(&gpu.device, &gpu.queue, &view, (width, height), &camera)
                }
            },
        }
        let size = self.layer.canvas_size();
        gpu.overlay.resize(&gpu.device, size);
        gpu.overlay.sync(&gpu.device, &gpu.queue, &mut self.canvas);
        gpu.overlay.scaling = self.opts.locale.ui_scaling;
        gpu.overlay.render(
            &gpu.device,
            &gpu.queue,
            &view,
            gpu.overlay.rect(width, height),
        );
        gpu.present(frame);
        shuppatsu_render::perf::frame(screen, size);

        if now - self.title_at >= Duration::from_millis(500) {
            let (frames, worst) = std::mem::take(&mut self.title_frames);
            let fps = frames as f32 / (now - self.title_at).as_secs_f32();
            self.title_at = now;
            let (station, _) = self.drive.next_station();
            // World draw calls: one per texture page with triangles (gpu.rs).
            let draws = self.frame.batches.iter().filter(|b| !b.indices.is_empty());
            crate::identity::set_debug_title(
                &gpu.window,
                &format!(
                    "shuppatsu play | {:?}{} | {:.1} km/h | next {} | {} m | {fps:.0} fps, worst {:.1} ms | {} tris, {} draws",
                    self.drive.phase(),
                    if self.flow.paused() { " (paused)" } else { "" },
                    self.drive.speed_kmh(),
                    self.drive.station_name(station).unwrap_or("?"),
                    self.drive.rules().dist_round_m,
                    worst.as_secs_f32() * 1000.0,
                    self.frame.triangle_count(),
                    draws.count(),
                ),
            );
        }
        Ok(())
    }

    fn init_gpu(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        self.gpu = Some(Gpu::create(
            event_loop,
            "shuppatsu play",
            self.opts.size,
            !self.opts.settings.vsync,
            self.opts.settings.fullscreen,
            self.opts.locale.ui_scale,
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
        self.clock.reset();
        let r = &mut gpu.renderer;
        r.texture_filtering = self.opts.settings.texture_filtering;
        r.shading = self.opts.settings.shading();
        r.fog.enabled = self.opts.settings.fog;
        r.projection.fov_tenths = (DRIVING_FOV_Y_DEG * 10.0) as u16;
        r.projection.stretch = if self.opts.settings.square_pixels {
            1.0
        } else {
            ORIGINAL_STRETCH
        };
        r.band_top = HUD_BAND_TOP;
        self.scene.geometry().configure(r);
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
                self.clock.reset();
                if let Some(g) = &self.gpu {
                    g.window.request_redraw();
                }
            }
            Err(e) => self.fail(event_loop, e.context("GPU init")),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // Alt+Enter / F11, kept from the run (`shuppatsu play`; in the game, `App` sees them).
        if let Some(t) = self.gpu.as_mut().and_then(|g| g.fullscreen_key(&event)) {
            if let crate::window::Toggle::Switched(on) = t {
                crate::window::remember_fullscreen(on);
            }
            return;
        }
        // F12: an issue mark (taken at the next frame).
        if crate::marks::key_event(&event) {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                self.idle.redraw(Instant::now());
                if crate::marks::take_request() && self.mark.is_none() {
                    self.mark = crate::marks::begin(self.gpu.as_mut());
                }
                if let Err(e) = self.redraw() {
                    self.fail(event_loop, e);
                    return;
                }
                self.finish_mark();
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

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(self.idle.control_flow(Instant::now()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bank out of [`load_bank`]'s result, alone or with the front tables beside it.
    trait BankOf {
        fn bank(self) -> SpriteBank;
    }

    impl BankOf for SpriteBank {
        fn bank(self) -> SpriteBank {
            self
        }
    }

    impl<T> BankOf for (SpriteBank, T) {
        fn bank(self) -> SpriteBank {
            self.0
        }
    }

    /// A headless run of stage 0 on a 16:9 window, in the HUD layout `hud_layout`, with the
    /// script's `keys`. Needs `SHUPPATSU_DATA_DIR`.
    fn headless_play(
        lang: &str,
        hud_layout: crate::presentation::HudLayout,
        keys: Vec<(u64, u8)>,
    ) -> Option<Play> {
        if std::env::var_os("SHUPPATSU_DATA_DIR").is_none() {
            eprintln!("skipped: SHUPPATSU_DATA_DIR not set");
            return None;
        }
        let data = GameData::from_env().unwrap();
        let exe_bytes = data.exe().unwrap();
        let exe = Exe::parse(&exe_bytes).unwrap();
        let locale = crate::locale::LocaleConfig {
            lang: lang.to_owned(),
            hud_layout,
            ..Default::default()
        };
        let pad = PadOptions {
            no_pad: true,
            ..Default::default()
        };
        let opts = PlayOptions {
            stage: 0,
            difficulty: 0,
            difficulty_from_save: false,
            station: 0,
            control_mode: 0,
            pad: pad.clone(),
            size: (2560, 1440),
            settings: crate::settings::Settings::default(),
            mute: true,
            inputs: None,
            save_file: None,
            global_tick: None,
            shot: None,
            speedometer: None,
            variant: false,
            ending: None,
            intro: false,
            locale,
        };
        let shared = Shared {
            bank: load_bank(&data, &exe, &opts.locale).unwrap().bank(),
            layer: SpriteLayer::default(),
            save: Save::open(&exe, None).unwrap(),
            controls: Controls::new(&exe, 0, &pad).unwrap(),
            sound: None,
            gpu: None,
            data,
            log: InputLog::default(),
        };
        let mut play = Play::new(shared, opts).unwrap();
        play.script.keys = keys;
        Some(play)
    }

    /// Pausing moves nothing of the driving HUD (user report: on the wide canvas the left
    /// and right groups jumped to their 4:3 places): every slot but the pause screen's own
    /// is where the last driving frame put it, anchor shift included, and the HUD's fans
    /// and widgets stay.
    #[test]
    fn pausing_keeps_the_hud_in_place() {
        use crate::presentation::HudLayout;
        for (lang, layout) in [
            ("en", HudLayout::Enhanced),
            ("ja", HudLayout::Enhanced),
            ("en", HudLayout::Original),
        ] {
            let Some(mut play) = headless_play(lang, layout, vec![(300, 0x01)]) else {
                return;
            };
            while play.ticks < 300 {
                play.tick().unwrap();
            }
            assert!(!play.flow.paused(), "{lang} {layout:?}");
            if layout == HudLayout::Enhanced {
                assert!(play.layer.wide > 0, "a wide canvas");
            }
            let placed = |l: &SpriteLayer| -> Vec<(Slot, i32)> {
                (0..SLOT_COUNT)
                    .map(|i| (l.slots[i], l.anchor_shift(l.anchors[i])))
                    .collect()
            };
            let driving = placed(&play.layer);
            let (fans, widgets) = (play.layer.fans.clone(), play.layer.widgets.clone());
            for _ in 0..20 {
                play.tick().unwrap();
                assert!(play.flow.paused(), "{lang} {layout:?}");
                let paused = placed(&play.layer);
                let drawn = play.flow.drawn();
                for (i, (a, b)) in driving.iter().zip(&paused).enumerate() {
                    if !drawn.contains(&i) {
                        assert_eq!(a, b, "slot {i:#x} moved ({lang} {layout:?})");
                    }
                }
                assert_eq!(play.layer.fans, fans, "{lang} {layout:?}");
                assert!(
                    play.layer.widgets.starts_with(&widgets),
                    "{lang} {layout:?}"
                );
            }
        }
    }
}

//! `shuppatsu` with no subcommand: the whole game, sequenced like `Game_MainFlow` 0x419510
//! (main-loop.md §5, menus.md §2–§7):
//!
//! attract → title menu (options, records, calibration, exit) → route select → lever check
//! (analog modes) → stage intro → departure board → [`Play`] → back to attract / the title.
//!
//! [`App`] owns the window and GPU, the sprite bank and the persistent sprite layer, the save,
//! the input and the sound device, and lends them to a run ([`play::Shared`]). The front-end
//! screens tick at the original's 33 ms, read the button edges polled at the end of the
//! previous tick, and draw the 640×480 canvas centred over black (`0x855ffc & 0x1000`; the
//! port always centres it). On a wide window the canvas takes its shape and the menu backdrops
//! continue to the edges (`wide_menus`, menus.md §14).

use crate::front::attract::{
    Attract, AttractCounters, AttractEnv, AttractStep, AttractTables, DemoKind as AttractDemo,
};
use crate::front::calibrate::CalibrationScreen;
use crate::front::dialog::ExitConfirm;
use crate::front::language::{Language, LanguageScreen, Look};
use crate::front::options::{self, ControlPage, ListResult, OptionsList, SetEffect, ValuePage};
use crate::front::port_settings::{self, PortSettings, UiText};
use crate::front::prerun::{DepartureBoard, LeverCheck};
use crate::front::records::{self, Records, RecordsStep};
use crate::front::route_select::{Chosen, RouteResult, RouteSelect};
use crate::front::tables::FrontTables;
use crate::front::title::{TitleChoice, TitleMenu, komachi_unlocked};
use crate::front::{Env, snd};
use crate::gamepad::{Controls, JoyState, PadOptions, PadProfile, PadSource};
use crate::movie::MoviePlayer;
use crate::play::{self, Play, PlayOptions, Shared};
use crate::scene::GameData;
use crate::screens::tables::ScreenTables;
use crate::screens::{MenuInput, Sfx};
use crate::script::{InputLog, Script};
use crate::window::{Gpu, Toggle};
use anyhow::{Context, Result};
use shuppatsu_engine::demo::DemoKind;
use shuppatsu_formats::exe::Exe;
use shuppatsu_render::sprites::SpriteLayer;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::WindowId;

mod mark;

/// The original's fixed tick (timeSetEvent(33, …) at 0x436f30).
const TICK: Duration = Duration::from_millis(33);
/// Title choice 0: BGM stopped, sprites cleared, three ticks (0x42a1c0), then the one-tick
/// "press START" of the PC build (0x4122d0) before `Stage_Run` creates the route select.
const TO_ROUTE_SELECT_TICKS: i32 = 4;
/// Title cancel (and the original's exit declined): three ticks (0x42a1c0), then attract from
/// the title picture.
const TO_ATTRACT_TICKS: i32 = 3;
/// The title menu's ゲーム終了 item (menus.md §3, index 3).
const EXIT_ITEM: i32 = 3;
/// Title choice 2: BGM stopped, sprites cleared, ten ticks before the records viewer.
const TO_RECORDS_TICKS: i32 = 10;
/// Route select cancelled: `Stage_Run`'s exit path (one tick, then two) and two more ticks of
/// `Game_MainFlow` before the title menu comes back without attract (`0x855ffc & 0x40`).
const BACK_TO_TITLE_TICKS: i32 = 5;
/// Between the route select (or the lever check) and the stage intro: BGM release, route data,
/// `0x425490(15)` and one `Task_Tick` (0x4080d0; the oracle trace shows 16 ticks with
/// `0x855ff8 == 2` before the intro's first).
const TO_INTRO_TICKS: i32 = 16;
/// After the stage intro (0x408ec0): `Stage_Run`'s two ticks and those after 0x407900 and
/// 0x40f7e0, then the departure board.
const TO_BOARD_TICKS: i32 = 4;
/// After the board: one tick, then the route load and the run.
const TO_PLAY_TICKS: i32 = 1;
/// After a run: `Stage_Run`'s tail and `Game_MainFlow`'s two ticks.
const AFTER_RUN_TICKS: i32 = 5;
/// Pixels per `tbl.rom` tile.
const TILE_PX: i16 = 16;
/// The cursor-frame tilemap whose width centres the title and route lists (`0x855ae8`).
const TBL_CURSOR: usize = 0x504;

#[derive(Clone, Debug)]
pub struct AppOptions {
    pub size: (u32, u32),
    /// The port's switches (docs/settings.md), passed on to every run.
    pub settings: crate::settings::Settings,
    pub mute: bool,
    pub pad: PadOptions,
    /// Scripted inputs ([`crate::script`]; app ticks, the runs' included).
    pub inputs: Option<PathBuf>,
    /// Check aid (issue marks): the save to start from, read only (`--save`).
    pub save_file: Option<PathBuf>,
    /// Headless: run this many app ticks, render one frame to a PPM and exit.
    pub shot: Option<(u64, PathBuf)>,
    pub locale: crate::locale::LocaleConfig,
    /// The settings a flag or the environment fixes for the run (the port settings screen
    /// shows them locked).
    pub locks: crate::config::Locks,
    /// Show the first-run language screen before attract (`crate::config::asks_language`).
    pub first_run: bool,
}

/// What comes after a tick gap.
#[derive(Debug, Clone)]
enum Next {
    Attract { skip_opening: bool },
    Title { cursor: i32 },
    RouteSelect,
    Records,
    Intro(Chosen),
    Board(Chosen),
    Play(Chosen),
}

enum Screen {
    /// The first-run language screen (port only).
    Language(LanguageScreen),
    Attract(Attract),
    Title(TitleMenu),
    Options(OptionsList),
    Control(ControlPage),
    Value(ValuePage),
    /// The port settings (port only), from the options list's seventh row.
    PortSettings(PortSettings),
    Records(Records),
    Calibrate(CalibrationScreen),
    Exit(ExitConfirm),
    RouteSelect(RouteSelect),
    LeverCheck(LeverCheck, Chosen),
    Board(DepartureBoard, Chosen),
    Wait(i32, Next),
    Play(Box<Play>),
    /// The Komachi movie (0x429110), from attract or the title menu.
    Movie(Box<MovieScreen>),
    /// Exit confirmed: the window closes (`DestroyWindow` → `WM_QUIT`).
    Quit,
}

/// Where the movie was started from, and so where it returns to.
enum MovieFrom {
    /// Attract item 3 (0x404e10), waiting for [`Attract::movie_done`].
    Attract(Attract),
    /// The title menu's sixth item (setting `movie_in_menu`; 0x42a1c0 case 5).
    Title,
}

struct MovieScreen {
    player: MoviePlayer,
    from: MovieFrom,
}

/// Exe tables of the front end, read once.
struct Defs {
    front: FrontTables,
    screens: ScreenTables,
    attract: AttractTables,
    cursor_x: i16,
    exit_frame: (i16, i16),
    language: Look,
    port: port_settings::Look,
}

pub struct App {
    opts: AppOptions,
    defs: Defs,
    /// The resources a run borrows; `None` while the run has them.
    res: Option<Shared>,
    screen: Screen,
    joystick: bool,
    /// The port's own strings in the current language (`[ui]`).
    ui: UiText,
    /// The replay-playback flag `0x816974` (R in the train list).
    replay: bool,
    /// Attract's walk and photo counters (`0x8573c8`, `0x8573cc`), kept across walks.
    attract_counters: AttractCounters,
    /// Attract waiting for its demo drive (the run in [`Screen::Play`]) to end.
    demo_attract: Option<Attract>,
    /// The movie's picture on the GPU (made on its first frame).
    movie_overlay: Option<shuppatsu_render::overlay::Overlay>,
    /// The movie read ahead of its start ([`crate::movie::MovieAhead`]).
    movie_ahead: Option<crate::movie::MovieAhead>,
    /// The stage intro is the run in [`Screen::Play`]; the departure board of this choice
    /// follows it.
    intro_chosen: Option<Chosen>,
    down: [bool; 256],
    menu_input: MenuInput,
    script: Script,
    canvas: shuppatsu_render::sprites::Composer,
    ticks: u64,
    accumulator: Duration,
    last_frame: Instant,
    /// How the event loop waits between redraws.
    idle: crate::pacing::IdleWait,
    error: Option<anyhow::Error>,
    /// An issue mark waiting for its frame ([`crate::marks`]).
    mark: Option<crate::marks::Pending>,
    /// What the session started from, for replaying it from an issue mark.
    boot: Boot,
}

/// The save and settings a session started with.
struct Boot {
    eeprom: Vec<u8>,
    locale: crate::locale::LocaleConfig,
    settings: crate::settings::Settings,
}

fn map_size(bank: &shuppatsu_render::sprites::SpriteBank, tbl: usize) -> (i16, i16) {
    bank.tilemaps().get(tbl).map_or((0, 0), |m| {
        (i16::from(m.width) * TILE_PX, i16::from(m.height) * TILE_PX)
    })
}

/// The exit dialog's いいえ (or cancel / pause). With setting `exit_no_to_menu` (decided port
/// behaviour, menus.md §14.1) the title menu again after the same three ticks, its cursor on
/// ゲーム終了 (the menu BGM restarts with it); off, the original's: attract at the title picture
/// (0x42a1c0 case 0).
fn exit_declined(to_menu: bool) -> (i32, Next) {
    let next = if to_menu {
        Next::Title { cursor: EXIT_ITEM }
    } else {
        Next::Attract { skip_opening: true }
    };
    (TO_ATTRACT_TICKS, next)
}

fn joystick_dat(opts: &PadOptions) -> Option<PathBuf> {
    opts.joystick_dat.clone().or_else(|| {
        crate::save::default_path().and_then(|p| Some(p.parent()?.join("joystick.dat")))
    })
}

impl App {
    pub fn new(mut opts: AppOptions) -> Result<Self> {
        let data = GameData::from_env()?;
        let exe_bytes = data.exe()?;
        let exe = Exe::parse(&exe_bytes)?;
        let headless = opts.shot.is_some();
        let save_path = if headless {
            None
        } else {
            crate::save::default_path()
        };
        let mut save = play::open_save(&exe, save_path, opts.save_file.as_deref())?;
        save.save_on_change = opts.settings.save_on_change;
        if let Some(p) = save.path() {
            eprintln!("save: {}", p.display());
        }
        let boot = Boot {
            eeprom: save.eeprom.bytes().to_vec(),
            locale: opts.locale.clone(),
            settings: opts.settings,
        };
        // A script with `pad` lines is the gamepad (a recorded session, docs/marks.md).
        let script = play::load_script(opts.inputs.as_deref())?;
        if script.has_pad() {
            opts.pad.scripted_pad = Some(script.pad_at(0).is_some());
        }
        // `0x8298a4`: a joystick is attached (never in headless runs, to stay deterministic,
        // unless the script has one).
        let joystick = match opts.pad.scripted_pad {
            Some(on) => on,
            None => {
                !headless && PadSource::open(PadProfile::Generic).is_some_and(|p| p.connected())
            }
        };
        // 0x434ed0: a joystick mode without a joystick falls back to mode 0.
        if save.eeprom.options().control_mode > 3 && !joystick {
            save.eeprom.set_option(options::item::CONTROL, 0);
        }
        let mut pad = opts.pad.clone();
        pad.joystick_dat = joystick_dat(&opts.pad);
        let controls = Controls::new(&exe, usize::from(save.eeprom.options().control_mode), &pad)?;
        let (mut bank, mut front) = play::load_bank(&data, &exe, &opts.locale, !headless)?;
        bank.set_upscale(opts.settings.hud_upscale);
        if opts.first_run {
            // The language screen's labels, before any language is installed.
            crate::locale::install_label_font(&mut bank)?;
        }
        let mut sound = if headless || opts.mute {
            None
        } else {
            crate::sound::Sound::open_device(data.data_dir(), &exe, opts.locale.audio_resampler)?
        };
        if let Some(s) = sound.as_mut() {
            s.set_enabled(save.eeprom.options().sound == 0);
        }
        let mut layer = SpriteLayer {
            scale: opts.locale.ui_scale,
            ..SpriteLayer::default()
        };
        let canvas = shuppatsu_render::sprites::Composer::blank(layer.canvas_size());
        crate::hud::Hud::install_palettes(&exe, &mut layer)?;
        front.map_rows = front
            .legend_maps()
            .into_iter()
            .filter_map(|m| {
                let rows = map_size(&bank, usize::try_from(m).ok()?).1 / TILE_PX;
                Some((m, i8::try_from(rows).ok()?))
            })
            .collect();
        crate::front::mark_backdrops(&mut bank, &front);
        let screens = ScreenTables::read(&exe)?;
        let cursor_x = crate::front::cursor_x(&front, map_size(&bank, TBL_CURSOR).0);
        let menu_palette = layer.palettes[usize::from(crate::front::PAL_MENU)].as_ref();
        let defs = Defs {
            cursor_x,
            language: Look::new(cursor_x, map_size(&bank, TBL_CURSOR), menu_palette),
            port: port_settings::Look::new(menu_palette),
            exit_frame: map_size(&bank, usize::try_from(front.exit_dialog.frame)?),
            front,
            screens,
            attract: AttractTables::read(&exe)?,
        };
        let mut log = InputLog::default();
        log.start(joystick);
        let now = Instant::now();
        let movie_ahead = Some(crate::movie::MovieAhead::start(data.data_dir()));
        let mut app = Self {
            defs,
            res: Some(Shared {
                data,
                bank,
                layer,
                save,
                controls,
                sound,
                gpu: None,
                log,
            }),
            // Boot: `Game_MainFlow` starts with attract (`0x855ffc & 0x40` clear).
            screen: if opts.first_run {
                Screen::Language(LanguageScreen::new())
            } else {
                Screen::Attract(Attract::new(false, AttractCounters::default()))
            },
            joystick,
            ui: UiText::load(&opts.locale.lang, opts.locale.dir.as_deref()),
            replay: false,
            attract_counters: AttractCounters::default(),
            demo_attract: None,
            movie_overlay: None,
            movie_ahead,
            intro_chosen: None,
            down: [false; 256],
            menu_input: MenuInput::NONE,
            script,
            canvas,
            ticks: 0,
            accumulator: Duration::ZERO,
            last_frame: now,
            idle: crate::pacing::IdleWait::from_env(),
            error: None,
            mark: None,
            boot,
            opts,
        };
        app.fit_front()?;
        Ok(app)
    }

    fn res(&mut self) -> &mut Shared {
        self.res.as_mut().expect("resources are lent to the run")
    }

    fn env(&self) -> Env {
        let Some(r) = self.res.as_ref() else {
            return Env::default();
        };
        let control_mode = r.save.eeprom.options().control_mode;
        Env {
            control_mode,
            analog: r.controls.is_analog(),
            // `hybrid_input` follows hot-plugging: a pad connected later offers the joystick
            // modes too.
            joystick: self.joystick || r.controls.pad_connected(),
            hint: crate::glyphs::Hint::new(control_mode, r.controls.hint_device()),
            // No force feedback is implemented: the vibration item stays hidden, as on a PC
            // without an FF device (`0x816838 == 0`).
            force_feedback: false,
            sound_device: r.sound.is_some() || self.opts.mute,
            movie_in_menu: self.opts.settings.movie_in_menu,
        }
    }

    fn play_sfx(&mut self, sfx: &[Sfx]) {
        let ungated = matches!(self.screen, Screen::Exit(_));
        let Some(s) = self.res.as_mut().and_then(|r| r.sound.as_mut()) else {
            return;
        };
        for &c in sfx {
            match c {
                Sfx::Play(code) if ungated => s.request_ungated(code),
                Sfx::Play(code) => s.request(code),
                Sfx::Stop(code) => s.release(code),
            }
        }
    }

    /// The input poll at the end of `Task_Tick`: the next tick's edges.
    fn poll(&mut self) {
        let tick = self.ticks;
        let mut down = self.script.down_at(tick);
        for (d, &held) in down.iter_mut().zip(&self.down) {
            *d |= held;
        }
        let Some(r) = self.res.as_mut() else {
            return;
        };
        if self.script.has_pad() {
            r.controls.feed_pad(self.script.pad_at(tick));
        }
        let b = r.controls.poll(|c| down[usize::from(c)], true);
        // Issue marks: what this poll read.
        if r.log.record(tick, &down, r.controls.last_pad()) {
            crate::marks::request();
        }
        let levers = r
            .controls
            .is_analog()
            .then(|| (r.controls.power(), r.controls.brake()));
        self.menu_input = MenuInput::from_word(&b.word, levers);
    }

    /// The raw joystick state for the calibration screen (stale zeros without a pad).
    fn joy_state(&mut self) -> JoyState {
        let Some((joy, pad)) = self.res().controls.pad_mut() else {
            return JoyState::default();
        };
        let row = *joy.row();
        pad.sample(&row).unwrap_or_default()
    }

    fn start(&mut self, next: Next, sfx: &mut Vec<Sfx>) -> Result<()> {
        self.screen = match next {
            Next::Attract { skip_opening } => {
                Screen::Attract(Attract::new(skip_opening, self.attract_counters))
            }
            Next::Title { cursor } => {
                // TitleMenu_Run 0x42a1c0 starts the menu BGM.
                sfx.push(Sfx::Play(snd::BGM_MENU));
                Screen::Title(TitleMenu::new(cursor, self.defs.cursor_x))
            }
            Next::RouteSelect => Screen::RouteSelect(RouteSelect::new(
                self.defs.cursor_x,
                self.opts.settings.unique_routes,
                sfx,
            )),
            Next::Records => {
                let layer = &mut self.res.as_mut().expect("resources").layer;
                crate::front::clear_all(layer);
                match Records::new(&self.defs.screens.ranking, true, layer) {
                    Some(r) => Screen::Records(r),
                    None => {
                        sfx.push(Sfx::Play(snd::BGM_MENU));
                        Screen::Title(TitleMenu::new(2, self.defs.cursor_x))
                    }
                }
            }
            Next::Board(chosen) => {
                // The run after the board: its route scene loads while the board shows.
                let r = self.res.as_ref().expect("resources");
                let up = play::page_uploader(r.gpu.as_ref());
                play::load_scene_ahead(&r.data, &self.play_options(r, chosen), up);
                let route = chosen.route;
                let layer = &mut self.res.as_mut().expect("resources").layer;
                let b = DepartureBoard::new(&self.defs.front, route, chosen.variant, layer);
                Screen::Board(b, chosen)
            }
            Next::Intro(chosen) if play::has_stage_intro(chosen.route) => {
                let mut play = self.start_play(chosen)?;
                play.start_stage_intro()?;
                self.intro_chosen = Some(chosen);
                Screen::Play(Box::new(play))
            }
            // 0x408ec0 has a flag for each of the 20 stages; nothing else reaches here.
            Next::Intro(chosen) => Screen::Wait(TO_BOARD_TICKS, Next::Board(chosen)),
            Next::Play(chosen) => Screen::Play(Box::new(self.start_play(chosen)?)),
        };
        Ok(())
    }

    /// An attract demo drive: attract waits in [`App::demo_attract`] while the run plays.
    fn start_demo(&mut self, kind: Option<AttractDemo>, stage: usize) -> Result<()> {
        let Screen::Attract(a) = std::mem::replace(&mut self.screen, Screen::Quit) else {
            return Ok(());
        };
        let chosen = Chosen {
            group: 0,
            stage,
            route: stage,
            variant: false,
        };
        let mut play = self.start_play(chosen)?;
        match kind {
            Some(AttractDemo::Route) => play.start_demo(DemoKind::Route)?,
            Some(AttractDemo::Lesson) => play.start_demo(DemoKind::Lesson)?,
            None => play.start_opening()?,
        }
        self.demo_attract = Some(a);
        self.screen = Screen::Play(Box::new(play));
        Ok(())
    }

    /// Starts the Komachi movie: the sprites cleared (black around the picture; the original
    /// blits over whatever the back buffer held, black in attract), the soundtrack on the
    /// device. Without the file the movie counts as played to its end.
    fn start_movie(&mut self, from: MovieFrom) -> Screen {
        let r = self.res.as_mut().expect("resources");
        crate::front::clear_all(&mut r.layer);
        let opened = match self.movie_ahead.take() {
            Some(ahead) => MoviePlayer::open_ahead(ahead),
            None => MoviePlayer::open(r.data.data_dir()),
        };
        match opened {
            Ok(mut player) => {
                let mut sound = false;
                if let (Some(snd), Some(rate)) = (r.sound.as_mut(), player.audio_rate())
                    && snd.has_device()
                {
                    let stream = shuppatsu_audio::PcmStream::new(rate);
                    player.attach(std::sync::Arc::clone(&stream));
                    snd.play_stream(Some(stream));
                    sound = true;
                }
                player.update();
                eprintln!(
                    "movie: Komachi.DAT, {}",
                    if sound {
                        "with sound"
                    } else {
                        "silent (no sound device)"
                    }
                );
                Screen::Movie(Box::new(MovieScreen { player, from }))
            }
            Err(e) => {
                eprintln!("movie: {e:#}");
                let mut sfx = Vec::new();
                let s = self.end_movie(from, false, String::new(), &mut sfx);
                self.play_sfx(&sfx);
                s
            }
        }
    }

    /// The movie ended (`skipped` by the confirm edge): the soundtrack stops, attract goes on
    /// ([`Attract::movie_done`]) or the title menu comes back on the movie item with its BGM
    /// (0x42a1c0 case 5).
    fn end_movie(
        &mut self,
        from: MovieFrom,
        skipped: bool,
        timing: String,
        sfx: &mut Vec<Sfx>,
    ) -> Screen {
        let env = self.env();
        let r = self.res.as_mut().expect("resources");
        if let Some(s) = r.sound.as_ref() {
            s.play_stream(None);
        }
        self.movie_overlay = None;
        // Read again for the next time it plays.
        self.movie_ahead = Some(crate::movie::MovieAhead::start(r.data.data_dir()));
        eprintln!(
            "movie: {} ({})",
            if skipped { "skipped" } else { "ended" },
            timing
        );
        match from {
            MovieFrom::Attract(mut a) => {
                a.movie_done(skipped, &mut r.layer);
                Screen::Attract(a)
            }
            MovieFrom::Title => {
                sfx.push(Sfx::Play(snd::BGM_MENU));
                Screen::Title(TitleMenu::new(
                    TitleMenu::movie_cursor(&env),
                    self.defs.cursor_x,
                ))
            }
        }
    }

    /// Where the movie goes in a `window`-sized target: the original's rectangle on the
    /// presented 640×480 design (`hud_layout original`) or the window filled (`enhanced`), at
    /// always at the stream's display aspect (2.03:1; the original's 2:1 doubling stretched it to
    /// 2.22:1, a deviation the user decided on, docs/re/movie.md).
    fn movie_rect(&self, player: &MoviePlayer, window: (u32, u32)) -> [f32; 4] {
        let canvas = self.canvas_size();
        let design =
            shuppatsu_render::overlay::design_rect(shuppatsu_render::overlay::present_rect(
                window.0,
                window.1,
                canvas,
                self.opts.locale.ui_scaling,
            ));
        let dar = player.display_aspect();
        let enhanced = self.opts.locale.hud_layout == crate::presentation::HudLayout::Enhanced;
        crate::movie::movie_rect(window, design, enhanced, dar)
    }

    /// The options of a run of `chosen` started from the front end.
    fn play_options(&self, shared: &Shared, chosen: Chosen) -> PlayOptions {
        let o = shared.save.eeprom.options();
        PlayOptions {
            stage: chosen.route,
            difficulty: usize::from(shared.save.eeprom.difficulty()),
            difficulty_from_save: false,
            station: 0,
            control_mode: usize::from(o.control_mode),
            pad: self.opts.pad.clone(),
            size: self.opts.size,
            settings: self.opts.settings,
            mute: self.opts.mute,
            inputs: None,
            save_file: None,
            global_tick: None,
            shot: None,
            speedometer: None,
            variant: chosen.variant,
            ending: None,
            intro: false,
            locale: self.opts.locale.clone(),
        }
    }

    fn start_play(&mut self, chosen: Chosen) -> Result<Play> {
        let mut shared = self.res.take().expect("resources");
        let opts = self.play_options(&shared, chosen);
        eprintln!(
            "select stage {} → route {}{}, difficulty {}",
            chosen.stage,
            chosen.route,
            if chosen.variant { " (variant)" } else { "" },
            opts.difficulty
        );
        crate::front::clear_all(&mut shared.layer);
        let mut play = Play::new(shared, opts)?;
        play.set_down(self.down);
        // 0x84c494 counts every tick since boot, menus included.
        play.set_global_tick(self.ticks as u32);
        // The run's polls are logged in app ticks.
        play.set_tick_base(self.ticks);
        Ok(play)
    }

    /// One 33 ms tick of the front end (a run ticks itself, see [`App::step_play`]).
    fn tick(&mut self) -> Result<()> {
        self.ticks += 1;
        let input = self.menu_input;
        let mut sfx = Vec::new();
        let env = self.env();
        let mut next: Option<Next> = None;
        let mut demo: Option<(Option<AttractDemo>, usize)> = None;
        let mut movie: Option<MovieFrom> = None;
        let mut movie_ended: Option<(MovieFrom, bool, String)> = None;
        let mut set_option: Option<(usize, u8)> = None;
        let mut calibrated = None;
        let mut language = None;
        let mut port_change: Option<usize> = None;
        let joy = if matches!(self.screen, Screen::Calibrate(_)) {
            self.joy_state()
        } else {
            JoyState::default()
        };
        let screen = std::mem::replace(&mut self.screen, Screen::Quit);
        let Defs {
            front: t, screens, ..
        } = &self.defs;
        let r = self.res.as_mut().expect("resources");
        r.layer.fans.clear();
        r.layer.widgets.clear();
        self.screen = match screen {
            Screen::Language(mut s) => match s.tick(&input, env.analog, &mut sfx) {
                None => {
                    s.draw(&self.defs.language, &mut r.layer);
                    Screen::Language(s)
                }
                Some(lang) => {
                    language = Some(lang);
                    crate::front::clear_all(&mut r.layer);
                    Screen::Wait(
                        TO_ATTRACT_TICKS,
                        Next::Attract {
                            skip_opening: false,
                        },
                    )
                }
            },
            Screen::Attract(mut a) => {
                let marks = |route| r.save.eeprom.clear_mark(route);
                let aenv = AttractEnv {
                    komachi: komachi_unlocked(t, marks),
                    difficulty: r.save.eeprom.difficulty(),
                };
                let step = a.tick(&input, &self.defs.attract, aenv, &mut r.layer, &mut sfx);
                self.attract_counters = a.counters;
                match step {
                    AttractStep::Running => Screen::Attract(a),
                    AttractStep::Demo { kind, stage } => {
                        demo = Some((Some(kind), stage));
                        Screen::Attract(a)
                    }
                    AttractStep::Opening => {
                        // The opening runs on stage 1's data (AKITA1C in its route part).
                        demo = Some((None, 1));
                        Screen::Attract(a)
                    }
                    AttractStep::KomachiMovie => {
                        movie = Some(MovieFrom::Attract(a));
                        Screen::Quit
                    }
                    AttractStep::Title => {
                        crate::front::clear_all(&mut r.layer);
                        next = Some(Next::Title { cursor: 0 });
                        Screen::Quit
                    }
                }
            }
            Screen::Title(mut m) => match m.tick(&input, &env, t, &mut r.layer, &mut sfx) {
                None => Screen::Title(m),
                Some(choice) => {
                    options::destroy(&mut r.layer);
                    match choice {
                        TitleChoice::Arcade => {
                            sfx.push(Sfx::Stop(snd::BGM_MENU));
                            crate::front::clear_all(&mut r.layer);
                            Screen::Wait(TO_ROUTE_SELECT_TICKS, Next::RouteSelect)
                        }
                        TitleChoice::Options => {
                            // The port's own rows are text; `ja` has no label font otherwise.
                            if !self.opts.locale.active() {
                                crate::locale::install_label_font(&mut r.bank)?;
                            }
                            Screen::Options(OptionsList::new(0))
                        }
                        TitleChoice::Ranking => {
                            sfx.push(Sfx::Stop(snd::BGM_MENU));
                            crate::front::clear_all(&mut r.layer);
                            Screen::Wait(TO_RECORDS_TICKS, Next::Records)
                        }
                        TitleChoice::Exit => {
                            sfx.push(Sfx::Stop(snd::BGM_MENU));
                            if let Some(s) = r.sound.as_mut() {
                                s.set_paused(true);
                            }
                            Screen::Exit(ExitConfirm::new(
                                t.exit_dialog.clone(),
                                self.defs.exit_frame,
                                &mut r.layer,
                            ))
                        }
                        TitleChoice::Calibrate => {
                            Screen::Calibrate(CalibrationScreen::new(t, &mut r.layer))
                        }
                        TitleChoice::Movie => {
                            // 0x42a1c0 case 5: the menu BGM stops for the movie.
                            sfx.push(Sfx::Stop(snd::BGM_MENU));
                            movie = Some(MovieFrom::Title);
                            Screen::Quit
                        }
                        TitleChoice::Back => {
                            sfx.push(Sfx::Stop(snd::BGM_MENU));
                            crate::front::clear_all(&mut r.layer);
                            Screen::Wait(TO_ATTRACT_TICKS, Next::Attract { skip_opening: true })
                        }
                    }
                }
            },
            Screen::Options(mut m) => {
                match m.tick(&input, &env, &r.save.eeprom, t, &mut r.layer, &mut sfx) {
                    None => {
                        if !OptionsList::draws_port_row(t) {
                            let selected = m.cursor() == options::item::PORT;
                            let (look, ui) = (&self.defs.port, &self.ui);
                            port_settings::draw_options_item(&mut r.layer, t, look, ui, selected);
                        }
                        Screen::Options(m)
                    }
                    Some(res) => {
                        options::destroy(&mut r.layer);
                        match res {
                            ListResult::Back => {
                                if !self.opts.locale.active() {
                                    r.bank.set_label_font(None);
                                }
                                Screen::Title(TitleMenu::new(1, self.defs.cursor_x))
                            }
                            ListResult::Open(options::item::PORT) => {
                                Screen::PortSettings(PortSettings::new())
                            }
                            ListResult::Open(options::item::CONTROL) => {
                                let mode = r.save.eeprom.options().control_mode;
                                Screen::Control(ControlPage::new(mode, r.controls.power()))
                            }
                            ListResult::Open(item) => {
                                let v = options::get(&r.save.eeprom, &env, item);
                                Screen::Value(ValuePage::new(item, v))
                            }
                        }
                    }
                }
            }
            Screen::Control(mut p) => match p.tick(&input, &env, t, &mut r.layer, &mut sfx) {
                None => {
                    if p.shows_pad_panel(&env) {
                        options::draw_pad_panel(&mut r.layer, t, &self.defs.port, &self.ui);
                    }
                    Screen::Control(p)
                }
                Some(v) => {
                    options::destroy(&mut r.layer);
                    set_option = v.map(|v| (options::item::CONTROL, v));
                    Screen::Options(OptionsList::new(options::item::CONTROL))
                }
            },
            Screen::Value(mut p) => match p.tick(&input, &env, t, &mut r.layer, &mut sfx) {
                None => Screen::Value(p),
                Some(v) => {
                    options::destroy(&mut r.layer);
                    let item = p.item();
                    set_option = v.map(|v| (item, v));
                    Screen::Options(OptionsList::new(item))
                }
            },
            Screen::PortSettings(mut p) => {
                let (locks, cfg, settings) = (
                    &self.opts.locks,
                    &mut self.opts.locale,
                    &mut self.opts.settings,
                );
                match p.tick(&input, locks, cfg, settings, &mut sfx) {
                    Some(port_settings::Step::Back) => {
                        options::destroy(&mut r.layer);
                        Screen::Options(OptionsList::new(options::item::PORT))
                    }
                    step => {
                        if let Some(port_settings::Step::Changed(i)) = step {
                            port_change = Some(i);
                        }
                        Screen::PortSettings(p)
                    }
                }
            }
            Screen::Records(mut rec) => {
                let legend = match env.hint {
                    crate::glyphs::Hint::Keys => records::Legend::Keys,
                    crate::glyphs::Hint::Pad | crate::glyphs::Hint::Levers => records::Legend::Pad,
                    crate::glyphs::Hint::Glyphs => records::Legend::Glyphs {
                        next: self.ui.get("hint_next"),
                        back: self.ui.get("hint_back_to_menu"),
                        paint: self.defs.port.bright,
                    },
                };
                let step = rec.tick(
                    &input,
                    &legend,
                    &screens.ranking,
                    &r.save.rankings,
                    &mut r.layer,
                    &mut sfx,
                );
                match step {
                    RecordsStep::Running => {
                        Records::draw_bars(&mut r.layer);
                        Screen::Records(rec)
                    }
                    RecordsStep::Done => {
                        Records::destroy(&mut r.layer);
                        sfx.push(Sfx::Play(snd::BGM_MENU));
                        Screen::Title(TitleMenu::new(2, self.defs.cursor_x))
                    }
                }
            }
            Screen::Calibrate(mut c) => match c.tick(&input, joy, t, &mut r.layer, &mut sfx) {
                None => Screen::Calibrate(c),
                Some(cal) => {
                    calibrated = cal;
                    Screen::Title(TitleMenu::new(4, self.defs.cursor_x))
                }
            },
            Screen::Exit(mut d) => {
                let text = &screens.dialog_text;
                match d.tick(&input, text, &mut r.layer, &mut sfx) {
                    None => Screen::Exit(d),
                    Some(true) => {
                        eprintln!("exit confirmed");
                        Screen::Quit
                    }
                    Some(false) => {
                        if let Some(s) = r.sound.as_mut() {
                            s.set_paused(false);
                        }
                        crate::front::clear_all(&mut r.layer);
                        let (ticks, then) = exit_declined(self.opts.settings.exit_no_to_menu);
                        Screen::Wait(ticks, then)
                    }
                }
            }
            Screen::RouteSelect(mut m) => {
                let save = &r.save;
                let marks = |route| save.eeprom.clear_mark(route);
                match m.tick(
                    &input,
                    &env,
                    t,
                    &mut self.replay,
                    marks,
                    &mut r.layer,
                    &mut sfx,
                ) {
                    None => Screen::RouteSelect(m),
                    Some(RouteResult::Back) => {
                        Screen::Wait(BACK_TO_TITLE_TICKS, Next::Title { cursor: 0 })
                    }
                    Some(RouteResult::Chosen(chosen)) => {
                        if env.analog {
                            Screen::LeverCheck(LeverCheck::new(t, &mut r.layer), chosen)
                        } else {
                            // Digital modes force P0 / EB (0x4332c0(0, 9)).
                            r.controls.reset_levers();
                            sfx.push(Sfx::Stop(snd::BGM_MENU));
                            Screen::Wait(TO_INTRO_TICKS, Next::Intro(chosen))
                        }
                    }
                }
            }
            Screen::LeverCheck(mut c, chosen) => {
                if c.tick(&input, &mut r.layer) {
                    sfx.push(Sfx::Stop(snd::BGM_MENU));
                    Screen::Wait(TO_INTRO_TICKS, Next::Intro(chosen))
                } else {
                    Screen::LeverCheck(c, chosen)
                }
            }
            Screen::Board(mut b, chosen) => {
                if b.tick(t, &mut r.layer, &mut sfx) {
                    Screen::Wait(TO_PLAY_TICKS, Next::Play(chosen))
                } else {
                    Screen::Board(b, chosen)
                }
            }
            Screen::Wait(n, then) => {
                if n <= 1 {
                    next = Some(then);
                    Screen::Quit
                } else {
                    Screen::Wait(n - 1, then)
                }
            }
            Screen::Movie(mut m) => {
                // 0x428f00 polls the input every sample: the confirm edge skips.
                let skipped = input.confirm;
                m.player.advance(TICK.as_secs_f64());
                m.player.update();
                if skipped || m.player.finished() {
                    movie_ended = Some((m.from, skipped, m.player.timing()));
                    Screen::Quit
                } else {
                    Screen::Movie(m)
                }
            }
            s @ (Screen::Play(_) | Screen::Quit) => s,
        };
        // The stage intro is a run of its own: its route scene loads during the wait before it.
        if let Screen::Wait(TO_INTRO_TICKS, Next::Intro(chosen)) = self.screen
            && play::has_stage_intro(chosen.route)
        {
            let r = self.res.as_ref().expect("resources");
            let up = play::page_uploader(r.gpu.as_ref());
            play::load_scene_ahead(&r.data, &self.play_options(r, chosen), up);
        }
        if let Some(from) = movie {
            self.screen = self.start_movie(from);
        }
        if let Some((from, skipped, timing)) = movie_ended {
            self.screen = self.end_movie(from, skipped, timing, &mut sfx);
        }
        if let Some((item, v)) = set_option {
            self.apply(item, v, &mut sfx)?;
        }
        if let Some(lang) = language {
            self.choose_language(lang)?;
        }
        if let Some(i) = port_change {
            self.apply_port_setting(i)?;
        }
        if let Screen::PortSettings(p) = &self.screen {
            let env = self.env();
            let r = self.res.as_mut().expect("resources");
            let o = &self.opts;
            let d = &self.defs;
            p.draw(
                &d.port,
                &self.ui,
                &o.locks,
                &o.locale,
                &o.settings,
                &env,
                &d.front,
                &mut r.layer,
            );
        }
        if let Some(cal) = calibrated {
            self.save_calibration(cal)?;
        }
        if let Some(n) = next {
            self.start(n, &mut sfx)?;
        }
        if let Some((kind, stage)) = demo {
            self.start_demo(kind, stage)?;
        }
        self.play_sfx(&sfx);
        if let Some(s) = self.res.as_mut().and_then(|r| r.sound.as_mut()) {
            s.menu_frame();
        }
        self.poll();
        self.compose();
        Ok(())
    }

    /// `Options_Set` 0x42a460 and what it implies.
    fn apply(&mut self, item: usize, value: u8, sfx: &mut Vec<Sfx>) -> Result<()> {
        let r = self.res.as_mut().expect("resources");
        match options::set(&mut r.save.eeprom, item, value) {
            SetEffect::None => {}
            SetEffect::ControlMode(mode) => self.rebuild_controls(mode)?,
            SetEffect::SoundOff => {
                if let Some(s) = r.sound.as_mut() {
                    s.stop_all();
                    s.set_enabled(false);
                }
            }
            SetEffect::SoundOn => {
                if let Some(s) = r.sound.as_mut() {
                    s.set_enabled(true);
                }
                sfx.push(Sfx::Play(snd::BGM_MENU));
            }
        }
        // Written at once (decided port behaviour; the original saves only at exit).
        self.res().save.persist("an options change");
        Ok(())
    }

    fn rebuild_controls(&mut self, mode: u8) -> Result<()> {
        let data = &self.res.as_ref().expect("resources").data;
        let exe_bytes = data.exe()?;
        let exe = Exe::parse(&exe_bytes)?;
        let mut pad = self.opts.pad.clone();
        pad.joystick_dat = joystick_dat(&self.opts.pad);
        self.res().controls = Controls::new(&exe, usize::from(mode), &pad)?;
        Ok(())
    }

    /// Writes `joystick.dat` (0x434a00 writes it to the current directory; the port keeps it
    /// next to `eeprom.dat`) and reloads the input with it. Both files are written at once,
    /// atomically (decided port behaviour; the original saves `eeprom.dat` only at exit).
    fn save_calibration(&mut self, cal: crate::gamepad::Calibration) -> Result<()> {
        if self.opts.shot.is_some() {
            return Ok(());
        }
        if let Some(path) = joystick_dat(&self.opts.pad) {
            crate::save::write_atomic(&path, &cal.to_bytes())?;
            eprintln!("calibration written to {}", path.display());
        }
        self.res().save.persist("a calibration");
        let mode = self.res().save.eeprom.options().control_mode;
        self.rebuild_controls(mode)
    }

    /// The first-run choice: installed on the sprite bank at once (the runs get it through
    /// `opts.locale`) and written to `config.toml`, except in headless runs.
    fn choose_language(&mut self, lang: Language) -> Result<()> {
        let code = lang.code();
        self.opts.locale.lang = code.to_owned();
        self.opts.locale.lang_chosen = true;
        self.install_language(false)?;
        if self.opts.shot.is_some() {
            return Ok(());
        }
        let Some(path) = crate::config::path() else {
            eprintln!("warning: no config dir; the language is not remembered");
            return Ok(());
        };
        match crate::config::store_lang(&path, code) {
            Ok(()) => eprintln!("language {code} written to {}", path.display()),
            Err(e) => eprintln!("warning: the language is not remembered: {e:#}"),
        }
        Ok(())
    }

    /// Installs `opts.locale`'s language on the sprite bank (the runs get it through
    /// `opts.locale`) and loads the port's own strings in it. `labels` keeps the bundled
    /// label font for `ja` too, for the port's text rows on the options screens.
    fn install_language(&mut self, labels: bool) -> Result<()> {
        let r = self.res.as_mut().expect("resources");
        let exe_bytes = r.data.exe()?;
        let exe = Exe::parse(&exe_bytes)?;
        crate::locale::install(&self.opts.locale, &exe, &mut r.bank)?;
        if labels && !self.opts.locale.active() {
            crate::locale::install_label_font(&mut r.bank)?;
        }
        let cfg = &self.opts.locale;
        self.ui = UiText::load(&cfg.lang, cfg.dir.as_deref());
        Ok(())
    }

    /// A port setting changed on the settings screen (already set on `opts`): applied where
    /// the front end can apply it at once, then written to `config.toml` (except in headless
    /// runs, which never read it). The runs read the rest from `opts` when they start.
    fn apply_port_setting(&mut self, index: usize) -> Result<()> {
        let item = &port_settings::ITEMS[index];
        let s = self.opts.settings;
        match item.key {
            "lang" => self.install_language(true)?,
            "ui_scaling" | "ui_scale" | "wide_menus" => {
                crate::shot::set_ui_scaling(self.opts.locale.ui_scaling);
                if let Some(g) = self.res.as_mut().and_then(|r| r.gpu.as_mut()) {
                    g.overlay.scaling = self.opts.locale.ui_scaling;
                }
                self.fit_front()?;
            }
            "fullscreen" => {
                if let Some(g) = self.gpu() {
                    g.set_fullscreen(s.fullscreen);
                }
            }
            // The canvas is composed again every frame.
            "hud_upscale" => self.res().bank.set_upscale(s.hud_upscale),
            "save_on_change" => self.res().save.save_on_change = s.save_on_change,
            "audio_resampler" => {
                let m = self.opts.locale.audio_resampler;
                if let Some(snd) = self.res.as_mut().and_then(|r| r.sound.as_mut()) {
                    snd.set_resampler(m);
                }
            }
            "keyboard_with_controller" | "hybrid_input" => {
                self.opts.pad.keyboard_with_controller = s.keyboard_with_controller;
                self.opts.pad.hybrid_input = s.hybrid_input;
                let mode = self.res().save.eeprom.options().control_mode;
                self.rebuild_controls(mode)?;
            }
            _ => {}
        }
        if self.opts.shot.is_some() {
            return Ok(());
        }
        let Some(path) = crate::config::path() else {
            eprintln!("warning: no config dir; {} is not remembered", item.key);
            return Ok(());
        };
        let value = item.file_value(&self.opts.locale, &self.opts.settings);
        if let Err(e) = crate::config::store(&path, item.key, value) {
            eprintln!("warning: {} is not remembered: {e:#}", item.key);
        }
        Ok(())
    }

    fn compose(&mut self) {
        let Some(r) = self.res.as_mut() else {
            return;
        };
        // `Records_Create` sets the grey clear colour (0x428990(0x70, 0x7070), menus.md §5).
        let backdrop =
            matches!(self.screen, Screen::Records(_)).then_some(crate::screens::RANKING_GREY);
        self.canvas.compose(&r.layer, &r.bank, backdrop);
    }

    /// A run's tick (headless) with the script's keys.
    fn step_play(&mut self) -> Result<()> {
        self.ticks += 1;
        let tick = self.ticks;
        let mut down = self.script.down_at(tick);
        for (d, &held) in down.iter_mut().zip(&self.down) {
            *d |= held;
        }
        let Screen::Play(p) = &mut self.screen else {
            return Ok(());
        };
        p.set_down(down);
        if self.script.has_pad() {
            p.feed_pad(self.script.pad_at(tick));
        }
        p.step()?;
        self.after_play()
    }

    /// Back from a run: the resources come back and attract starts again.
    fn after_play(&mut self) -> Result<()> {
        let Screen::Play(p) = &mut self.screen else {
            return Ok(());
        };
        if let Some(e) = p.take_error() {
            return Err(e);
        }
        if !p.left() {
            return Ok(());
        }
        let next = match self.demo_attract.take() {
            Some(mut a) => {
                // The demo task's destroy (0x40e670), then attract's own `Task_Tick`.
                if p.in_opening() {
                    a.opening_done(p.opening_ended_by_start());
                } else {
                    a.demo_done(p.demo_ended_by_start());
                }
                Screen::Attract(a)
            }
            // The stage intro (0x408ec0) returns to `Stage_Run`: the departure board next.
            None => match self.intro_chosen.take() {
                Some(chosen) => Screen::Wait(TO_BOARD_TICKS, Next::Board(chosen)),
                None => Screen::Wait(
                    AFTER_RUN_TICKS,
                    Next::Attract {
                        skip_opening: p.quit(),
                    },
                ),
            },
        };
        let Screen::Play(p) = std::mem::replace(&mut self.screen, next) else {
            unreachable!()
        };
        let mut shared = p.into_shared();
        crate::front::clear_all(&mut shared.layer);
        self.res = Some(shared);
        self.fit_front()?;
        self.menu_input = MenuInput::NONE;
        self.accumulator = Duration::ZERO;
        self.last_frame = Instant::now();
        Ok(())
    }

    /// The screen's name in the frame trace ([`shuppatsu_render::perf`]).
    fn screen_name(&self) -> &'static str {
        match &self.screen {
            Screen::Language(_) => "language",
            Screen::Attract(_) => "attract",
            Screen::Title(_) => "title",
            Screen::Options(_) => "options",
            Screen::Control(_) => "control",
            Screen::Value(_) => "value",
            Screen::PortSettings(_) => "port",
            Screen::Records(_) => "records",
            Screen::Calibrate(_) => "calibrate",
            Screen::Exit(_) => "exit",
            Screen::RouteSelect(_) => "route",
            Screen::LeverCheck(..) => "levercheck",
            Screen::Board(..) => "board",
            Screen::Wait(..) => "wait",
            Screen::Play(_) => "play",
            Screen::Movie(_) => "movie",
            Screen::Quit => "quit",
        }
    }

    fn quitting(&self) -> bool {
        matches!(self.screen, Screen::Quit)
    }

    /// The front end's canvas: 640×480 at the UI scale, widened to the window's shape so the
    /// menu backdrops reach its edges (enhancement `wide_menus`; the screens stay centred).
    fn canvas_size(&self) -> (usize, usize) {
        self.res.as_ref().map_or_else(
            || {
                let s = self.opts.locale.ui_scale.max(1);
                (
                    shuppatsu_render::sprites::CANVAS_W * s,
                    shuppatsu_render::sprites::CANVAS_H * s,
                )
            },
            |r| r.layer.canvas_size(),
        )
    }

    /// Fits the front end's canvas to the window (the headless size without one): the
    /// canvas multiple follows its height (presentation.rs) and the width its shape, so wide
    /// windows show the menu backdrops continued sideways (`wide_menus`, menus.md §14).
    fn fit_front(&mut self) -> Result<()> {
        let Some(r) = self.res.as_mut() else {
            return Ok(());
        };
        let window = r.gpu.as_ref().map_or(self.opts.size, Gpu::size);
        // Without a window yet the translated text waits for its size (`install_deferred`).
        let generation = r.bank.generation();
        crate::presentation::fit_layer_with(
            &mut self.opts.locale,
            &r.data,
            &mut r.bank,
            &mut r.layer,
            window,
            self.opts.settings.wide_menus,
            r.gpu.is_none(),
        )?;
        let (w, h) = r.layer.canvas_size();
        if self.canvas.size() != (w, h) || r.bank.generation() != generation {
            self.canvas.compose(&r.layer, &r.bank, None);
        }
        Ok(())
    }

    /// Headless: runs `ticks` app ticks and renders the frame.
    fn shot(mut self, ticks: u64, out: &std::path::Path) -> Result<()> {
        while self.ticks < ticks && !self.quitting() {
            if matches!(self.screen, Screen::Play(_)) {
                self.step_play()?;
            } else {
                self.tick()?;
            }
        }
        match &self.screen {
            Screen::Play(p) => p.shot(out)?,
            Screen::Movie(m) => {
                let rect = self.movie_rect(&m.player, self.opts.size);
                let scaling = self.opts.locale.movie_scaling;
                crate::shot::render_to_ppm_with(
                    self.opts.size,
                    |device, queue, view, format| {
                        crate::shot::clear_black(device, queue, view);
                        let mut o = shuppatsu_render::overlay::Overlay::new(device, format);
                        o.resize(device, m.player.size());
                        o.upload(queue, m.player.rgba());
                        scaling.configure(&mut o);
                        o.render(device, queue, view, rect);
                    },
                    None,
                    out,
                )?;
                eprintln!("movie picture {:?}", m.player.shown());
            }
            _ => {
                let hud = Some(&self.canvas);
                crate::shot::render_to_ppm(None, self.opts.size, |_| {}, hud, out)?
            }
        }
        eprintln!("wrote {} at app tick {}", out.display(), self.ticks);
        Ok(())
    }

    fn redraw(&mut self) -> Result<()> {
        if let Screen::Play(p) = &mut self.screen {
            // The run's ticks are app ticks too, as in the headless loop ([`App::step_play`]):
            // the global tick of later runs and the input log count them.
            let before = p.ticks();
            p.redraw()?;
            self.ticks += p.ticks() - before;
            return self.after_play();
        }
        let now = Instant::now();
        let dt = now - self.last_frame;
        self.last_frame = now;
        self.accumulator += dt.min(Duration::from_millis(250));
        while self.accumulator >= TICK && !self.quitting() {
            self.accumulator -= TICK;
            shuppatsu_render::perf::count(shuppatsu_render::perf::Counter::Ticks, 1);
            let perf = shuppatsu_render::perf::scope(shuppatsu_render::perf::Section::Tick);
            self.tick()?;
            drop(perf);
            if matches!(self.screen, Screen::Play(_)) {
                return Ok(());
            }
        }
        self.fit_front()?;
        // The movie: the picture for the clock, and where it goes.
        let new_picture = match &mut self.screen {
            Screen::Movie(m) => {
                let _perf = shuppatsu_render::perf::scope(shuppatsu_render::perf::Section::Movie);
                m.player.update_display();
                m.player.take_new_picture()
            }
            _ => false,
        };
        let movie_rect = match (&self.screen, self.res.as_ref().and_then(|r| r.gpu.as_ref())) {
            (Screen::Movie(m), Some(g)) => Some(self.movie_rect(&m.player, g.size())),
            _ => None,
        };
        let movie_scaling = self.opts.locale.movie_scaling;
        let size = self.canvas_size();
        let Some(gpu) = self.res.as_mut().and_then(|r| r.gpu.as_mut()) else {
            return Ok(());
        };
        let (width, height) = gpu.size();
        let Some(frame) = gpu.acquire() else {
            return Ok(());
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        crate::shot::clear_black(&gpu.device, &gpu.queue, &view);
        if let (Screen::Movie(m), Some(rect)) = (&mut self.screen, movie_rect) {
            // Black around the picture, nothing of the canvas.
            let mut fresh = false;
            let o = self.movie_overlay.get_or_insert_with(|| {
                fresh = true;
                let mut o = shuppatsu_render::overlay::Overlay::new(&gpu.device, gpu.config.format);
                o.resize(&gpu.device, m.player.size());
                o
            });
            let upload = (fresh || new_picture) && !m.player.rgba().is_empty();
            if upload {
                o.upload(&gpu.queue, m.player.rgba());
            }
            movie_scaling.configure(o);
            if !m.player.rgba().is_empty() {
                o.render(&gpu.device, &gpu.queue, &view, rect);
            }
            gpu.present(frame);
            m.player.trace_present(upload, now.elapsed());
            shuppatsu_render::perf::frame("movie", m.player.size());
            return Ok(());
        }
        // A run may have left the overlay at its wide size.
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
        shuppatsu_render::perf::frame(self.screen_name(), size);
        Ok(())
    }

    fn gpu(&self) -> Option<&Gpu> {
        match &self.screen {
            Screen::Play(p) => p.gpu(),
            _ => self.res.as_ref().and_then(|r| r.gpu.as_ref()),
        }
    }

    fn gpu_mut(&mut self) -> Option<&mut Gpu> {
        match &mut self.screen {
            Screen::Play(p) => p.gpu_mut(),
            _ => self.res.as_mut().and_then(|r| r.gpu.as_mut()),
        }
    }

    /// Alt+Enter / F11 ([`Gpu::fullscreen_key`]): `true` when `event` was one, kept from the
    /// game. A switch is mirrored in setting `fullscreen` (the settings screen shows it) and
    /// remembered.
    fn fullscreen_key(&mut self, event: &WindowEvent) -> bool {
        match self.gpu_mut().and_then(|g| g.fullscreen_key(event)) {
            None => false,
            Some(Toggle::Swallowed) => true,
            Some(Toggle::Switched(on)) => {
                self.opts.settings.fullscreen = on;
                if self.opts.shot.is_none() {
                    crate::window::remember_fullscreen(on);
                }
                true
            }
        }
    }

    fn exit(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.exit();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu().is_some() {
            return;
        }
        let Some(r) = self.res.as_mut() else {
            return;
        };
        match Gpu::create(
            event_loop,
            "shuppatsu",
            self.opts.size,
            !self.opts.settings.vsync,
            self.opts.settings.fullscreen,
            self.opts.locale.ui_scale,
        ) {
            Ok(gpu) => {
                gpu.window.request_redraw();
                r.gpu = Some(gpu);
                self.last_frame = Instant::now();
            }
            Err(e) => {
                self.error = Some(e.context("GPU init"));
                self.exit(event_loop);
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if self.fullscreen_key(&event) {
            return;
        }
        // F12: an issue mark (taken at the next frame).
        if crate::marks::key_event(&event) {
            return;
        }
        match &event {
            WindowEvent::CloseRequested => return self.exit(event_loop),
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
                if let Some(d) = crate::input::dik(*code) {
                    self.down[usize::from(d)] = *state == ElementState::Pressed;
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(g) = self.res.as_mut().and_then(|r| r.gpu.as_mut()) {
                    g.resize(size.width, size.height);
                }
            }
            _ => {}
        }
        if let Screen::Play(p) = &mut self.screen
            && !matches!(event, WindowEvent::RedrawRequested)
        {
            p.window_input(&event);
            return;
        }
        if matches!(event, WindowEvent::RedrawRequested) {
            self.idle.redraw(Instant::now());
            if crate::marks::take_request() && self.mark.is_none() {
                self.mark = crate::marks::begin(self.gpu_mut());
            }
            if let Err(e) = self.redraw() {
                self.error = Some(e);
                return self.exit(event_loop);
            }
            self.finish_mark();
            if self.quitting() {
                return self.exit(event_loop);
            }
            if let Some(g) = self.gpu() {
                g.window.request_redraw();
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        // A screen starts within a few ticks (a run: tens of ms of work in one tick): poll
        // through it and the first frames after, so it runs on a core that has not clocked down.
        let soon = match &self.screen {
            Screen::Wait(n, _) => *n <= WARM_TICKS,
            Screen::Board(b, _) => b.ticks_left().is_some_and(|n| n <= WARM_TICKS),
            _ => false,
        };
        if soon {
            self.idle.keep_warm(now + WARM_AFTER);
        }
        event_loop.set_control_flow(self.idle.control_flow(now));
    }
}

/// Ticks before a screen starts from which the event loop polls ([`App::about_to_wait`]).
const WARM_TICKS: i32 = 3;
/// How long it keeps polling after that (the start's tick and the first frames after it).
const WARM_AFTER: Duration = Duration::from_millis(150);

/// `shuppatsu` with no subcommand.
pub fn run(opts: AppOptions) -> Result<()> {
    let app = App::new(opts)?;
    if let Some((ticks, out)) = app.opts.shot.clone() {
        return app.shot(ticks, &out);
    }
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = app;
    event_loop.run_app(&mut app)?;
    shuppatsu_render::perf::flush();
    // The save is written at exit (0x434f50 on WinMain's shutdown path).
    let save = match (&mut app.screen, app.res.as_ref()) {
        (Screen::Play(p), _) => p.save().write(),
        (_, Some(r)) => r.save.write(),
        _ => Ok(()),
    };
    match app.error {
        Some(e) => Err(e),
        None => save.context("writing the save"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::tables::TextDesc;

    /// Decided port behaviour (menus.md §14): いいえ in the exit dialog leads back to the title
    /// menu with the cursor on ゲーム終了, not to attract.
    #[test]
    fn exit_no_returns_to_the_menu_on_quit() {
        let t = crate::front::tests_support::tables();
        let env = Env::default();
        let text = vec![TextDesc::default(); 29];
        let mut l = SpriteLayer::default();
        let mut sfx = Vec::new();
        let confirm = MenuInput {
            confirm: true,
            ..MenuInput::NONE
        };

        // The dialog opens on いいえ; confirming it declines.
        let mut d = ExitConfirm::new(t.exit_dialog.clone(), (320, 192), &mut l);
        let mut declined = None;
        for i in 0..8 {
            let input = if i == 1 { confirm } else { MenuInput::NONE };
            declined = declined.or(d.tick(&input, &text, &mut l, &mut sfx));
        }
        assert_eq!(declined, Some(false));

        let (ticks, then) = exit_declined(crate::settings::Settings::default().exit_no_to_menu);
        assert_eq!(ticks, TO_ATTRACT_TICKS);
        let Next::Title { cursor } = then else {
            panic!("{then:?}");
        };
        // The menu comes back on ゲーム終了: confirming it opens the dialog again.
        let mut m = TitleMenu::new(cursor, 0);
        let mut choice = None;
        for i in 0..40 {
            let input = if i == 0 { confirm } else { MenuInput::NONE };
            choice = choice.or(m.tick(&input, &env, &t, &mut l, &mut sfx));
        }
        assert_eq!(choice, Some(TitleChoice::Exit));
    }

    /// The port settings screen's live hooks: the language is installed on the bank with
    /// the port's strings in it, the UI scale refits the canvas, the save and exit switches
    /// reach what reads them. Headless, so nothing is written. Needs `SHUPPATSU_DATA_DIR`.
    #[test]
    fn port_settings_apply_live() {
        if std::env::var_os("SHUPPATSU_DATA_DIR").is_none() {
            eprintln!("skipped: SHUPPATSU_DATA_DIR not set");
            return;
        }
        let opts = AppOptions {
            size: (1280, 800),
            settings: crate::settings::Settings::default(),
            mute: true,
            pad: PadOptions::default(),
            inputs: None,
            save_file: None,
            shot: Some((0, std::env::temp_dir().join("shuppatsu-unused.ppm"))),
            locale: crate::locale::LocaleConfig::default(),
            locks: crate::config::Locks::default(),
            first_run: false,
        };
        let mut app = App::new(opts).unwrap();
        let index = |k| {
            port_settings::ITEMS
                .iter()
                .position(|i| i.key == k)
                .unwrap()
        };
        let (cfg, s) = (&mut app.opts.locale, &mut app.opts.settings);
        port_settings::ITEMS[index("lang")].step(cfg, s, 1);
        app.apply_port_setting(index("lang")).unwrap();
        assert_eq!(app.ui.get("title"), "Port Settings");
        assert!(app.res().bank.text().is_some(), "the English overlay");
        let (cfg, s) = (&mut app.opts.locale, &mut app.opts.settings);
        port_settings::ITEMS[index("lang")].step(cfg, s, 1);
        app.apply_port_setting(index("lang")).unwrap();
        assert_eq!(app.ui.get("title"), "ポート設定");
        let ja = app.res().bank.text().map(|t| t.len());
        assert_eq!(
            ja,
            Some(2),
            "the original art, but for the port's seventh options row"
        );

        let before = app.res().layer.scale;
        app.opts.locale.ui_scale_fixed = Some(if before == 3 { 2 } else { 3 });
        app.apply_port_setting(index("ui_scale")).unwrap();
        let want = app.opts.locale.ui_scale_fixed.unwrap();
        assert_eq!(app.res().layer.scale, want);
        let (w, h) = app.res().layer.canvas_size();
        assert_eq!(
            app.canvas.rgba().len(),
            w * h * 4,
            "a canvas of the new size"
        );

        app.opts.settings.save_on_change = false;
        app.apply_port_setting(index("save_on_change")).unwrap();
        assert!(!app.res().save.save_on_change);
        app.opts.settings.wide_menus = false;
        app.apply_port_setting(index("wide_menus")).unwrap();
        assert_eq!(app.res().layer.wide, 0, "the 4:3 menus");
    }

    /// A headless app for the attract START harness. Needs `SHUPPATSU_DATA_DIR`.
    fn headless_app() -> Option<App> {
        if std::env::var_os("SHUPPATSU_DATA_DIR").is_none() {
            eprintln!("skipped: SHUPPATSU_DATA_DIR not set");
            return None;
        }
        let opts = AppOptions {
            size: (640, 480),
            settings: crate::settings::Settings::default(),
            mute: true,
            pad: PadOptions::default(),
            inputs: None,
            save_file: None,
            shot: Some((0, std::env::temp_dir().join("shuppatsu-unused.ppm"))),
            locale: crate::locale::LocaleConfig::default(),
            locks: crate::config::Locks::default(),
            first_run: false,
        };
        Some(App::new(opts).unwrap())
    }

    impl App {
        /// One tick as the headless `--shot` loop runs it.
        fn step_headless(&mut self) {
            if matches!(self.screen, Screen::Play(_)) {
                self.step_play().unwrap();
            } else {
                self.tick().unwrap();
            }
        }

        /// What the app shows now.
        fn shown(&self) -> &'static str {
            match &self.screen {
                Screen::Play(p) if p.in_opening() => "opening",
                Screen::Play(_) => "demo",
                Screen::Attract(a) => a.shown(),
                Screen::Title(_) => "title",
                Screen::Movie(_) => "movie",
                Screen::Wait(..) => "wait",
                _ => "other",
            }
        }

        /// Ticks until `what` shows; panics after `max` ticks.
        fn run_until(&mut self, what: &str, max: u32) {
            for _ in 0..max {
                if self.shown() == what {
                    return;
                }
                self.step_headless();
            }
            panic!("no {what} within {max} ticks (at {})", self.shown());
        }

        /// Enter on the next tick (the script key the poll at its end reads), then the
        /// screens shown until the title picture or the title menu, at most 30 ticks.
        fn press_start(&mut self) -> Vec<&'static str> {
            self.script.keys.push((self.ticks + 1, 0x1c));
            let mut seen = vec![self.shown()];
            for _ in 0..30 {
                self.step_headless();
                let s = self.shown();
                if seen.last() != Some(&s) {
                    seen.push(s);
                }
                if seen.len() > 1 && (s == "picture" || s == "title") {
                    break;
                }
            }
            seen
        }
    }

    /// START in each attract item, on the real data with the app's own loop (0x402700):
    /// the cold-start opening, the route demo, a photo and the poster all land on the title
    /// picture without the opening again; START on the picture goes to the title menu. The
    /// lesson demo shares the route demo's path (`Attract::demo_done`).
    #[test]
    fn start_in_attract_never_replays_the_opening() {
        let Some(mut app) = headless_app() else {
            return;
        };
        app.run_until("opening", 600);
        for _ in 0..100 {
            app.step_headless();
        }
        assert_eq!(app.press_start(), ["opening", "attract", "picture"]);
        for item in ["demo", "photo", "poster"] {
            // The restarted walk reaches the item again from the picture.
            app.run_until(item, 4000);
            for _ in 0..30 {
                app.step_headless();
            }
            assert_eq!(app.press_start(), [item, "attract", "picture"], "{item}");
        }
        for _ in 0..30 {
            app.step_headless();
        }
        assert_eq!(app.press_start(), ["picture", "leaving", "title"]);
    }

    /// START in the logo: before its state 3 it only jumps there (the opening still
    /// follows); afterwards (0x402a70 sets `0x855ffc |= 0x10`) the title picture follows.
    #[test]
    fn start_in_the_logo() {
        let Some(mut app) = headless_app() else {
            return;
        };
        app.run_until("logo", 10);
        for _ in 0..10 {
            app.step_headless();
        }
        app.script.keys.push((app.ticks + 1, 0x1c));
        app.run_until("opening", 600);

        let Some(mut app) = headless_app() else {
            return;
        };
        app.run_until("logo", 10);
        for _ in 0..400 {
            app.step_headless();
        }
        assert_eq!(app.press_start(), ["logo", "attract", "picture"]);
    }

    /// Setting `exit_no_to_menu` off: the original's いいえ, attract from the title picture
    /// after the same three ticks (0x42a1c0 case 0), like the title menu's cancel.
    #[test]
    fn exit_no_without_the_setting_goes_to_attract() {
        let (ticks, then) = exit_declined(false);
        assert_eq!(ticks, TO_ATTRACT_TICKS);
        assert!(
            matches!(then, Next::Attract { skip_opening: true }),
            "{then:?}"
        );
    }
}

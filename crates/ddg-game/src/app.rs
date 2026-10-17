//! `ddg-game` with no subcommand: the whole game, sequenced like `Game_MainFlow` 0x419510
//! (main-loop.md §5, menus.md §2–§7):
//!
//! attract → title menu (options, records, calibration, exit) → route select → lever check
//! (analog modes) → departure board → [`Play`] → back to attract / the title.
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
use crate::front::prerun::{DepartureBoard, LeverCheck};
use crate::front::records::{Records, RecordsStep};
use crate::front::route_select::{Chosen, RouteResult, RouteSelect};
use crate::front::tables::FrontTables;
use crate::front::title::{TitleChoice, TitleMenu, komachi_unlocked};
use crate::front::{Env, snd};
use crate::gamepad::{Controls, JoyState, PadOptions, PadProfile, PadSource};
use crate::play::{self, Play, PlayOptions, Shared};
use crate::save::Save;
use crate::scene::GameData;
use crate::screens::tables::ScreenTables;
use crate::screens::{MenuInput, Sfx};
use crate::window::Gpu;
use anyhow::{Context, Result};
use ddg_engine::demo::DemoKind;
use ddg_formats::exe::Exe;
use ddg_render::sprites::SpriteLayer;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::WindowId;

/// The original's fixed tick (timeSetEvent(33, …) at 0x436f30).
const TICK: Duration = Duration::from_millis(33);
/// Title choice 0: BGM stopped, sprites cleared, three ticks (0x42a1c0), then the one-tick
/// "press START" of the PC build (0x4122d0) before `Stage_Run` creates the route select.
const TO_ROUTE_SELECT_TICKS: i32 = 4;
/// Title cancel / exit declined: three ticks (0x42a1c0), then attract from the title picture.
const TO_ATTRACT_TICKS: i32 = 3;
/// Title choice 2: BGM stopped, sprites cleared, ten ticks before the records viewer.
const TO_RECORDS_TICKS: i32 = 10;
/// Route select cancelled: `Stage_Run`'s exit path (one tick, then two) and two more ticks of
/// `Game_MainFlow` before the title menu comes back without attract (`0x855ffc & 0x40`).
const BACK_TO_TITLE_TICKS: i32 = 5;
/// Between the route select (or the lever check) and the departure board: BGM release, route
/// data, `0x425490(15)` and the setup calls' five `Task_Tick`s (0x4080d0).
const TO_BOARD_TICKS: i32 = 20;
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
    pub uncapped: bool,
    pub square_pixels: bool,
    pub original_visibility: bool,
    pub stable_track_variants: bool,
    pub texture_filtering: bool,
    pub fog: bool,
    pub mute: bool,
    pub pad: PadOptions,
    /// Scripted keys, `key <tick>[-<last>] <name>` (app ticks, the run's included).
    pub inputs: Option<PathBuf>,
    /// Headless: run this many app ticks, render one frame to a PPM and exit.
    pub shot: Option<(u64, PathBuf)>,
    pub locale: crate::locale::LocaleConfig,
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
    Records(Records),
    Calibrate(CalibrationScreen),
    Exit(ExitConfirm),
    RouteSelect(RouteSelect),
    LeverCheck(LeverCheck, Chosen),
    Board(DepartureBoard, Chosen),
    Wait(i32, Next),
    Play(Box<Play>),
    /// Exit confirmed: the window closes (`DestroyWindow` → `WM_QUIT`).
    Quit,
}

/// Exe tables of the front end, read once.
struct Defs {
    front: FrontTables,
    screens: ScreenTables,
    attract: AttractTables,
    cursor_x: i16,
    exit_frame: (i16, i16),
    language: Look,
}

pub struct App {
    opts: AppOptions,
    defs: Defs,
    /// The resources a run borrows; `None` while the run has them.
    res: Option<Shared>,
    screen: Screen,
    joystick: bool,
    /// The replay-playback flag `0x816974` (R in the train list).
    replay: bool,
    /// Attract's walk and photo counters (`0x8573c8`, `0x8573cc`), kept across walks.
    attract_counters: AttractCounters,
    /// Attract waiting for its demo drive (the run in [`Screen::Play`]) to end.
    demo_attract: Option<Attract>,
    down: [bool; 256],
    menu_input: MenuInput,
    script: play::Script,
    canvas: Vec<u8>,
    canvas_dirty: bool,
    ticks: u64,
    accumulator: Duration,
    last_frame: Instant,
    error: Option<anyhow::Error>,
}

fn map_size(bank: &ddg_render::sprites::SpriteBank, tbl: usize) -> (i16, i16) {
    bank.tilemaps.get(tbl).map_or((0, 0), |m| {
        (i16::from(m.width) * TILE_PX, i16::from(m.height) * TILE_PX)
    })
}

fn joystick_dat(opts: &PadOptions) -> Option<PathBuf> {
    opts.joystick_dat.clone().or_else(|| {
        crate::save::default_path().and_then(|p| Some(p.parent()?.join("joystick.dat")))
    })
}

impl App {
    pub fn new(opts: AppOptions) -> Result<Self> {
        let data = GameData::from_env()?;
        let exe_bytes = data.exe()?;
        let exe = Exe::parse(&exe_bytes)?;
        let headless = opts.shot.is_some();
        let save_path = if headless {
            None
        } else {
            crate::save::default_path()
        };
        let mut save = Save::open(&exe, save_path)?;
        if let Some(p) = save.path() {
            eprintln!("save: {}", p.display());
        }
        // `0x8298a4`: a joystick is attached (never in headless runs, to stay deterministic).
        let joystick =
            !headless && PadSource::open(PadProfile::Generic).is_some_and(|p| p.connected());
        // 0x434ed0: a joystick mode without a joystick falls back to mode 0.
        if save.eeprom.options().control_mode > 3 && !joystick {
            save.eeprom.set_option(options::item::CONTROL, 0);
        }
        let mut pad = opts.pad.clone();
        pad.joystick_dat = joystick_dat(&opts.pad);
        let controls = Controls::new(&exe, usize::from(save.eeprom.options().control_mode), &pad)?;
        let mut bank = play::load_bank(&data, &exe, &opts.locale)?;
        if opts.first_run {
            // The language screen's labels, before any language is installed.
            crate::locale::install_label_font(&mut bank)?;
        }
        let mut sound = if headless || opts.mute {
            None
        } else {
            crate::sound::Sound::open_device(data.data_dir(), &exe)?
        };
        if let Some(s) = sound.as_mut() {
            s.set_enabled(save.eeprom.options().sound == 0);
        }
        let mut layer = SpriteLayer {
            scale: opts.locale.ui_scale,
            ..SpriteLayer::default()
        };
        let canvas = layer.new_canvas();
        crate::hud::Hud::install_palettes(&exe, &mut layer)?;
        let front = FrontTables::read(&exe)?;
        crate::front::mark_backdrops(&mut bank, &front);
        let screens = ScreenTables::read(&exe)?;
        let cursor_x = crate::front::cursor_x(&front, map_size(&bank, TBL_CURSOR).0);
        let menu_palette = layer.palettes[usize::from(crate::front::PAL_MENU)].as_ref();
        let defs = Defs {
            cursor_x,
            language: Look::new(cursor_x, map_size(&bank, TBL_CURSOR), menu_palette),
            exit_frame: map_size(&bank, usize::try_from(front.exit_dialog.frame)?),
            front,
            screens,
            attract: AttractTables::read(&exe)?,
        };
        let script = match &opts.inputs {
            Some(p) => play::parse_script(
                &std::fs::read_to_string(p).with_context(|| p.display().to_string())?,
            )?,
            None => play::Script::default(),
        };
        let now = Instant::now();
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
            }),
            // Boot: `Game_MainFlow` starts with attract (`0x855ffc & 0x40` clear).
            screen: if opts.first_run {
                Screen::Language(LanguageScreen::new())
            } else {
                Screen::Attract(Attract::new(false, AttractCounters::default()))
            },
            joystick,
            replay: false,
            attract_counters: AttractCounters::default(),
            demo_attract: None,
            down: [false; 256],
            menu_input: MenuInput::NONE,
            script,
            canvas,
            canvas_dirty: true,
            ticks: 0,
            accumulator: Duration::ZERO,
            last_frame: now,
            error: None,
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
        Env {
            control_mode: r.save.eeprom.options().control_mode,
            analog: r.controls.is_analog(),
            joystick: self.joystick,
            // No force feedback is implemented: the vibration item stays hidden, as on a PC
            // without an FF device (`0x816838 == 0`).
            force_feedback: false,
            sound_device: r.sound.is_some() || self.opts.mute,
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
        let down = self.down;
        let keys = &self.script.keys;
        let Some(r) = self.res.as_mut() else {
            return;
        };
        let b = r.controls.poll(
            |c| down[usize::from(c)] || keys.iter().any(|&(t, k)| t == tick && k == c),
            true,
        );
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
            Next::RouteSelect => Screen::RouteSelect(RouteSelect::new(self.defs.cursor_x, sfx)),
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
                let route = chosen.route;
                let layer = &mut self.res.as_mut().expect("resources").layer;
                let b = DepartureBoard::new(&self.defs.front, route, chosen.variant, layer);
                Screen::Board(b, chosen)
            }
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

    fn start_play(&mut self, chosen: Chosen) -> Result<Play> {
        let mut shared = self.res.take().expect("resources");
        let o = shared.save.eeprom.options();
        let opts = PlayOptions {
            stage: chosen.route,
            difficulty: usize::from(shared.save.eeprom.difficulty()),
            station: 0,
            control_mode: usize::from(o.control_mode),
            pad: self.opts.pad.clone(),
            size: self.opts.size,
            uncapped: self.opts.uncapped,
            square_pixels: self.opts.square_pixels,
            original_visibility: self.opts.original_visibility,
            stable_track_variants: self.opts.stable_track_variants,
            texture_filtering: self.opts.texture_filtering,
            fog: self.opts.fog,
            mute: self.opts.mute,
            inputs: None,
            shot: None,
            speedometer: None,
            variant: chosen.variant,
            ending: None,
            locale: self.opts.locale.clone(),
        };
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
        let mut set_option: Option<(usize, u8)> = None;
        let mut calibrated = None;
        let mut language = None;
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
                        eprintln!("attract: the Komachi movie would play here (postponed, D2)");
                        Screen::Attract(a)
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
                        TitleChoice::Options => Screen::Options(OptionsList::new(0)),
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
                    None => Screen::Options(m),
                    Some(res) => {
                        options::destroy(&mut r.layer);
                        match res {
                            ListResult::Back => {
                                Screen::Title(TitleMenu::new(1, self.defs.cursor_x))
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
                None => Screen::Control(p),
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
            Screen::Records(mut rec) => {
                let pad_legend = env.analog || env.control_mode == 4;
                let step = rec.tick(
                    &input,
                    pad_legend,
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
                        Screen::Wait(TO_ATTRACT_TICKS, Next::Attract { skip_opening: true })
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
                            Screen::Wait(TO_BOARD_TICKS, Next::Board(chosen))
                        }
                    }
                }
            }
            Screen::LeverCheck(mut c, chosen) => {
                if c.tick(&input, &mut r.layer) {
                    sfx.push(Sfx::Stop(snd::BGM_MENU));
                    Screen::Wait(TO_BOARD_TICKS, Next::Board(chosen))
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
            s @ (Screen::Play(_) | Screen::Quit) => s,
        };
        if let Some((item, v)) = set_option {
            self.apply(item, v, &mut sfx)?;
        }
        if let Some(lang) = language {
            self.choose_language(lang)?;
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
        let r = self.res.as_mut().expect("resources");
        let exe_bytes = r.data.exe()?;
        let exe = Exe::parse(&exe_bytes)?;
        crate::locale::install(&self.opts.locale, &exe, &mut r.bank)?;
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

    fn compose(&mut self) {
        let Some(r) = self.res.as_mut() else {
            return;
        };
        r.layer.compose(&r.bank, &mut self.canvas);
        self.canvas_dirty = true;
    }

    /// A run's tick (headless) with the script's keys.
    fn step_play(&mut self) -> Result<()> {
        self.ticks += 1;
        let tick = self.ticks;
        let mut down = self.down;
        for &(t, k) in &self.script.keys {
            if t == tick {
                down[usize::from(k)] = true;
            }
        }
        let Screen::Play(p) = &mut self.screen else {
            return Ok(());
        };
        p.set_down(down);
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
            None => Screen::Wait(
                AFTER_RUN_TICKS,
                Next::Attract {
                    skip_opening: p.quit(),
                },
            ),
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
                    ddg_render::sprites::CANVAS_W * s,
                    ddg_render::sprites::CANVAS_H * s,
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
        crate::presentation::fit_layer(
            &mut self.opts.locale,
            &r.data,
            &mut r.bank,
            &mut r.layer,
            window,
            true,
        )?;
        let (w, h) = r.layer.canvas_size();
        if self.canvas.len() != w * h * 4 {
            self.canvas = r.layer.new_canvas();
            r.layer.compose(&r.bank, &mut self.canvas);
            self.canvas_dirty = true;
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
            _ => {
                let size = self.canvas_size();
                let hud = Some((self.canvas.as_slice(), size));
                crate::shot::render_to_ppm(None, self.opts.size, |_| {}, hud, out)?
            }
        }
        eprintln!("wrote {} at app tick {}", out.display(), self.ticks);
        Ok(())
    }

    fn redraw(&mut self) -> Result<()> {
        if let Screen::Play(p) = &mut self.screen {
            p.redraw()?;
            return self.after_play();
        }
        let now = Instant::now();
        let dt = now - self.last_frame;
        self.last_frame = now;
        self.accumulator += dt.min(Duration::from_millis(250));
        while self.accumulator >= TICK && !self.quitting() {
            self.accumulator -= TICK;
            self.tick()?;
            if matches!(self.screen, Screen::Play(_)) {
                return Ok(());
            }
        }
        self.fit_front()?;
        let mut dirty = std::mem::take(&mut self.canvas_dirty);
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
        // A run may have left the overlay at its wide size.
        dirty |= gpu.overlay.resize(&gpu.device, size);
        if dirty {
            gpu.overlay.upload(&gpu.queue, &self.canvas);
        }
        gpu.overlay.scaling = self.opts.locale.ui_scaling;
        gpu.overlay.render(
            &gpu.device,
            &gpu.queue,
            &view,
            gpu.overlay.rect(width, height),
        );
        gpu.window.pre_present_notify();
        gpu.queue.present(frame);
        Ok(())
    }

    fn gpu(&self) -> Option<&Gpu> {
        match &self.screen {
            Screen::Play(p) => p.gpu(),
            _ => self.res.as_ref().and_then(|r| r.gpu.as_ref()),
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
            "ddg-game",
            self.opts.size,
            self.opts.uncapped,
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
            if let Err(e) = self.redraw() {
                self.error = Some(e);
                return self.exit(event_loop);
            }
            if self.quitting() {
                return self.exit(event_loop);
            }
            if let Some(g) = self.gpu() {
                g.window.request_redraw();
            }
        }
    }
}

/// `ddg-game` with no subcommand.
pub fn run(opts: AppOptions) -> Result<()> {
    let app = App::new(opts)?;
    if let Some((ticks, out)) = app.opts.shot.clone() {
        return app.shot(ticks, &out);
    }
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = app;
    event_loop.run_app(&mut app)?;
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

//! Shuppatsu: an engine for playing Densha de GO! 2 Kōsoku-hen 3000-bandai from your own disc.
//!
//! Phase 3: `shuppatsu view <route.com>` flies through a route; `shuppatsu shot` renders one frame
//! headless for checks.
// Release builds on Windows have no console window; logs go to a file (win_console.rs).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod board_en;
mod cab;
mod captions;
mod config;
mod coupling_hud;
mod data_dir;
mod departure_hud;
mod desktop;
mod front;
mod gamepad;
mod glyphs;
mod gpu_backend;
mod hints;
mod hud;
mod hybrid;
mod identity;
mod input;
mod label_audit;
mod locale;
mod logo;
mod marks;
mod movie;
mod navigator;
mod pacing;
mod play;
mod popups;
mod presentation;
mod result_view;
mod save;
mod scene;
mod screens;
mod script;
mod settings;
mod shot;
mod slots;
mod smooth;
mod sound;
mod subtitles;
mod texpack;
mod traffic;
mod weather;
mod win_console;
mod window;

use anyhow::{Context, Result, bail};
use scene::{RouteScene, Visibility};
use shuppatsu_formats::route::UNITS_PER_POINT;
use shuppatsu_formats::scene::{DRIVING_FOV_Y_DEG, TrackPos};
use shuppatsu_render::camera::ORIGINAL_STRETCH;
use shuppatsu_render::{FrameGeometry, Renderer};
use std::time::{Duration, Instant};
use window::Gpu;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::WindowId;

const USAGE: &str = "\
usage: shuppatsu [options] [--joystick-dat <file>] [--combo <file>] [--mute]
                [--inputs <script>] [--shot <ticks>:<file.ppm>] [--save <eeprom.dat>]
       shuppatsu play <stage 0..19> [--difficulty <0..3, default: the save's>] [--station <n>] [--keys <0..6>] [options]
                     [--joystick-dat <file>] [--combo <file>]
                     [--inputs <script>] [--shot <ticks>:<file.ppm>] [--speedometer <0|1|2>]
                     [--save <eeprom.dat>] [--global-tick <n>]
       shuppatsu view <route.com> [options]
       shuppatsu shot <route.com> --at <metres> --out <file.ppm> [options]
       shuppatsu pad <monitor|calibrate|learn> [--mode <4..6>] [--joystick-dat <file>] [--combo <file>]
       shuppatsu install-desktop [--exec <path>] [--uninstall]
                     desktop entry + icon under $XDG_DATA_HOME (opt-in; see --help)
       shuppatsu label-audit [--out <dir>] [--catalogue <en.toml>] [--palette <n>]
                     check the catalogue's regions against the art (report + crops)

options:
  --data <dir>             the extracted disc or installed game (after env SHUPPATSU_DATA_DIR; else
                           data\\ next to the exe, else config.toml data_dir; docs/building.md)
  --console                Windows: log to the starting shell's console or a new one, not to
                           %APPDATA%\\shuppatsu\\shuppatsu.log
  --speed <km/h>           fly-through speed (default 80)
  --gradient-variant <0|1> sky/ground gradient set (1 = the timed alternate set)
  --fog                    view: linear fog everywhere (off by default); elsewhere the switch
  --hud                    the driving HUD over the view (test values)
  --debug-title            live debug text in the window title (env SHUPPATSU_DEBUG_TITLE=1)
  --size <WxH>             window/frame size (default 1280x800, 16:10)
  --lang <en|ja>           language (ja = the original art; env SHUPPATSU_LANG; remembered in
                           config.toml: on the first start the game asks once)
  --font <files>           TTF/OTF fonts for translated text, primary first, :-separated
                           fallbacks (env SHUPPATSU_FONT; default: bundled M+ 2 Medium)
  --display-font <files>   the same for display text: headers, menu titles, banners
                           (env SHUPPATSU_DISPLAY_FONT; default the --font list)
  --locale-dir <dir>       extra catalogue directory, <dir>/<lang>.toml (env SHUPPATSU_LOCALE_DIR)
  --texture-pack <dir>     replacement 3D texture pages (env SHUPPATSU_TEXTURE_PACK; default: the
                           original textures; docs/texture-packs.md, shuppatsu-tools upscale-textures)
  --hud-layout <original|enhanced>
                           2D layout (env SHUPPATSU_HUD_LAYOUT; default original = the 4:3 design);
                           enhanced: driving HUD at the window's edges, wider station plates
                           (bilingual with a translation)
  --ui-scaling <smooth|integer|nearest>
                           how the 2D layer reaches the window (env SHUPPATSU_UI_SCALING): smooth =
                           sharp bilinear (default), integer = largest whole multiple, centred,
                           nearest = plain nearest neighbour (uneven pixels)
  --ui-scale <1..4>        fixed 2D canvas multiple (640x480 x n; env SHUPPATSU_UI_SCALE); default:
                           follows the window height (smooth: ceil(h/480), integer: floor(h/480))
  --draw-distance <original|2x|3x|max>
                           the 3D view's far plane (env SHUPPATSU_DRAW_DISTANCE): original 320 m,
                           2x (default), 3x, max 1280 m; scenery and track are built out to it
  --audio-resampler <linear|nearest|sinc>
                           how each sound is resampled to the device rate (env
                           SHUPPATSU_AUDIO_RESAMPLER): sinc (default) = band-limited, no aliasing or
                           images; linear or nearest = the candidates for DirectSound's own
  --movie-scaling <smooth|sharp|nearest>
                           the Komachi movie's filter (env SHUPPATSU_MOVIE_SCALING): smooth =
                           bilinear (default), sharp = sharp bilinear like the UI, nearest
  --hint-device <pad|keyboard>
                           the button hints shown with hybrid input, fixed (env
                           SHUPPATSU_HINT_DEVICE; headless shots have no pad to follow)
  --inputs <script>        scripted keys, notches and gamepad (docs/marks.md, \"Input scripts\")
  --save <eeprom.dat>      start from this save, read only (an issue mark's replay)
  --global-tick <n>        play: the global tick counter at the start (an issue mark's replay)

switches (docs/settings.md): --<name> turns one on, --no-<name> off; config.toml key <name> with
  _ for -, env SHUPPATSU_<NAME> = 1/0. Off is the original's behaviour. [default]
  --wide-visibility        scenery for the window's wider view [on] (--original-visibility)
  --square-pixels          no 7.9 % horizontal stretch of the 3D view [off]
  --stable-track-variants  side-track models don't switch every point (no flicker) [off]
  --texture-filtering      mipmaps + trilinear filtering (no shimmer; blurs close-ups) [off]
  --ground-variation       soft brightness/tint variation on the ground [off]
  --ground-detiling        ground textures without a regular tile grid [off]
  --anisotropic <1|2|4|8|16>
                           anisotropic filtering with mipmaps, close-ups stay sharp
                           (env SHUPPATSU_ANISOTROPIC; 1 or --no-anisotropic = off) [off]
  --msaa <1|2|4|8>         multisampled 3D view, smooth cut-out edges (env SHUPPATSU_MSAA;
                           1 or --no-msaa = off) [off]
  --fog                    the snow fog of DengoSetting's FOGENABLE [on]
  --wide-weather           raindrops and snow over the whole width [on] (--original-weather)
  --wide-coupling-band     the coupling game's black band over the whole width [on]
  --far-traffic            AI trains drawn to the far plane, not 150 m [on] (--original-traffic)
  --max-lod                track and trains in their most detailed models at any distance [on]
  --even-track-width       rails and ballast of even width, no steps at the joins [on]
  --early-activation       scenery and side tracks activated at the far plane [on]
  --distance-fade          the far edge dissolves into the background [on]
  --fine-depth             depth steps that stay fine with distance (16-bit Z: off) [on]
  --interpolate            camera, trains, wiper, snow, needle between ticks [on]
  --vsync                  present with vsync [on] (--uncapped)
  --fullscreen             borderless full screen; Alt+Enter or F11 switch it [off]
  --wide-menus             menu backdrops to the edges of a wide window [on]
  --hud-upscale            HUD and menu art enlarged with xBRZ edges, not pixel blocks [off]
  --one-line-captions      translation: captions and banners as one line [on]
  --staff-kanji            translation: kanji after the staff roll's names [on]
  --landmark-names         translation: names under the landmark plates [on]
  --board-names            translation: romaji under the route board's kanji [on]
  --subtitles              translation: English subtitles for the Japanese voices [on]
  --save-on-change         eeprom.dat written after every change, not only at exit [on]
  --save-timed-out-names   a timed-out name entry is saved [on]
  --hybrid-input           keyboard and controller at once in every control mode, hints
                           follow the last device used [on]
  --keyboard-with-controller
                           hybrid input off: keyboard horn and ATS confirm in controller
                           modes [on]
  --exit-no-to-menu        exit dialog No returns to the title menu, not attract [on]
  --movie-in-menu          the Komachi movie as item 6 of the title menu [on]
  --unique-routes          route select lists each stage of a group once [on]

settings file: $XDG_CONFIG_HOME/shuppatsu/config.toml (~/.config/shuppatsu; %APPDATA%\\shuppatsu on Windows;
  copied once from the old ddg folder if only that exists),
  keys lang, hud_layout, ui_scaling, ui_scale, font, display_font, texture_pack and the
  switches above (docs/settings.md, docs/localisation.md). Flags override the environment,
  which overrides the file. Headless --shot runs never read it.

play keys (mode 0): Down/Up power +/-, Right/Left brake +/- (one more press past B8 = EB),
  Space horn, Enter ATS confirm / panel / menu confirm, Esc pause / cancel, Up/Down menu cursor
pad (hybrid input, Xbox layout): d-pad or left stick down/up power +/-, d-pad right/left or
  right stick up/down brake +/-, LT/LB power +/-, RT/RB brake +/-, A ATS confirm / panel /
  confirm, B cancel, X horn, Y R (auto run), Menu pause, View issue mark
issue mark (docs/marks.md): F12, or the pad's View button, saves the frame, the state
  and the inputs so far under $XDG_DATA_HOME/shuppatsu/marks (%APPDATA%\\shuppatsu\\marks)
view keys: Space pause, Up/Down speed, Left/Right jump 100 m, L track lines, F fog, Esc quit";

/// The original's fixed tick (timeSetEvent(33, …) at 0x436f30).
const TICK: Duration = Duration::from_millis(33);
/// Track units per tick per km/h (scene.md §3: `speed × 0.000925`).
const UNITS_PER_KMH_TICK: f32 = 0.000925;
/// Metres per track unit (docs/re/routes.md).
const METRES_PER_UNIT: f32 = shuppatsu_formats::route::METRES_PER_UNIT;
/// Visibility is widened for at least this aspect, so resizing up to 21:9 needs no rebuild.
const MIN_WIDE_ASPECT: f32 = 21.0 / 9.0;

struct Options {
    shot: bool,
    at_m: f32,
    out: Option<std::path::PathBuf>,
    route: String,
    speed_kmh: f32,
    settings: settings::Settings,
    /// `view`: linear fog everywhere (`--fog`; the `fog` setting is the snow fog of play).
    fog: bool,
    hud: bool,
    gradient_variant: usize,
    size: (u32, u32),
    locale: locale::LocaleConfig,
}

fn parse_play_args() -> Result<play::PlayOptions> {
    let mut args: Vec<String> = cli_args().into_iter().skip(2).collect();
    let headless = args.iter().any(|a| a == "--shot");
    let (locale, settings) = config::load(&mut args, headless)?;
    identity::take_debug_title_flag(&mut args);
    let mut opts = play::PlayOptions {
        stage: args
            .first()
            .context(USAGE)?
            .parse()
            .context("stage must be a number")?,
        difficulty: 0,
        difficulty_from_save: true,
        station: 0,
        control_mode: 0,
        pad: pad_options(&settings, headless)?,
        size: (1280, 800),
        settings,
        mute: false,
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
    let mut rest = args[1..].iter();
    while let Some(a) = rest.next() {
        let mut value = |what: &str| {
            rest.next()
                .with_context(|| format!("{a} needs {what}"))
                .cloned()
        };
        match a.as_str() {
            "--difficulty" => {
                opts.difficulty = value("a number")?.parse()?;
                opts.difficulty_from_save = false;
            }
            "--station" => opts.station = value("a number")?.parse()?,
            "--ending" => opts.ending = Some(value("full or plain")? == "full"),
            "--intro" => opts.intro = true,
            "--keys" => opts.control_mode = value("a mode")?.parse()?,
            "--hint-device" => {
                opts.pad.hint_device = Some(hint_device(&value("pad or keyboard")?)?)
            }
            "--joystick-dat" => opts.pad.joystick_dat = Some(value("a file")?.into()),
            "--combo" => opts.pad.combo_table = Some(value("a file")?.into()),
            "--mute" => opts.mute = true,
            "--speedometer" => opts.speedometer = Some(value("0, 1 or 2")?.parse()?),
            "--inputs" => opts.inputs = Some(value("a file")?.into()),
            "--save" => opts.save_file = Some(value("a file")?.into()),
            "--global-tick" => opts.global_tick = Some(value("a number")?.parse()?),
            "--shot" => {
                let s = value("TICKS:FILE")?;
                let (t, f) = s.split_once(':').context("--shot needs TICKS:FILE")?;
                opts.shot = Some((t.parse()?, f.into()));
            }
            "--size" => {
                let s = value("WxH")?;
                let (w, h) = s.split_once('x').context("--size needs WxH")?;
                opts.size = (w.parse()?, h.parse()?);
            }
            other => bail!("unknown option {other}\n\n{USAGE}"),
        }
    }
    opts.locale.fit_height(opts.size.1);
    shot::set_ui_scaling(opts.locale.ui_scaling);
    Ok(opts)
}

/// `shuppatsu [options]`: the whole game ([`app`]).
fn parse_app_args() -> Result<app::AppOptions> {
    let mut args: Vec<String> = cli_args().into_iter().skip(1).collect();
    let headless = args.iter().any(|a| a == "--shot");
    let (locale, settings, locks) = config::load_locked(&mut args, headless)?;
    identity::take_debug_title_flag(&mut args);
    // Hidden: the first-run language screen even when a language is set (headless checks).
    let forced = args.iter().any(|a| a == "--first-run");
    args.retain(|a| a != "--first-run");
    let mut opts = app::AppOptions {
        first_run: config::asks_language(&locale, headless, forced),
        locks,
        size: (1280, 800),
        settings,
        mute: false,
        pad: pad_options(&settings, headless)?,
        inputs: None,
        save_file: None,
        shot: None,
        locale,
    };
    let mut rest = args.iter();
    while let Some(a) = rest.next() {
        let mut value = |what: &str| {
            rest.next()
                .with_context(|| format!("{a} needs {what}"))
                .cloned()
        };
        match a.as_str() {
            "--joystick-dat" => opts.pad.joystick_dat = Some(value("a file")?.into()),
            "--hint-device" => {
                opts.pad.hint_device = Some(hint_device(&value("pad or keyboard")?)?)
            }
            "--combo" => opts.pad.combo_table = Some(value("a file")?.into()),
            "--mute" => opts.mute = true,
            "--inputs" => opts.inputs = Some(value("a file")?.into()),
            "--save" => opts.save_file = Some(value("a file")?.into()),
            "--shot" => {
                let s = value("TICKS:FILE")?;
                let (t, f) = s.split_once(':').context("--shot needs TICKS:FILE")?;
                opts.shot = Some((t.parse()?, f.into()));
            }
            "--size" => {
                let s = value("WxH")?;
                let (w, h) = s.split_once('x').context("--size needs WxH")?;
                opts.size = (w.parse()?, h.parse()?);
            }
            other => bail!("unknown option {other}\n\n{USAGE}"),
        }
    }
    opts.locale.fit_height(opts.size.1);
    shot::set_ui_scaling(opts.locale.ui_scaling);
    Ok(opts)
}

fn parse_args() -> Result<Options> {
    let mut args: Vec<String> = cli_args().into_iter().skip(1).collect();
    // `shot` renders headless: no config file.
    let headless = args.first().map(String::as_str) == Some("shot");
    // In `view`, `--fog` is fog everywhere, not the snow fog setting (which it also sets).
    let view_fog = args.iter().any(|a| a == "--fog");
    let (locale, settings) = config::load(&mut args, headless)?;
    identity::take_debug_title_flag(&mut args);
    let shot = match args.first().map(String::as_str) {
        Some("view") => false,
        Some("shot") => true,
        _ => bail!("{USAGE}"),
    };
    let mut opts = Options {
        shot,
        at_m: 0.0,
        out: None,
        route: args.get(1).context(USAGE)?.clone(),
        speed_kmh: 80.0,
        settings,
        fog: view_fog,
        hud: false,
        gradient_variant: 0,
        size: (1280, 800),
        locale,
    };
    let mut rest = args[2..].iter();
    while let Some(a) = rest.next() {
        match a.as_str() {
            "--speed" => opts.speed_kmh = rest.next().context("--speed needs km/h")?.parse()?,
            "--at" => opts.at_m = rest.next().context("--at needs metres")?.parse()?,
            "--out" => opts.out = Some(rest.next().context("--out needs a path")?.into()),
            "--hud" => opts.hud = true,
            "--gradient-variant" => {
                opts.gradient_variant = rest
                    .next()
                    .context("--gradient-variant needs 0 or 1")?
                    .parse()?
            }
            "--size" => {
                let s = rest.next().context("--size needs WxH")?;
                let (w, h) = s.split_once('x').context("--size needs WxH")?;
                opts.size = (w.parse()?, h.parse()?);
            }
            other => bail!("unknown option {other}\n\n{USAGE}"),
        }
    }
    opts.locale.fit_height(opts.size.1);
    shot::set_ui_scaling(opts.locale.ui_scaling);
    Ok(opts)
}

/// Converts a distance along the main line (track units from the camera's base point) into the
/// original's engine index + fraction.
fn track_pos(units: f32) -> TrackPos {
    let engine_index = (units / UNITS_PER_POINT).floor().max(0.0) as usize;
    TrackPos {
        engine_index,
        t: units - engine_index as f32 * UNITS_PER_POINT,
    }
}

fn configure_renderer(renderer: &mut Renderer, opts: &Options) {
    renderer.texture_filtering = opts.settings.texture_filtering;
    renderer.shading = opts.settings.shading();
    // The original reads FOGENABLE from the registry; with no key (fresh install, never run
    // DengoSetting) it parses as 0, so fog is off.
    renderer.fog.enabled = opts.fog;
    renderer.projection.fov_tenths = (DRIVING_FOV_Y_DEG * 10.0) as u16;
    renderer.projection.stretch = if opts.settings.square_pixels {
        1.0
    } else {
        ORIGINAL_STRETCH
    };
    geometry(opts).configure(renderer);
}

/// The geometry settings of a `view` / `shot` run.
fn geometry(opts: &Options) -> scene::Geometry {
    scene::Geometry {
        max_lod: opts.settings.max_lod,
        even_track_width: opts.settings.even_track_width,
        early_activation: opts.settings.early_activation,
        draw_distance: opts.locale.draw_distance,
        distance_fade: opts.settings.distance_fade,
    }
}

struct App {
    opts: Options,
    scene: RouteScene,
    frame: FrameGeometry,
    gpu: Option<Gpu>,
    /// Simulation state: distance along the main line in track units, previous and current tick.
    prev: f32,
    cur: f32,
    limits: (f32, f32),
    accumulator: Duration,
    last_frame: Instant,
    paused: bool,
    speed_kmh: f32,
    show_lines: bool,
    hud: Option<HudView>,
    frames: u32,
    frame_window_start: Instant,
    worst_frame: Duration,
    worst_compose: Duration,
}

use play::HUD_BAND_TOP;

/// The HUD for the viewer, driven by test values (the fly-through speed and a running clock).
struct HudView {
    /// For rasterising the translated text again when the canvas multiple changes.
    data: scene::GameData,
    locale: locale::LocaleConfig,
    bank: shuppatsu_render::sprites::SpriteBank,
    layer: shuppatsu_render::sprites::SpriteLayer,
    hud: hud::Hud,
    state: hud::HudState,
    canvas: shuppatsu_render::sprites::Composer,
}

impl HudView {
    /// `upscale`: the `hud_upscale` setting (xBRZ-shaped sprite art), applied as `App`/`Play` do.
    fn load(data: &scene::GameData, locale: &locale::LocaleConfig, upscale: bool) -> Result<Self> {
        use shuppatsu_formats::cg;
        use shuppatsu_render::sprites::{SpriteBank, SpriteLayer};
        let exe_bytes = data.exe()?;
        let exe = shuppatsu_formats::exe::Exe::parse(&exe_bytes)?;
        let cg_bytes = data.read("cg.rom")?;
        let mut bank = SpriteBank::new(
            &cg::CgFile::parse(&cg_bytes)?,
            cg::parse_tbl(&data.read("tbl.rom")?)?,
        );
        locale::install(locale, &exe, &mut bank)?;
        bank.set_upscale(upscale);
        let mut layer = SpriteLayer {
            scale: locale.ui_scale,
            ..SpriteLayer::default()
        };
        let canvas = shuppatsu_render::sprites::Composer::blank(layer.canvas_size());
        hud::Hud::install_palettes(&exe, &mut layer)?;
        let hud = hud::Hud::new(&exe, 0)?;
        hud.create(&mut layer);
        hud::anchor_slots(&mut layer);
        let state = hud::HudState {
            remaining_s: 30,
            clock: 10 * 108_000 + 30 * 1800,
            scheduled: 10 * 108_000 + 32 * 1800,
            clocks_visible: true,
            remaining_visible: true,
            panel_active: true,
            panel_on: true,
            next_is_stop: true,
            dist_m: 1234.5,
            dist_round_m: 1235,
            tolerance_m: 5,
            train_driving: true,
            speed_mode: 1,
            ..Default::default()
        };
        Ok(Self {
            data: scene::GameData::from_env()?,
            locale: locale.clone(),
            bank,
            layer,
            hud,
            state,
            canvas,
        })
    }

    /// Follows a `window`: the canvas multiple and, in the enhanced layout, its shape.
    fn fit(&mut self, window: (u32, u32)) -> Result<()> {
        let wide = self.locale.enhanced_hud();
        let resized = presentation::fit_layer(
            &mut self.locale,
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

    /// The 3D view's top edge in a `window` (see `Play::band_top`).
    fn band_top(&self, (w, h): (u32, u32)) -> f32 {
        let rect = shuppatsu_render::overlay::present_rect(
            w,
            h,
            self.layer.canvas_size(),
            self.locale.ui_scaling,
        );
        presentation::band_top(rect, h, HUD_BAND_TOP)
    }

    fn tick(&mut self, speed_kmh: f32, paused: bool) {
        let s = &mut self.state;
        s.speed_kmh = speed_kmh;
        s.power = if paused { 0 } else { 3 };
        s.brake = if paused { 9 } else { 0 };
        s.hud_counter += 1;
        if !paused {
            s.clock += 1;
            s.dist_m -= speed_kmh / 3.6 / 30.0;
            s.dist_round_m = s.dist_m.round() as i32;
        }
        self.layer.reset_transient();
        self.hud.update(s, &mut self.layer);
        self.canvas.compose(&self.layer, &self.bank, None);
    }
}

pub(crate) use window::block_on;

impl App {
    fn init_gpu(&mut self, event_loop: &ActiveEventLoop) -> Result<Gpu> {
        let mut gpu = Gpu::create(
            event_loop,
            "shuppatsu view",
            self.opts.size,
            !self.opts.settings.vsync,
            self.opts.settings.fullscreen,
            self.opts.locale.ui_scale,
        )?;
        configure_renderer(&mut gpu.renderer, &self.opts);
        gpu.renderer
            .upload_pages(&gpu.device, &gpu.queue, &self.scene.library.pages);
        if self.hud.is_some() {
            gpu.renderer.band_top = HUD_BAND_TOP;
        }
        Ok(gpu)
    }

    fn tick(&mut self) {
        self.prev = self.cur;
        if !self.paused {
            self.cur = (self.cur + self.speed_kmh * UNITS_PER_KMH_TICK)
                .clamp(self.limits.0, self.limits.1);
        }
        if let Some(h) = self.hud.as_mut() {
            h.tick(self.speed_kmh, self.paused);
        }
    }

    fn jump(&mut self, metres: f32) {
        let u = (self.cur + metres / METRES_PER_UNIT).clamp(self.limits.0, self.limits.1);
        self.cur = u;
        self.prev = u;
    }

    fn redraw(&mut self) {
        let now = Instant::now();
        let dt = now - self.last_frame;
        self.last_frame = now;
        self.worst_frame = self.worst_frame.max(dt);
        self.accumulator += dt.min(Duration::from_millis(250));
        while self.accumulator >= TICK {
            self.accumulator -= TICK;
            self.tick();
        }
        // Interpolate the distance between the last two ticks (setting `interpolate`; off: the
        // last tick), then evaluate the original's camera on the track at that distance.
        let alpha = if self.opts.settings.interpolate {
            self.accumulator.as_secs_f32() / TICK.as_secs_f32()
        } else {
            1.0
        };
        let pos = track_pos(self.prev + (self.cur - self.prev) * alpha);
        let camera = self.scene.cab_camera(pos);
        let t = Instant::now();
        if let (Some(h), Some(g)) = (self.hud.as_mut(), self.gpu.as_ref())
            && let Err(e) = h.fit(g.size())
        {
            eprintln!("ui: {e:#}");
        }
        let band_top = match (&self.hud, &self.gpu) {
            (Some(h), Some(g)) => h.band_top(g.size()),
            (Some(_), None) => HUD_BAND_TOP,
            (None, _) => 0.0,
        };
        let aspect = self.gpu.as_ref().map_or(1.6, |g| {
            g.config.width as f32 / (g.config.height.max(1) as f32 * (1.0 - band_top))
        });
        self.scene
            .compose(&mut self.frame, pos, self.show_lines, aspect);
        self.worst_compose = self.worst_compose.max(t.elapsed());

        let Some(gpu) = self.gpu.as_mut() else { return };
        let Some(frame) = gpu.acquire() else { return };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        gpu.renderer.show_lines = self.show_lines;
        if self.hud.is_some() {
            gpu.renderer.band_top = band_top;
        }
        gpu.renderer
            .set_geometry(&gpu.device, &gpu.queue, &self.frame);
        gpu.renderer.render(
            &gpu.device,
            &gpu.queue,
            &view,
            (gpu.config.width, gpu.config.height),
            &camera,
        );
        if let Some(h) = self.hud.as_mut() {
            gpu.overlay.resize(&gpu.device, h.layer.canvas_size());
            gpu.overlay.sync(&gpu.queue, &mut h.canvas);
            gpu.overlay.scaling = h.locale.ui_scaling;
            let rect = gpu.overlay.rect(gpu.config.width, gpu.config.height);
            gpu.overlay.render(&gpu.device, &gpu.queue, &view, rect);
        }
        gpu.window.pre_present_notify();
        gpu.queue.present(frame);

        self.frames += 1;
        let elapsed = now - self.frame_window_start;
        if elapsed >= Duration::from_secs(1) {
            let fps = self.frames as f32 / elapsed.as_secs_f32();
            let km = self.cur * METRES_PER_UNIT / 1000.0;
            identity::set_debug_title(
                &gpu.window,
                &format!(
                    "shuppatsu view {} | {fps:.0} fps, worst {:.1} ms (compose {:.2} ms) | {} tris | {km:.2} km at {:.0} km/h{}",
                    self.opts.route,
                    self.worst_frame.as_secs_f32() * 1000.0,
                    self.worst_compose.as_secs_f32() * 1000.0,
                    self.frame.triangle_count(),
                    self.speed_kmh,
                    if self.paused { " (paused)" } else { "" }
                ),
            );
            self.frames = 0;
            self.frame_window_start = now;
            self.worst_frame = Duration::ZERO;
            self.worst_compose = Duration::ZERO;
        }
    }

    fn key(&mut self, event_loop: &ActiveEventLoop, code: KeyCode) {
        match code {
            KeyCode::Escape => event_loop.exit(),
            KeyCode::Space => self.paused = !self.paused,
            KeyCode::ArrowUp => self.speed_kmh += 10.0,
            KeyCode::ArrowDown => self.speed_kmh = (self.speed_kmh - 10.0).max(0.0),
            KeyCode::ArrowRight => self.jump(100.0),
            KeyCode::ArrowLeft => self.jump(-100.0),
            KeyCode::KeyL => self.show_lines = !self.show_lines,
            KeyCode::KeyF => {
                if let Some(g) = self.gpu.as_mut() {
                    g.renderer.fog.enabled = !g.renderer.fog.enabled;
                }
            }
            _ => {}
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        match self.init_gpu(event_loop) {
            Ok(gpu) => {
                gpu.window.request_redraw();
                self.gpu = Some(gpu);
                self.last_frame = Instant::now();
            }
            Err(e) => {
                eprintln!("GPU init failed: {e:#}");
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // Alt+Enter / F11: full screen for this viewer only (not remembered).
        if self
            .gpu
            .as_mut()
            .and_then(|g| g.fullscreen_key(&event))
            .is_some()
        {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(g) = self.gpu.as_mut() {
                    g.resize(size.width, size.height);
                }
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state: ElementState::Pressed,
                        ..
                    },
                ..
            } => self.key(event_loop, code),
            WindowEvent::RedrawRequested => {
                self.redraw();
                if let Some(g) = &self.gpu {
                    g.window.request_redraw();
                }
            }
            _ => {}
        }
    }
}

fn run_main() -> Result<()> {
    let first = cli_args().into_iter().nth(1);
    if matches!(first.as_deref(), Some("-h" | "--help")) {
        println!("{USAGE}");
        return Ok(());
    }
    if first.as_deref().is_none_or(|a| a.starts_with("--")) {
        return app::run(parse_app_args()?);
    }
    if cli_args().into_iter().nth(1).as_deref() == Some("play") {
        return play::run(parse_play_args()?);
    }
    if cli_args().into_iter().nth(1).as_deref() == Some("install-desktop") {
        let args: Vec<String> = cli_args().into_iter().skip(2).collect();
        return desktop::run(&args);
    }
    if cli_args().into_iter().nth(1).as_deref() == Some("label-audit") {
        let args: Vec<String> = cli_args().into_iter().skip(2).collect();
        return label_audit::run(&args);
    }
    if cli_args().into_iter().nth(1).as_deref() == Some("pad") {
        let data = scene::GameData::from_env()?;
        let exe = data.exe()?;
        let args: Vec<String> = cli_args().into_iter().skip(2).collect();
        return gamepad::run_tool(&shuppatsu_formats::exe::Exe::parse(&exe)?, &args);
    }
    let opts = parse_args()?;
    let mut data = scene::GameData::from_env()?;
    data.texture_pack = opts.locale.texture_pack.clone();

    // Visibility: the original lists, or widened for the wider of the window and 21:9.
    let visibility = if !opts.settings.wide_visibility {
        Visibility::Original
    } else {
        let mut p = shuppatsu_render::Projection {
            fov_tenths: (DRIVING_FOV_Y_DEG * 10.0) as u16,
            ..Default::default()
        };
        if opts.settings.square_pixels {
            p.stretch = 1.0;
        }
        let aspect = (opts.size.0 as f32 / opts.size.1 as f32).max(MIN_WIDE_ASPECT);
        Visibility::Wide {
            tan_half_fov_x: p.tan_half_horizontal(aspect),
        }
    };
    let t = Instant::now();
    let mut scene = RouteScene::load(&data, &opts.route, visibility)?;
    scene.stable_track_variants = opts.settings.stable_track_variants;
    scene.set_geometry(geometry(&opts));
    scene.gradient_variant = opts.gradient_variant;
    eprintln!(
        "{}: area {}, COM {}, eye height {:.3}, {} placements, visibility {:?}, loaded in {:.2?}",
        opts.route,
        scene.ctx.area,
        scene.ctx.com,
        scene.eye_height,
        scene.route.placements.len() - 1,
        visibility,
        t.elapsed()
    );
    let (min, max) = scene.limits();
    let limits = (min as f32 * UNITS_PER_POINT, max as f32 * UNITS_PER_POINT);
    // Start at the first stop of the segment, like a run start.
    let start = scene
        .route
        .stops
        .first()
        .map_or(limits.0, |&s| s.saturating_sub(1) as f32 * UNITS_PER_POINT)
        .clamp(limits.0, limits.1);

    if opts.shot {
        let out = opts.out.clone().context("shot needs --out <file.ppm>")?;
        let units = (start + opts.at_m / METRES_PER_UNIT).clamp(limits.0, limits.1);
        let mut hud = if opts.hud {
            Some(HudView::load(
                &data,
                &opts.locale,
                opts.settings.hud_upscale,
            )?)
        } else {
            None
        };
        if let Some(h) = hud.as_mut() {
            h.fit(opts.size)?;
            h.tick(opts.speed_kmh, false);
        }
        let band = hud.as_ref().map_or(HUD_BAND_TOP, |h| h.band_top(opts.size));
        shot::render_to_ppm(
            Some((&scene, track_pos(units))),
            opts.size,
            |r| {
                configure_renderer(r, &opts);
                if opts.hud {
                    r.band_top = band;
                }
            },
            hud.as_ref()
                .map(|h| (h.canvas.rgba(), h.layer.canvas_size())),
            &out,
        )?;
        eprintln!("wrote {}", out.display());
        return Ok(());
    }

    let hud = if opts.hud {
        Some(HudView::load(
            &data,
            &opts.locale,
            opts.settings.hud_upscale,
        )?)
    } else {
        None
    };
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let now = Instant::now();
    let mut app = App {
        speed_kmh: opts.speed_kmh,
        opts,
        scene,
        frame: FrameGeometry::default(),
        gpu: None,
        prev: start,
        cur: start,
        limits,
        accumulator: Duration::ZERO,
        last_frame: now,
        paused: false,
        show_lines: false,
        hud,
        frames: 0,
        frame_window_start: now,
        worst_frame: Duration::ZERO,
        worst_compose: Duration::ZERO,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

/// The command line without `--data <dir>` (data_dir.rs) and `--console` (win_console.rs).
fn cli_args() -> Vec<String> {
    data_dir::args()
        .into_iter()
        .filter(|a| a != "--console")
        .collect()
}

fn main() -> Result<()> {
    let log = win_console::init(std::env::args().any(|a| a == "--console"));
    let result = run_main();
    if let Err(e) = &result {
        win_console::fatal(e, log.as_deref());
    }
    result
}

/// The input options from the settings: headless runs open no gamepad (they stay
/// deterministic); `SHUPPATSU_HINT_DEVICE` fixes the hint device (`--hint-device` overrides it).
fn pad_options(settings: &settings::Settings, headless: bool) -> Result<gamepad::PadOptions> {
    let hint = match std::env::var("SHUPPATSU_HINT_DEVICE") {
        Ok(v) if !v.is_empty() => Some(hint_device(&v)?),
        _ => None,
    };
    Ok(gamepad::PadOptions {
        keyboard_with_controller: settings.keyboard_with_controller,
        hybrid_input: settings.hybrid_input,
        no_pad: headless,
        hint_device: hint,
        ..Default::default()
    })
}

fn hint_device(v: &str) -> Result<hybrid::HintDevice> {
    hybrid::HintDevice::parse(v).with_context(|| format!("hint device {v:?}: pad or keyboard"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The viewer's HUD preview honours `hud_upscale` as the game does.
    #[test]
    fn hud_view_applies_hud_upscale() {
        let Ok(data) = scene::GameData::from_env() else {
            eprintln!("skipped: SHUPPATSU_DATA_DIR not set");
            return;
        };
        let locale = locale::LocaleConfig::default();
        for on in [false, true] {
            let mut h = HudView::load(&data, &locale, on).unwrap();
            h.fit((2560, 1440)).unwrap();
            h.tick(80.0, false);
            assert_eq!(h.bank.upscale(), on);
            assert_eq!(h.bank.upscale_builds() > 0, on);
        }
    }
}

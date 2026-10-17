//! Densha de GO! 2 Kōsoku-hen 3000-bandai reimplementation.
//!
//! Phase 3: `ddg-game view <route.com>` flies through a route; `ddg-game shot` renders one frame
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
mod gpu_backend;
mod hints;
mod hud;
mod identity;
mod input;
mod locale;
mod navigator;
mod play;
mod popups;
mod presentation;
mod result_view;
mod save;
mod scene;
mod screens;
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
use ddg_formats::route::UNITS_PER_POINT;
use ddg_formats::scene::{DRIVING_FOV_Y_DEG, TrackPos};
use ddg_render::camera::ORIGINAL_STRETCH;
use ddg_render::{FrameGeometry, Renderer};
use scene::{RouteScene, Visibility};
use std::time::{Duration, Instant};
use window::Gpu;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::WindowId;

const USAGE: &str = "\
usage: ddg-game [options] [--joystick-dat <file>] [--combo <file>] [--mute]
                [--inputs <script>] [--shot <ticks>:<file.ppm>]
       ddg-game play <stage 0..19> [--difficulty <0..3, default: the save's>] [--station <n>] [--keys <0..6>] [options]
                     [--joystick-dat <file>] [--combo <file>]
                     [--inputs <script>] [--shot <ticks>:<file.ppm>] [--speedometer <0|1|2>]
       ddg-game view <route.com> [options]
       ddg-game shot <route.com> --at <metres> --out <file.ppm> [options]
       ddg-game pad <monitor|calibrate|learn> [--mode <4..6>] [--joystick-dat <file>] [--combo <file>]
       ddg-game install-desktop [--exec <path>] [--exe <dengo3k.exe>] [--uninstall]
                     desktop entry + icon under $XDG_DATA_HOME (opt-in; see --help)

options:
  --data <dir>             the extracted disc or installed game (after env DDG_DATA_DIR; else
                           data\\ next to the exe, else config.toml data_dir; docs/building.md)
  --console                Windows: log to the starting shell's console or a new one, not to
                           %APPDATA%\\ddg\\ddg-game.log
  --speed <km/h>           fly-through speed (default 80)
  --gradient-variant <0|1> sky/ground gradient set (1 = the timed alternate set)
  --fog                    view: linear fog everywhere (off by default); elsewhere the switch
  --hud                    the driving HUD over the view (test values)
  --debug-title            live debug text in the window title (env DDG_DEBUG_TITLE=1)
  --size <WxH>             window/frame size (default 1280x800, 16:10)
  --lang <en|ja>           language (ja = the original art; env DDG_LANG; remembered in
                           config.toml: on the first start the game asks once)
  --font <files>           TTF/OTF fonts for translated text, primary first, :-separated
                           fallbacks (env DDG_FONT; default: bundled M+ 2 Medium)
  --display-font <files>   the same for display text: headers, menu titles, banners
                           (env DDG_DISPLAY_FONT; default the --font list)
  --locale-dir <dir>       extra catalogue directory, <dir>/<lang>.toml (env DDG_LOCALE_DIR)
  --texture-pack <dir>     replacement 3D texture pages (env DDG_TEXTURE_PACK; default: the
                           original textures; docs/texture-packs.md, ddg-tools upscale-textures)
  --hud-layout <original|enhanced>
                           2D layout (env DDG_HUD_LAYOUT; default original = the 4:3 design);
                           enhanced: driving HUD at the window's edges, wider station plates
                           (bilingual with a translation)
  --ui-scaling <smooth|integer|nearest>
                           how the 2D layer reaches the window (env DDG_UI_SCALING): smooth =
                           sharp bilinear (default), integer = largest whole multiple, centred,
                           nearest = plain nearest neighbour (uneven pixels)
  --ui-scale <1..4>        fixed 2D canvas multiple (640x480 x n; env DDG_UI_SCALE); default:
                           follows the window height (smooth: ceil(h/480), integer: floor(h/480))
  --draw-distance <original|2x|3x|max>
                           the 3D view's far plane (env DDG_DRAW_DISTANCE): original 320 m,
                           2x (default), 3x, max 1280 m; scenery and track are built out to it

switches (docs/settings.md): --<name> turns one on, --no-<name> off; config.toml key <name> with
  _ for -, env DDG_<NAME> = 1/0. Off is the original's behaviour. [default]
  --wide-visibility        scenery for the window's wider view [on] (--original-visibility)
  --square-pixels          no 7.9 % horizontal stretch of the 3D view [off]
  --stable-track-variants  side-track models don't switch every point (no flicker) [off]
  --texture-filtering      mipmaps + trilinear filtering (no shimmer; blurs close-ups) [off]
  --ground-variation       soft brightness/tint variation on the ground [off]
  --ground-detiling        ground textures without a regular tile grid [off]
  --anisotropic <1|2|4|8|16>
                           anisotropic filtering with mipmaps, close-ups stay sharp
                           (env DDG_ANISOTROPIC; 1 or --no-anisotropic = off) [off]
  --msaa <1|2|4|8>         multisampled 3D view, smooth cut-out edges (env DDG_MSAA;
                           1 or --no-msaa = off) [off]
  --fog                    the snow fog of DengoSetting's FOGENABLE [on]
  --wide-weather           raindrops and snow over the whole width [on] (--original-weather)
  --far-traffic            AI trains drawn to the far plane, not 150 m [on] (--original-traffic)
  --max-lod                track and trains in their most detailed models at any distance [on]
  --early-activation       scenery and side tracks activated at the far plane [on]
  --distance-fade          the far edge dissolves into the background [on]
  --interpolate            camera, trains, wiper, snow, needle between ticks [on]
  --vsync                  present with vsync [on] (--uncapped)
  --fullscreen             borderless full screen; Alt+Enter or F11 switch it [off]
  --wide-menus             menu backdrops to the edges of a wide window [on]
  --hud-upscale            HUD and menu art enlarged with xBRZ edges, not pixel blocks [off]
  --one-line-captions      translation: captions and banners as one line [on]
  --landmark-names         translation: names under the landmark plates [on]
  --board-names            translation: romaji under the route board's kanji [on]
  --subtitles              translation: English subtitles for the Japanese voices [on]
  --save-on-change         eeprom.dat written after every change, not only at exit [on]
  --save-timed-out-names   a timed-out name entry is saved [on]
  --keyboard-with-controller
                           keyboard horn and ATS confirm in controller modes [on]
  --exit-no-to-menu        exit dialog No returns to the title menu, not attract [on]

settings file: $XDG_CONFIG_HOME/ddg/config.toml (~/.config/ddg; %APPDATA%\\ddg on Windows),
  keys lang, hud_layout, ui_scaling, ui_scale, font, display_font, texture_pack and the
  switches above (docs/settings.md, docs/localisation.md). Flags override the environment,
  which overrides the file. Headless --shot runs never read it.

play keys (mode 0): Down/Up power +/-, Right/Left brake +/- (one more press past B8 = EB),
  Space horn, Enter ATS confirm / panel / menu confirm, Esc pause / cancel, Up/Down menu cursor
view keys: Space pause, Up/Down speed, Left/Right jump 100 m, L track lines, F fog, Esc quit";

/// The original's fixed tick (timeSetEvent(33, …) at 0x436f30).
const TICK: Duration = Duration::from_millis(33);
/// Track units per tick per km/h (scene.md §3: `speed × 0.000925`).
const UNITS_PER_KMH_TICK: f32 = 0.000925;
/// Metres per track unit (docs/re/routes.md).
const METRES_PER_UNIT: f32 = ddg_formats::route::METRES_PER_UNIT;
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
        pad: gamepad::PadOptions {
            keyboard_with_controller: settings.keyboard_with_controller,
            ..Default::default()
        },
        size: (1280, 800),
        settings,
        mute: false,
        inputs: None,
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
            "--joystick-dat" => opts.pad.joystick_dat = Some(value("a file")?.into()),
            "--combo" => opts.pad.combo_table = Some(value("a file")?.into()),
            "--mute" => opts.mute = true,
            "--speedometer" => opts.speedometer = Some(value("0, 1 or 2")?.parse()?),
            "--inputs" => opts.inputs = Some(value("a file")?.into()),
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

/// `ddg-game [options]`: the whole game ([`app`]).
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
        pad: gamepad::PadOptions {
            keyboard_with_controller: settings.keyboard_with_controller,
            ..Default::default()
        },
        inputs: None,
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
            "--combo" => opts.pad.combo_table = Some(value("a file")?.into()),
            "--mute" => opts.mute = true,
            "--inputs" => opts.inputs = Some(value("a file")?.into()),
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
    bank: ddg_render::sprites::SpriteBank,
    layer: ddg_render::sprites::SpriteLayer,
    hud: hud::Hud,
    state: hud::HudState,
    canvas: Vec<u8>,
    dirty: bool,
}

impl HudView {
    fn load(data: &scene::GameData, locale: &locale::LocaleConfig) -> Result<Self> {
        use ddg_formats::cg;
        use ddg_render::sprites::{SpriteBank, SpriteLayer};
        let exe_bytes = data.exe()?;
        let exe = ddg_formats::exe::Exe::parse(&exe_bytes)?;
        let cg_bytes = data.read("cg.rom")?;
        let mut bank = SpriteBank::new(
            &cg::CgFile::parse(&cg_bytes)?,
            cg::parse_tbl(&data.read("tbl.rom")?)?,
        );
        locale::install(locale, &exe, &mut bank)?;
        let mut layer = SpriteLayer {
            scale: locale.ui_scale,
            ..SpriteLayer::default()
        };
        let canvas = layer.new_canvas();
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
            dirty: true,
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
            self.canvas = self.layer.new_canvas();
            self.layer.compose(&self.bank, &mut self.canvas);
            self.dirty = true;
        }
        Ok(())
    }

    /// The 3D view's top edge in a `window` (see `Play::band_top`).
    fn band_top(&self, (w, h): (u32, u32)) -> f32 {
        let rect = ddg_render::overlay::present_rect(
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
        self.layer.compose(&self.bank, &mut self.canvas);
        self.dirty = true;
    }
}

pub(crate) use window::block_on;

impl App {
    fn init_gpu(&mut self, event_loop: &ActiveEventLoop) -> Result<Gpu> {
        let mut gpu = Gpu::create(
            event_loop,
            "ddg-game view",
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
            let resized = gpu.overlay.resize(&gpu.device, h.layer.canvas_size());
            if std::mem::take(&mut h.dirty) || resized {
                gpu.overlay.upload(&gpu.queue, &h.canvas);
            }
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
                    "ddg-game view {} | {fps:.0} fps, worst {:.1} ms (compose {:.2} ms) | {} tris | {km:.2} km at {:.0} km/h{}",
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
    if cli_args().into_iter().nth(1).as_deref() == Some("pad") {
        let data = scene::GameData::from_env()?;
        let exe = data.exe()?;
        let args: Vec<String> = cli_args().into_iter().skip(2).collect();
        return gamepad::run_tool(&ddg_formats::exe::Exe::parse(&exe)?, &args);
    }
    let opts = parse_args()?;
    let mut data = scene::GameData::from_env()?;
    data.texture_pack = opts.locale.texture_pack.clone();

    // Visibility: the original lists, or widened for the wider of the window and 21:9.
    let visibility = if !opts.settings.wide_visibility {
        Visibility::Original
    } else {
        let mut p = ddg_render::Projection {
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
            Some(HudView::load(&data, &opts.locale)?)
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
                .map(|h| (h.canvas.as_slice(), h.layer.canvas_size())),
            &out,
        )?;
        eprintln!("wrote {}", out.display());
        return Ok(());
    }

    let hud = if opts.hud {
        Some(HudView::load(&data, &opts.locale)?)
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

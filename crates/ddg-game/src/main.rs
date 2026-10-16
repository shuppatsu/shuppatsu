//! Densha de GO! 2 Kōsoku-hen 3000-bandai reimplementation.
//!
//! Phase 3: `ddg-game view <route.com>` flies through a route; `ddg-game shot` renders one frame
//! headless for checks.

mod app;
mod cab;
mod captions;
mod departure_hud;
mod front;
mod gamepad;
mod hints;
mod hud;
mod input;
mod locale;
mod play;
mod result_view;
mod popups;
mod save;
mod scene;
mod screens;
mod shot;
mod slots;
mod sound;
mod traffic;
mod weather;
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
       ddg-game play <stage 0..19> [--difficulty <0..>] [--station <n>] [--keys <0..6>] [options]
                     [--joystick-dat <file>] [--combo <file>]
                     [--inputs <script>] [--shot <ticks>:<file.ppm>] [--speedometer <0|1|2>]
       ddg-game view <route.com> [options]
       ddg-game shot <route.com> --at <metres> --out <file.ppm> [options]
       ddg-game pad <monitor|calibrate|learn> [--mode <4..6>] [--joystick-dat <file>] [--combo <file>]

options:
  --speed <km/h>           fly-through speed (default 80)
  --uncapped               present without vsync
  --square-pixels          disable the original's 7.9 % horizontal stretch
  --original-visibility    the original 4:3 visibility lists, even in a wider window
  --stable-track-variants  enhancement: side-track models don't switch every point (no flicker)
  --texture-filtering      enhancement: mipmaps + trilinear/anisotropic filtering (no shimmer)
  --gradient-variant <0|1> sky/ground gradient set (1 = the timed alternate set)
  --fog                    linear fog as DengoSetting's FOGENABLE (off on a fresh install)
  --hud                    the driving HUD over the view (test values)
  --size <WxH>             window/frame size (default 1280x800, 16:10)
  --lang <code>            text-sprite language (default ja = the original art; env DDG_LANG)
  --font <file>            TTF/OTF font for translated text sprites (env DDG_FONT)
  --locale-dir <dir>       extra catalogue directory, <dir>/<lang>.toml (env DDG_LOCALE_DIR)

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
    uncapped: bool,
    square_pixels: bool,
    original_visibility: bool,
    stable_track_variants: bool,
    texture_filtering: bool,
    fog: bool,
    hud: bool,
    gradient_variant: usize,
    size: (u32, u32),
    locale: locale::LocaleConfig,
}

fn parse_play_args() -> Result<play::PlayOptions> {
    let mut args: Vec<String> = std::env::args().skip(2).collect();
    let mut locale = locale::LocaleConfig::from_env();
    locale.take_args(&mut args)?;
    let mut opts = play::PlayOptions {
        stage: args
            .first()
            .context(USAGE)?
            .parse()
            .context("stage must be a number")?,
        difficulty: 0,
        station: 0,
        control_mode: 0,
        pad: gamepad::PadOptions::default(),
        size: (1280, 800),
        uncapped: false,
        square_pixels: false,
        original_visibility: false,
        stable_track_variants: false,
        texture_filtering: false,
        fog: false,
        mute: false,
        inputs: None,
        shot: None,
        speedometer: None,
        variant: false,
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
            "--difficulty" => opts.difficulty = value("a number")?.parse()?,
            "--station" => opts.station = value("a number")?.parse()?,
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
            "--uncapped" => opts.uncapped = true,
            "--square-pixels" => opts.square_pixels = true,
            "--original-visibility" => opts.original_visibility = true,
            "--stable-track-variants" => opts.stable_track_variants = true,
            "--texture-filtering" => opts.texture_filtering = true,
            "--fog" => opts.fog = true,
            other => bail!("unknown option {other}\n\n{USAGE}"),
        }
    }
    Ok(opts)
}

/// `ddg-game [options]`: the whole game ([`app`]).
fn parse_app_args() -> Result<app::AppOptions> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut locale = locale::LocaleConfig::from_env();
    locale.take_args(&mut args)?;
    let mut opts = app::AppOptions {
        size: (1280, 800),
        uncapped: false,
        square_pixels: false,
        original_visibility: false,
        stable_track_variants: false,
        texture_filtering: false,
        fog: false,
        mute: false,
        pad: gamepad::PadOptions::default(),
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
            "--uncapped" => opts.uncapped = true,
            "--square-pixels" => opts.square_pixels = true,
            "--original-visibility" => opts.original_visibility = true,
            "--stable-track-variants" => opts.stable_track_variants = true,
            "--texture-filtering" => opts.texture_filtering = true,
            "--fog" => opts.fog = true,
            other => bail!("unknown option {other}\n\n{USAGE}"),
        }
    }
    Ok(opts)
}

fn parse_args() -> Result<Options> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut locale = locale::LocaleConfig::from_env();
    locale.take_args(&mut args)?;
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
        uncapped: false,
        square_pixels: false,
        original_visibility: false,
        stable_track_variants: false,
        texture_filtering: false,
        fog: false,
        hud: false,
        gradient_variant: 0,
        size: (1280, 800),
        locale,
    };
    let mut rest = args[2..].iter();
    while let Some(a) = rest.next() {
        match a.as_str() {
            "--speed" => opts.speed_kmh = rest.next().context("--speed needs km/h")?.parse()?,
            "--uncapped" => opts.uncapped = true,
            "--at" => opts.at_m = rest.next().context("--at needs metres")?.parse()?,
            "--out" => opts.out = Some(rest.next().context("--out needs a path")?.into()),
            "--square-pixels" => opts.square_pixels = true,
            "--original-visibility" => opts.original_visibility = true,
            "--stable-track-variants" => opts.stable_track_variants = true,
            "--texture-filtering" => opts.texture_filtering = true,
            "--fog" => opts.fog = true,
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
    renderer.texture_filtering = opts.texture_filtering;
    // The original reads FOGENABLE from the registry; with no key (fresh install, never run
    // DengoSetting) it parses as 0, so fog is off.
    renderer.fog.enabled = opts.fog;
    renderer.projection.fov_tenths = (DRIVING_FOV_Y_DEG * 10.0) as u16;
    renderer.projection.stretch = if opts.square_pixels {
        1.0
    } else {
        ORIGINAL_STRETCH
    };
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
        use ddg_render::sprites::{CANVAS_H, CANVAS_W, SpriteBank, SpriteLayer};
        let exe_bytes = data.exe()?;
        let exe = ddg_formats::exe::Exe::parse(&exe_bytes)?;
        let cg_bytes = data.read("cg.rom")?;
        let mut bank = SpriteBank::new(
            &cg::CgFile::parse(&cg_bytes)?,
            cg::parse_tbl(&data.read("tbl.rom")?)?,
        );
        locale::install(locale, &exe, &mut bank)?;
        let mut layer = SpriteLayer::default();
        hud::Hud::install_palettes(&exe, &mut layer)?;
        let hud = hud::Hud::new(&exe, 0)?;
        hud.create(&mut layer);
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
            bank,
            layer,
            hud,
            state,
            canvas: vec![0; CANVAS_W * CANVAS_H * 4],
            dirty: true,
        })
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
            self.opts.uncapped,
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
        // Interpolate the distance between the last two ticks, then evaluate the original's
        // camera on the track at that distance.
        let alpha = self.accumulator.as_secs_f32() / TICK.as_secs_f32();
        let pos = track_pos(self.prev + (self.cur - self.prev) * alpha);
        let camera = self.scene.cab_camera(pos);
        let t = Instant::now();
        let band_top = if self.hud.is_some() {
            HUD_BAND_TOP
        } else {
            0.0
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
            if std::mem::take(&mut h.dirty) {
                gpu.overlay.upload(&gpu.queue, &h.canvas);
            }
            gpu.overlay.render(
                &gpu.device,
                &gpu.queue,
                &view,
                ddg_render::overlay::canvas_rect(gpu.config.width, gpu.config.height),
            );
        }
        gpu.window.pre_present_notify();
        gpu.queue.present(frame);

        self.frames += 1;
        let elapsed = now - self.frame_window_start;
        if elapsed >= Duration::from_secs(1) {
            let fps = self.frames as f32 / elapsed.as_secs_f32();
            let km = self.cur * METRES_PER_UNIT / 1000.0;
            gpu.window.set_title(&format!(
                "ddg-game view {} | {fps:.0} fps, worst {:.1} ms (compose {:.2} ms) | {} tris | {km:.2} km at {:.0} km/h{}",
                self.opts.route,
                self.worst_frame.as_secs_f32() * 1000.0,
                self.worst_compose.as_secs_f32() * 1000.0,
                self.frame.triangle_count(),
                self.speed_kmh,
                if self.paused { " (paused)" } else { "" }
            ));
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

fn main() -> Result<()> {
    let first = std::env::args().nth(1);
    if first.as_deref().is_none_or(|a| a.starts_with("--")) {
        return app::run(parse_app_args()?);
    }
    if std::env::args().nth(1).as_deref() == Some("play") {
        return play::run(parse_play_args()?);
    }
    if std::env::args().nth(1).as_deref() == Some("pad") {
        let data = scene::GameData::from_env()?;
        let exe = data.exe()?;
        let args: Vec<String> = std::env::args().skip(2).collect();
        return gamepad::run_tool(&ddg_formats::exe::Exe::parse(&exe)?, &args);
    }
    let opts = parse_args()?;
    let data = scene::GameData::from_env()?;

    // Visibility: the original lists, or widened for the wider of the window and 21:9.
    let visibility = if opts.original_visibility {
        Visibility::Original
    } else {
        let mut p = ddg_render::Projection {
            fov_tenths: (DRIVING_FOV_Y_DEG * 10.0) as u16,
            ..Default::default()
        };
        if opts.square_pixels {
            p.stretch = 1.0;
        }
        let aspect = (opts.size.0 as f32 / opts.size.1 as f32).max(MIN_WIDE_ASPECT);
        Visibility::Wide {
            tan_half_fov_x: p.tan_half_horizontal(aspect),
        }
    };
    let t = Instant::now();
    let mut scene = RouteScene::load(&data, &opts.route, visibility)?;
    scene.stable_track_variants = opts.stable_track_variants;
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
            h.tick(opts.speed_kmh, false);
        }
        shot::render_to_ppm(
            Some((&scene, track_pos(units))),
            opts.size,
            |r| {
                configure_renderer(r, &opts);
                if opts.hud {
                    r.band_top = HUD_BAND_TOP;
                }
            },
            hud.as_ref().map(|h| h.canvas.as_slice()),
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

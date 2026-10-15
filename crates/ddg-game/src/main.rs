//! Densha de GO! 2 Kōsoku-hen 3000-bandai reimplementation.
//!
//! Phase 3: `ddg-game view <route.com>` flies through a route; `ddg-game shot` renders one frame
//! headless for checks.

mod scene;
mod shot;

use anyhow::{Context, Result, bail};
use ddg_formats::route::UNITS_PER_POINT;
use ddg_formats::scene::{DRIVING_FOV_Y_DEG, TrackPos};
use ddg_render::camera::ORIGINAL_STRETCH;
use ddg_render::{FrameGeometry, Renderer};
use scene::{RouteScene, Visibility};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

const USAGE: &str = "\
usage: ddg-game view <route.com> [options]
       ddg-game shot <route.com> --at <metres> --out <file.ppm> [options]

options:
  --speed <km/h>           fly-through speed (default 80)
  --uncapped               present without vsync
  --square-pixels          disable the original's 7.9 % horizontal stretch
  --original-visibility    the original 4:3 visibility lists, even in a wider window
  --stable-track-variants  enhancement: side-track models don't switch every point (no flicker)
  --texture-filtering      enhancement: mipmaps + trilinear/anisotropic filtering (no shimmer)
  --size <WxH>             window/frame size (default 1280x800, 16:10)

keys: Space pause, Up/Down speed, Left/Right jump 100 m, L track lines, F fog, Esc quit";

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
    size: (u32, u32),
}

fn parse_args() -> Result<Options> {
    let args: Vec<String> = std::env::args().skip(1).collect();
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
        size: (1280, 800),
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
    renderer.projection.fov_tenths = (DRIVING_FOV_Y_DEG * 10.0) as u16;
    renderer.projection.stretch = if opts.square_pixels {
        1.0
    } else {
        ORIGINAL_STRETCH
    };
}

struct Gpu {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
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
    frames: u32,
    frame_window_start: Instant,
    worst_frame: Duration,
    worst_compose: Duration,
}

/// Minimal executor for wgpu's setup futures (no extra dependency).
pub(crate) fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    let mut fut = std::pin::pin!(fut);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}

impl App {
    fn init_gpu(&mut self, event_loop: &ActiveEventLoop) -> Result<Gpu> {
        let attrs = Window::default_attributes()
            .with_title("ddg-game view")
            .with_inner_size(winit::dpi::PhysicalSize::new(
                self.opts.size.0,
                self.opts.size.1,
            ));
        let window = Arc::new(event_loop.create_window(attrs)?);
        let instance = wgpu::Instance::new(
            wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(window.clone())),
        );
        let surface = instance.create_surface(window.clone())?;
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))?;
        let info = adapter.get_info();
        eprintln!("adapter: {} ({:?})", info.name, info.backend);
        let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .context("surface not supported by adapter")?;
        let caps = surface.get_capabilities(&adapter);
        config.present_mode = if self.opts.uncapped {
            [wgpu::PresentMode::Immediate, wgpu::PresentMode::Mailbox]
                .into_iter()
                .find(|m| caps.present_modes.contains(m))
                .unwrap_or(wgpu::PresentMode::Fifo)
        } else {
            wgpu::PresentMode::Fifo
        };
        // sRGB target so the sRGB-encoded texture data round-trips unchanged.
        if let Some(f) = caps.formats.iter().find(|f| f.is_srgb()) {
            config.format = *f;
        }
        surface.configure(&device, &config);
        let mut renderer = Renderer::new(&device, config.format);
        configure_renderer(&mut renderer, &self.opts);
        renderer.upload_pages(&device, &queue, &self.scene.library.pages);
        eprintln!(
            "present mode {:?}, format {:?}, {}x{}",
            config.present_mode, config.format, config.width, config.height
        );
        Ok(Gpu {
            window,
            surface,
            device,
            queue,
            config,
            renderer,
        })
    }

    fn tick(&mut self) {
        self.prev = self.cur;
        if !self.paused {
            self.cur = (self.cur + self.speed_kmh * UNITS_PER_KMH_TICK)
                .clamp(self.limits.0, self.limits.1);
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
        self.scene.compose(&mut self.frame, pos, self.show_lines);
        self.worst_compose = self.worst_compose.max(t.elapsed());

        let Some(gpu) = self.gpu.as_mut() else { return };
        let frame = match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => f,
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => {
                gpu.surface.configure(&gpu.device, &gpu.config);
                f
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                gpu.surface.configure(&gpu.device, &gpu.config);
                return;
            }
            other => {
                eprintln!("surface: skipping frame ({other:?})");
                return;
            }
        };
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
                    g.config.width = size.width.max(1);
                    g.config.height = size.height.max(1);
                    g.surface.configure(&g.device, &g.config);
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
        shot::render_to_ppm(
            &scene,
            track_pos(units),
            opts.size,
            |r| configure_renderer(r, &opts),
            &out,
        )?;
        eprintln!("wrote {}", out.display());
        return Ok(());
    }

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
        frames: 0,
        frame_window_start: now,
        worst_frame: Duration::ZERO,
        worst_compose: Duration::ZERO,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

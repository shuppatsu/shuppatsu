//! Densha de GO! 2 Kōsoku-hen 3000-bandai reimplementation.
//!
//! Phase 3: `ddg-game view <route.com>` flies through a route.

mod scene;
mod shot;
mod track;

use anyhow::{Context, Result, bail};
use ddg_render::{Camera, Renderer, camera::ORIGINAL_STRETCH};
use std::sync::Arc;
use std::time::{Duration, Instant};
use track::TrackPath;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

const USAGE: &str = "\
usage: ddg-game view <route.com> [options]
       ddg-game shot <route.com> --at <metres> --out <file.ppm> [--size WxH] [--square-pixels]

options:
  --speed <km/h>     fly-through speed (default 80)
  --uncapped         present without vsync
  --square-pixels    disable the original's 7.9 % horizontal stretch
  --size <WxH>       initial window size (default 1280x800, 16:10)

keys: Space pause, Up/Down speed, Left/Right jump 100 m, L track lines, F fog, Esc quit";

/// The original's fixed tick (timeSetEvent(33, …) at 0x436f30).
const TICK: Duration = Duration::from_millis(33);
/// Track point spacing (docs/re/routes.md).
const METRES_PER_POINT: f32 = ddg_formats::route::METRES_PER_POINT;
/// Placeholder cab camera until the scene RE lands: eye height and look-ahead in game units.
const EYE_HEIGHT: f32 = 0.3;
const LOOK_AHEAD_POINTS: f32 = 8.0;

struct Options {
    shot: bool,
    at_m: f32,
    out: Option<std::path::PathBuf>,
    route: String,
    speed_kmh: f32,
    uncapped: bool,
    square_pixels: bool,
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

/// Simulation state: only the distance along the main line for now.
#[derive(Clone, Copy)]
struct Sim {
    /// Distance in track points.
    distance: f32,
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
    scene: scene::RouteScene,
    path: TrackPath,
    gpu: Option<Gpu>,
    prev: Sim,
    cur: Sim,
    accumulator: Duration,
    last_frame: Instant,
    paused: bool,
    speed_kmh: f32,
    frames: u32,
    frame_window_start: Instant,
    worst_frame: Duration,
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
        if self.opts.square_pixels {
            renderer.projection.stretch = 1.0;
        }
        renderer.upload(&device, &queue, &self.scene.world);
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
            let metres = self.speed_kmh / 3.6 * TICK.as_secs_f32();
            self.cur.distance =
                (self.cur.distance + metres / METRES_PER_POINT).min(self.path.len_points());
        }
    }

    fn camera(&self, alpha: f32) -> Camera {
        let d = self.prev.distance + (self.cur.distance - self.prev.distance) * alpha;
        cab_camera(&self.path, d)
    }

    fn jump(&mut self, metres: f32) {
        let d = (self.cur.distance + metres / METRES_PER_POINT).clamp(0.0, self.path.len_points());
        self.cur.distance = d;
        self.prev.distance = d;
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
        let alpha = self.accumulator.as_secs_f32() / TICK.as_secs_f32();
        let camera = self.camera(alpha);
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
        gpu.renderer.render(
            &gpu.device,
            &gpu.queue,
            &view,
            gpu.config.width,
            gpu.config.height,
            &camera,
        );
        gpu.window.pre_present_notify();
        gpu.queue.present(frame);

        self.frames += 1;
        let elapsed = now - self.frame_window_start;
        if elapsed >= Duration::from_secs(1) {
            let fps = self.frames as f32 / elapsed.as_secs_f32();
            let km = self.cur.distance * METRES_PER_POINT / 1000.0;
            gpu.window.set_title(&format!(
                "ddg-game view {} | {fps:.0} fps, worst {:.1} ms | {km:.2} km at {:.0} km/h{}",
                self.opts.route,
                self.worst_frame.as_secs_f32() * 1000.0,
                self.speed_kmh,
                if self.paused { " (paused)" } else { "" }
            ));
            self.frames = 0;
            self.frame_window_start = now;
            self.worst_frame = Duration::ZERO;
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
            KeyCode::KeyL => {
                if let Some(g) = self.gpu.as_mut() {
                    g.renderer.show_lines = !g.renderer.show_lines;
                }
            }
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
    let t = Instant::now();
    let scene = scene::load_route(&data, &opts.route)?;
    let tris: usize = scene
        .world
        .batches
        .iter()
        .map(|b| b.indices.len() / 3)
        .sum();
    eprintln!(
        "{}: area R{}, {} placements ({} missing parts), {tris} triangles, {} track points, loaded in {:.2?}",
        opts.route,
        scene.area,
        scene.placements,
        scene.missing_parts,
        scene.main_line.len(),
        t.elapsed()
    );
    eprintln!(
        "projection: 30.0° vertical, Hor+, stretch {}",
        if opts.square_pixels {
            1.0
        } else {
            ORIGINAL_STRETCH
        }
    );
    let path = TrackPath::new(scene.main_line.clone());
    if opts.shot {
        let out = opts.out.clone().context("shot needs --out <file.ppm>")?;
        let d = (opts.at_m / METRES_PER_POINT).clamp(0.0, path.len_points());
        let camera = cab_camera(&path, d);
        let (w, h) = opts.size;
        shot::render_to_ppm(&scene.world, &camera, w, h, opts.square_pixels, &out)?;
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
        path,
        gpu: None,
        prev: Sim { distance: 0.0 },
        cur: Sim { distance: 0.0 },
        accumulator: Duration::ZERO,
        last_frame: now,
        paused: false,
        frames: 0,
        frame_window_start: now,
        worst_frame: Duration::ZERO,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}

/// Cab camera at `d` track points (placeholder eye height and look-ahead).
fn cab_camera(path: &TrackPath, d: f32) -> Camera {
    let up = glam::Vec3::Y * EYE_HEIGHT;
    let eye = path.at(d) + up;
    let mut target = path.at(d + LOOK_AHEAD_POINTS) + up;
    if (target - eye).length_squared() < 1e-8 {
        target = eye + glam::Vec3::Z;
    }
    Camera { eye, target }
}

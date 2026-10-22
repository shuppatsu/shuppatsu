//! The window, the wgpu device and the surface, shared by `view` and `play`.

use anyhow::{Context, Result};
use shuppatsu_render::Renderer;
use shuppatsu_render::overlay::Overlay;
use std::sync::Arc;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Fullscreen, Window};

pub struct Gpu {
    pub window: Arc<Window>,
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub renderer: Renderer,
    pub overlay: Overlay,
    /// Whether an Alt key is held, for Alt+Enter ([`Gpu::fullscreen_key`]).
    alt: bool,
    /// The mouse pointer over the window ([`Cursor`]).
    cursor: Cursor,
    /// Issue marks (marks.rs): the next presented frame is written here as a PNG.
    pub capture: Option<std::path::PathBuf>,
    /// The surface can be copied from (`COPY_SRC`), for [`Gpu::capture`].
    copyable: bool,
    /// A frame to capture on a surface that cannot be copied from (GL): drawn here instead
    /// ([`Gpu::target`]), read back, then put on the surface ([`Gpu::present`]).
    stand_in: Option<wgpu::Texture>,
    /// A short message in the window's top-right corner and when it goes ([`Gpu::show_toast`]).
    toast: Option<(Overlay, std::time::Instant)>,
    /// Screenshots being written, finished before the program ends.
    writes: Vec<std::thread::JoinHandle<()>>,
}

/// The mouse pointer over the game window. The game never reads the mouse, so the pointer is
/// hidden: always in full screen; in a window it shows while the mouse moves (to reach the
/// title bar) and hides again after [`Cursor::IDLE`] without movement.
#[derive(Debug, Default)]
struct Cursor {
    /// Whether the pointer is currently shown (as last set on the window).
    shown: bool,
    /// When the mouse last moved over the window, while windowed.
    moved: Option<std::time::Instant>,
}

impl Cursor {
    /// How long a still pointer stays visible in a window.
    const IDLE: std::time::Duration = std::time::Duration::from_millis(1500);

    /// Whether the pointer should be shown `now`.
    fn wanted(&self, fullscreen: bool, now: std::time::Instant) -> bool {
        !fullscreen
            && self
                .moved
                .is_some_and(|t| now.duration_since(t) < Self::IDLE)
    }
}

/// Minimal executor for wgpu's setup futures (no extra dependency).
pub fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    let mut fut = std::pin::pin!(fut);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    loop {
        if let std::task::Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}

/// The GPU instance, adapter and device opened on a thread before the window exists (on the
/// event loop's display), so their set-up (130 ms on Vulkan with the system's implicit
/// layers) runs beside the game's own start-up; [`Gpu::with_window_early`] takes them.
pub struct EarlyGpu(std::thread::JoinHandle<Result<crate::gpu_backend::Opened>>);

impl EarlyGpu {
    pub fn start(display: winit::event_loop::OwnedDisplayHandle) -> Self {
        Self(std::thread::spawn(move || {
            crate::gpu_backend::open(
                || {
                    wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(
                        display.clone(),
                    ))
                },
                |_| Ok(None),
            )
        }))
    }
}

impl Gpu {
    /// Opens a window of `size` pixels with an sRGB surface; `uncapped` presents without vsync,
    /// `fullscreen` covers the current monitor borderless (setting `fullscreen`; `size` is then
    /// the windowed size to return to).
    pub fn create(
        event_loop: &ActiveEventLoop,
        title: &str,
        size: (u32, u32),
        uncapped: bool,
        fullscreen: bool,
        ui_scale: usize,
    ) -> Result<Self> {
        let window = Self::open_window(event_loop, title, size, fullscreen)?;
        Self::with_window(window, uncapped, ui_scale)
    }

    /// The window of [`Gpu::create`] alone: its size is known at once (the GPU set-up, tens
    /// of ms, can run beside work that needs it).
    pub fn open_window(
        event_loop: &ActiveEventLoop,
        title: &str,
        size: (u32, u32),
        fullscreen: bool,
    ) -> Result<Arc<Window>> {
        let attrs = Window::default_attributes()
            .with_title(crate::identity::initial_title(title))
            .with_inner_size(winit::dpi::PhysicalSize::new(size.0, size.1))
            .with_fullscreen(fullscreen.then_some(Fullscreen::Borderless(None)));
        let attrs = crate::identity::apply(attrs);
        let window = Arc::new(event_loop.create_window(attrs)?);
        // The game never reads the mouse: the pointer starts hidden ([`Cursor`]).
        window.set_cursor_visible(false);
        Ok(window)
    }

    /// [`Gpu::create`] on a window from [`Gpu::open_window`].
    pub fn with_window(window: Arc<Window>, uncapped: bool, ui_scale: usize) -> Result<Self> {
        Self::with_window_early(window, None, uncapped, ui_scale)
    }

    /// [`Gpu::with_window`] with the device of an [`EarlyGpu`], when it can present to the
    /// window; else (or on its error) the device is opened for the window as usual.
    pub fn with_window_early(
        window: Arc<Window>,
        early: Option<EarlyGpu>,
        uncapped: bool,
        ui_scale: usize,
    ) -> Result<Self> {
        let early = early.and_then(|e| match e.0.join() {
            Ok(Ok(o)) => Some(o),
            Ok(Err(e)) => {
                eprintln!("graphics: early set-up failed ({e:#}); opening for the window");
                None
            }
            Err(_) => None,
        });
        let with_surface = early.and_then(|(instance, _, adapter, device, queue)| {
            let surface = instance.create_surface(window.clone()).ok()?;
            adapter.is_surface_supported(&surface).then_some((
                instance,
                Some(surface),
                adapter,
                device,
                queue,
            ))
        });
        let (_instance, surface, adapter, device, queue) = match with_surface {
            Some(o) => o,
            None => crate::gpu_backend::open(
                || {
                    wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(
                        window.clone(),
                    ))
                },
                |instance| Ok(Some(instance.create_surface(window.clone())?)),
            )?,
        };
        let surface = surface.context("no surface")?;
        let info = adapter.get_info();
        eprintln!("adapter: {} ({:?})", info.name, info.backend);
        let inner = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, inner.width.max(1), inner.height.max(1))
            .context("surface not supported by adapter")?;
        let caps = surface.get_capabilities(&adapter);
        config.present_mode = if uncapped {
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
        // Issue marks read the presented frame back.
        let copyable = caps.usages.contains(wgpu::TextureUsages::COPY_SRC);
        if copyable {
            config.usage |= wgpu::TextureUsages::COPY_SRC;
        }
        surface.configure(&device, &config);
        let renderer = Renderer::new(&device, config.format);
        let overlay = Overlay::with_scale(&device, config.format, ui_scale);
        eprintln!(
            "present mode {:?}, format {:?}, {}x{}",
            config.present_mode, config.format, config.width, config.height
        );
        Ok(Self {
            window,
            surface,
            device,
            queue,
            config,
            renderer,
            overlay,
            alt: false,
            cursor: Cursor::default(),
            capture: None,
            copyable,
            stand_in: None,
            toast: None,
            writes: Vec::new(),
        })
    }

    /// Shows or hides the pointer as [`Cursor::wanted`] says; called once per frame.
    fn update_cursor(&mut self) {
        let want = self
            .cursor
            .wanted(self.is_fullscreen(), std::time::Instant::now());
        if want != self.cursor.shown {
            self.window.set_cursor_visible(want);
            self.cursor.shown = want;
        }
    }

    /// Whether the window covers its monitor.
    pub fn is_fullscreen(&self) -> bool {
        self.window.fullscreen().is_some()
    }

    /// Borderless full screen on the window's monitor, or back to the window.
    pub fn set_fullscreen(&self, on: bool) {
        if on != self.is_fullscreen() {
            self.window
                .set_fullscreen(on.then_some(Fullscreen::Borderless(None)));
        }
    }

    /// Alt+Enter and F11 switch between full screen and the window. `Some` when `event` was
    /// one of them (press, repeat or release): the caller keeps it from the game, so the Enter
    /// of Alt+Enter is not a confirm, and remembers a [`Toggle::Switched`]
    /// ([`remember_fullscreen`]).
    /// It also sees the mouse's moves for the pointer ([`Cursor`]); every window's event
    /// handler calls it first.
    pub fn fullscreen_key(&mut self, event: &WindowEvent) -> Option<Toggle> {
        match event {
            WindowEvent::CursorMoved { .. } => {
                self.cursor.moved = Some(std::time::Instant::now());
                self.update_cursor();
                None
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor.moved = None;
                None
            }
            WindowEvent::ModifiersChanged(m) => {
                self.alt = m.state().alt_key();
                None
            }
            WindowEvent::Focused(false) => {
                self.alt = false;
                None
            }
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state,
                        repeat,
                        ..
                    },
                ..
            } if is_fullscreen_key(*code, self.alt) => {
                if *state != ElementState::Pressed || *repeat {
                    return Some(Toggle::Swallowed);
                }
                let on = !self.is_fullscreen();
                self.set_fullscreen(on);
                Some(Toggle::Switched(on))
            }
            _ => None,
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        // The first Resized repeats the size the surface was configured with; configuring the
        // 4K swap chain again took 28 ms of the first frame.
        if (width, height) == (self.config.width, self.config.height) {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    /// The next frame to draw into, or `None` when this frame should be skipped.
    pub fn acquire(&mut self) -> Option<wgpu::SurfaceTexture> {
        self.update_cursor();
        let _perf = shuppatsu_render::perf::scope(shuppatsu_render::perf::Section::Acquire);
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => Some(f),
            wgpu::CurrentSurfaceTexture::Suboptimal(f) => {
                self.surface.configure(&self.device, &self.config);
                Some(f)
            }
            wgpu::CurrentSurfaceTexture::Outdated => {
                self.surface.configure(&self.device, &self.config);
                None
            }
            other => {
                eprintln!("surface: skipping frame ({other:?})");
                None
            }
        }
    }

    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    /// Presents `frame`. A frame asked for by [`Gpu::capture`] is read back first and written
    /// as a PNG on a thread of its own; the toast ([`Gpu::show_toast`]) is drawn over the
    /// frame after that, so screenshots never show it.
    pub fn present(&mut self, frame: wgpu::SurfaceTexture) {
        if let Some(stand_in) = self.stand_in.take() {
            self.present_stand_in(&stand_in, &frame.texture);
        } else if let Some(path) = self.capture.take() {
            self.save_frame(&frame.texture, path, false);
        }
        self.draw_toast(&frame.texture);
        self.window.pre_present_notify();
        let _perf = shuppatsu_render::perf::scope(shuppatsu_render::perf::Section::Present);
        self.queue.present(frame);
    }

    /// The view to draw `frame` into: the surface's, or for a frame to capture on a surface
    /// that cannot be read back (GL), a texture of the same size and format standing in for
    /// it ([`Gpu::present`] reads it back and puts it on the surface).
    pub fn target(&mut self, frame: &wgpu::SurfaceTexture) -> wgpu::TextureView {
        let texture = if self.capture.is_some() && !self.copyable {
            let t = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("mark frame"),
                size: frame.texture.size(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: frame.texture.format(),
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            self.stand_in.insert(t)
        } else {
            &frame.texture
        };
        texture.create_view(&wgpu::TextureViewDescriptor::default())
    }

    /// Saves the captured frame drawn into `stand_in` and draws it onto `surface`, opaque and
    /// unscaled (the sRGB overlay passes the bytes through unchanged).
    fn present_stand_in(&mut self, stand_in: &wgpu::Texture, surface: &wgpu::Texture) {
        let Some(image) = self
            .capture
            .take()
            .and_then(|path| self.save_frame(stand_in, path, true))
        else {
            return;
        };
        let mut opaque = image.rgba;
        for px in opaque.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        let mut o = Overlay::new(&self.device, self.config.format);
        o.resize(&self.device, (image.width as usize, image.height as usize));
        o.upload(&self.queue, &opaque);
        o.scaling = shuppatsu_render::overlay::UiScaling::Nearest;
        let view = surface.create_view(&wgpu::TextureViewDescriptor::default());
        let rect = [0.0, 0.0, image.width as f32, image.height as f32];
        o.render(&self.device, &self.queue, &view, rect);
    }

    /// Reads `texture` back and writes it to `path` as a PNG on a thread; returns the image
    /// with `keep`.
    fn save_frame(
        &mut self,
        texture: &wgpu::Texture,
        path: std::path::PathBuf,
        keep: bool,
    ) -> Option<shuppatsu_render::rgba::RgbaImage> {
        if !texture.usage().contains(wgpu::TextureUsages::COPY_SRC) {
            eprintln!("mark: this display surface cannot be read back; no screenshot");
            return None;
        }
        let image = match crate::shot::read_rgba(&self.device, &self.queue, texture) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("mark: no screenshot: {e:#}");
                return None;
            }
        };
        let kept = keep.then(|| image.clone());
        self.writes.retain(|h| !h.is_finished());
        self.writes.push(std::thread::spawn(move || {
            let png = crate::desktop::encode_png(&image);
            if let Err(e) = std::fs::write(&path, png) {
                eprintln!("mark: writing {}: {e}", path.display());
            }
        }));
        kept
    }

    /// Shows `image` (premultiplied RGBA) in the window's top-right corner for `seconds`.
    pub fn show_toast(&mut self, image: &shuppatsu_render::rgba::RgbaImage, seconds: f32) {
        let mut o = Overlay::new(&self.device, self.config.format);
        o.resize(&self.device, (image.width as usize, image.height as usize));
        o.upload(&self.queue, &image.rgba);
        o.scaling = shuppatsu_render::overlay::UiScaling::Nearest;
        let until = std::time::Instant::now() + std::time::Duration::from_secs_f32(seconds);
        self.toast = Some((o, until));
    }

    fn draw_toast(&mut self, texture: &wgpu::Texture) {
        if self
            .toast
            .as_ref()
            .is_some_and(|(_, until)| std::time::Instant::now() >= *until)
        {
            self.toast = None;
        }
        let Some((o, _)) = &self.toast else {
            return;
        };
        let (w, h) = o.size();
        let margin = (self.config.height / 60).max(4);
        let x = self.config.width.saturating_sub(w + margin);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let rect = [x as f32, margin as f32, w as f32, h as f32];
        o.render(&self.device, &self.queue, &view, rect);
    }
}

impl Drop for Gpu {
    /// Screenshots still being written are finished first.
    fn drop(&mut self) {
        for h in self.writes.drain(..) {
            let _ = h.join();
        }
    }
}

/// A full-screen key seen by [`Gpu::fullscreen_key`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Toggle {
    /// The press switched the window: full screen now or not.
    Switched(bool),
    /// Its repeat or release, kept from the game too.
    Swallowed,
}

/// F11, or Enter with Alt held.
fn is_fullscreen_key(code: KeyCode, alt: bool) -> bool {
    code == KeyCode::F11 || (alt && matches!(code, KeyCode::Enter | KeyCode::NumpadEnter))
}

/// Writes setting `fullscreen` into `config.toml` after a toggle by key, so the next start
/// opens the same way; not when a flag or `SHUPPATSU_FULLSCREEN` set it for this run (they override
/// the file). Headless runs have no window, so no keys.
pub fn remember_fullscreen(on: bool) {
    let args: Vec<String> = std::env::args().collect();
    let locks = crate::config::locks(|k| std::env::var_os(k), &args);
    if locks.get("fullscreen").is_some() {
        return;
    }
    let Some(path) = crate::config::path() else {
        return;
    };
    if let Err(e) = crate::config::store(&path, "fullscreen", Some(toml::Value::Boolean(on))) {
        eprintln!("warning: fullscreen is not remembered: {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fullscreen_keys() {
        assert!(is_fullscreen_key(KeyCode::F11, false));
        assert!(is_fullscreen_key(KeyCode::F11, true));
        assert!(is_fullscreen_key(KeyCode::Enter, true));
        assert!(is_fullscreen_key(KeyCode::NumpadEnter, true));
        assert!(!is_fullscreen_key(KeyCode::Enter, false));
        assert!(!is_fullscreen_key(KeyCode::KeyF, true));
    }

    #[test]
    fn pointer_shows_only_while_moving_in_a_window() {
        let t0 = std::time::Instant::now();
        let mut c = Cursor::default();
        assert!(!c.wanted(false, t0), "hidden until the mouse moves");
        c.moved = Some(t0);
        assert!(c.wanted(false, t0));
        assert!(c.wanted(false, t0 + Cursor::IDLE / 2));
        assert!(
            !c.wanted(false, t0 + Cursor::IDLE),
            "hidden again when still"
        );
        assert!(!c.wanted(true, t0), "always hidden in full screen");
    }
}

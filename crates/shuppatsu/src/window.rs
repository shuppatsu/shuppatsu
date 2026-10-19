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
        let attrs = Window::default_attributes()
            .with_title(crate::identity::initial_title(title))
            .with_inner_size(winit::dpi::PhysicalSize::new(size.0, size.1))
            .with_fullscreen(fullscreen.then_some(Fullscreen::Borderless(None)));
        let attrs = crate::identity::apply(attrs);
        let window = Arc::new(event_loop.create_window(attrs)?);
        // The game never reads the mouse: the pointer starts hidden ([`Cursor`]).
        window.set_cursor_visible(false);
        let (_instance, surface, adapter, device, queue) = crate::gpu_backend::open(
            || wgpu::InstanceDescriptor::new_with_display_handle_from_env(Box::new(window.clone())),
            |instance| Ok(Some(instance.create_surface(window.clone())?)),
        )?;
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
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    /// The next frame to draw into, or `None` when this frame should be skipped.
    pub fn acquire(&mut self) -> Option<wgpu::SurfaceTexture> {
        self.update_cursor();
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

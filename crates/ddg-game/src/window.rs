//! The window, the wgpu device and the surface, shared by `view` and `play`.

use anyhow::{Context, Result};
use ddg_render::Renderer;
use ddg_render::overlay::Overlay;
use std::sync::Arc;
use winit::event_loop::ActiveEventLoop;
use winit::window::Window;

pub struct Gpu {
    pub window: Arc<Window>,
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub renderer: Renderer,
    pub overlay: Overlay,
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
    /// Opens a window of `size` pixels with an sRGB surface; `uncapped` presents without vsync.
    pub fn create(
        event_loop: &ActiveEventLoop,
        title: &str,
        size: (u32, u32),
        uncapped: bool,
        ui_scale: usize,
    ) -> Result<Self> {
        let attrs = Window::default_attributes()
            .with_title(crate::identity::initial_title(title))
            .with_inner_size(winit::dpi::PhysicalSize::new(size.0, size.1));
        let attrs = crate::identity::apply(attrs);
        let window = Arc::new(event_loop.create_window(attrs)?);
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
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.config.width = width.max(1);
        self.config.height = height.max(1);
        self.surface.configure(&self.device, &self.config);
    }

    /// The next frame to draw into, or `None` when this frame should be skipped.
    pub fn acquire(&mut self) -> Option<wgpu::SurfaceTexture> {
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

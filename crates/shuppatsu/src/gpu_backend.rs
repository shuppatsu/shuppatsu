//! Picks the wgpu backend: DX12 first on Windows with Vulkan (then GL) as the fallback, wgpu's
//! defaults elsewhere (Vulkan on Linux). `WGPU_BACKEND` (wgpu's own variable, e.g. `vulkan`,
//! `dx12`) pins the choice.
//!
//! A backend is skipped when its shader compiler rejects a small textured pipeline. On DX12 naga
//! puts samplers in a shader-model 5.1 resource array; Microsoft's FXC accepts that, but Wine's
//! vkd3d-shader stand-in does not, so under Wine the game falls back to Vulkan by itself.

use crate::block_on;
use anyhow::{Context, Result, bail};

/// What [`open`] returns: the instance, the surface made by the caller (if any), the adapter,
/// the device and its queue.
pub type Opened = (
    wgpu::Instance,
    Option<wgpu::Surface<'static>>,
    wgpu::Adapter,
    wgpu::Device,
    wgpu::Queue,
);

/// The backend sets to try, in order.
fn candidates(from_env: wgpu::Backends) -> Vec<wgpu::Backends> {
    if cfg!(windows) && std::env::var_os("WGPU_BACKEND").is_none() {
        [
            wgpu::Backends::DX12,
            wgpu::Backends::VULKAN,
            wgpu::Backends::GL,
        ]
        .into_iter()
        .filter(|b| from_env.contains(*b))
        .collect()
    } else {
        vec![from_env]
    }
}

/// Opens a device on the first backend that works. `descriptor` builds a fresh instance
/// descriptor (its `backends` is replaced per attempt); `surface` makes the window surface for an
/// instance, or `None` for headless rendering.
pub fn open(
    descriptor: impl Fn() -> wgpu::InstanceDescriptor,
    surface: impl Fn(&wgpu::Instance) -> Result<Option<wgpu::Surface<'static>>>,
) -> Result<Opened> {
    let tries = candidates(descriptor().backends);
    let mut last_error = None;
    for (i, &backends) in tries.iter().enumerate() {
        let is_last = i + 1 == tries.len();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends,
            ..descriptor()
        });
        let attempt = (|| {
            let surface = surface(&instance)?;
            let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: surface.as_ref(),
                ..Default::default()
            }))?;
            // The adapter's own sample counts (MSAA 2 and 8, enhancement `msaa`), where it has
            // them; without the feature only 1 and 4 are allowed.
            let msaa_counts =
                adapter.features() & wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES;
            let (device, queue) = block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                required_features: msaa_counts,
                ..Default::default()
            }))?;
            if !is_last {
                probe(&device).with_context(|| {
                    format!(
                        "{:?} cannot compile the shaders",
                        adapter.get_info().backend
                    )
                })?;
            }
            Ok::<_, anyhow::Error>((surface, adapter, device, queue))
        })();
        match attempt {
            Ok((surface, adapter, device, queue)) => {
                return Ok((instance, surface, adapter, device, queue));
            }
            Err(e) => {
                if !is_last {
                    eprintln!(
                        "graphics: {backends:?} unavailable ({e:#}); trying the next backend"
                    );
                }
                last_error = Some(e);
            }
        }
    }
    match last_error {
        Some(e) => Err(e),
        None => bail!("no graphics backend enabled (WGPU_BACKEND?)"),
    }
}

/// Builds a textured pipeline in an error scope, the shape every world and overlay shader has.
fn probe(device: &wgpu::Device) -> Result<()> {
    const WGSL: &str = "
@group(0) @binding(0) var t: texture_2d<f32>;
@group(0) @binding(1) var s: sampler;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(f32(i), 0.0, 0.0, 1.0);
}
@fragment fn fs() -> @location(0) vec4<f32> {
    return textureSample(t, s, vec2<f32>(0.5, 0.5));
}";
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("backend probe"),
        source: wgpu::ShaderSource::Wgsl(WGSL.into()),
    });
    let _pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("backend probe"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::TextureFormat::Rgba8UnormSrgb.into())],
        }),
        multiview_mask: None,
        cache: None,
    });
    let errors = [block_on(internal.pop()), block_on(validation.pop())];
    match errors.into_iter().flatten().next() {
        // One line: the compiler message is several.
        Some(e) => Err(anyhow::anyhow!(
            "{}",
            e.to_string()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        )),
        None => Ok(()),
    }
}

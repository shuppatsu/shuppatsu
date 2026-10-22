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
                adapt(&adapter, &device);
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

/// What a device can do of the features that fall back without it (gathered by [`adapt`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caps {
    /// OpenGL (wgpu's GL backend; GLES 3 or desktop GL through EGL/WGL).
    pub gl: bool,
    /// Runs the GPU 2D compositor ([`shuppatsu_render::gpu2d::supported`]).
    pub compose_2d: bool,
    /// Anisotropic filtering (`DownlevelFlags::ANISOTROPIC_FILTERING`).
    pub anisotropic: bool,
    /// The largest MSAA sample count of the 3D pass's colour and depth formats.
    pub max_samples: u32,
}

/// The features that fall back on a device with `caps`, one short phrase each (empty: none).
pub fn fallbacks(caps: Caps) -> Vec<String> {
    let mut out = Vec::new();
    if !caps.compose_2d {
        out.push("2D layer composed on the CPU (no GPU compositor)".to_owned());
    }
    if caps.gl {
        out.push("world pages through one sampler (GL)".to_owned());
    }
    if !caps.anisotropic {
        out.push("anisotropic filtering off".to_owned());
    }
    if caps.max_samples < 8 {
        out.push(format!("msaa at most {}", caps.max_samples));
    }
    out
}

/// The largest sample count `adapter` renders `formats` with (a device without
/// `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES` only allows 1 and 4).
fn max_samples(adapter: &wgpu::Adapter, device: &wgpu::Device) -> u32 {
    use wgpu::TextureFormatFeatureFlags as F;
    let adapter_specific = device
        .features()
        .contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES);
    let flags = [
        wgpu::TextureFormat::Rgba8UnormSrgb,
        wgpu::TextureFormat::Bgra8UnormSrgb,
        shuppatsu_render::gpu::DEPTH_FORMAT,
    ]
    .map(|f| adapter.get_texture_format_features(f).flags);
    // The colour target is one of the two sRGB formats; the depth format is always used.
    let colour = flags[0] | flags[1];
    [
        (16, F::MULTISAMPLE_X16),
        (8, F::MULTISAMPLE_X8),
        (4, F::MULTISAMPLE_X4),
        (2, F::MULTISAMPLE_X2),
    ]
    .into_iter()
    .find(|&(n, f)| colour.contains(f) && flags[2].contains(f) && (adapter_specific || n == 4))
    .map_or(1, |(n, _)| n)
}

/// Fits the game to the device it opened: the 2D layer is composed on the CPU from now on
/// when the device cannot run the GPU compositor (composers already made switch at their
/// next sync, `Overlay::sync`), and one line lists what falls back.
fn adapt(adapter: &wgpu::Adapter, device: &wgpu::Device) {
    use shuppatsu_render::sprites::{ComposeMode, set_default_mode};
    let info = adapter.get_info();
    let caps = Caps {
        gl: info.backend == wgpu::Backend::Gl,
        compose_2d: shuppatsu_render::gpu2d::supported(device),
        anisotropic: adapter
            .get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::ANISOTROPIC_FILTERING),
        max_samples: max_samples(adapter, device),
    };
    if !caps.compose_2d {
        set_default_mode(ComposeMode::Cpu);
    }
    let notes = fallbacks(caps);
    if !notes.is_empty() {
        eprintln!(
            "graphics: {:?} ({}): fallbacks: {}",
            info.backend,
            info.name,
            notes.join("; ")
        );
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

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: Caps = Caps {
        gl: false,
        compose_2d: true,
        anisotropic: true,
        max_samples: 8,
    };

    #[test]
    fn a_full_device_falls_back_on_nothing() {
        assert!(fallbacks(FULL).is_empty());
        let more = Caps {
            max_samples: 16,
            ..FULL
        };
        assert!(fallbacks(more).is_empty());
    }

    #[test]
    fn gl_lists_every_feature_it_lacks() {
        let gl = Caps {
            gl: true,
            compose_2d: false,
            anisotropic: false,
            max_samples: 4,
        };
        assert_eq!(
            fallbacks(gl),
            [
                "2D layer composed on the CPU (no GPU compositor)",
                "world pages through one sampler (GL)",
                "anisotropic filtering off",
                "msaa at most 4",
            ]
        );
        // Mesa's desktop GL: everything but the compositor and the second sampler.
        let mesa = Caps {
            anisotropic: true,
            max_samples: 8,
            ..gl
        };
        assert_eq!(fallbacks(mesa).len(), 2);
    }
}

//! wgpu renderer: texture pages are uploaded once; geometry is streamed every frame.

use crate::camera::{Camera, FINE_DEPTH_STEPS, Projection};
use crate::effects::EffectGeometry;
use crate::world::{BgTexVertex, BgVertex, FrameGeometry, LineVertex, PageImage, Vertex};
use glam::Mat4;

/// Fog parameters (docs/re/renderer.md §7): linear, eye-space, colour #BFBFBF.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fog {
    pub enabled: bool,
    pub start: f32,
    pub end: f32,
    pub colour: [f32; 3],
}

impl Default for Fog {
    fn default() -> Self {
        Self {
            enabled: true,
            start: 1.0,
            end: 10.0,
            colour: [0xbf as f32 / 255.0; 3],
        }
    }
}

/// The shading enhancements of the 3D pass (docs/settings.md; renderer.md §14). The default
/// is the original: none of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shading {
    /// `ground_variation`: soft world-space brightness and tint noise on ground surfaces.
    pub ground_variation: bool,
    /// `ground_detiling`: per-region offsets and mirrors of repeating ground textures.
    pub ground_detiling: bool,
    /// `msaa`: samples per pixel of the 3D pass, resolved into the target (1 = off). A count
    /// the device cannot do falls back to the next lower one it can.
    pub msaa: u32,
    /// `anisotropic`: the anisotropy clamp (1 = off). Above 1 the pages get mipmaps; without
    /// `texture_filtering` only minified texels are filtered (renderer.md §14).
    pub anisotropic: u16,
    /// `fine_depth`: depth in constant relative steps
    /// ([`crate::camera::DepthSteps::Fine`]); off = the original's 16-bit Z buffer
    /// ([`crate::camera::DepthSteps::D16`]).
    pub fine_depth: bool,
}

impl Default for Shading {
    fn default() -> Self {
        Self {
            ground_variation: false,
            ground_detiling: false,
            msaa: 1,
            anisotropic: 1,
            fine_depth: false,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FrameUniform {
    view_proj: [[f32; 4]; 4],
    view: [[f32; 4]; 4],
    fog_colour: [f32; 4],
    fog: [f32; 4],
    /// Sampling mode, alpha to coverage, ground variation, ground de-tiling (shader.wgsl).
    shade: [f32; 4],
    /// Enhancement `distance_fade`: x = start, y = end (eye depth), z = 1 to enable.
    fade: [f32; 4],
    /// Depth steps (shader.wgsl `stepped_depth`): x = 1 for
    /// [`crate::camera::DepthSteps::Fine`] (else D16),
    /// y, z = [`Projection::depth_terms`], w = [`FINE_DEPTH_STEPS`].
    depth: [f32; 4],
    /// xyz = [`FrameGeometry::origin`] (vertex positions are relative to it), w = 1 /
    /// [`FrameGeometry::instances`] (scales `Vertex::order` into its depth step).
    origin: [f32; 4],
}

/// The pipelines of the world pass, for one sample count.
struct Pipelines {
    samples: u32,
    world: wgpu::RenderPipeline,
    line: wgpu::RenderPipeline,
    bg: wgpu::RenderPipeline,
    bg_tex: wgpu::RenderPipeline,
}

/// A multisampled colour or depth target and its size and sample count.
struct Target {
    view: wgpu::TextureView,
    size: (u32, u32),
    samples: u32,
}

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// A GPU buffer that grows to fit what is written into it.
struct StreamBuffer {
    buffer: Option<wgpu::Buffer>,
    usage: wgpu::BufferUsages,
    label: &'static str,
}

impl StreamBuffer {
    fn new(usage: wgpu::BufferUsages, label: &'static str) -> Self {
        Self {
            buffer: None,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            label,
        }
    }

    fn write(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.reserve(device, bytes.len() as u64);
        queue.write_buffer(self.buffer.as_ref().expect("allocated above"), 0, bytes);
    }

    /// Writes `parts` one after another (each a whole number of words) through one staging
    /// copy; `offsets` gets where each starts.
    fn write_parts<'a>(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        parts: impl Iterator<Item = &'a [u8]> + Clone,
        offsets: &mut Vec<u64>,
    ) {
        offsets.clear();
        let mut total = 0;
        for p in parts.clone() {
            offsets.push(total);
            total += p.len() as u64;
        }
        let Some(size) = wgpu::BufferSize::new(total) else {
            return;
        };
        self.reserve(device, total);
        let buffer = self.buffer.as_ref().expect("allocated above");
        if let Some(mut view) = queue.write_buffer_with(buffer, 0, size) {
            let mut at = 0;
            for p in parts {
                view.slice(at..at + p.len()).copy_from_slice(p);
                at += p.len();
            }
        }
    }

    /// A buffer of at least `size` bytes.
    fn reserve(&mut self, device: &wgpu::Device, size: u64) {
        if self.buffer.as_ref().is_none_or(|b| b.size() < size) {
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                // Round up so small growth doesn't reallocate every frame.
                size: size.next_power_of_two().max(256),
                usage: self.usage,
                mapped_at_creation: false,
            }));
        }
    }

    fn slice(&self, len: u64) -> Option<wgpu::BufferSlice<'_>> {
        self.buffer.as_ref().map(|b| b.slice(..len))
    }

    /// `len` bytes from `offset`.
    fn range(&self, offset: u64, len: u64) -> Option<wgpu::BufferSlice<'_>> {
        self.buffer.as_ref().map(|b| b.slice(offset..offset + len))
    }
}

/// A texture page and where its geometry of the frame is in the renderer's shared buffers
/// (byte offsets; one buffer for all pages' vertices, one for their indices, and so on: a
/// write per buffer and frame, not per page).
struct PageGpu {
    bind_group: wgpu::BindGroup,
    vertex_range: (u64, u64),
    index_offset: u64,
    index_count: u32,
    bg_tex_offset: u64,
    bg_tex_count: u32,
    fx_tex_offset: u64,
    fx_tex_count: u32,
}

/// A rectangle of the target with its own projection, for [`Renderer::render_view`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SubView {
    /// x, y, width, height in target pixels.
    pub rect: [f32; 4],
    pub projection: Projection,
    /// Clear the colour target first (the first view of a frame).
    pub clear: bool,
}

/// Whether pages get mipmaps: with `texture_filtering` or `anisotropic`.
fn mipmaps(texture_filtering: bool, anisotropic: u16) -> bool {
    texture_filtering || anisotropic > 1
}

/// The device, queue, layout and samplers [`Renderer::upload_pages`] uses, owned, so a
/// route's texture pages can be uploaded on the thread that loads it (about 5 ms for the
/// 344 pages of a stage, mostly per-texture overhead).
#[derive(Clone)]
pub struct PageUploader {
    device: wgpu::Device,
    queue: wgpu::Queue,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    filtered_sampler: wgpu::Sampler,
}

/// Texture pages on the GPU for [`Renderer::use_pages`].
pub struct UploadedPages {
    pages: Vec<PageGpu>,
    filtering: bool,
    mipmaps: bool,
}

impl PageUploader {
    /// Uploads `pages` as [`Renderer::upload_pages`] does with `texture_filtering` and
    /// `anisotropic` set so.
    pub fn upload(
        &self,
        pages: &[PageImage],
        texture_filtering: bool,
        anisotropic: u16,
    ) -> UploadedPages {
        let (device, queue) = (&self.device, &self.queue);
        let mipmaps = mipmaps(texture_filtering, anisotropic);
        // The smaller mip levels of every page, on all cores.
        let mips = if mipmaps {
            crate::par::par_map(pages, |p| crate::mip::below(p.width, p.height, &p.rgba))
        } else {
            pages.iter().map(|_| Vec::new()).collect()
        };
        let pages = pages
            .iter()
            .zip(&mips)
            .map(|(page, mips)| {
                let size = wgpu::Extent3d {
                    width: page.width,
                    height: page.height,
                    depth_or_array_layers: 1,
                };
                let levels: Vec<(u32, u32, &[u8])> =
                    std::iter::once((page.width, page.height, &page.rgba[..]))
                        .chain(mips.iter().map(|(w, h, d)| (*w, *h, &d[..])))
                        .collect();
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("page"),
                    size,
                    mip_level_count: levels.len() as u32,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                for (level, (w, h, rgba)) in levels.iter().enumerate() {
                    queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &texture,
                            mip_level: level as u32,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        rgba,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(4 * w),
                            rows_per_image: Some(*h),
                        },
                        wgpu::Extent3d {
                            width: *w,
                            height: *h,
                            depth_or_array_layers: 1,
                        },
                    );
                }
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("page"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(if texture_filtering {
                                &self.filtered_sampler
                            } else {
                                &self.sampler
                            }),
                        },
                    ],
                });
                PageGpu {
                    bind_group,
                    vertex_range: (0, 0),
                    index_offset: 0,
                    index_count: 0,
                    bg_tex_offset: 0,
                    bg_tex_count: 0,
                    fx_tex_offset: 0,
                    fx_tex_count: 0,
                }
            })
            .collect();
        UploadedPages {
            pages,
            filtering: texture_filtering,
            mipmaps,
        }
    }
}

pub struct Renderer {
    shader: wgpu::ShaderModule,
    target_format: wgpu::TextureFormat,
    pipelines: Pipelines,
    frame_buffer: wgpu::Buffer,
    frame_layout: wgpu::BindGroupLayout,
    /// The frame bind group and its samplers.
    frame_bind: (wgpu::BindGroup, FrameSamplers),
    /// The device samples a texture through one sampler per shader only (GL;
    /// [`world_shader_source`]).
    one_sampler: bool,
    page_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    filtered_sampler: wgpu::Sampler,
    pages: Vec<PageGpu>,
    /// Every page's world vertices, indices, background and effect quads of the frame
    /// ([`PageGpu`] has where).
    page_vertices: StreamBuffer,
    page_indices: StreamBuffer,
    page_bg_tex: StreamBuffer,
    page_fx_tex: StreamBuffer,
    /// This frame's [`FrameGeometry::origin`].
    origin: glam::Vec3,
    /// This frame's [`FrameGeometry::instances`].
    instances: u32,
    lines: StreamBuffer,
    line_count: u32,
    background: StreamBuffer,
    background_count: u32,
    fx_flat: StreamBuffer,
    fx_flat_count: u32,
    depth: Option<Target>,
    /// The multisampled colour target of the world pass (with `shading.msaa`).
    msaa_colour: Option<Target>,
    /// Sample counts found unusable on this device (validation failed once).
    msaa_unsupported: Vec<u32>,
    pub fog: Fog,
    /// Enhancement `distance_fade`: the world dissolves (ordered dither) into what is behind
    /// it between these eye depths (start, end); `None` = the original's hard far plane.
    pub distance_fade: Option<(f32, f32)>,
    pub projection: Projection,
    pub show_lines: bool,
    /// Fraction of the target height above the 3D view. The original's 3D viewport is virtual
    /// y 52..400 of 400 (renderer.md §5), leaving the top 13 % to the HUD; 0 fills the target.
    pub band_top: f32,
    /// Enhancement `texture_filtering` (off = the original's nearest sampling, no mipmaps).
    /// Takes effect at the next [`Renderer::upload_pages`].
    pub texture_filtering: bool,
    /// Enhancements `ground_variation`, `ground_detiling`, `msaa` and `anisotropic`. Take
    /// effect at the next render, except that `anisotropic` builds mipmaps only at the next
    /// [`Renderer::upload_pages`].
    pub shading: Shading,
    /// Screen-space effect quads drawn over the world in the 3D band (weather overlay,
    /// renderer.md §13). Uploaded by [`Renderer::render`]; empty draws nothing.
    pub effects: EffectGeometry,
}

fn depth_state(write: bool, compare: wgpu::CompareFunction) -> wgpu::DepthStencilState {
    wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(write),
        depth_compare: Some(compare),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    }
}

/// Trilinear, repeat, with the anisotropy clamp `anisotropy` (1 = none).
fn filtered_sampler(device: &wgpu::Device, anisotropy: u16) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("page sampler (filtered)"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        anisotropy_clamp: anisotropy.clamp(1, 16),
        ..Default::default()
    })
}

/// The world shader (shader.wgsl) for a device that does, or (`one_sampler`) does not, let a
/// shader sample one texture through two samplers.
///
/// OpenGL binds a texture and its sampler as one GLSL `sampler2D`, so there the world pass
/// samples its page only through `filtered_sampler`, which is bound to the nearest sampler in
/// the original's sampling mode ([`FrameSamplers`]); the alpha-to-coverage lookup of `msaa`,
/// bilinear in that mode, is then filtered by hand from four texel loads (`key_lookup`). The
/// other backends get the file unchanged.
fn world_shader_source(one_sampler: bool) -> std::borrow::Cow<'static, str> {
    const SOURCE: &str = include_str!("shader.wgsl");
    if !one_sampler {
        return SOURCE.into();
    }
    let mut s = SOURCE.to_owned();
    for (from, to) in GL_EDITS {
        debug_assert_eq!(s.matches(from).count(), 1, "shader.wgsl: {from}");
        s = s.replacen(from, to, 1);
    }
    s.push_str(GL_KEY_LOOKUP);
    s.into()
}

/// The GL world shader's edits of shader.wgsl ([`world_shader_source`]).
const GL_EDITS: [(&str, &str); 2] = [
    (
        "textureSampleGrad(page, nearest_sampler, uv, gx_in, gy_in)",
        "textureSampleGrad(page, filtered_sampler, uv, gx_in, gy_in)",
    ),
    (
        "let lin = textureSampleGrad(page, filtered_sampler, in.uv, gx, gy);",
        "let lin = key_lookup(in.uv, gx, gy);",
    ),
];

/// The GL world shader's alpha-to-coverage lookup ([`world_shader_source`]): the filtered
/// sampler where it is bound, else (nearest sampling: no mipmaps) bilinear at level 0 from
/// four texel loads, wrapping like the repeat sampler.
const GL_KEY_LOOKUP: &str = "
fn wrapped_texel(p: vec2<i32>, size: vec2<i32>) -> vec4<f32> {
    return textureLoad(page, ((p % size) + size) % size, 0);
}

fn key_lookup(uv: vec2<f32>, gx: vec2<f32>, gy: vec2<f32>) -> vec4<f32> {
    if (frame.shade.x >= 0.5) {
        return textureSampleGrad(page, filtered_sampler, uv, gx, gy);
    }
    let size = vec2<i32>(textureDimensions(page, 0));
    let t = uv * vec2<f32>(size) - vec2<f32>(0.5);
    let i = vec2<i32>(floor(t));
    let f = t - floor(t);
    let top = mix(wrapped_texel(i, size), wrapped_texel(i + vec2<i32>(1, 0), size), f.x);
    let bottom = mix(
        wrapped_texel(i + vec2<i32>(0, 1), size), wrapped_texel(i + vec2<i32>(1, 1), size), f.x,
    );
    return mix(top, bottom, f.y);
}
";

/// What the frame bind group's two samplers are: the anisotropy clamp of the filtered one,
/// and whether its slot holds the nearest sampler instead (GL in the original's sampling
/// mode, [`world_shader_source`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FrameSamplers {
    anisotropy: u16,
    nearest_in_filtered: bool,
}

/// Group 0: the frame uniform, the nearest sampler and a filtered one with `anisotropy` (or
/// the nearest one again, `nearest_in_filtered`).
fn frame_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
    nearest: &wgpu::Sampler,
    samplers: FrameSamplers,
) -> wgpu::BindGroup {
    let filtered = if samplers.nearest_in_filtered {
        nearest.clone()
    } else {
        filtered_sampler(device, samplers.anisotropy)
    };
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("frame"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(nearest),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&filtered),
            },
        ],
    })
}

/// The world pass's pipelines for `samples` per pixel. With more than one, the world
/// pipeline turns its alpha output into coverage (the shader's sharpened colour-key alpha).
fn build_pipelines(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    [frame_layout, page_layout]: [&wgpu::BindGroupLayout; 2],
    target_format: wgpu::TextureFormat,
    samples: u32,
) -> Pipelines {
    let pipeline = |label: &str,
                    layouts: &[Option<&wgpu::BindGroupLayout>],
                    (vs, fs): (&str, &str),
                    buffer: wgpu::VertexBufferLayout<'_>,
                    primitive: wgpu::PrimitiveState,
                    alpha_to_coverage: bool| {
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: layouts,
            immediate_size: 0,
        });
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some(vs),
                compilation_options: Default::default(),
                buffers: &[Some(buffer)],
            },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some(fs),
                compilation_options: Default::default(),
                targets: &[Some(target_format.into())],
            }),
            primitive,
            // Every pipeline of the pass tests and writes depth (the original's 2D background
            // quads too, at far depth words; renderer.md §8).
            depth_stencil: Some(depth_state(true, wgpu::CompareFunction::GreaterEqual)),
            multisample: wgpu::MultisampleState {
                count: samples,
                alpha_to_coverage_enabled: alpha_to_coverage && samples > 1,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        })
    };
    let world = pipeline(
        "world",
        &[Some(frame_layout), Some(page_layout)],
        ("vs_main", "fs_main"),
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x3, 1 => Float32x2, 2 => Float32x2, 3 => Float32x4, 4 => Uint32,
                5 => Uint32
            ],
        },
        // With the original's (mirrored) view, visible faces wind counter-clockwise in wgpu's
        // y-up NDC, matching D3DCULL_CW in D3D's y-down screen space (models.md).
        wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        },
        true,
    );
    let line = pipeline(
        "lines",
        &[Some(frame_layout)],
        ("vs_line", "fs_line"),
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LineVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4],
        },
        wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::LineList,
            ..Default::default()
        },
        false,
    );
    let bg = pipeline(
        "background",
        &[],
        ("vs_bg", "fs_bg"),
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<BgVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4],
        },
        wgpu::PrimitiveState::default(),
        false,
    );
    let bg_tex = pipeline(
        "background textured",
        &[Some(frame_layout), Some(page_layout)],
        ("vs_bgtex", "fs_bgtex"),
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<BgTexVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
        },
        wgpu::PrimitiveState::default(),
        false,
    );
    Pipelines {
        samples,
        world,
        line,
        bg,
        bg_tex,
    }
}

/// Whether `device` can render `format` (and the depth format) with `samples` per pixel:
/// tries a tiny target inside an error scope. Counts other than 1 and 4 need the adapter's
/// format features (`TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES`, requested by the game when
/// the adapter has it).
fn samples_supported(device: &wgpu::Device, format: wgpu::TextureFormat, samples: u32) -> bool {
    if samples <= 1 {
        return true;
    }
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    for f in [format, DEPTH_FORMAT] {
        let _probe = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("msaa probe"),
            size: wgpu::Extent3d {
                width: 4,
                height: 4,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format: f,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
    }
    // Native backends report validation errors at once; a pending result counts as a "no".
    let mut future = std::pin::pin!(scope.pop());
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    matches!(future.as_mut().poll(&mut cx), std::task::Poll::Ready(None))
}

impl Renderer {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let one_sampler = device.adapter_info().backend == wgpu::Backend::Gl;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("world shader"),
            source: wgpu::ShaderSource::Wgsl(world_shader_source(one_sampler)),
        });
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // The world's own samplers (nearest; filtered with the anisotropy clamp).
                sampler_entry(1),
                sampler_entry(2),
            ],
        });
        let page_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("page"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                sampler_entry(1),
            ],
        });
        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame uniform"),
            size: std::mem::size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Nearest filtering and wrap addressing: the original sets no filter or address states.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("page sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        // Enhancement `texture_filtering`, for the 2D background and effect quads: trilinear +
        // anisotropic over CPU-built mipmaps. (The world's filtered sampler follows
        // `anisotropic`, see `frame_bind_group`.)
        let filtered_sampler = filtered_sampler(device, 16);
        let samplers = FrameSamplers {
            anisotropy: 1,
            nearest_in_filtered: one_sampler,
        };
        let frame_bind = (
            frame_bind_group(device, &frame_layout, &frame_buffer, &sampler, samplers),
            samplers,
        );
        let pipelines = build_pipelines(
            device,
            &shader,
            [&frame_layout, &page_layout],
            target_format,
            1,
        );

        Self {
            shader,
            target_format,
            pipelines,
            frame_buffer,
            frame_layout,
            frame_bind,
            one_sampler,
            page_layout,
            sampler,
            filtered_sampler,
            pages: Vec::new(),
            page_vertices: StreamBuffer::new(wgpu::BufferUsages::VERTEX, "page vertices"),
            page_indices: StreamBuffer::new(wgpu::BufferUsages::INDEX, "page indices"),
            page_bg_tex: StreamBuffer::new(wgpu::BufferUsages::VERTEX, "page background"),
            page_fx_tex: StreamBuffer::new(wgpu::BufferUsages::VERTEX, "page effects"),
            origin: glam::Vec3::ZERO,
            instances: 0,
            lines: StreamBuffer::new(wgpu::BufferUsages::VERTEX, "lines"),
            line_count: 0,
            background: StreamBuffer::new(wgpu::BufferUsages::VERTEX, "background"),
            background_count: 0,
            fx_flat: StreamBuffer::new(wgpu::BufferUsages::VERTEX, "effects"),
            fx_flat_count: 0,
            depth: None,
            msaa_colour: None,
            msaa_unsupported: Vec::new(),
            fog: Fog::default(),
            distance_fade: None,
            projection: Projection::default(),
            show_lines: false,
            band_top: 0.0,
            texture_filtering: false,
            shading: Shading::default(),
            effects: EffectGeometry::default(),
        }
    }

    /// Uploads texture pages (replacing earlier ones). Page `i` here is batch `i` of every
    /// [`FrameGeometry`].
    pub fn upload_pages(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pages: &[PageImage],
    ) {
        let up = self.page_uploader(device, queue);
        self.pages = up
            .upload(pages, self.texture_filtering, self.shading.anisotropic)
            .pages;
    }

    /// What uploading texture pages takes, to do it on another thread
    /// ([`PageUploader::upload`], then [`Renderer::use_pages`]).
    pub fn page_uploader(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> PageUploader {
        PageUploader {
            device: device.clone(),
            queue: queue.clone(),
            layout: self.page_layout.clone(),
            sampler: self.sampler.clone(),
            filtered_sampler: self.filtered_sampler.clone(),
        }
    }

    /// Takes pages uploaded by [`PageUploader::upload`] if they were made for the current
    /// `texture_filtering` and `anisotropic` (the same sampler and mipmaps
    /// [`Renderer::upload_pages`] would give them); false (and nothing changed) otherwise.
    pub fn use_pages(&mut self, uploaded: UploadedPages) -> bool {
        if (uploaded.filtering, uploaded.mipmaps) != (self.texture_filtering, self.wants_mipmaps())
        {
            return false;
        }
        self.pages = uploaded.pages;
        true
    }

    /// Whether pages get mipmaps: with `texture_filtering` or `anisotropic`.
    fn wants_mipmaps(&self) -> bool {
        mipmaps(self.texture_filtering, self.shading.anisotropic)
    }

    /// Streams this frame's geometry to the GPU.
    pub fn set_geometry(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &FrameGeometry,
    ) {
        let _perf = crate::perf::scope(crate::perf::Section::Upload3d);
        let n = self.pages.len();
        let mut offsets = Vec::with_capacity(n);
        for page in &mut self.pages {
            (page.bg_tex_count, page.index_count) = (0, 0);
        }
        let quads = frame.background_tex.iter().take(n);
        self.page_bg_tex.write_parts(
            device,
            queue,
            quads.clone().map(|q| bytemuck::cast_slice(q)),
            &mut offsets,
        );
        for ((page, q), &at) in self.pages.iter_mut().zip(quads).zip(&offsets) {
            page.bg_tex_count = q.len() as u32;
            page.bg_tex_offset = at;
        }
        let batches = frame.batches.iter().take(n);
        self.page_vertices.write_parts(
            device,
            queue,
            batches.clone().map(|b| bytemuck::cast_slice(&b.vertices)),
            &mut offsets,
        );
        for ((page, b), &at) in self.pages.iter_mut().zip(batches.clone()).zip(&offsets) {
            let len = std::mem::size_of_val(b.vertices.as_slice()) as u64;
            page.vertex_range = (at, len);
        }
        self.page_indices.write_parts(
            device,
            queue,
            batches.clone().map(|b| bytemuck::cast_slice(&b.indices)),
            &mut offsets,
        );
        for ((page, b), &at) in self.pages.iter_mut().zip(batches).zip(&offsets) {
            page.index_count = b.indices.len() as u32;
            page.index_offset = at;
        }
        self.origin = frame.origin;
        self.instances = frame.instances;
        self.line_count = frame.lines.len() as u32;
        self.lines
            .write(device, queue, bytemuck::cast_slice(&frame.lines));
        self.background_count = frame.background.len() as u32;
        self.background
            .write(device, queue, bytemuck::cast_slice(&frame.background));
    }

    /// A render target of `format` with `samples` per pixel, kept in `slot` while the size
    /// and count stay the same.
    fn target(
        slot: &mut Option<Target>,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        size: (u32, u32),
        samples: u32,
    ) -> wgpu::TextureView {
        if slot
            .as_ref()
            .is_none_or(|t| (t.size, t.samples) != (size, samples))
        {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(if format == DEPTH_FORMAT {
                    "depth"
                } else {
                    "msaa colour"
                }),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            *slot = Some(Target {
                view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
                size,
                samples,
            });
        }
        slot.as_ref().expect("target just created").view.clone()
    }

    /// The sample count to render with: `shading.msaa` (a power of two up to 16), or the next
    /// lower count this device can do.
    fn sample_count(&mut self, device: &wgpu::Device) -> u32 {
        let mut samples = self.shading.msaa.clamp(1, 16).next_power_of_two();
        while samples > 1 {
            if !self.msaa_unsupported.contains(&samples) {
                if samples == self.pipelines.samples
                    || samples_supported(device, self.target_format, samples)
                {
                    break;
                }
                eprintln!("warning: msaa {samples} is not supported here; trying fewer samples");
                self.msaa_unsupported.push(samples);
            }
            samples /= 2;
        }
        samples
    }

    /// Brings the pipelines and the frame bind group in line with [`Renderer::shading`].
    fn prepare_shading(&mut self, device: &wgpu::Device) {
        let samples = self.sample_count(device);
        if samples != self.pipelines.samples {
            self.pipelines = build_pipelines(
                device,
                &self.shader,
                [&self.frame_layout, &self.page_layout],
                self.target_format,
                samples,
            );
        }
        let samplers = FrameSamplers {
            anisotropy: self.shading.anisotropic.clamp(1, 16),
            nearest_in_filtered: self.one_sampler && self.sampling_mode() == 0.0,
        };
        if samplers != self.frame_bind.1 {
            self.frame_bind = (
                frame_bind_group(
                    device,
                    &self.frame_layout,
                    &self.frame_buffer,
                    &self.sampler,
                    samplers,
                ),
                samplers,
            );
        }
    }

    /// The shader's sampling mode: 0 nearest (the original), 1 filtered, 2 nearest where
    /// magnified and filtered where minified.
    fn sampling_mode(&self) -> f32 {
        if self.texture_filtering {
            1.0
        } else if self.shading.anisotropic > 1 {
            2.0
        } else {
            0.0
        }
    }

    /// The `shade` uniform: sampling mode, alpha to coverage, ground variation, de-tiling.
    fn shade_uniform(&self) -> [f32; 4] {
        let sampling = self.sampling_mode();
        let on = |b: bool| if b { 1.0 } else { 0.0 };
        [
            sampling,
            on(self.pipelines.samples > 1),
            on(self.shading.ground_variation),
            on(self.shading.ground_detiling),
        ]
    }

    /// Draws the current geometry into `target` (`width`×`height` pixels).
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &wgpu::TextureView,
        (width, height): (u32, u32),
        camera: &Camera,
    ) {
        let band_y = (height as f32 * self.band_top).round();
        let band_h = (height as f32 - band_y).max(1.0);
        let view = SubView {
            rect: [0.0, band_y, width as f32, band_h],
            projection: self.projection,
            clear: true,
        };
        self.render_view(device, queue, target, (width, height), camera, &view);
    }

    /// Draws the current geometry into one rectangle of `target` with its own projection
    /// (the station result's two views, scene.md §8). Without `clear` the colour target is
    /// kept; depth is always cleared.
    pub fn render_view(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &wgpu::TextureView,
        (width, height): (u32, u32),
        camera: &Camera,
        sub: &SubView,
    ) {
        let _perf = crate::perf::scope(crate::perf::Section::Render);
        let [vx, vy, vw, vh] = sub.rect;
        let (vw, vh) = (vw.max(1.0), vh.max(1.0));
        // The geometry is stored relative to its origin; so is the camera.
        let view = Camera {
            eye: camera.eye - self.origin,
            target: camera.target - self.origin,
        }
        .view();
        let view_proj: Mat4 = sub.projection.matrix(vw / vh) * view;
        let fog = self.fog;
        let linear = |c: f32| c.powf(2.2);
        let uniform = FrameUniform {
            view_proj: view_proj.to_cols_array_2d(),
            view: view.to_cols_array_2d(),
            fog_colour: [
                linear(fog.colour[0]),
                linear(fog.colour[1]),
                linear(fog.colour[2]),
                1.0,
            ],
            fog: [fog.start, fog.end, if fog.enabled { 1.0 } else { 0.0 }, 0.0],
            shade: [0.0; 4],
            fade: match self.distance_fade {
                Some((start, end)) => [start, end, 1.0, 0.0],
                None => [0.0; 4],
            },
            depth: {
                let (k, c) = sub.projection.depth_terms();
                let fine = if self.shading.fine_depth { 1.0 } else { 0.0 };
                [fine, k, c, FINE_DEPTH_STEPS]
            },
            origin: self
                .origin
                .extend(1.0 / self.instances.max(1) as f32)
                .to_array(),
        };
        self.prepare_shading(device);
        let uniform = FrameUniform {
            shade: self.shade_uniform(),
            ..uniform
        };
        queue.write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&uniform));
        self.upload_effects(device, queue);
        let samples = self.pipelines.samples;
        let depth = Self::target(
            &mut self.depth,
            device,
            DEPTH_FORMAT,
            (width, height),
            samples,
        );
        // Enhancement `msaa`: the pass draws into a multisampled target that is resolved into
        // `target`. That target keeps its samples between views, so a view that does not
        // clear (the station result's second view) loads what the first one left.
        let msaa = (samples > 1).then(|| {
            Self::target(
                &mut self.msaa_colour,
                device,
                self.target_format,
                (width, height),
                samples,
            )
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frame"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("world"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: msaa.as_ref().unwrap_or(target),
                    depth_slice: None,
                    resolve_target: msaa.as_ref().map(|_| target),
                    ops: wgpu::Operations {
                        // The original clears to black (scene.md §6).
                        load: if sub.clear {
                            wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth,
                    depth_ops: Some(wgpu::Operations {
                        // Reversed depth: 0 is the far plane.
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_viewport(vx, vy, vw, vh, 0.0, 1.0);
            let bg_bytes = u64::from(self.background_count) * size_of::<BgVertex>() as u64;
            if self.background_count > 0
                && let Some(slice) = self.background.slice(bg_bytes)
            {
                pass.set_pipeline(&self.pipelines.bg);
                pass.set_vertex_buffer(0, slice);
                pass.draw(0..self.background_count, 0..1);
            }
            pass.set_pipeline(&self.pipelines.bg_tex);
            pass.set_bind_group(0, &self.frame_bind.0, &[]);
            for page in self.pages.iter().filter(|p| p.bg_tex_count > 0) {
                let bytes = u64::from(page.bg_tex_count) * size_of::<BgTexVertex>() as u64;
                if let Some(slice) = self.page_bg_tex.range(page.bg_tex_offset, bytes) {
                    pass.set_bind_group(1, &page.bind_group, &[]);
                    pass.set_vertex_buffer(0, slice);
                    pass.draw(0..page.bg_tex_count, 0..1);
                }
            }
            pass.set_pipeline(&self.pipelines.world);
            pass.set_bind_group(0, &self.frame_bind.0, &[]);
            for page in self.pages.iter().filter(|p| p.index_count > 0) {
                let (vertices, len) = page.vertex_range;
                let (Some(v), Some(i)) = (
                    self.page_vertices.range(vertices, len),
                    self.page_indices
                        .range(page.index_offset, u64::from(page.index_count) * 4),
                ) else {
                    continue;
                };
                pass.set_bind_group(1, &page.bind_group, &[]);
                pass.set_vertex_buffer(0, v);
                pass.set_index_buffer(i, wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..page.index_count, 0, 0..1);
            }
            let line_bytes = u64::from(self.line_count) * size_of::<LineVertex>() as u64;
            if self.show_lines
                && self.line_count > 0
                && let Some(slice) = self.lines.slice(line_bytes)
            {
                pass.set_pipeline(&self.pipelines.line);
                pass.set_bind_group(0, &self.frame_bind.0, &[]);
                pass.set_vertex_buffer(0, slice);
                pass.draw(0..self.line_count, 0..1);
            }
            self.draw_effects(&mut pass);
        }
        queue.submit([encoder.finish()]);
    }

    /// Streams [`Renderer::effects`] to the GPU.
    fn upload_effects(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let fx = &self.effects;
        self.fx_flat_count = fx.flat.len() as u32;
        self.fx_flat
            .write(device, queue, bytemuck::cast_slice(&fx.flat));
        let quads = |i: usize| fx.textured.get(i).map_or(&[][..], Vec::as_slice);
        let mut offsets = Vec::with_capacity(self.pages.len());
        self.page_fx_tex.write_parts(
            device,
            queue,
            (0..self.pages.len()).map(|i| bytemuck::cast_slice(quads(i))),
            &mut offsets,
        );
        for (i, (page, &at)) in self.pages.iter_mut().zip(&offsets).enumerate() {
            page.fx_tex_count = quads(i).len() as u32;
            page.fx_tex_offset = at;
        }
    }

    /// The effect quads, after the world in the same band viewport and with the background
    /// pipelines (the original's 2D quads keep the Z test and write; renderer.md §13).
    fn draw_effects(&self, pass: &mut wgpu::RenderPass<'_>) {
        let flat_bytes = u64::from(self.fx_flat_count) * size_of::<BgVertex>() as u64;
        if self.fx_flat_count > 0
            && let Some(slice) = self.fx_flat.slice(flat_bytes)
        {
            pass.set_pipeline(&self.pipelines.bg);
            pass.set_vertex_buffer(0, slice);
            pass.draw(0..self.fx_flat_count, 0..1);
        }
        if self.pages.iter().all(|p| p.fx_tex_count == 0) {
            return;
        }
        pass.set_pipeline(&self.pipelines.bg_tex);
        pass.set_bind_group(0, &self.frame_bind.0, &[]);
        for page in self.pages.iter().filter(|p| p.fx_tex_count > 0) {
            let bytes = u64::from(page.fx_tex_count) * size_of::<BgTexVertex>() as u64;
            if let Some(slice) = self.page_fx_tex.range(page.fx_tex_offset, bytes) {
                pass.set_bind_group(1, &page.bind_group, &[]);
                pass.set_vertex_buffer(0, slice);
                pass.draw(0..page.fx_tex_count, 0..1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gl_world_shader_samples_the_page_through_one_sampler() {
        let source = include_str!("shader.wgsl");
        for (from, _) in GL_EDITS {
            assert_eq!(source.matches(from).count(), 1, "{from}");
        }
        assert_eq!(world_shader_source(false), source);
        let gl = world_shader_source(true);
        assert!(!gl.contains("textureSampleGrad(page, nearest_sampler"));
        assert!(gl.contains("fn key_lookup("));
    }

    fn block_on<F: std::future::Future>(fut: F) -> F::Output {
        let mut fut = std::pin::pin!(fut);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        loop {
            if let std::task::Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
                return v;
            }
            std::thread::yield_now();
        }
    }

    /// The world pass on an OpenGL device, in every sampling mode with and without MSAA,
    /// raises no validation error. Skipped without a GL adapter.
    #[test]
    fn world_pass_builds_and_renders_on_gl() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::GL,
            ..wgpu::InstanceDescriptor::new_without_display_handle_from_env()
        });
        let Ok(adapter) = block_on(instance.request_adapter(&Default::default())) else {
            eprintln!("no GL adapter; skipped");
            return;
        };
        let (device, queue) =
            block_on(adapter.request_device(&Default::default())).expect("GL device");
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let camera = Camera {
            eye: glam::Vec3::ZERO,
            target: glam::Vec3::Z,
        };
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut renderer = Renderer::new(&device, format);
        for (filtering, anisotropic, msaa) in
            [(false, 1, 1), (false, 1, 4), (true, 1, 4), (false, 4, 4)]
        {
            renderer.texture_filtering = filtering;
            renderer.shading.anisotropic = anisotropic;
            renderer.shading.msaa = msaa;
            renderer.render(&device, &queue, &view, (16, 16), &camera);
        }
        assert!(block_on(scope.pop()).is_none());
    }
}

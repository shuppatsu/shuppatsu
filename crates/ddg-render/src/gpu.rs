//! wgpu renderer: texture pages are uploaded once; geometry is streamed every frame.

use crate::camera::{Camera, Projection};
use crate::world::{BgVertex, FrameGeometry, LineVertex, PageImage, Vertex};
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

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FrameUniform {
    view_proj: [[f32; 4]; 4],
    view: [[f32; 4]; 4],
    fog_colour: [f32; 4],
    fog: [f32; 4],
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
        let size = bytes.len() as u64;
        if self.buffer.as_ref().is_none_or(|b| b.size() < size) {
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                // Round up so small growth doesn't reallocate every frame.
                size: size.next_power_of_two().max(256),
                usage: self.usage,
                mapped_at_creation: false,
            }));
        }
        queue.write_buffer(self.buffer.as_ref().expect("allocated above"), 0, bytes);
    }

    fn slice(&self, len: u64) -> Option<wgpu::BufferSlice<'_>> {
        self.buffer.as_ref().map(|b| b.slice(..len))
    }
}

struct PageGpu {
    bind_group: wgpu::BindGroup,
    vertices: StreamBuffer,
    indices: StreamBuffer,
    index_count: u32,
}

pub struct Renderer {
    world_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    bg_pipeline: wgpu::RenderPipeline,
    frame_buffer: wgpu::Buffer,
    frame_bind: wgpu::BindGroup,
    page_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pages: Vec<PageGpu>,
    lines: StreamBuffer,
    line_count: u32,
    background: StreamBuffer,
    background_count: u32,
    depth: Option<(wgpu::TextureView, u32, u32)>,
    pub fog: Fog,
    pub projection: Projection,
    pub show_lines: bool,
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

impl Renderer {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("world shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
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
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame uniform"),
            size: std::mem::size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: frame_buffer.as_entire_binding(),
            }],
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

        let pipeline = |label: &str,
                        layouts: &[Option<&wgpu::BindGroupLayout>],
                        (vs, fs): (&str, &str),
                        buffer: wgpu::VertexBufferLayout<'_>,
                        primitive: wgpu::PrimitiveState,
                        depth: wgpu::DepthStencilState| {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: layouts,
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[Some(buffer)],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(target_format.into())],
                }),
                primitive,
                depth_stencil: Some(depth),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let world_pipeline = pipeline(
            "world",
            &[Some(&frame_layout), Some(&page_layout)],
            ("vs_main", "fs_main"),
            wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
            },
            // Visible faces wind clockwise on screen in this left-handed setup (verified by
            // rendering: with CCW fronts, rails, platforms and most scenery were culled).
            wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Cw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_state(true, wgpu::CompareFunction::LessEqual),
        );
        let line_pipeline = pipeline(
            "lines",
            &[Some(&frame_layout)],
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
            depth_state(true, wgpu::CompareFunction::LessEqual),
        );
        let bg_pipeline = pipeline(
            "background",
            &[],
            ("vs_bg", "fs_bg"),
            wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<BgVertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4],
            },
            wgpu::PrimitiveState::default(),
            depth_state(false, wgpu::CompareFunction::Always),
        );

        Self {
            world_pipeline,
            line_pipeline,
            bg_pipeline,
            frame_buffer,
            frame_bind,
            page_layout,
            sampler,
            pages: Vec::new(),
            lines: StreamBuffer::new(wgpu::BufferUsages::VERTEX, "lines"),
            line_count: 0,
            background: StreamBuffer::new(wgpu::BufferUsages::VERTEX, "background"),
            background_count: 0,
            depth: None,
            fog: Fog::default(),
            projection: Projection::default(),
            show_lines: false,
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
        self.pages = pages
            .iter()
            .map(|page| {
                let size = wgpu::Extent3d {
                    width: page.width,
                    height: page.height,
                    depth_or_array_layers: 1,
                };
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("page"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                queue.write_texture(
                    texture.as_image_copy(),
                    &page.rgba,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(4 * page.width),
                        rows_per_image: Some(page.height),
                    },
                    size,
                );
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("page"),
                    layout: &self.page_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                });
                PageGpu {
                    bind_group,
                    vertices: StreamBuffer::new(wgpu::BufferUsages::VERTEX, "page vertices"),
                    indices: StreamBuffer::new(wgpu::BufferUsages::INDEX, "page indices"),
                    index_count: 0,
                }
            })
            .collect();
    }

    /// Streams this frame's geometry to the GPU.
    pub fn set_geometry(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &FrameGeometry,
    ) {
        for (page, batch) in self.pages.iter_mut().zip(&frame.batches) {
            page.index_count = batch.indices.len() as u32;
            page.vertices
                .write(device, queue, bytemuck::cast_slice(&batch.vertices));
            page.indices
                .write(device, queue, bytemuck::cast_slice(&batch.indices));
        }
        self.line_count = frame.lines.len() as u32;
        self.lines
            .write(device, queue, bytemuck::cast_slice(&frame.lines));
        self.background_count = frame.background.len() as u32;
        self.background
            .write(device, queue, bytemuck::cast_slice(&frame.background));
    }

    fn depth_view(&mut self, device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
        let stale = self
            .depth
            .as_ref()
            .is_none_or(|(_, w, h)| (*w, *h) != (width, height));
        if stale {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("depth"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.depth = Some((view, width, height));
        }
        self.depth
            .as_ref()
            .expect("depth view just created")
            .0
            .clone()
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
        let aspect = width as f32 / height.max(1) as f32;
        let view = camera.view();
        let view_proj: Mat4 = self.projection.matrix(aspect) * view;
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
        };
        queue.write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&uniform));
        let depth = self.depth_view(device, width, height);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frame"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("world"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // The original clears to black (scene.md §6).
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let bg_bytes = u64::from(self.background_count) * size_of::<BgVertex>() as u64;
            if self.background_count > 0
                && let Some(slice) = self.background.slice(bg_bytes)
            {
                pass.set_pipeline(&self.bg_pipeline);
                pass.set_vertex_buffer(0, slice);
                pass.draw(0..self.background_count, 0..1);
            }
            pass.set_pipeline(&self.world_pipeline);
            pass.set_bind_group(0, &self.frame_bind, &[]);
            for page in self.pages.iter().filter(|p| p.index_count > 0) {
                let (Some(v), Some(i)) = (
                    page.vertices.buffer.as_ref(),
                    page.indices.slice(u64::from(page.index_count) * 4),
                ) else {
                    continue;
                };
                pass.set_bind_group(1, &page.bind_group, &[]);
                pass.set_vertex_buffer(0, v.slice(..));
                pass.set_index_buffer(i, wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..page.index_count, 0, 0..1);
            }
            let line_bytes = u64::from(self.line_count) * size_of::<LineVertex>() as u64;
            if self.show_lines
                && self.line_count > 0
                && let Some(slice) = self.lines.slice(line_bytes)
            {
                pass.set_pipeline(&self.line_pipeline);
                pass.set_bind_group(0, &self.frame_bind, &[]);
                pass.set_vertex_buffer(0, slice);
                pass.draw(0..self.line_count, 0..1);
            }
        }
        queue.submit([encoder.finish()]);
    }
}

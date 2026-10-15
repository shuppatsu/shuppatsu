//! wgpu renderer for a [`World`].

use crate::camera::{Camera, Projection};
use crate::world::{LineVertex, Vertex, World};
use glam::Mat4;
use wgpu::util::DeviceExt;

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

struct GpuBatch {
    bind_group: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
}

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

pub struct Renderer {
    world_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    frame_buffer: wgpu::Buffer,
    frame_bind: wgpu::BindGroup,
    page_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    batches: Vec<GpuBatch>,
    lines: Option<(wgpu::Buffer, u32)>,
    depth: Option<(wgpu::TextureView, u32, u32)>,
    pub fog: Fog,
    pub projection: Projection,
    pub show_lines: bool,
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

        let depth_state = || wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        };
        let world_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("world"),
            bind_group_layouts: &[Some(&frame_layout), Some(&page_layout)],
            immediate_size: 0,
        });
        let world_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("world"),
            layout: Some(&world_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(target_format.into())],
            }),
            // Left-handed space with D3DCULL_CW: visible triangles wind counter-clockwise.
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(depth_state()),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let line_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("lines"),
            bind_group_layouts: &[Some(&frame_layout)],
            immediate_size: 0,
        });
        let line_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("lines"),
            layout: Some(&line_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_line"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<LineVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_line"),
                compilation_options: Default::default(),
                targets: &[Some(target_format.into())],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            depth_stencil: Some(depth_state()),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Self {
            world_pipeline,
            line_pipeline,
            frame_buffer,
            frame_bind,
            page_layout,
            sampler,
            batches: Vec::new(),
            lines: None,
            depth: None,
            fog: Fog::default(),
            projection: Projection::default(),
            show_lines: false,
        }
    }

    /// Uploads a world, replacing the previous one. Pages without geometry are skipped.
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, world: &World) {
        self.batches.clear();
        for (page, batch) in world.pages.iter().zip(&world.batches) {
            if batch.indices.is_empty() {
                continue;
            }
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
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
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
            let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("batch vertices"),
                contents: bytemuck::cast_slice(&batch.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("batch indices"),
                contents: bytemuck::cast_slice(&batch.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            self.batches.push(GpuBatch {
                bind_group,
                vertices,
                indices,
                index_count: batch.indices.len() as u32,
            });
        }
        self.lines = (!world.lines.is_empty()).then(|| {
            let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("lines"),
                contents: bytemuck::cast_slice(&world.lines),
                usage: wgpu::BufferUsages::VERTEX,
            });
            (buf, world.lines.len() as u32)
        });
    }

    fn depth_view(&mut self, device: &wgpu::Device, width: u32, height: u32) -> &wgpu::TextureView {
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
        &self.depth.as_ref().expect("depth view just created").0
    }

    /// Draws the world into `target` (`width`×`height` pixels).
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &wgpu::TextureView,
        width: u32,
        height: u32,
        camera: &Camera,
    ) {
        let aspect = width as f32 / height.max(1) as f32;
        let view = camera.view();
        let view_proj: Mat4 = self.projection.matrix(aspect) * view;
        let fog = self.fog;
        let srgb = |c: f32| c.powf(2.2);
        let uniform = FrameUniform {
            view_proj: view_proj.to_cols_array_2d(),
            view: view.to_cols_array_2d(),
            fog_colour: [
                srgb(fog.colour[0]),
                srgb(fog.colour[1]),
                srgb(fog.colour[2]),
                1.0,
            ],
            fog: [fog.start, fog.end, if fog.enabled { 1.0 } else { 0.0 }, 0.0],
        };
        queue.write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&uniform));
        let clear = wgpu::Color {
            r: uniform.fog_colour[0] as f64,
            g: uniform.fog_colour[1] as f64,
            b: uniform.fog_colour[2] as f64,
            a: 1.0,
        };
        let depth = self.depth_view(device, width, height).clone();
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
                        load: wgpu::LoadOp::Clear(clear),
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
            pass.set_pipeline(&self.world_pipeline);
            pass.set_bind_group(0, &self.frame_bind, &[]);
            for b in &self.batches {
                pass.set_bind_group(1, &b.bind_group, &[]);
                pass.set_vertex_buffer(0, b.vertices.slice(..));
                pass.set_index_buffer(b.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..b.index_count, 0, 0..1);
            }
            if let (true, Some((buf, count))) = (self.show_lines, &self.lines) {
                pass.set_pipeline(&self.line_pipeline);
                pass.set_bind_group(0, &self.frame_bind, &[]);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..*count, 0..1);
            }
        }
        queue.submit([encoder.finish()]);
    }
}

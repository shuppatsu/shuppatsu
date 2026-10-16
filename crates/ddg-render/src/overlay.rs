//! Draws the 640×480 sprite canvas ([`crate::sprites`]) over the 3D frame: one texture,
//! nearest sampling (the original blits unscaled native pixels), index-0 pixels discarded.

use crate::sprites::{CANVAS_H, CANVAS_W};

const SHADER: &str = r"
@group(0) @binding(0) var canvas: texture_2d<f32>;
@group(0) @binding(1) var canvas_sampler: sampler;

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// A quad covering the viewport, from the vertex index (triangle strip).
@vertex
fn vs(@builtin(vertex_index) i: u32) -> Out {
    let uv = vec2<f32>(f32(i & 1u), f32(i >> 1u));
    var out: Out;
    out.clip = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    let texel = textureSample(canvas, canvas_sampler, in.uv);
    if (texel.a < 0.5) {
        discard;
    }
    return vec4<f32>(texel.rgb, 1.0);
}
";

pub struct Overlay {
    pipeline: wgpu::RenderPipeline,
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    size: (u32, u32),
}

impl Overlay {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        Self::with_scale(device, target_format, 1)
    }

    /// An overlay for a canvas of 640×480 × `scale` (see `SpriteLayer::scale`).
    pub fn with_scale(
        device: &wgpu::Device,
        target_format: wgpu::TextureFormat,
        scale: usize,
    ) -> Self {
        let size = (
            (CANVAS_W * scale.max(1)) as u32,
            (CANVAS_H * scale.max(1)) as u32,
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("overlay"),
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
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("overlay canvas"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // sRGB, so the palette colours reach the sRGB target unchanged.
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("overlay sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("overlay"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(target_format.into())],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipeline,
            texture,
            bind_group,
            size,
        }
    }

    /// Uploads a composed canvas of this overlay's size (RGBA8).
    pub fn upload(&self, queue: &wgpu::Queue, rgba: &[u8]) {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.size.0 * 4),
                rows_per_image: Some(self.size.1),
            },
            wgpu::Extent3d {
                width: self.size.0,
                height: self.size.1,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Draws the canvas into `rect` (x, y, width, height in target pixels) over what is there.
    pub fn render(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &wgpu::TextureView,
        rect: [f32; 4],
    ) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("overlay"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("overlay"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_viewport(rect[0], rect[1], rect[2], rect[3], 0.0, 1.0);
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.draw(0..4, 0..1);
        }
        queue.submit([encoder.finish()]);
    }
}

/// Where the 640×480 canvas goes in a `width × height` window: full height, centred, keeping
/// the original 4:3 shape (x, y, w, h in pixels).
pub fn canvas_rect(width: u32, height: u32) -> [f32; 4] {
    let h = height as f32;
    let w = h * CANVAS_W as f32 / CANVAS_H as f32;
    [(width as f32 - w) / 2.0, 0.0, w, h]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_is_centred_4_3() {
        let r = canvas_rect(2560, 1600);
        assert!((r[0] - 213.333).abs() < 1e-3 && (r[2] - 2133.333).abs() < 1e-3);
        assert_eq!(canvas_rect(640, 480), [0.0, 0.0, 640.0, 480.0]);
    }
}

//! Draws the 640×480 sprite canvas ([`crate::sprites`]) over the 3D frame: one texture,
//! nearest sampling (the original blits unscaled native pixels), index-0 pixels discarded.
//!
//! The canvas is always mapped to the window with one scale factor for x and y
//! ([`canvas_rect_for`]), so art keeps square pixels. A wider window gets a wider canvas
//! (`SpriteLayer::fit_window`, enhancement `wide_hud`), never a stretched one. The original's
//! horizontal stretch of the 3D projection does not apply here.

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
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("overlay sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let (texture, bind_group) = Self::canvas_texture(device, &layout, &sampler, size);
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
            layout,
            sampler,
            texture,
            bind_group,
            size,
        }
    }

    fn canvas_texture(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        size: (u32, u32),
    ) -> (wgpu::Texture, wgpu::BindGroup) {
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
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("overlay"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        (texture, bind_group)
    }

    /// The canvas size in pixels.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Makes the canvas texture `width × height` (a wide canvas follows the window's shape).
    /// Returns true when it changed (the new texture is empty: upload again).
    pub fn resize(&mut self, device: &wgpu::Device, (width, height): (usize, usize)) -> bool {
        let size = (width.max(1) as u32, height.max(1) as u32);
        if size == self.size {
            return false;
        }
        (self.texture, self.bind_group) =
            Self::canvas_texture(device, &self.layout, &self.sampler, size);
        self.size = size;
        true
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
    canvas_rect_for(width, height, (CANVAS_W, CANVAS_H))
}

/// Where a canvas of `canvas` pixels goes in a `width × height` window: full height, centred,
/// one scale factor for both axes (x, y, w, h in pixels). A canvas widened to the window's
/// shape covers the whole window.
pub fn canvas_rect_for(width: u32, height: u32, canvas: (usize, usize)) -> [f32; 4] {
    let h = height as f32;
    let w = h * canvas.0 as f32 / canvas.1.max(1) as f32;
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

    /// Enhancement `wide_hud`: at 4:3, 16:10 and 21:9 the widened canvas covers the window
    /// (within a pixel) and a 16×16 sprite's footprint in window pixels is square.
    #[test]
    fn wide_canvas_keeps_square_pixels() {
        use crate::sprites::SpriteLayer;
        for (win, scale) in [
            ((1280, 960), 2),
            ((1280, 800), 2),
            ((2560, 1600), 2),
            ((2560, 1080), 2),
            ((3440, 1440), 3),
            ((1920, 1080), 1),
        ] {
            let mut l = SpriteLayer {
                scale,
                ..SpriteLayer::default()
            };
            l.fit_window(win.0, win.1);
            let (cw, ch) = l.canvas_size();
            let r = canvas_rect_for(win.0, win.1, (cw, ch));
            let (kx, ky) = (r[2] / cw as f32, r[3] / ch as f32);
            assert!((kx - ky).abs() < 1e-6, "{win:?}: {kx} vs {ky}");
            // A 16×16 tile of art is 16·scale canvas pixels square.
            let side = (16 * scale) as f32;
            assert!(((side * kx) - (side * ky)).abs() < 1e-3, "{win:?}");
            assert!(
                r[0].abs() <= kx,
                "{win:?}: canvas does not cover the window: {r:?}"
            );
            assert!((r[2] - win.0 as f32).abs() <= 2.0 * kx, "{win:?}: {r:?}");
        }
        // 4:3 keeps the original canvas.
        let mut l = SpriteLayer {
            scale: 2,
            ..SpriteLayer::default()
        };
        l.fit_window(1280, 960);
        assert_eq!(l.canvas_size(), (1280, 960));
    }
}

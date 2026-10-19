//! Draws the 640×480 sprite canvas ([`crate::sprites`]) over the 3D frame: one texture,
//! index-0 pixels transparent.
//!
//! The canvas is always mapped to the window with one scale factor for x and y
//! ([`present_rect`]), so art keeps square pixels. A wider window gets a wider canvas
//! (`SpriteLayer::wide`, enhancement `wide_hud`), never a stretched one. The original's
//! horizontal stretch of the 3D projection does not apply here.
//!
//! How canvas pixels reach window pixels is [`UiScaling`] (docs/localisation.md, "UI scaling"):
//! sharp bilinear by default, whole multiples on request, or the plain nearest neighbour of
//! earlier builds. The canvas itself is 640×480 × `SpriteLayer::scale`; [`auto_multiple`] picks
//! that multiple from the window height.

use crate::sprites::{CANVAS_H, CANVAS_W, Composer, Pending};

/// How the canvas is presented in the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UiScaling {
    /// Sharp bilinear (enhancement `ui_scaling smooth`): enlarged with nearest neighbour to the
    /// smallest whole multiple at least the target size, then linearly resampled to the target.
    /// Every canvas pixel gets the same size; only the pixels on art edges are blended.
    #[default]
    Smooth,
    /// The largest whole multiple of the canvas that fits, centred, unfiltered. The rest of
    /// the window shows the 3D view (driving) or black (menus).
    Integer,
    /// Nearest neighbour at the fitting (usually fractional) factor: uneven pixel sizes. The
    /// presentation of earlier builds, kept for comparisons.
    Nearest,
}

impl UiScaling {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "smooth" => Some(Self::Smooth),
            "integer" => Some(Self::Integer),
            "nearest" => Some(Self::Nearest),
            _ => None,
        }
    }
}

/// The largest canvas multiple: 2560×1920 at 4:3 (more only costs memory and compose time).
pub const MAX_MULTIPLE: usize = 4;

/// The canvas multiple (`SpriteLayer::scale`) for a window `height` pixels high.
///
/// * Smooth and nearest: `ceil(height / 480)`, so the canvas is at least as tall as the window
///   and text is rasterised at or above the window's resolution; the presentation then only
///   shrinks it, by a factor between 1/2 and 1 (800 → 2, 1080 → 3, 1440 → 3, 1600 → 4). Above
///   1920 the cap of [`MAX_MULTIPLE`] makes it enlarge instead (2160 → 4, ×1.125).
/// * Integer: `floor(height / 480)`, the largest canvas that fits whole (800 → 1, 1080 → 2,
///   1440 → 3, 2160 → 4).
pub fn auto_multiple(height: u32, scaling: UiScaling) -> usize {
    let h = height.max(1) as usize;
    let s = match scaling {
        UiScaling::Integer => h / CANVAS_H,
        UiScaling::Smooth | UiScaling::Nearest => h.div_ceil(CANVAS_H),
    };
    s.clamp(1, MAX_MULTIPLE)
}

/// The whole multiple a `canvas` is shown at in a `width × height` window in
/// [`UiScaling::Integer`]; 0 when not even 1× fits (it is then fitted like `Smooth`).
pub fn integer_factor(width: u32, height: u32, canvas: (usize, usize)) -> usize {
    let kx = width as usize / canvas.0.max(1);
    let ky = height as usize / canvas.1.max(1);
    kx.min(ky)
}

/// The width of a canvas `canvas_h` pixels high that follows a `width × height` window's shape
/// (enhancement `wide_hud`; never narrower than 4:3 is up to the caller). In
/// [`UiScaling::Integer`] it fills the window's width at the whole multiple that fits the
/// height, so edge-anchored elements sit at the integer-scaled area's edges.
pub fn wide_canvas_width(width: u32, height: u32, canvas_h: usize, scaling: UiScaling) -> usize {
    let k = height as usize / canvas_h.max(1);
    if scaling == UiScaling::Integer && k >= 1 {
        return width as usize / k;
    }
    (canvas_h as f64 * f64::from(width) / f64::from(height.max(1))).round() as usize
}

/// Where a `canvas` goes in a `width × height` window under `scaling` (x, y, w, h in pixels),
/// one scale factor for both axes.
pub fn present_rect(
    width: u32,
    height: u32,
    canvas: (usize, usize),
    scaling: UiScaling,
) -> [f32; 4] {
    let k = integer_factor(width, height, canvas);
    if scaling == UiScaling::Integer && k >= 1 {
        let (w, h) = (canvas.0 * k, canvas.1 * k);
        let x = (width as usize - w) / 2;
        let y = (height as usize - h) / 2;
        return [x as f32, y as f32, w as f32, h as f32];
    }
    canvas_rect_for(width, height, canvas)
}

/// The per-axis whole multiple sharp bilinear enlarges a `src`-pixel axis to before it is
/// resampled to `dst` pixels: the smallest one with `src · p ≥ dst` (1 when shrinking).
pub fn prescale(src: f32, dst: f32) -> f32 {
    // The tolerance keeps an exact multiple (2.0000001) from rounding up a step.
    (dst / src.max(1.0) - 1e-4).ceil().max(1.0)
}

/// Sharp bilinear along one axis: for a position `x` in source texels (texel `i` spans
/// `i..i + 1`), the coordinate at which one bilinear tap of the source equals one bilinear tap
/// of the source enlarged `p` times with nearest neighbour. Inside a texel it is the texel's
/// centre; within `0.5 / p` of a texel edge it ramps across the edge. The overlay shader does
/// the same per fragment.
pub fn sharp_bilinear_coord(x: f32, p: f32) -> f32 {
    let base = x.floor();
    let d = x - base - 0.5;
    let range = 0.5 - 0.5 / p;
    base + (d - d.clamp(-range, range)) * p + 0.5
}

const SHADER: &str = r"
struct Params {
    // Canvas size in texels, target rectangle size in pixels.
    tex: vec2<f32>,
    dst: vec2<f32>,
    // 0 nearest, 1 sharp bilinear, 2 plain bilinear.
    mode: u32,
};

@group(0) @binding(0) var canvas: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: Params;

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

// One canvas texel, premultiplied, with the alpha test of the original blit (index 0 and
// coverage under one half are transparent).
fn fetch(p: vec2<f32>) -> vec4<f32> {
    let hi = vec2<i32>(params.tex) - vec2<i32>(1);
    let c = clamp(vec2<i32>(p), vec2<i32>(0), hi);
    let t = textureLoad(canvas, c, 0);
    let a = select(0.0, 1.0, t.a >= 0.5);
    return vec4<f32>(t.rgb * a, a);
}

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    let x = in.uv * params.tex;
    var c: vec4<f32>;
    if (params.mode == 0u) {
        c = fetch(floor(x));
    } else {
        // Sharp bilinear (overlay.rs `sharp_bilinear_coord`), then one bilinear tap; with a
        // prescale of 1 (mode 2) that is plain bilinear.
        var p = max(ceil(params.dst / params.tex - vec2<f32>(1e-4)), vec2<f32>(1.0));
        if (params.mode == 2u) {
            p = vec2<f32>(1.0);
        }
        let base = floor(x);
        let d = x - base - vec2<f32>(0.5);
        let range = vec2<f32>(0.5) - vec2<f32>(0.5) / p;
        let q = base + (d - clamp(d, -range, range)) * p;
        let i = floor(q);
        let w = q - i;
        let top = mix(fetch(i), fetch(i + vec2<f32>(1.0, 0.0)), w.x);
        let bottom = mix(fetch(i + vec2<f32>(0.0, 1.0)), fetch(i + vec2<f32>(1.0, 1.0)), w.x);
        c = mix(top, bottom, w.y);
    }
    if (c.a <= 0.0) {
        discard;
    }
    return c;
}
";

/// Size of the shader's `Params` (vec2, vec2, u32, padded to 16 bytes).
const PARAMS_SIZE: u64 = 32;

pub struct Overlay {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    size: (u32, u32),
    /// The [`Composer`] whose canvas the texture holds ([`Overlay::sync`]); `None` after a
    /// resize or a plain [`Overlay::upload`].
    source: Option<u64>,
    /// How [`Overlay::render`] maps the canvas to its rectangle.
    pub scaling: UiScaling,
    /// Plain bilinear filtering instead of [`Overlay::scaling`]: for live-action pictures
    /// (the Komachi movie, setting `movie_scaling smooth`), not pixel art.
    pub bilinear: bool,
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
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(PARAMS_SIZE),
                    },
                    count: None,
                },
            ],
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("overlay params"),
            size: PARAMS_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let (texture, bind_group) = Self::canvas_texture(device, &layout, &params, size);
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
                // Premultiplied: opaque texels replace, the blended edges of sharp bilinear
                // mix with what is under them.
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
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
            params,
            texture,
            bind_group,
            size,
            source: None,
            scaling: UiScaling::default(),
            bilinear: false,
        }
    }

    fn canvas_texture(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        params: &wgpu::Buffer,
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
            // sRGB, so the palette colours reach the sRGB target unchanged (and blending
            // happens in linear light).
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
                    resource: params.as_entire_binding(),
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
            Self::canvas_texture(device, &self.layout, &self.params, size);
        self.size = size;
        self.source = None;
        true
    }

    /// Uploads a composed canvas of this overlay's size (RGBA8).
    pub fn upload(&mut self, queue: &wgpu::Queue, rgba: &[u8]) {
        self.source = None;
        let _perf = crate::perf::scope(crate::perf::Section::Upload2d);
        crate::perf::count(crate::perf::Counter::Upload2dBytes, rgba.len() as u64);
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

    /// Brings the texture up to date with `canvas`: everything when it holds another canvas
    /// (or none), else only the rectangles composed since the last sync. A canvas of another
    /// size than the texture is not uploaded ([`Overlay::resize`] first).
    pub fn sync(&mut self, queue: &wgpu::Queue, canvas: &mut Composer) {
        let pending = canvas.take_pending();
        let (w, h) = canvas.size();
        if (w as u32, h as u32) != self.size {
            self.source = None;
            return;
        }
        if self.source != Some(canvas.id()) {
            self.upload(queue, canvas.rgba());
            self.source = Some(canvas.id());
            return;
        }
        match pending {
            Pending::None => {}
            Pending::Full => self.upload_rect(queue, canvas.rgba(), [0, 0, w, h]),
            Pending::Rects(rects) => {
                for r in rects {
                    self.upload_rect(queue, canvas.rgba(), r);
                }
            }
        }
    }

    /// Uploads the `[x0, y0, x1, y1]` pixels of a whole canvas `rgba` of this overlay's size.
    fn upload_rect(&self, queue: &wgpu::Queue, rgba: &[u8], [x0, y0, x1, y1]: [usize; 4]) {
        let _perf = crate::perf::scope(crate::perf::Section::Upload2d);
        let w = self.size.0 as usize;
        let start = (y0 * w + x0) * 4;
        let end = ((y1 - 1) * w + x1) * 4;
        crate::perf::count(
            crate::perf::Counter::Upload2dBytes,
            ((x1 - x0) * (y1 - y0) * 4) as u64,
        );
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: x0 as u32,
                    y: y0 as u32,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &rgba[start..end],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(self.size.0 * 4),
                rows_per_image: Some((y1 - y0) as u32),
            },
            wgpu::Extent3d {
                width: (x1 - x0) as u32,
                height: (y1 - y0) as u32,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Where this overlay's canvas goes in a `width × height` target under its
    /// [`Overlay::scaling`] ([`present_rect`]).
    pub fn rect(&self, width: u32, height: u32) -> [f32; 4] {
        let size = (self.size.0 as usize, self.size.1 as usize);
        present_rect(width, height, size, self.scaling)
    }

    /// Draws the canvas into `rect` (x, y, width, height in target pixels) over what is there:
    /// plain bilinear with [`Overlay::bilinear`]; else unfiltered when [`Overlay::scaling`] is
    /// `Nearest`, or `Integer` with a whole multiple in `rect`; sharp bilinear otherwise.
    pub fn render(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target: &wgpu::TextureView,
        rect: [f32; 4],
    ) {
        let _perf = crate::perf::scope(crate::perf::Section::Render);
        let (tw, th) = (self.size.0 as f32, self.size.1 as f32);
        let whole = |dst: f32, src: f32| dst >= src && (dst / src).fract() == 0.0;
        let nearest = match self.scaling {
            UiScaling::Nearest => true,
            UiScaling::Integer => whole(rect[2], tw) && whole(rect[3], th),
            UiScaling::Smooth => false,
        };
        let mut params = [0u8; PARAMS_SIZE as usize];
        for (i, v) in [tw, th, rect[2], rect[3]].into_iter().enumerate() {
            params[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        let mode = if self.bilinear {
            2u32
        } else {
            u32::from(!nearest)
        };
        params[16..20].copy_from_slice(&mode.to_le_bytes());
        queue.write_buffer(&self.params, 0, &params);
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

/// The original 4:3 part of a presented canvas `rect` (the centred 640×480 design of a wide
/// canvas): where the fixed screens and the station result's views go.
pub fn design_rect(rect: [f32; 4]) -> [f32; 4] {
    let w = rect[3] * CANVAS_W as f32 / CANVAS_H as f32;
    [rect[0] + (rect[2] - w) / 2.0, rect[1], w, rect[3]]
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
            for scaling in [UiScaling::Smooth, UiScaling::Nearest] {
                let r = present_rect(win.0, win.1, (cw, ch), scaling);
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
        }
        // 4:3 keeps the original canvas.
        let mut l = SpriteLayer {
            scale: 2,
            ..SpriteLayer::default()
        };
        l.fit_window(1280, 960);
        assert_eq!(l.canvas_size(), (1280, 960));
    }

    #[test]
    fn auto_multiple_follows_the_window_height() {
        let smooth = |h| auto_multiple(h, UiScaling::Smooth);
        let integer = |h| auto_multiple(h, UiScaling::Integer);
        assert_eq!(
            [480, 481, 800, 960, 1080, 1440, 1600, 2160].map(smooth),
            [1, 2, 2, 2, 3, 3, 4, 4]
        );
        assert_eq!(
            [200, 480, 800, 960, 1080, 1440, 1600, 2160].map(integer),
            [1, 1, 1, 2, 2, 3, 3, 4]
        );
        assert_eq!(auto_multiple(0, UiScaling::Smooth), 1);
        assert_eq!(auto_multiple(10_000, UiScaling::Smooth), MAX_MULTIPLE);
        // Smooth never shrinks the canvas by more than half, up to the cap.
        for h in 240..=1920 {
            let k = h as f32 / (CANVAS_H * smooth(h)) as f32;
            assert!((0.5..=1.0).contains(&k), "{h}: {k}");
        }
    }

    #[test]
    fn integer_fit_is_whole_and_centred() {
        // 1920×1080 at multiple 2 (1280×960): 1×, centred with 60-px bars above and below.
        let r = present_rect(1920, 1080, (1280, 960), UiScaling::Integer);
        assert_eq!(r, [320.0, 60.0, 1280.0, 960.0]);
        // 640×480 in 2560×1600: 3× (1920×1440).
        let r = present_rect(2560, 1600, (640, 480), UiScaling::Integer);
        assert_eq!(r, [320.0, 80.0, 1920.0, 1440.0]);
        assert_eq!(integer_factor(2560, 1600, (640, 480)), 3);
        // Too small for 1×: fitted like smooth.
        let r = present_rect(600, 400, (640, 480), UiScaling::Integer);
        assert_eq!(r, canvas_rect_for(600, 400, (640, 480)));
        // The wide canvas fills the width at the whole multiple: 1080 / 960 → 1×, 1920 wide.
        assert_eq!(wide_canvas_width(1920, 1080, 960, UiScaling::Integer), 1920);
        // 3840×2160 at multiple 2 (960 high): 2×, 1920 wide.
        assert_eq!(wide_canvas_width(3840, 2160, 960, UiScaling::Integer), 1920);
        let r = present_rect(3840, 2160, (1920, 960), UiScaling::Integer);
        assert_eq!(r, [0.0, 120.0, 3840.0, 1920.0]);
        // Smooth follows the window's shape.
        assert_eq!(wide_canvas_width(1920, 1080, 1440, UiScaling::Smooth), 2560);
    }

    /// Reference: the source enlarged `p`× with nearest neighbour, then one bilinear tap at
    /// `x · p` (texel centres at `j + 0.5`), as a blend of source texels.
    fn reference(src: &[f32], x: f32, p: usize) -> f32 {
        let at = |j: isize| src[(j.max(0) as usize / p).min(src.len() - 1)];
        let q = x * p as f32 - 0.5;
        let j = q.floor();
        let w = q - j;
        at(j as isize) * (1.0 - w) + at(j as isize + 1) * w
    }

    /// One bilinear tap of the source at `c` (texel centres at `i + 0.5`).
    fn bilinear(src: &[f32], c: f32) -> f32 {
        let at = |i: isize| src[(i.max(0) as usize).min(src.len() - 1)];
        let q = c - 0.5;
        let i = q.floor();
        let w = q - i;
        at(i as isize) * (1.0 - w) + at(i as isize + 1) * w
    }

    #[test]
    fn sharp_bilinear_matches_nearest_then_linear() {
        let src = [0.0, 1.0, 0.25, 0.75, 0.0];
        for p in 1..=5 {
            for n in 0..=500 {
                let x = n as f32 / 100.0;
                let got = bilinear(&src, sharp_bilinear_coord(x, p as f32));
                let want = reference(&src, x, p);
                assert!((got - want).abs() < 1e-4, "p {p}, x {x}: {got} vs {want}");
            }
        }
        // Texel interiors sample the centre exactly; p 1 is plain bilinear.
        assert_eq!(sharp_bilinear_coord(2.5, 3.0), 2.5);
        assert_eq!(sharp_bilinear_coord(2.4, 3.0), 2.5);
        assert!((sharp_bilinear_coord(2.1, 1.0) - 2.1).abs() < 1e-6);
    }

    #[test]
    fn prescale_is_the_smallest_whole_multiple_covering_the_target() {
        assert_eq!(prescale(960.0, 1080.0), 2.0);
        assert_eq!(prescale(960.0, 1920.0), 2.0);
        assert_eq!(prescale(1440.0, 1080.0), 1.0);
        assert_eq!(prescale(480.0, 1600.0), 4.0);
    }

    #[test]
    fn design_rect_is_the_centred_4_3() {
        let r = design_rect([0.0, 60.0, 1920.0, 960.0]);
        assert_eq!(r, [320.0, 60.0, 1280.0, 960.0]);
    }
}

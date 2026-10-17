// World renderer: pre-lit textured triangles (the original never uses D3D lighting; vertex
// colour is constant white), colour-key transparency, linear eye-space fog.
//
// Port enhancements (docs/settings.md; renderer.md §14), all off by default, where the
// world pass is then exactly the original's nearest, colour-keyed sampling:
// `texture_filtering` / `anisotropic` (how pages are sampled), `msaa` (alpha to coverage for
// the colour-keyed cut-outs), `ground_variation` and `ground_detiling` (ground surfaces only,
// marked per vertex by `ddg_render::ground`).

struct Frame {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    fog_colour: vec4<f32>,
    // x = fog start, y = fog end (eye-space depth), z = 1 to enable fog
    fog: vec4<f32>,
    // x = sampling: 0 nearest (the original), 1 filtered (`texture_filtering`), 2 nearest
    //     where magnified and filtered where minified (`anisotropic` alone);
    // y = 1: alpha to coverage (`msaa`); z = 1: `ground_variation`; w = 1: `ground_detiling`
    shade: vec4<f32>,
    // Enhancement distance_fade: x = start, y = end (eye-space depth), z = 1 to enable
    fade: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
// Nearest, repeat: the original's sampling (no filter or address states set; renderer.md §6).
@group(0) @binding(1) var nearest_sampler: sampler;
// Trilinear with the `anisotropic` clamp, repeat (over CPU mipmaps when they are built).
@group(0) @binding(2) var filtered_sampler: sampler;
@group(1) @binding(0) var page: texture_2d<f32>;
@group(1) @binding(1) var page_sampler: sampler;

const SURFACE_GROUND: u32 = 1u;
const SURFACE_TILE_U: u32 = 2u;
const SURFACE_TILE_V: u32 = 4u;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) tile: vec2<f32>,
    @location(3) rect: vec4<f32>,
    @location(4) surface: u32,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) eye_depth: f32,
    @location(2) world: vec3<f32>,
    @location(3) tile: vec2<f32>,
    @location(4) @interpolate(flat) rect: vec4<f32>,
    @location(5) @interpolate(flat) surface: u32,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var out: VsOut;
    let world = vec4<f32>(v.pos, 1.0);
    out.clip = frame.view_proj * world;
    out.eye_depth = (frame.view * world).z;
    out.uv = v.uv;
    out.world = v.pos;
    out.tile = v.tile;
    out.rect = v.rect;
    out.surface = v.surface;
    return out;
}

fn apply_fog(colour: vec3<f32>, depth: f32) -> vec3<f32> {
    if (frame.fog.z == 0.0) {
        return colour;
    }
    let f = clamp((frame.fog.y - depth) / (frame.fog.y - frame.fog.x), 0.0, 1.0);
    return mix(frame.fog_colour.rgb, colour, f);
}

// --- Noise (no texture asset): an integer hash and smooth value noise. -----------------

fn hash_u(x: u32) -> u32 {
    var h = x * 747796405u + 2891336453u;
    h = ((h >> ((h >> 28u) + 4u)) ^ h) * 277803737u;
    return (h >> 22u) ^ h;
}

// Uniform in 0..1 for an integer lattice point (and a channel `seed`).
fn hash2(p: vec2<f32>, seed: u32) -> f32 {
    let q = vec2<i32>(p);
    let h = hash_u(u32(q.x) ^ hash_u(u32(q.y) ^ hash_u(seed)));
    return f32(h) / 4294967295.0;
}

fn value_noise(p: vec2<f32>, seed: u32) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let s = f * f * (3.0 - 2.0 * f);
    let a = hash2(i, seed);
    let b = hash2(i + vec2<f32>(1.0, 0.0), seed);
    let c = hash2(i + vec2<f32>(0.0, 1.0), seed);
    let d = hash2(i + vec2<f32>(1.0, 1.0), seed);
    return mix(mix(a, b, s.x), mix(c, d, s.x), s.y);
}

// --- Sampling --------------------------------------------------------------------------

// Caps a UV derivative at `limit` (page UV), so filtered sampling of a sub-rect never picks a
// mip level where the rect is under four texels and its neighbours in the atlas bleed in.
fn cap(g: vec2<f32>, limit: f32) -> vec2<f32> {
    let l = length(g);
    if (l > limit) {
        return g * (limit / l);
    }
    return g;
}

// One page lookup at `uv` with the screen derivatives `gx`, `gy` (explicit, so lookups in
// branches and at wrapped UVs pick the same footprint), as the sampling setting asks.
fn sample_page(uv: vec2<f32>, gx_in: vec2<f32>, gy_in: vec2<f32>, rect: vec4<f32>) -> vec4<f32> {
    let mode = frame.shade.x;
    if (mode < 0.5) {
        return textureSampleGrad(page, nearest_sampler, uv, gx_in, gy_in);
    }
    // Whole-page rects wrap through the sampler; sub-rects get the LOD cap.
    var limit = 1e9;
    if (min(rect.z, rect.w) < 0.99) {
        limit = 0.25 * min(rect.z, rect.w);
    }
    let gx = cap(gx_in, limit);
    let gy = cap(gy_in, limit);
    if (mode < 1.5) {
        return textureSampleGrad(page, filtered_sampler, uv, gx, gy);
    }
    // `anisotropic` without `texture_filtering`: along an axis where a texel covers more than
    // a pixel, the coordinate is pulled to the texel centre except within one pixel of its
    // edge, so magnified texels stay the original's sharp squares (with an antialiased edge
    // instead of a jagged one); along a minified axis it is left alone and the trilinear,
    // anisotropic lookup removes the shimmer. The derivatives stay the true ones, so the mip
    // level and the anisotropy follow the real footprint.
    let size = vec2<f32>(textureDimensions(page, 0));
    let t = uv * size;
    let footprint = max(abs(gx_in * size) + abs(gy_in * size), vec2<f32>(1e-6));
    let sharp = floor(t) + 0.5 + clamp((fract(t) - 0.5) / footprint, vec2<f32>(-0.5), vec2<f32>(0.5));
    let snapped = select(t, sharp, footprint < vec2<f32>(1.0));
    return textureSampleGrad(page, filtered_sampler, snapped / size, gx, gy);
}

// `ground_detiling`: Quilez's "texture repetition" technique 3 (a smooth noise picks one of
// eight per-region variants of the texture, neighbouring variants cross-faded), with variants
// that are offsets and mirrors along the axes the texture repeats on, keyed on world x/z so
// the pattern runs on across part and LOD seams. The lookups wrap inside the group's rect.
const DETILE_SCALE: f32 = 0.6;

fn variant(i: f32, axes: vec2<f32>) -> vec4<f32> {
    let k = vec2<f32>(i, 0.0);
    let offset = vec2<f32>(hash2(k, 11u), hash2(k, 12u)) * axes;
    let flip = select(vec2<f32>(1.0), vec2<f32>(-1.0), vec2<f32>(hash2(k, 13u), hash2(k, 14u)) < vec2<f32>(0.5));
    return vec4<f32>(offset, mix(vec2<f32>(1.0), flip, axes));
}

fn variant_sample(
    v: vec4<f32>, tile: vec2<f32>, axes: vec2<f32>, rect: vec4<f32>, gx: vec2<f32>, gy: vec2<f32>,
) -> vec4<f32> {
    let t = tile * v.zw + v.xy;
    // Repeating axes wrap inside the rect; the others keep the part's own coordinate.
    let local = mix(tile, fract(t), axes);
    return sample_page(rect.xy + local * rect.zw, gx * v.zw, gy * v.zw, rect);
}

fn detiled(in: VsOut, gx: vec2<f32>, gy: vec2<f32>) -> vec4<f32> {
    let axes = vec2<f32>(
        select(0.0, 1.0, (in.surface & SURFACE_TILE_U) != 0u),
        select(0.0, 1.0, (in.surface & SURFACE_TILE_V) != 0u),
    );
    let l = value_noise(in.world.xz * DETILE_SCALE, 7u) * 8.0;
    let i = floor(l);
    let f = fract(l);
    let a = variant_sample(variant(i, axes), in.tile, axes, in.rect, gx, gy);
    let b = variant_sample(variant(i + 1.0, axes), in.tile, axes, in.rect, gx, gy);
    // Cross-fade, biased by the texels' own contrast so the seam follows their detail.
    let w = smoothstep(0.2, 0.8, f - 0.1 * dot(a.rgb - b.rgb, vec3<f32>(1.0)));
    return mix(a, b, w);
}

// `ground_variation`: soft world-space brightness (up to ±15 %, mostly within ±6 %) and a
// slight warm/cool tint (±4 %) over 5–25 m (1 world unit ≈ 10 m), multiplied into the
// texel in linear colour.
fn ground_variation(xz: vec2<f32>) -> vec3<f32> {
    let broad = value_noise(xz * 0.4, 1u);
    let fine = value_noise(xz * 1.5 + vec2<f32>(37.0, 11.0), 2u);
    let n = broad * 0.7 + fine * 0.3 - 0.5;
    let t = value_noise(xz * 0.25 + vec2<f32>(5.0, 91.0), 3u) - 0.5;
    let tint = vec3<f32>(1.0 + 0.06 * t, 1.0, 1.0 - 0.08 * t);
    return tint * (1.0 + 0.3 * n);
}

// Enhancement distance_fade: a screen-door (4x4 ordered dither) fade over the last stretch
// before the far plane, so the background shows through instead of a hard edge.
fn faded_out(depth: f32, pixel: vec2<f32>) -> bool {
    if (frame.fade.z == 0.0) {
        return false;
    }
    let t = clamp((depth - frame.fade.x) / (frame.fade.y - frame.fade.x), 0.0, 1.0);
    var bayer = array<f32, 16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0,
                               3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);
    let p = vec2<u32>(pixel) & vec2<u32>(3u);
    return t > (bayer[p.y * 4u + p.x] + 0.5) / 16.0;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Derivatives first, in uniform control flow.
    let gx = dpdx(in.uv);
    let gy = dpdy(in.uv);
    let ground = (in.surface & SURFACE_GROUND) != 0u;
    var texel: vec4<f32>;
    if (ground && frame.shade.w > 0.5 && (in.surface & (SURFACE_TILE_U | SURFACE_TILE_V)) != 0u) {
        texel = detiled(in, gx, gy);
    } else {
        texel = sample_page(in.uv, gx, gy, in.rect);
    }
    var alpha = 1.0;
    if (frame.shade.y > 0.5) {
        // MSAA: alpha to coverage from the bilinear key alpha, sharpened to about one pixel, so
        // cut-out edges (wires, fences, leaves) get the multisampled edge the polygons get.
        // Where the texel itself is keyed, its colour comes from the opaque neighbours (keyed
        // texels are RGB 0, so the bilinear colour is premultiplied by the key alpha).
        let lin = textureSampleGrad(page, filtered_sampler, in.uv, gx, gy);
        alpha = clamp((lin.a - 0.5) / max(fwidth(lin.a), 1e-4) + 0.5, 0.0, 1.0);
        if (texel.a < 0.5 && lin.a > 0.0) {
            texel = vec4<f32>(lin.rgb / lin.a, 1.0);
        }
        if (alpha <= 0.0) {
            discard;
        }
    } else if (texel.a < 0.5) {
        // Colour key: RGB565 0x0000 was uploaded with alpha 0.
        discard;
    }
    if (faded_out(in.eye_depth, in.clip.xy)) {
        discard;
    }
    var colour = texel.rgb;
    if (ground && frame.shade.z > 0.5) {
        colour = colour * ground_variation(in.world.xz);
    }
    return vec4<f32>(apply_fog(colour, in.eye_depth), alpha);
}

// Debug lines (track centre lines), unlit and unfogged.
struct LineIn {
    @location(0) pos: vec3<f32>,
    @location(1) colour: vec4<f32>,
};

struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) colour: vec4<f32>,
};

@vertex
fn vs_line(v: LineIn) -> LineOut {
    var out: LineOut;
    out.clip = frame.view_proj * vec4<f32>(v.pos, 1.0);
    out.colour = v.colour;
    return out;
}

@fragment
fn fs_line(in: LineOut) -> @location(0) vec4<f32> {
    return in.colour;
}

// Background: screen-space gradient quads (sky, ground), drawn first without depth.
struct BgIn {
    @location(0) pos: vec3<f32>,
    @location(1) colour: vec4<f32>,
};

struct BgOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) colour: vec4<f32>,
};

@vertex
fn vs_bg(v: BgIn) -> BgOut {
    var out: BgOut;
    out.clip = vec4<f32>(v.pos, 1.0);
    out.colour = v.colour;
    return out;
}

@fragment
fn fs_bg(in: BgOut) -> @location(0) vec4<f32> {
    return in.colour;
}

// Textured background (sky panorama, textured ground): screen-space, colour-keyed, no depth.
struct BgTexIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

struct BgTexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_bgtex(v: BgTexIn) -> BgTexOut {
    var out: BgTexOut;
    out.clip = vec4<f32>(v.pos, 1.0);
    out.uv = v.uv;
    return out;
}

@fragment
fn fs_bgtex(in: BgTexOut) -> @location(0) vec4<f32> {
    let texel = textureSample(page, page_sampler, in.uv);
    if (texel.a < 0.5) {
        discard;
    }
    return vec4<f32>(texel.rgb, 1.0);
}

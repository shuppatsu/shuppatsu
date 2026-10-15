// World renderer: pre-lit textured triangles (the original never uses D3D lighting; vertex
// colour is constant white), colour-key transparency, linear eye-space fog.

struct Frame {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    fog_colour: vec4<f32>,
    // x = fog start, y = fog end (eye-space depth), z = 1 to enable fog
    fog: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(0) var page: texture_2d<f32>;
@group(1) @binding(1) var page_sampler: sampler;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) eye_depth: f32,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var out: VsOut;
    let world = vec4<f32>(v.pos, 1.0);
    out.clip = frame.view_proj * world;
    out.eye_depth = (frame.view * world).z;
    out.uv = v.uv;
    return out;
}

fn apply_fog(colour: vec3<f32>, depth: f32) -> vec3<f32> {
    if (frame.fog.z == 0.0) {
        return colour;
    }
    let f = clamp((frame.fog.y - depth) / (frame.fog.y - frame.fog.x), 0.0, 1.0);
    return mix(frame.fog_colour.rgb, colour, f);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let texel = textureSample(page, page_sampler, in.uv);
    // Colour key: RGB565 0x0000 was uploaded with alpha 0.
    if (texel.a < 0.5) {
        discard;
    }
    return vec4<f32>(apply_fog(texel.rgb, in.eye_depth), 1.0);
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
    @location(0) pos: vec2<f32>,
    @location(1) colour: vec4<f32>,
};

struct BgOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) colour: vec4<f32>,
};

@vertex
fn vs_bg(v: BgIn) -> BgOut {
    var out: BgOut;
    out.clip = vec4<f32>(v.pos, 0.0, 1.0);
    out.colour = v.colour;
    return out;
}

@fragment
fn fs_bg(in: BgOut) -> @location(0) vec4<f32> {
    return in.colour;
}

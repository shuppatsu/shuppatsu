// The 2D layer composed on the GPU (gpu2d.rs, sprites/gpu.rs): one invocation per canvas
// pixel applies, in order, every draw record whose box holds the pixel, with the integer
// arithmetic of the CPU compose (sprites.rs), which is the reference.

struct Globals {
    cw: u32,
    ch: u32,
    s: u32,
    bins_x: u32,
    ntiles: u32,
    // Always 0: or-ed into floats so the compiler cannot fuse a multiply and an add.
    zero: u32,
    bd_on: u32,
    bd_rgb: u32,
    bd_x0: u32,
    bd_x1: u32,
    bd_y1: u32,
    bins_off: u32,
    entries_off: u32,
    // The bins to compose.
    dirty_off: u32,
    pad1: u32,
    pad2: u32,
};

@group(0) @binding(0) var<uniform> g: Globals;
// Tiles (64 words each: 256 indices, 4 per word) then the tilemaps (a tile id per word).
@group(0) @binding(1) var<storage, read> bank: array<u32>;
// 128 palettes of 256 words (bytes B, R, G, unused).
@group(0) @binding(2) var<storage, read> pals: array<u32>;
// Upscaled images, rasters, paper fills, text parts.
@group(0) @binding(3) var<storage, read> blobs: array<u32>;
// Draw records (32 words each), bin starts, bin entries.
@group(0) @binding(4) var<storage, read> frame: array<u32>;
@group(0) @binding(5) var out: texture_storage_2d<rgba8unorm, write>;

const BIN_SHIFT: u32 = 6u;
const PARAM: u32 = 9u;

fn unpack(c: u32) -> vec4<u32> {
    return vec4<u32>(c & 255u, (c >> 8u) & 255u, (c >> 16u) & 255u, c >> 24u);
}

// Palette colour (sprites.rs `Palette::rgba`): index 0 is transparent.
fn pal(p: u32, i: u32) -> vec4<u32> {
    if (i == 0u) {
        return vec4<u32>(0u);
    }
    let v = pals[p * 256u + i];
    return vec4<u32>((v >> 8u) & 255u, (v >> 16u) & 255u, v & 255u, 255u);
}

fn byte_at(off: u32, i: u32) -> u32 {
    return (blobs[off + (i >> 2u)] >> ((i & 3u) * 8u)) & 255u;
}

// Floor division and Euclidean remainder by a positive divisor.
fn fdiv(a: i32, b: i32) -> i32 {
    let q = a / b;
    return select(q, q - 1, (a % b != 0) && (a < 0));
}

fn emod(a: i32, b: i32) -> i32 {
    return a - fdiv(a, b) * b;
}

fn mix8(u: u32, f: u32, t: u32) -> u32 {
    return (u * (255u - t) + f * t) / 255u;
}

fn p(b: u32, k: u32) -> u32 {
    return frame[b + PARAM + k];
}

fn pi(b: u32, k: u32) -> i32 {
    return bitcast<i32>(frame[b + PARAM + k]);
}

// A slot's tiles or upscaled image (sprites.rs `SlotDraw::draw`).
fn slot(b: u32, x: i32, y: i32, cur: vec4<u32>) -> vec4<u32> {
    let s = i32(g.s);
    let sx0 = pi(b, 0u);
    let sy0 = pi(b, 1u);
    let shift = pi(b, 2u);
    // Original pixels from the slot's top left.
    let ax = fdiv(x - shift, s) - sx0;
    let ay = y / s - sy0;
    let tx = fdiv(ax, 16);
    let ty = fdiv(ay, 16);
    if (tx < pi(b, 3u) || tx >= pi(b, 4u) || ty < 0 || ty >= pi(b, 5u)) {
        return cur;
    }
    let w = pi(b, 6u);
    var mc = pi(b, 7u) + tx;
    let mr = pi(b, 8u) + ty;
    let period = pi(b, 14u);
    if (period > 0 && (tx < 0 || tx >= w)) {
        let start = pi(b, 13u);
        mc = start + emod(tx - start, period);
    }
    let mw = pi(b, 9u);
    let mh = pi(b, 10u);
    if (mc < 0 || mc >= mw || mr < 0 || mr >= mh) {
        return cur;
    }
    let lx = ax - tx * 16;
    let ly = ay - ty * 16;
    var idx = 0u;
    if (p(b, 21u) != 0u) {
        var ic = mc;
        var ir = mr;
        if (p(b, 17u) != 0u) {
            ic = tx - pi(b, 18u);
            ir = ty;
        }
        let t = 16 * s;
        let hx = ic * t + (x - ((sx0 + tx * 16) * s + shift));
        let hy = ir * t + (y - (sy0 + ty * 16) * s);
        let hw = pi(b, 16u);
        if (hx < 0 || hx >= hw || hy < 0 || hy >= pi(b, 22u)) {
            return cur;
        }
        idx = byte_at(p(b, 15u), u32(hy * hw + hx));
    } else {
        let id = bank[p(b, 11u) + u32(mr * mw + mc)];
        if (id == 0u || id >= g.ntiles) {
            return cur;
        }
        let k = u32(ly * 16 + lx);
        idx = (bank[id * 64u + (k >> 2u)] >> ((k & 3u) * 8u)) & 255u;
    }
    if (idx == 0u) {
        return cur;
    }
    // Art under translated text (the parts' regions) is not drawn; the slot's own columns only.
    let n = p(b, 20u);
    if (n > 0u && tx >= 0 && tx < w) {
        let bx = (tx * 16 + lx) * s;
        let by = (ty * 16 + ly) * s;
        let r = p(b, 19u);
        for (var k = 0u; k < n; k++) {
            let rx = i32(blobs[r + 4u * k]);
            let ry = i32(blobs[r + 4u * k + 1u]);
            let rw = i32(blobs[r + 4u * k + 2u]);
            let rh = i32(blobs[r + 4u * k + 3u]);
            if (bx >= rx && bx < rx + rw && by >= ry && by < ry + rh) {
                return cur;
            }
        }
    }
    return pal(p(b, 12u), idx);
}

// A slot's paper fill (sprites.rs `draw_paper`, paper.rs `PaperFill::at`).
fn paper(b: u32, x: i32, y: i32, cur: vec4<u32>) -> vec4<u32> {
    if (x >= pi(b, 3u) && x < pi(b, 4u)) {
        return cur;
    }
    let s = i32(g.s);
    let px = fdiv(x - pi(b, 2u), s) - pi(b, 0u);
    let py = y / s - pi(b, 1u);
    if (py < 0 || py >= pi(b, 6u)) {
        return cur;
    }
    let width = pi(b, 5u);
    let ramp = pi(b, 7u);
    var off = p(b, 8u);
    var first = 0;
    var cols = 0;
    if (px < ramp) {
        first = pi(b, 9u);
        cols = pi(b, 10u);
    } else if (px >= width - ramp) {
        off += p(b, 11u);
        first = pi(b, 12u);
        cols = pi(b, 13u);
    } else {
        return cur;
    }
    let c = px - first;
    if (c < 0 || c >= cols) {
        return cur;
    }
    return unpack(blobs[off + u32(py * cols + c)]);
}

// One translated text part (text.rs `draw_parts`).
fn part(b: u32, x: i32, y: i32, cur: vec4<u32>) -> vec4<u32> {
    let rx = x - pi(b, 0u) - pi(b, 2u);
    let ry = y - pi(b, 1u) - pi(b, 3u);
    let i = u32(ry * pi(b, 4u) + rx);
    let base = byte_at(p(b, 6u), i);
    let c = byte_at(p(b, 8u), i);
    if (base == 0u && c == 0u) {
        return cur;
    }
    let pl = p(b, 7u);
    let fg_word = p(b, 10u);
    var fg_index = fg_word >> 8u;
    if ((fg_word & 1u) != 0u) {
        fg_index = byte_at(p(b, 9u), i);
    }
    let fg = pal(pl, fg_index);
    var under = cur;
    if (base != 0u) {
        under = pal(pl, base);
    }
    if (c == 255u || (under.a == 0u && c > 0u)) {
        return vec4<u32>(fg.rgb, select(c, 255u, c == 255u));
    }
    return vec4<u32>(mix8(under.r, fg.r, c), mix8(under.g, fg.g, c), mix8(under.b, fg.b, c), 255u);
}

// A label text widget (sprites.rs `draw_widget`, `WidgetKind::Text`).
fn text(b: u32, x: i32, y: i32, cur: vec4<u32>) -> vec4<u32> {
    let d = blobs[p(b, 3u) + u32((y - pi(b, 1u)) * pi(b, 2u) + x - pi(b, 0u))];
    let flags = p(b, 7u);
    var px = cur;
    if ((flags & 2u) != 0u && (d & 512u) != 0u) {
        px = unpack(p(b, 6u));
    }
    if ((flags & 1u) != 0u && (d & 256u) != 0u) {
        px = unpack(p(b, 5u));
    }
    let cov = d & 255u;
    if (cov != 0u) {
        let fg = unpack(p(b, 4u));
        if (px.a == 0u) {
            px = vec4<u32>(fg.rgb, cov);
        } else {
            px = vec4<u32>(mix8(px.r, fg.r, cov), mix8(px.g, fg.g, cov), mix8(px.b, fg.b, cov), 255u);
        }
    }
    return px;
}

// One group of a legend's blends (legend.rs `blend`).
fn legend_blend(b: u32, x: i32, y: i32, cur: vec4<u32>) -> vec4<u32> {
    let i = u32((y - pi(b, 2u)) * pi(b, 3u) + x - pi(b, 1u));
    let cov = byte_at(p(b, 4u), i);
    if (cov == 0u) {
        return cur;
    }
    let c = unpack(p(b, 0u));
    if (cur.a == 0u) {
        return vec4<u32>(c.rgb, cov);
    }
    return vec4<u32>(mix8(cur.r, c.r, cov), mix8(cur.g, c.g, cov), mix8(cur.b, c.b, cov), max(cur.a, cov));
}

// A picture over the canvas (sprites.rs `over`).
fn image(b: u32, x: i32, y: i32, cur: vec4<u32>) -> vec4<u32> {
    let i = u32((y - pi(b, 1u)) * pi(b, 2u) + x - pi(b, 0u));
    let src = unpack(blobs[p(b, 3u) + i]);
    var under = cur;
    if (under.a == 0u) {
        if (p(b, 5u) == 0u) {
            return src;
        }
        under = unpack(p(b, 4u));
    }
    let a = src.a;
    if (a == 0u) {
        return under;
    }
    let ua = under.a;
    let oa = a * 255u + ua * (255u - a);
    let k = ua * (255u - a);
    let r = (src.r * a * 255u + under.r * k + oa / 2u) / oa;
    let gg = (src.g * a * 255u + under.g * k + oa / 2u) / oa;
    let bb = (src.b * a * 255u + under.b * k + oa / 2u) / oa;
    return vec4<u32>(r, gg, bb, (oa + 127u) / 255u);
}

fn opaque(v: f32) -> f32 {
    return bitcast<f32>(bitcast<u32>(v) | g.zero);
}

// Rust's `f32::round`: halves away from zero (values here are positive).
fn round_away(v: f32) -> f32 {
    let r = floor(v);
    return select(r, r + 1.0, v - r >= 0.5);
}

fn fmix(u: u32, v: u32, a: f32, oma: f32) -> u32 {
    return u32(round_away(opaque(f32(u) * oma) + opaque(f32(v) * a)));
}

// A resampled sprite widget (sprites.rs `resample_sprite`).
fn resample(b: u32, x: i32, y: i32, cur: vec4<u32>) -> vec4<u32> {
    // The pixel's list of art pixels, then that list's colour with this palette.
    let list = blobs[p(b, 3u) + u32((y - pi(b, 1u)) * pi(b, 2u) + x - pi(b, 0u))];
    if (list == 0xffffffffu) {
        return cur;
    }
    let o = p(b, 4u) + 4u * list;
    if (blobs[o + 3u] == 0u) {
        return cur;
    }
    let c = unpack(blobs[o]);
    let a = bitcast<f32>(blobs[o + 1u]);
    if (a >= 1.0) {
        return vec4<u32>(c.rgb, 255u);
    }
    if (cur.a == 0u) {
        return c;
    }
    let oma = bitcast<f32>(blobs[o + 2u]);
    return vec4<u32>(fmix(cur.r, c.r, a, oma), fmix(cur.g, c.g, a, oma), fmix(cur.b, c.b, a, oma), max(cur.a, c.a));
}

// One row of 16 workgroups per bin to compose (its 64 × 64 pixels).
@compute @workgroup_size(16, 16)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) local: vec3<u32>) {
    let bin = frame[g.dirty_off + wg.y];
    let id = vec2<u32>(
        (bin % g.bins_x) * 64u + (wg.x % 4u) * 16u + local.x,
        (bin / g.bins_x) * 64u + (wg.x / 4u) * 16u + local.y,
    );
    if (id.x >= g.cw || id.y >= g.ch) {
        return;
    }
    let x = i32(id.x);
    let y = i32(id.y);
    var px = vec4<u32>(0u);
    let first = frame[g.bins_off + bin];
    let last = frame[g.bins_off + bin + 1u];
    for (var e = first; e < last; e++) {
        let b = frame[g.entries_off + e] * 32u;
        if (id.x < frame[b + 1u] || id.y < frame[b + 2u] || id.x >= frame[b + 3u] || id.y >= frame[b + 4u]) {
            continue;
        }
        // A framed slot keeps out of the columns outside the design on its rows.
        if (id.y >= frame[b + 7u] && id.y < frame[b + 8u] && (id.x < frame[b + 5u] || id.x >= frame[b + 6u])) {
            continue;
        }
        switch frame[b] {
            case 1u: {
                px = slot(b, x, y, px);
            }
            case 2u: {
                px = paper(b, x, y, px);
            }
            case 3u: {
                px = part(b, x, y, px);
            }
            case 4u: {
                px = unpack(p(b, 0u));
            }
            case 5u: {
                let i = u32((y - pi(b, 2u)) * i32(frame[b + 3u] - frame[b + 1u]) + x - pi(b, 1u));
                if (byte_at(p(b, 3u), i) != 0u) {
                    px = unpack(p(b, 0u));
                }
            }
            case 6u: {
                px = text(b, x, y, px);
            }
            case 7u: {
                px = legend_blend(b, x, y, px);
            }
            case 8u: {
                px = image(b, x, y, px);
            }
            case 9u: {
                px = resample(b, x, y, px);
            }
            default: {}
        }
    }
    // The clear colour under the design rectangle (sprites.rs `put_backdrop`).
    if (g.bd_on != 0u && id.x >= g.bd_x0 && id.x < g.bd_x1 && id.y < g.bd_y1) {
        let under = 255u - px.a;
        let c = unpack(g.bd_rgb);
        px = vec4<u32>(
            (px.r + c.r * under / 255u) & 255u,
            (px.g + c.g * under / 255u) & 255u,
            (px.b + c.b * under / 255u) & 255u,
            255u,
        );
    }
    textureStore(out, vec2<i32>(x, y), vec4<f32>(px) * (1.0 / 255.0));
}

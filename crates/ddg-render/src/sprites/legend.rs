//! Controller button glyphs and legend lines built from them (port only, the `hybrid_input`
//! hints): round face buttons with a letter, rounded keys for the shoulder, trigger and centre
//! buttons, a d-pad and a stick, drawn in code at canvas resolution with anti-aliased edges,
//! next to text in the label font. No image assets: the shapes are signed distances and the
//! letters come from the label font.

use super::Canvas;
use crate::label::LabelFont;
use crate::text::Align;

/// An sRGB colour.
pub type Rgb = [u8; 3];

/// What a glyph looks like.
#[derive(Clone, Debug, PartialEq)]
pub enum GlyphShape {
    /// A round button with `label` in the middle (the face buttons).
    Round,
    /// A rounded key with `label` in it, as wide as the label needs (LB, RT, Menu, …).
    Key,
    /// A d-pad cross; the arms whose flag is set (up, down, left, right) are lit.
    DPad { lit: [bool; 4] },
    /// A thumbstick: a ring around a cap with `label` on it.
    Stick,
}

/// One button glyph.
#[derive(Clone, Debug, PartialEq)]
pub struct Glyph {
    pub shape: GlyphShape,
    pub label: String,
    /// The body colour.
    pub fill: Rgb,
    /// The letter colour (and the lit arms of a d-pad).
    pub ink: Rgb,
}

/// A piece of a legend line.
#[derive(Clone, Debug, PartialEq)]
pub enum LegendPart {
    Glyph(Glyph),
    /// Text in the line's colour and size.
    Text(String),
    /// Space, in multiples of the line's cap height.
    Gap(f32),
}

/// Glyph height in multiples of the text's cap height: a button sits a little taller than the
/// capitals beside it, like the keycap art of the original legends.
pub const GLYPH_PER_CAP: f32 = 1.75;
/// Space after a glyph, in cap heights.
const AFTER_GLYPH: f32 = 0.35;

/// The width of a glyph `h` canvas pixels high.
fn glyph_width(g: &Glyph, h: f32, font: &LabelFont) -> f32 {
    match g.shape {
        GlyphShape::Round | GlyphShape::DPad { .. } | GlyphShape::Stick => h,
        GlyphShape::Key => (font.width(&g.label, h * 0.42) + h * 0.55).max(h * 1.3),
    }
}

/// Lays out `parts` on one line: the widths in canvas pixels of each part, the text cap and
/// the glyph height, scaled down evenly when the line is wider than `max_w`.
fn layout(parts: &[LegendPart], cap: f32, max_w: f32, font: &LabelFont) -> (Vec<f32>, f32) {
    let widths = |cap: f32| -> Vec<f32> {
        let h = cap * GLYPH_PER_CAP;
        parts
            .iter()
            .enumerate()
            .map(|(i, p)| match p {
                LegendPart::Glyph(g) => {
                    let gap = if matches!(parts.get(i + 1), Some(LegendPart::Text(_))) {
                        cap * AFTER_GLYPH
                    } else {
                        cap * 0.15
                    };
                    glyph_width(g, h, font) + gap
                }
                LegendPart::Text(t) => font.width(t, cap),
                LegendPart::Gap(k) => k * cap,
            })
            .collect()
    };
    let w = widths(cap);
    let total: f32 = w.iter().sum();
    if total <= max_w || total <= 0.0 {
        return (w, cap);
    }
    let cap = cap * max_w / total;
    (widths(cap), cap)
}

/// Draws a legend line into the canvas rectangle `[x0, x1) × [y0, y1)`: parts placed left to
/// right along `align`, vertically centred, text in `text_rgb`, on a rounded `backdrop` box
/// as wide as the line (plus half a cap each side) when one is given.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_legend(
    parts: &[LegendPart],
    cap: f32,
    align: Align,
    text_rgb: Rgb,
    backdrop: Option<Rgb>,
    font: &LabelFont,
    [x0, y0, x1, y1]: [usize; 4],
    c: Canvas,
    canvas: &mut [u8],
) {
    let (mw, mh) = ((x1 - x0) as f32, (y1 - y0) as f32);
    let cap = cap.min(mh / GLYPH_PER_CAP);
    let (widths, cap) = layout(parts, cap, mw, font);
    let total: f32 = widths.iter().sum();
    let mut x = x0 as f32
        + match align {
            Align::Left => 0.0,
            Align::Center => ((mw - total) / 2.0).max(0.0),
            Align::Right => (mw - total).max(0.0),
        };
    let h = cap * GLYPH_PER_CAP;
    let cy = (y0 + y1) as f32 / 2.0;
    if let Some(rgb) = backdrop {
        let pad = cap * 0.6;
        let (bx, bw) = (x - pad, total + 2.0 * pad);
        let half = (h * 0.62).min(mh / 2.0);
        let r = half * 0.5;
        let b = [bx, cy - half, bw, 2.0 * half];
        fill_sd(b, rgb, c, canvas, |px, py| {
            rounded_box(px, py, (bx + bw / 2.0, cy), (bw / 2.0, half), r)
        });
    }
    for (p, w) in parts.iter().zip(widths) {
        match p {
            LegendPart::Glyph(g) => {
                let gw = glyph_width(g, h, font);
                draw_glyph(g, [x, cy - h / 2.0, gw, h], font, c, canvas);
            }
            LegendPart::Text(t) => {
                // The text box: the part's width (plus a pixel of slack), the full line height.
                let bx = x.round().max(0.0) as usize;
                let bw = (w.ceil() as usize + 2).min(c.w.saturating_sub(bx));
                if bw > 0 {
                    let mask = font.mask(t, bw, y1 - y0, Some(cap), Align::Left, false);
                    for py in 0..(y1 - y0) {
                        for px in 0..bw {
                            let cov = mask[py * bw + px];
                            if cov > 0 && bx + px < x1 {
                                blend(canvas, c, bx + px, y0 + py, text_rgb, cov);
                            }
                        }
                    }
                }
            }
            LegendPart::Gap(_) => {}
        }
        x += w;
    }
}

/// Blends `rgb` at `cov` (0..=255) into the canvas pixel; over transparency the coverage
/// becomes the alpha, as for label text (the overlay's alpha test decides).
fn blend(canvas: &mut [u8], c: Canvas, px: usize, py: usize, rgb: Rgb, cov: u8) {
    if px >= c.w || py >= c.h || cov == 0 {
        return;
    }
    let o = (py * c.w + px) * 4;
    let under = [canvas[o], canvas[o + 1], canvas[o + 2], canvas[o + 3]];
    let out = if under[3] == 0 {
        [rgb[0], rgb[1], rgb[2], cov]
    } else {
        let t = u16::from(cov);
        let mix = |a: u8, b: u8| ((u16::from(a) * (255 - t) + u16::from(b) * t) / 255) as u8;
        [
            mix(under[0], rgb[0]),
            mix(under[1], rgb[1]),
            mix(under[2], rgb[2]),
            under[3].max(cov),
        ]
    };
    canvas[o..o + 4].copy_from_slice(&out);
}

/// Fills the shape whose signed distance (canvas pixels, negative inside) is `sd`, over the
/// box `[x, y, w, h]`, with one-pixel anti-aliasing.
fn fill_sd(
    [x, y, w, h]: [f32; 4],
    rgb: Rgb,
    c: Canvas,
    canvas: &mut [u8],
    sd: impl Fn(f32, f32) -> f32,
) {
    let (px0, py0) = (x.floor().max(0.0) as usize, y.floor().max(0.0) as usize);
    let px1 = ((x + w).ceil().max(0.0) as usize).min(c.w);
    let py1 = ((y + h).ceil().max(0.0) as usize).min(c.h);
    for py in py0..py1 {
        for px in px0..px1 {
            let d = sd(px as f32 + 0.5, py as f32 + 0.5);
            let cov = (0.5 - d).clamp(0.0, 1.0);
            if cov > 0.0 {
                blend(canvas, c, px, py, rgb, (cov * 255.0).round() as u8);
            }
        }
    }
}

/// Signed distance to a rounded rectangle centred at `(cx, cy)` with half sizes `(hw, hh)`
/// and corner radius `r`.
fn rounded_box(px: f32, py: f32, (cx, cy): (f32, f32), (hw, hh): (f32, f32), r: f32) -> f32 {
    let qx = (px - cx).abs() - (hw - r);
    let qy = (py - cy).abs() - (hh - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

fn darker(c: Rgb, k: f32) -> Rgb {
    c.map(|v| (f32::from(v) * k).round() as u8)
}

/// Draws one glyph in the canvas box `[x, y, w, h]` (canvas pixels).
fn draw_glyph(g: &Glyph, b: [f32; 4], font: &LabelFont, c: Canvas, canvas: &mut [u8]) {
    let [x, y, w, h] = b;
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    // A dark rim one-twelfth of the height wide keeps every glyph readable on any backdrop.
    let rim = (h / 12.0).max(1.0);
    let rim_rgb = darker(g.fill, 0.45);
    match &g.shape {
        GlyphShape::Round => {
            let r = h / 2.0;
            fill_sd(b, rim_rgb, c, canvas, |px, py| {
                ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - r
            });
            fill_sd(b, g.fill, c, canvas, |px, py| {
                ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - (r - rim)
            });
            label(&g.label, b, h * 0.5, g.ink, font, c, canvas);
        }
        GlyphShape::Key => {
            let (hw, hh) = (w / 2.0, h * 0.42);
            let r = hh * 0.55;
            fill_sd(b, rim_rgb, c, canvas, |px, py| {
                rounded_box(px, py, (cx, cy), (hw, hh), r)
            });
            fill_sd(b, g.fill, c, canvas, |px, py| {
                rounded_box(px, py, (cx, cy), (hw - rim, hh - rim), (r - rim).max(0.5))
            });
            label(&g.label, b, h * 0.36, g.ink, font, c, canvas);
        }
        GlyphShape::DPad { lit } => {
            // Two bars a third of the size wide make the cross; lit arms carry a triangle.
            let (arm, half) = (h / 6.0, h / 2.0);
            let cross = |px: f32, py: f32, inset: f32| {
                let v = rounded_box(px, py, (cx, cy), (arm - inset, half - inset), arm * 0.3);
                let hz = rounded_box(px, py, (cx, cy), (half - inset, arm - inset), arm * 0.3);
                v.min(hz)
            };
            fill_sd(b, rim_rgb, c, canvas, |px, py| cross(px, py, 0.0));
            fill_sd(b, g.fill, c, canvas, |px, py| cross(px, py, rim));
            let t = h * 0.13;
            let tips = [
                ((0.0, -1.0), lit[0]),
                ((0.0, 1.0), lit[1]),
                ((-1.0, 0.0), lit[2]),
                ((1.0, 0.0), lit[3]),
            ];
            for ((dx, dy), on) in tips {
                let ink = if on { g.ink } else { darker(g.fill, 0.6) };
                // A triangle pointing outwards, centred on the arm.
                let (ax, ay) = (cx + dx * half * 0.62, cy + dy * half * 0.62);
                let tri = [
                    (ax + dx * t, ay + dy * t),
                    (ax - dx * t * 0.6 + dy * t, ay - dy * t * 0.6 + dx * t),
                    (ax - dx * t * 0.6 - dy * t, ay - dy * t * 0.6 - dx * t),
                ];
                fill_sd(b, ink, c, canvas, |px, py| triangle(px, py, tri));
            }
        }
        GlyphShape::Stick => {
            let r = h / 2.0;
            fill_sd(b, rim_rgb, c, canvas, |px, py| {
                ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - r
            });
            fill_sd(b, darker(g.fill, 0.75), c, canvas, |px, py| {
                ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - (r - rim)
            });
            fill_sd(b, g.fill, c, canvas, |px, py| {
                ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - r * 0.62
            });
            label(&g.label, b, h * 0.4, g.ink, font, c, canvas);
        }
    }
}

/// Signed distance to a triangle (approximate outside the corners, exact enough for AA).
fn triangle(px: f32, py: f32, t: [(f32, f32); 3]) -> f32 {
    let area = (t[1].0 - t[0].0) * (t[2].1 - t[0].1) - (t[1].1 - t[0].1) * (t[2].0 - t[0].0);
    let sign = if area < 0.0 { -1.0 } else { 1.0 };
    let mut d = f32::MIN;
    for i in 0..3 {
        let (a, b) = (t[i], t[(i + 1) % 3]);
        let (ex, ey) = (b.0 - a.0, b.1 - a.1);
        let len = (ex * ex + ey * ey).sqrt().max(1e-6);
        // Outward normal distance of the point from edge a→b.
        let e = -sign * ((px - a.0) * ey - (py - a.1) * ex) / len;
        d = d.max(-e);
    }
    d
}

/// The glyph's label centred in its box, capitals `cap` canvas pixels high.
fn label(
    text: &str,
    [x, y, w, h]: [f32; 4],
    cap: f32,
    ink: Rgb,
    font: &LabelFont,
    c: Canvas,
    canvas: &mut [u8],
) {
    if text.is_empty() {
        return;
    }
    let (bx, by) = (x.round().max(0.0) as usize, y.round().max(0.0) as usize);
    let bw = (w.round() as usize).min(c.w.saturating_sub(bx));
    let bh = (h.round() as usize).min(c.h.saturating_sub(by));
    if bw == 0 || bh == 0 {
        return;
    }
    let mask = font.mask(text, bw, bh, Some(cap), Align::Center, false);
    for py in 0..bh {
        for px in 0..bw {
            blend(canvas, c, bx + px, by + py, ink, mask[py * bw + px]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::tests::BoxGlyphs;

    fn canvas(w: usize, h: usize) -> (Canvas, Vec<u8>) {
        (Canvas { w, h, s: 1 }, vec![0; w * h * 4])
    }

    fn round(label: &str, fill: Rgb) -> Glyph {
        Glyph {
            shape: GlyphShape::Round,
            label: label.into(),
            fill,
            ink: [255; 3],
        }
    }

    #[test]
    fn round_glyph_is_a_disc_with_its_colour() {
        let font = LabelFont::new(Box::new(BoxGlyphs));
        let (c, mut px) = canvas(40, 40);
        draw_glyph(
            &round("", [0, 200, 0]),
            [0.0, 0.0, 40.0, 40.0],
            &font,
            c,
            &mut px,
        );
        let at = |x: usize, y: usize| &px[(y * 40 + x) * 4..(y * 40 + x) * 4 + 4];
        assert_eq!(at(20, 20), [0, 200, 0, 255]);
        // The corners stay transparent; the rim is darker.
        assert_eq!(at(1, 1)[3], 0);
        assert!(at(20, 1)[1] < 200);
    }

    #[test]
    fn legend_lays_out_left_to_right_and_shrinks_to_fit() {
        let font = LabelFont::new(Box::new(BoxGlyphs));
        let parts = vec![
            LegendPart::Glyph(round("A", [0, 200, 0])),
            LegendPart::Text("Confirm".into()),
        ];
        let (w, cap) = layout(&parts, 10.0, 1000.0, &font);
        assert_eq!(cap, 10.0);
        assert!((w[0] - (17.5 + 3.5)).abs() < 1e-3);
        let total: f32 = w.iter().sum();
        let (w2, cap2) = layout(&parts, 10.0, total / 2.0, &font);
        assert!(cap2 < 10.0);
        assert!((w2.iter().sum::<f32>() - total / 2.0).abs() < 0.5);
        // Drawn: something green on the left, text ink to the right of it.
        let (c, mut px) = canvas(200, 24);
        draw_legend(
            &parts,
            10.0,
            Align::Left,
            [255, 255, 255],
            None,
            &font,
            [0, 0, 200, 24],
            c,
            &mut px,
        );
        let green = (0..24).any(|x| px[(12 * 200 + x) * 4 + 1] == 200);
        assert!(green);
        assert!(px.chunks(4).skip(30).any(|p| p == [255, 255, 255, 255]));
    }
}

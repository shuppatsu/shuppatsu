//! Which drawn surfaces are ground, for the shader enhancements `ground_variation` and
//! `ground_detiling` (docs/settings.md, renderer.md §14).
//!
//! The original has no notion of ground: terrain is ordinary part groups, drawn like
//! everything else (models.md). The port tells it apart from what the scene composer knows
//! and from the geometry:
//!
//! * **Source.** Only instances the composer marks as terrain candidates are looked at: the
//!   non-rail runs (kinds 3/4/7/8/≥ 9: field, snow, gravel and verge strips along the line,
//!   but also platforms; scene.md §2.7) and the live scenery (ground slabs, but also
//!   buildings and signs). Rail pieces, buffer stops, the tunnel plug, AI trains and the
//!   cab-view props never are.
//! * **Facing up.** A group counts when at least [`MIN_UP_AREA`] of its triangle area (in
//!   world space, after the placement's non-uniform scale) lies within [`MAX_SLOPE_DEG`] of
//!   horizontal. Walls, trees, poles and signs are upright.
//! * **Not raised on walls.** A part that also has walls (at least [`MIN_WALL_AREA`] of its
//!   area steeper than [`WALL_DEG`]) is a structure: platforms, canopies, flat roofs, viaduct
//!   decks. Its upward faces count only near the part's lowest point (within [`RAISED`]), so
//!   a platform top or a roof is not ground, but a verge at the foot of a wall is.
//! * **Repeating, per axis** (only for de-tiling). An axis may be shifted or mirrored only if
//!   the group repeats the texture along it (its UVs span at least one whole rect) **and** the
//!   rect's texture wraps without a seam along it (its first and last texel columns, or rows,
//!   differ no more than neighbouring columns do inside it). Transition textures (grass into
//!   gravel, a kerb along asphalt) fail the second test along the axis that would move their
//!   edge.

use crate::world::PageImage;
use ddg_formats::part::Part;
use ddg_formats::tex::TexRect;
use glam::{Mat4, Vec3};

/// Vertex surface bits (`Vertex::surface`, read by the shader).
pub const SURFACE_GROUND: u32 = 1;
/// The texture repeats seamlessly along u in this group: de-tiling may move it along u.
pub const SURFACE_TILE_U: u32 = 2;
/// The same along v.
pub const SURFACE_TILE_V: u32 = 4;

/// Steepest triangle still counted as facing up.
pub const MAX_SLOPE_DEG: f32 = 50.0;
/// Share of a group's area that must face up for it to be ground.
pub const MIN_UP_AREA: f32 = 0.6;
/// Steeper than this is a wall ([`PartShape`]).
pub const WALL_DEG: f32 = 70.0;
/// Share of a part's area in walls that makes it a structure.
pub const MIN_WALL_AREA: f32 = 0.12;
/// How far above a structure's lowest point (world units, ≈ 10 m; scene.md §1) an upward
/// group may lie and still be ground.
pub const RAISED: f32 = 0.04;
/// An edge pair may differ this much more than the average interior neighbour pair and still
/// count as seamless.
const SEAM_TOLERANCE: f32 = 2.0;
/// Plus this absolute margin (0..255 per channel), for flat textures whose interior
/// differences are near zero.
const SEAM_MARGIN: f32 = 6.0;

/// Area statistics of a set of triangles in world space.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Areas {
    total: f32,
    up: f32,
    wall: f32,
    /// Area-weighted sum of the triangles' centroid heights.
    weighted_y: f32,
}

fn areas(points: &[Vec3], triangles: impl Iterator<Item = [u16; 3]>) -> Areas {
    let up_cos = MAX_SLOPE_DEG.to_radians().cos();
    let wall_cos = WALL_DEG.to_radians().cos();
    let point = |i: u16| points.get(usize::from(i)).copied();
    let mut s = Areas::default();
    for [i, j, k] in triangles {
        let (Some(a), Some(b), Some(c)) = (point(i), point(j), point(k)) else {
            continue;
        };
        let n = (b - a).cross(c - a);
        let area = n.length();
        if area <= 0.0 {
            continue;
        }
        s.total += area;
        s.weighted_y += area * (a.y + b.y + c.y) / 3.0;
        // Either winding: the culling decides which side shows.
        let cos = n.y.abs() / area;
        if cos >= up_cos {
            s.up += area;
        } else if cos <= wall_cos {
            s.wall += area;
        }
    }
    s
}

/// Fraction of the triangles' area (`points` in world space) that faces up.
pub fn up_fraction(points: &[Vec3], triangles: impl Iterator<Item = [u16; 3]>) -> f32 {
    let s = areas(points, triangles);
    if s.total > 0.0 { s.up / s.total } else { 0.0 }
}

/// A whole part as placed: its lowest point and whether it is a structure (has walls).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartShape {
    pub min_y: f32,
    pub walled: bool,
}

impl PartShape {
    pub fn of(part: &Part, transform: &Mat4) -> Self {
        let mut min_y = f32::INFINITY;
        let (mut total, mut wall) = (0.0, 0.0);
        for g in &part.groups {
            let points: Vec<Vec3> = g
                .vertices
                .iter()
                .map(|v| transform.transform_point3(Vec3::from(v.position())))
                .collect();
            min_y = points.iter().map(|p| p.y).fold(min_y, f32::min);
            let s = areas(&points, g.triangles());
            total += s.total;
            wall += s.wall;
        }
        Self {
            min_y,
            walled: total > 0.0 && wall >= MIN_WALL_AREA * total,
        }
    }
}

/// Whether `rect`'s texels on `page` wrap without a seam along u and along v.
pub fn seamless_axes(page: &PageImage, rect: &TexRect) -> [bool; 2] {
    let (pw, ph) = (page.width as usize, page.height as usize);
    let span = |start: u8, size_minus_1: u8, px: usize| {
        let a = usize::from(start) * px / 256;
        let b = ((usize::from(start) + usize::from(size_minus_1) + 1) * px / 256).min(px);
        (a, b.max(a))
    };
    let (x0, x1) = span(rect.u, rect.w, pw);
    let (y0, y1) = span(rect.v, rect.h, ph);
    let texel = |(x, y): (usize, usize)| {
        let i = (y * pw + x) * 4;
        page.rgba.get(i..i + 4).map(|p| [p[0], p[1], p[2], p[3]])
    };
    // Mean absolute RGB difference between two lines of texels (opaque pairs only).
    let line_diff = |pairs: &mut dyn Iterator<Item = ((usize, usize), (usize, usize))>| {
        let (mut sum, mut n) = (0.0f32, 0u32);
        for (p, q) in pairs {
            let (Some(a), Some(b)) = (texel(p), texel(q)) else {
                continue;
            };
            if a[3] < 128 || b[3] < 128 {
                continue;
            }
            sum += (0..3)
                .map(|c| (f32::from(a[c]) - f32::from(b[c])).abs())
                .sum::<f32>()
                / 3.0;
            n += 1;
        }
        (n > 0).then(|| sum / n as f32)
    };
    // `edge`: the last line against the first; `inner`: each line against the next.
    let seamless = |len: usize, edge: Option<f32>, inner: Vec<f32>| match edge {
        Some(e) if len >= 4 && !inner.is_empty() => {
            e <= inner.iter().sum::<f32>() / inner.len() as f32 * SEAM_TOLERANCE + SEAM_MARGIN
        }
        _ => false,
    };
    let u = seamless(
        x1 - x0,
        line_diff(&mut (y0..y1).map(|y| ((x0, y), (x1 - 1, y)))),
        (x0..x1.saturating_sub(1))
            .filter_map(|x| line_diff(&mut (y0..y1).map(|y| ((x, y), (x + 1, y)))))
            .collect(),
    );
    let v = seamless(
        y1 - y0,
        line_diff(&mut (x0..x1).map(|x| ((x, y0), (x, y1 - 1)))),
        (y0..y1.saturating_sub(1))
            .filter_map(|y| line_diff(&mut (x0..x1).map(|x| ((x, y), (x, y + 1)))))
            .collect(),
    );
    [u, v]
}

/// The surface bits of one group of a terrain candidate: `points` in world space, `uvs` the
/// stored part UVs (1/256 rect units), `seams` from [`seamless_axes`], `shape` the whole
/// part's.
pub fn surface_bits(
    points: &[Vec3],
    triangles: impl Iterator<Item = [u16; 3]>,
    uvs: impl Iterator<Item = [i16; 2]>,
    seams: [bool; 2],
    shape: &PartShape,
) -> u32 {
    let s = areas(points, triangles);
    if s.total <= 0.0 || s.up < MIN_UP_AREA * s.total {
        return 0;
    }
    if shape.walled && s.weighted_y / s.total - shape.min_y > RAISED {
        return 0;
    }
    let (mut lo, mut hi) = ([i32::MAX; 2], [i32::MIN; 2]);
    for uv in uvs {
        for k in 0..2 {
            lo[k] = lo[k].min(i32::from(uv[k]));
            hi[k] = hi[k].max(i32::from(uv[k]));
        }
    }
    // A whole rect (256) or more along an axis: the texture repeats along it.
    let repeats = |k: usize| hi[k] >= lo[k] && hi[k] - lo[k] >= 255;
    let mut bits = SURFACE_GROUND;
    if repeats(0) && seams[0] {
        bits |= SURFACE_TILE_U;
    }
    if repeats(1) && seams[1] {
        bits |= SURFACE_TILE_V;
    }
    bits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> PageImage {
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgba.extend(f(x, y));
            }
        }
        PageImage {
            width: w,
            height: h,
            rgba,
        }
    }

    const FULL: TexRect = TexRect {
        id: 1,
        page: 0,
        u: 0,
        v: 0,
        w: 255,
        h: 255,
    };

    #[test]
    fn noise_wraps_but_a_gradient_does_not() {
        // A periodic pattern wraps both ways.
        let p = page(16, 16, |x, y| {
            let c = (((x * 7 + y * 3) % 4) * 40) as u8;
            [c, c, c, 255]
        });
        assert_eq!(seamless_axes(&p, &FULL), [true, true]);
        // A ramp across u (grass into gravel) has a seam along u only.
        let p = page(16, 16, |x, _| [(x * 16) as u8, 0, 0, 255]);
        assert_eq!(seamless_axes(&p, &FULL), [false, true]);
        // A sub-rect (the left half) is tested on its own texels.
        let half = TexRect { w: 127, ..FULL };
        let p = page(16, 16, |x, _| {
            if x < 8 {
                [9, 9, 9, 255]
            } else {
                [200, 0, 0, 255]
            }
        });
        assert_eq!(seamless_axes(&p, &half), [true, true]);
    }

    const OPEN: PartShape = PartShape {
        min_y: 0.0,
        walled: false,
    };

    #[test]
    fn flat_and_upright_groups() {
        let flat = [Vec3::ZERO, Vec3::X, Vec3::Z, Vec3::new(1.0, 0.0, 1.0)];
        let wall = [Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::new(1.0, 1.0, 0.0)];
        let tris = || [[0u16, 1, 2], [1, 3, 2]].into_iter();
        assert_eq!(up_fraction(&flat, tris()), 1.0);
        assert_eq!(up_fraction(&wall, tris()), 0.0);
        let uvs = || [[0i16, 0], [512, 0], [0, 100], [512, 100]].into_iter();
        // Repeats along u only (span 2 rects); v spans less than one rect.
        assert_eq!(
            surface_bits(&flat, tris(), uvs(), [true, true], &OPEN),
            SURFACE_GROUND | SURFACE_TILE_U
        );
        assert_eq!(
            surface_bits(&flat, tris(), uvs(), [false, true], &OPEN),
            SURFACE_GROUND
        );
        assert_eq!(surface_bits(&wall, tris(), uvs(), [true, true], &OPEN), 0);
        // On a structure, only near its foot.
        let raised = flat.map(|p| p + Vec3::Y * 0.1);
        let walled = PartShape {
            min_y: 0.0,
            walled: true,
        };
        assert_eq!(
            surface_bits(&raised, tris(), uvs(), [true, true], &walled),
            0
        );
        assert_eq!(
            surface_bits(&raised, tris(), uvs(), [true, true], &OPEN),
            SURFACE_GROUND | SURFACE_TILE_U
        );
        assert_ne!(surface_bits(&flat, tris(), uvs(), [true, true], &walled), 0);
    }

    #[test]
    fn a_platform_is_a_structure() {
        use ddg_formats::part::{Group, Vertex};
        let v = |x: i16, y: i16, z: i16| Vertex {
            pos: [x, y, z],
            uv: [0, 0],
        };
        // A 1-unit square top 0.4 units up on a front wall.
        let part = Part {
            groups: vec![
                Group {
                    rect_id: 1,
                    vertices: vec![
                        v(0, 100, 0),
                        v(256, 100, 0),
                        v(0, 100, 256),
                        v(256, 100, 256),
                    ],
                    indices: vec![0, 1, 2, 1, 3, 2],
                },
                Group {
                    rect_id: 2,
                    vertices: vec![v(0, 0, 0), v(256, 0, 0), v(0, 100, 0), v(256, 100, 0)],
                    indices: vec![0, 1, 2, 1, 3, 2],
                },
            ],
        };
        let shape = PartShape::of(&part, &Mat4::IDENTITY);
        assert!(shape.walled);
        assert_eq!(shape.min_y, 0.0);
    }
}

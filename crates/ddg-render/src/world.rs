//! CPU-side geometry. A [`PartLibrary`] holds texture pages, rects and parts (loaded once); a
//! [`FrameGeometry`] collects what to draw this frame: part instances transformed into
//! per-page batches, 2D background quads and debug lines. No GPU types here.

use ddg_formats::part::Part;
use ddg_formats::tex::{TexFile, TexRect};
use glam::{Mat4, Vec3};
use std::collections::HashMap;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineVertex {
    pub pos: [f32; 3],
    pub colour: [f32; 4],
}

/// Screen-space background vertex: normalised device coordinates and a linear-RGB colour.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BgVertex {
    pub pos: [f32; 2],
    pub colour: [f32; 4],
}

/// A texture page as RGBA8 in the **stored row order** (bottom-up picture), which is what the
/// game uploads and what its UVs address.
#[derive(Clone, Debug)]
pub struct PageImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Triangles that share one texture page.
#[derive(Clone, Debug, Default)]
pub struct Batch {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

/// Texture pages, rects and parts, following the game's lookup rules.
#[derive(Default)]
pub struct PartLibrary {
    pub pages: Vec<PageImage>,
    /// Global rect id → (global page index, rect). Later texture files win (table 0x825144).
    rects: HashMap<u16, (usize, TexRect)>,
    /// Part id → part. The first parts file wins (table 0x817c7c).
    parts: HashMap<u16, Part>,
}

impl PartLibrary {
    /// Adds a texture file; its pages get global indices after those already added.
    pub fn add_textures(&mut self, tex: &TexFile) {
        let base = self.pages.len();
        for page in &tex.pages {
            self.pages.push(PageImage {
                width: page.width.into(),
                height: page.height.into(),
                rgba: page.to_rgba8(),
            });
        }
        for r in &tex.rects {
            self.rects.insert(r.id, (base + usize::from(r.page), *r));
        }
    }

    /// Adds parts; ids already present are kept.
    pub fn add_parts(&mut self, parts: impl IntoIterator<Item = (u16, Part)>) {
        for (id, part) in parts {
            self.parts.entry(id).or_insert(part);
        }
    }

    pub fn part(&self, id: u32) -> Option<&Part> {
        self.parts.get(&u16::try_from(id).ok()?)
    }

    pub fn rect(&self, id: u16) -> Option<(usize, TexRect)> {
        self.rects.get(&id).copied()
    }

    /// Horizontal (XZ) bounding radius of a part around its origin, in world units.
    pub fn horizontal_radius(&self, id: u32) -> f32 {
        self.part(id).map_or(0.0, |p| {
            p.groups
                .iter()
                .flat_map(|g| &g.vertices)
                .map(|v| {
                    let [x, _, z] = v.position();
                    (x * x + z * z).sqrt()
                })
                .fold(0.0, f32::max)
        })
    }
}

/// Everything to draw in one frame.
#[derive(Clone, Debug, Default)]
pub struct FrameGeometry {
    /// Batch per page index (length = number of pages in the library).
    pub batches: Vec<Batch>,
    /// Background triangles (screen space), drawn first without depth.
    pub background: Vec<BgVertex>,
    /// Line list, for debug overlays such as track centre lines.
    pub lines: Vec<LineVertex>,
    pub missing_parts: usize,
}

impl FrameGeometry {
    /// Empties the frame but keeps allocations; sizes the batch list for `library`.
    pub fn clear(&mut self, library: &PartLibrary) {
        self.batches
            .resize_with(library.pages.len(), Batch::default);
        for b in &mut self.batches {
            b.vertices.clear();
            b.indices.clear();
        }
        self.background.clear();
        self.lines.clear();
        self.missing_parts = 0;
    }

    /// Adds part `id` with a model-to-world transform. Returns false if the part is unknown.
    pub fn add_instance(&mut self, library: &PartLibrary, id: u32, transform: &Mat4) -> bool {
        let Some(part) = library.part(id) else {
            self.missing_parts += 1;
            return false;
        };
        for g in &part.groups {
            let Some((page, rect)) = library.rect(g.rect_id) else {
                continue;
            };
            let batch = &mut self.batches[page];
            let base = batch.vertices.len() as u32;
            batch.vertices.extend(g.vertices.iter().map(|v| {
                Vertex {
                    pos: transform
                        .transform_point3(Vec3::from(v.position()))
                        .to_array(),
                    uv: rect.map_uv(v.uv),
                }
            }));
            batch
                .indices
                .extend(g.triangles().flatten().map(|i| base + u32::from(i)));
        }
        true
    }

    /// Adds a screen-space quad (corners in fan order) with per-corner colours.
    pub fn add_bg_quad(&mut self, corners: [[f32; 2]; 4], colours: [[f32; 4]; 4]) {
        for i in [0, 1, 2, 0, 2, 3] {
            self.background.push(BgVertex {
                pos: corners[i],
                colour: colours[i],
            });
        }
    }

    pub fn add_line_strip(&mut self, points: &[Vec3], colour: [f32; 4]) {
        for w in points.windows(2) {
            for p in w {
                self.lines.push(LineVertex {
                    pos: p.to_array(),
                    colour,
                });
            }
        }
    }

    pub fn triangle_count(&self) -> usize {
        self.batches.iter().map(|b| b.indices.len() / 3).sum()
    }
}

/// Placement transform in game space: translate, then rotate about +Y by `yaw` radians.
/// Same axes as the game's 0x4186a0 (local x → (cos, 0, −sin), local z → (sin, 0, cos)).
pub fn placement_transform(pos: Vec3, yaw: f32) -> Mat4 {
    Mat4::from_translation(pos) * Mat4::from_rotation_y(yaw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ddg_formats::part::{Group, Vertex as PartVertex};
    use ddg_formats::tex::TexPage;

    fn library() -> PartLibrary {
        let mut lib = PartLibrary::default();
        lib.add_textures(&TexFile {
            rects: vec![TexRect {
                id: 9,
                page: 1,
                u: 0,
                v: 0,
                w: 255,
                h: 255,
            }],
            pages: vec![
                TexPage {
                    width: 1,
                    height: 1,
                    pixels: vec![1],
                },
                TexPage {
                    width: 1,
                    height: 1,
                    pixels: vec![2],
                },
            ],
        });
        let v = |x, z| PartVertex {
            pos: [x, 0, z],
            uv: [0, 0],
        };
        lib.add_parts([(
            5,
            Part {
                groups: vec![Group {
                    rect_id: 9,
                    vertices: vec![v(0, 0), v(256, 0), v(512, 768)],
                    indices: vec![0, 1, 2],
                }],
            },
        )]);
        lib
    }

    #[test]
    fn instance_goes_into_its_page_batch() {
        let lib = library();
        let mut f = FrameGeometry::default();
        f.clear(&lib);
        assert!(f.add_instance(
            &lib,
            5,
            &placement_transform(Vec3::new(10.0, 0.0, 0.0), 0.0)
        ));
        assert!(!f.add_instance(&lib, 6, &Mat4::IDENTITY));
        assert!(f.batches[0].indices.is_empty());
        assert_eq!(f.batches[1].indices, [0, 1, 2]);
        assert_eq!(f.batches[1].vertices[1].pos, [11.0, 0.0, 0.0]);
        assert_eq!(f.missing_parts, 1);
        f.clear(&lib);
        assert_eq!(f.triangle_count(), 0);
    }

    #[test]
    fn radius_and_yaw_axes() {
        let lib = library();
        assert!((lib.horizontal_radius(5) - (4.0f32 + 9.0).sqrt()).abs() < 1e-6);
        let m = placement_transform(Vec3::ZERO, std::f32::consts::FRAC_PI_2);
        // local x → (cos, 0, −sin) = (0, 0, −1)
        assert!(m.transform_vector3(Vec3::X).abs_diff_eq(Vec3::NEG_Z, 1e-6));
    }
}

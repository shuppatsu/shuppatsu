//! CPU-side world geometry: parts and texture pages turned into per-page vertex batches in
//! game space, ready for upload. No GPU types here.

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

/// Everything the renderer draws for one scene.
#[derive(Clone, Debug, Default)]
pub struct World {
    pub pages: Vec<PageImage>,
    /// Batch per page index (same length as `pages`).
    pub batches: Vec<Batch>,
    /// Line list (pairs of vertices), for debug overlays such as track centre lines.
    pub lines: Vec<LineVertex>,
}

/// Builds a [`World`] from texture files and parts, following the game's lookup rules.
#[derive(Default)]
pub struct WorldBuilder {
    world: World,
    /// Global rect id → (global page index, rect). Later texture files win (table 0x825144).
    rects: HashMap<u16, (usize, TexRect)>,
    /// Part id → part. The first parts file wins (table 0x817c7c).
    parts: HashMap<u16, Part>,
    pub missing_parts: usize,
    pub missing_rects: usize,
}

impl WorldBuilder {
    /// Adds a texture file; its pages get global indices after those already added.
    pub fn add_textures(&mut self, tex: &TexFile) {
        let base = self.world.pages.len();
        for page in &tex.pages {
            self.world.pages.push(PageImage {
                width: page.width.into(),
                height: page.height.into(),
                rgba: page.to_rgba8(),
            });
            self.world.batches.push(Batch::default());
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

    pub fn has_part(&self, id: u16) -> bool {
        self.parts.contains_key(&id)
    }

    /// Instances part `id` with a model-to-world transform. Returns false if the part is unknown.
    pub fn add_instance(&mut self, id: u16, transform: Mat4) -> bool {
        let Some(part) = self.parts.get(&id) else {
            self.missing_parts += 1;
            return false;
        };
        for g in &part.groups {
            let Some(&(page, rect)) = self.rects.get(&g.rect_id) else {
                self.missing_rects += 1;
                continue;
            };
            let batch = &mut self.world.batches[page];
            let base = batch.vertices.len() as u32;
            for v in &g.vertices {
                let p = transform.transform_point3(Vec3::from(v.position()));
                batch.vertices.push(Vertex {
                    pos: p.to_array(),
                    uv: rect.map_uv(v.uv),
                });
            }
            batch
                .indices
                .extend(g.triangles().flatten().map(|i| base + u32::from(i)));
        }
        true
    }

    pub fn add_line_strip(&mut self, points: &[Vec3], colour: [f32; 4]) {
        for w in points.windows(2) {
            for p in w {
                self.world.lines.push(LineVertex {
                    pos: p.to_array(),
                    colour,
                });
            }
        }
    }

    pub fn finish(self) -> World {
        self.world
    }
}

/// Placement transform in game space: translate, then rotate about +Y by `yaw` radians
/// (verified visually against track direction in the Phase 2 glTF export).
pub fn placement_transform(pos: Vec3, yaw: f32) -> Mat4 {
    Mat4::from_translation(pos) * Mat4::from_rotation_y(yaw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ddg_formats::part::{Group, Vertex as PartVertex};
    use ddg_formats::tex::TexPage;

    #[test]
    fn instance_goes_into_its_page_batch() {
        let mut b = WorldBuilder::default();
        let tex = TexFile {
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
        };
        b.add_textures(&tex);
        let v = |x| PartVertex {
            pos: [x, 0, 0],
            uv: [0, 0],
        };
        let part = Part {
            groups: vec![Group {
                rect_id: 9,
                vertices: vec![v(0), v(256), v(512)],
                indices: vec![0, 1, 2],
            }],
        };
        b.add_parts([(5, part)]);
        assert!(b.add_instance(5, placement_transform(Vec3::new(10.0, 0.0, 0.0), 0.0)));
        assert!(!b.add_instance(6, Mat4::IDENTITY));
        let w = b.finish();
        assert!(w.batches[0].indices.is_empty());
        assert_eq!(w.batches[1].indices, [0, 1, 2]);
        assert_eq!(w.batches[1].vertices[1].pos, [11.0, 0.0, 0.0]);
    }
}

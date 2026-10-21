//! CPU-side geometry. A [`PartLibrary`] holds texture pages, rects and parts (loaded once); a
//! [`FrameGeometry`] collects what to draw this frame: part instances transformed into
//! per-page batches, 2D background quads and debug lines. No GPU types here.

use glam::{Mat4, Vec3};
use shuppatsu_formats::part::Part;
use shuppatsu_formats::tex::{TexFile, TexRect};
use std::collections::HashMap;

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    /// The stored part UV in rect units (1.0 = one repeat of the rect): `uv = rect.xy +
    /// tile · rect.zw`. For the shader's in-rect wrap (`ground_detiling`).
    pub tile: [f32; 2],
    /// The group's rect in page UV: origin and the size of one repeat ([`TexRect::map_uv`]).
    pub rect: [f32; 4],
    /// [`crate::ground`] surface bits (0 for everything that is not ground).
    pub surface: u32,
    /// Which instance of the frame the vertex belongs to, counting from 0 in the order they
    /// were added ([`FrameGeometry::instances`]). The original draws part by part
    /// (`Model_Draw` 0x431f70) under a LESSEQUAL Z test, so of two coplanar surfaces the later
    /// one shows; the shader orders surfaces inside one depth step by this, so the batches can
    /// be drawn by texture page and still resolve like the original (renderer.md §6.1).
    pub order: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineVertex {
    pub pos: [f32; 3],
    pub colour: [f32; 4],
}

/// Screen-space background vertex: normalised device coordinates (z = reversed depth, see
/// [`bg_depth`]) and a linear-RGB colour.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BgVertex {
    pub pos: [f32; 3],
    pub colour: [f32; 4],
}

/// Screen-space textured background vertex: NDC (z = reversed depth) and a page UV.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BgTexVertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
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
    /// Global rect id → whether its texture wraps seamlessly along u and v
    /// ([`crate::ground::seamless_axes`] of the page it was added with), worked out the first
    /// time a terrain group uses the rect.
    seams: HashMap<u16, (usize, std::sync::OnceLock<[bool; 2]>)>,
}

impl PartLibrary {
    /// Adds a texture file; its pages get global indices after those already added.
    pub fn add_textures(&mut self, tex: &TexFile) {
        let base = self.pages.len();
        // Pages convert independently: on all cores (a stage start's scene load waits for them).
        self.pages
            .extend(crate::par::par_map(&tex.pages, |page| PageImage {
                width: page.width.into(),
                height: page.height.into(),
                rgba: page.to_rgba8(),
            }));
        for r in &tex.rects {
            self.rects.insert(r.id, (base + usize::from(r.page), *r));
            self.seams.insert(
                r.id,
                (base + usize::from(r.page), std::sync::OnceLock::new()),
            );
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

    /// Whether rect `id`'s texture wraps seamlessly along u and v ([`crate::ground`]).
    pub fn seams(&self, id: u16) -> [bool; 2] {
        let (Some((page, cell)), Some((_, rect))) = (self.seams.get(&id), self.rects.get(&id))
        else {
            return [false; 2];
        };
        *cell.get_or_init(|| {
            self.pages
                .get(*page)
                .map_or([false; 2], |p| crate::ground::seamless_axes(p, rect))
        })
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
    /// Instances added so far ([`Vertex::order`] of the next one).
    pub instances: u32,
    /// Background triangles (screen space), drawn first without depth.
    pub background: Vec<BgVertex>,
    /// Textured background triangles per page (screen space), drawn after `background`,
    /// before the world, without depth. Same length as `batches`.
    pub background_tex: Vec<Vec<BgTexVertex>>,
    /// Line list, for debug overlays such as track centre lines.
    pub lines: Vec<LineVertex>,
    pub missing_parts: usize,
    /// The far plane the background's depth words are placed for (enhancement
    /// `draw_distance`, [`crate::camera::background_depth`]); `None` = the original's.
    pub far: Option<f32>,
    /// World point the vertex positions are stored relative to: set it near the camera before
    /// adding instances, for depth precision far from the world origin (the renderer adds it
    /// back). Kept by [`FrameGeometry::clear`].
    pub origin: Vec3,
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
        self.instances = 0;
        self.background.clear();
        self.background_tex
            .resize_with(library.pages.len(), Vec::new);
        for b in &mut self.background_tex {
            b.clear();
        }
        self.lines.clear();
        self.missing_parts = 0;
    }

    /// Adds part `id` with a model-to-world transform. Returns false if the part is unknown.
    pub fn add_instance(&mut self, library: &PartLibrary, id: u32, transform: &Mat4) -> bool {
        self.add_part(library, id, transform, false)
    }

    /// [`FrameGeometry::add_instance`] for a terrain candidate (non-rail runs and scenery):
    /// its groups that face up are marked as ground for the `ground_variation` and
    /// `ground_detiling` enhancements ([`crate::ground`]). Drawn the same otherwise.
    pub fn add_terrain_instance(
        &mut self,
        library: &PartLibrary,
        id: u32,
        transform: &Mat4,
    ) -> bool {
        self.add_part(library, id, transform, true)
    }

    fn add_part(
        &mut self,
        library: &PartLibrary,
        id: u32,
        transform: &Mat4,
        terrain: bool,
    ) -> bool {
        let Some(part) = library.part(id) else {
            self.missing_parts += 1;
            return false;
        };
        let shape = terrain.then(|| crate::ground::PartShape::of(part, transform));
        // In f64 and relative to the origin: coplanar parts placed with different transforms
        // then differ only by the GPU's rounding of small numbers (renderer.md §6.1).
        let to_world = transform.as_dmat4();
        let origin = self.origin.as_dvec3();
        for g in &part.groups {
            let Some((page, rect)) = library.rect(g.rect_id) else {
                continue;
            };
            let batch = &mut self.batches[page];
            let base = batch.vertices.len() as u32;
            let [u0, v0] = rect.map_uv([0, 0]);
            let [u1, v1] = rect.map_uv([256, 256]);
            let rect_uv = [u0, v0, u1 - u0, v1 - v0];
            batch.vertices.extend(g.vertices.iter().map(|v| {
                let world = to_world.transform_point3(Vec3::from(v.position()).as_dvec3());
                Vertex {
                    pos: (world - origin).as_vec3().to_array(),
                    uv: rect.map_uv(v.uv),
                    tile: v.uv.map(|c| f32::from(c) / 256.0),
                    rect: rect_uv,
                    surface: 0,
                    order: self.instances,
                }
            }));
            if let Some(shape) = &shape {
                let added = &mut batch.vertices[base as usize..];
                let points: Vec<Vec3> = added
                    .iter()
                    .map(|v| Vec3::from(v.pos) + self.origin)
                    .collect();
                let bits = crate::ground::surface_bits(
                    &points,
                    g.triangles(),
                    g.vertices.iter().map(|v| v.uv),
                    library.seams(g.rect_id),
                    shape,
                );
                for v in added {
                    v.surface = bits;
                }
            }
            batch
                .indices
                .extend(g.triangles().flatten().map(|i| base + u32::from(i)));
        }
        self.instances += 1;
        true
    }

    /// Adds a screen-space quad (corners in fan order) with per-corner colours at the
    /// original's 2D depth word `depth` (z = depth / 32768).
    pub fn add_bg_quad(&mut self, corners: [[f32; 2]; 4], colours: [[f32; 4]; 4], depth: u16) {
        let z = self.bg_z(depth);
        for i in [0, 1, 2, 0, 2, 3] {
            self.background.push(BgVertex {
                pos: [corners[i][0], corners[i][1], z],
                colour: colours[i],
            });
        }
    }

    /// Adds a textured screen-space quad (corners in fan order) on texture page `page` at the
    /// original's 2D depth word `depth`.
    pub fn add_bg_tex_quad(
        &mut self,
        page: usize,
        corners: [[f32; 2]; 4],
        uvs: [[f32; 2]; 4],
        depth: u16,
    ) {
        let z = self.bg_z(depth);
        if let Some(list) = self.background_tex.get_mut(page) {
            for i in [0, 1, 2, 0, 2, 3] {
                list.push(BgTexVertex {
                    pos: [corners[i][0], corners[i][1], z],
                    uv: uvs[i],
                });
            }
        }
    }

    pub fn add_line_strip(&mut self, points: &[Vec3], colour: [f32; 4]) {
        for w in points.windows(2) {
            for p in w {
                self.lines.push(LineVertex {
                    pos: (*p - self.origin).to_array(),
                    colour,
                });
            }
        }
    }

    /// The renderer's depth of a 2D depth word for this frame's far plane.
    fn bg_z(&self, depth: u16) -> f32 {
        match self.far {
            Some(far) => crate::camera::background_depth(depth, far),
            None => bg_depth(depth),
        }
    }

    pub fn triangle_count(&self) -> usize {
        self.batches.iter().map(|b| b.indices.len() / 3).sum()
    }
}

/// The original's 2D quads carry a depth word (z = depth / 32768 in D3D's 0 = near … 1 = far
/// range; renderer.md §8). The renderer uses reversed depth, so this returns 1 − z.
pub fn bg_depth(depth: u16) -> f32 {
    1.0 - f32::from(depth) / 32768.0
}

/// Placement transform in game space: translate, then rotate about +Y by `yaw` radians.
/// Same axes as the game's 0x4186a0 (local x → (cos, 0, −sin), local z → (sin, 0, cos)).
pub fn placement_transform(pos: Vec3, yaw: f32) -> Mat4 {
    Mat4::from_translation(pos) * Mat4::from_rotation_y(yaw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use shuppatsu_formats::part::{Group, Vertex as PartVertex};
    use shuppatsu_formats::tex::TexPage;

    fn library() -> PartLibrary {
        let mut lib = PartLibrary::default();
        lib.add_textures(&TexFile {
            rects: vec![
                TexRect {
                    id: 9,
                    page: 1,
                    u: 0,
                    v: 0,
                    w: 255,
                    h: 255,
                },
                TexRect {
                    id: 8,
                    page: 0,
                    u: 0,
                    v: 0,
                    w: 255,
                    h: 255,
                },
            ],
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
        lib.add_parts([
            (
                5,
                Part {
                    groups: vec![Group {
                        rect_id: 9,
                        vertices: vec![v(0, 0), v(256, 0), v(512, 768)],
                        indices: vec![0, 1, 2],
                    }],
                },
            ),
            (
                7,
                Part {
                    groups: vec![Group {
                        rect_id: 8,
                        vertices: vec![v(0, 0), v(256, 0), v(512, 768)],
                        indices: vec![0, 1, 2],
                    }],
                },
            ),
        ]);
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
    fn positions_are_relative_to_the_origin() {
        let lib = library();
        let mut f = FrameGeometry {
            origin: Vec3::new(1000.0, 2.0, -500.0),
            ..FrameGeometry::default()
        };
        f.clear(&lib);
        let at = placement_transform(Vec3::new(1000.5, 2.0, -500.0), 0.0);
        f.add_instance(&lib, 5, &at);
        assert_eq!(f.batches[1].vertices[1].pos, [1.5, 0.0, 0.0]);
        f.add_line_strip(&[Vec3::new(1000.0, 3.0, -500.0), Vec3::ZERO], [1.0; 4]);
        assert_eq!(f.lines[0].pos, [0.0, 1.0, 0.0]);
    }

    #[test]
    fn vertices_carry_their_instance_order() {
        let lib = library();
        let mut f = FrameGeometry::default();
        f.clear(&lib);
        for id in [5, 7, 6, 5] {
            f.add_instance(&lib, id, &Mat4::IDENTITY);
        }
        // The unknown part 6 takes no number.
        assert_eq!(f.instances, 3);
        let orders = |page: usize| -> Vec<u32> {
            f.batches[page].vertices.iter().map(|v| v.order).collect()
        };
        assert_eq!(orders(1), [0, 0, 0, 2, 2, 2]);
        assert_eq!(orders(0), [1, 1, 1]);
        f.clear(&lib);
        assert_eq!(f.instances, 0);
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

//! Screen-space effect quads drawn over the 3D view: the driving view's weather overlay (wiper,
//! drops, snow layers; docs/re/renderer.md §13).
//!
//! The original submits these as TLVERTEX fans from task draw callbacks, with the same Z test
//! and write as every other 2D quad and a depth word close to 0 (near). The renderer draws them
//! after the world in the 3D band with the background pipelines, so the depth words resolve
//! overlaps the way they did in the original: the wiper (depth 0) covers the drops (2), which
//! cover the snow layers (0x10 / 0x100 / 0x200), and colour-keyed texels write nothing.

use crate::world::{BgTexVertex, BgVertex, bg_depth};

/// Fan order 0-1-2-3 as two triangles.
const FAN: [usize; 6] = [0, 1, 2, 0, 2, 3];

/// One frame's effect quads. Corners are normalised device coordinates of the 3D band (the
/// caller maps the original's 496×400 virtual space, like the background quads).
#[derive(Clone, Debug, Default)]
pub struct EffectGeometry {
    /// Untextured triangles.
    pub flat: Vec<BgVertex>,
    /// Textured triangles per texture page (index = page index of the frame's library).
    pub textured: Vec<Vec<BgTexVertex>>,
}

impl EffectGeometry {
    /// Empties the geometry but keeps allocations.
    pub fn clear(&mut self) {
        self.flat.clear();
        for t in &mut self.textured {
            t.clear();
        }
    }

    pub fn is_empty(&self) -> bool {
        self.flat.is_empty() && self.textured.iter().all(Vec::is_empty)
    }

    /// Adds a flat quad (corners in fan order) with one linear-RGB colour at the original's 2D
    /// depth word `depth`.
    pub fn add_flat_quad(&mut self, corners: [[f32; 2]; 4], colour: [f32; 4], depth: u16) {
        let z = bg_depth(depth);
        self.flat.extend(FAN.map(|i| BgVertex {
            pos: [corners[i][0], corners[i][1], z],
            colour,
        }));
    }

    /// Adds a textured quad (corners in fan order) on page `page`; `uvs` are page UVs
    /// (1.0 = page size, wrapping).
    pub fn add_tex_quad(
        &mut self,
        page: usize,
        corners: [[f32; 2]; 4],
        uvs: [[f32; 2]; 4],
        depth: u16,
    ) {
        if self.textured.len() <= page {
            self.textured.resize_with(page + 1, Vec::new);
        }
        let z = bg_depth(depth);
        self.textured[page].extend(FAN.map(|i| BgTexVertex {
            pos: [corners[i][0], corners[i][1], z],
            uv: uvs[i],
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quads_become_two_triangles() {
        let mut g = EffectGeometry::default();
        assert!(g.is_empty());
        let c = [[-1.0, 1.0], [1.0, 1.0], [1.0, -1.0], [-1.0, -1.0]];
        g.add_flat_quad(c, [0.0, 0.0, 0.0, 1.0], 0);
        g.add_tex_quad(2, c, [[0.0; 2]; 4], 2);
        assert_eq!(g.flat.len(), 6);
        assert_eq!(g.textured.len(), 3);
        assert_eq!(g.textured[2].len(), 6);
        assert_eq!(g.flat[4].pos[..2], c[2]);
        // Depth word 0 is the nearest 2D depth (reversed depth 1.0).
        assert_eq!(g.flat[0].pos[2], 1.0);
        g.clear();
        assert!(g.is_empty());
    }
}

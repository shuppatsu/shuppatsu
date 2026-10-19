//! Model parts: the meshes stored in `parts*.rom` (docs/re/models.md).
//!
//! A parts entry is `u16 part id` followed by a packed block; [`crate::lz::unpack_block`]
//! turns the block into the byte stream parsed here:
//!
//! ```text
//! u16 nGroups
//! nGroups × {
//!     u16 rectId                       texture rectangle (tex*.rom record id)
//!     u16 nVerts
//!     nVerts × { i16 x, y, z, u, v }   all fixed point, 1/256
//!     u16 nIdx
//!     nIdx × index                     u8 if nVerts <= 255, else u16; no padding
//! }
//! ```
//!
//! Each group is one textured triangle list, drawn pre-lit with a constant white diffuse.

use crate::error::{FormatError, Result};
use crate::reader::Bytes;
use crate::tex::TexRect;

const WHAT: &str = "part";

/// Fixed-point scale of positions and UVs (the float 1/256 at exe 0x47c95c).
pub const FIXED_SCALE: f32 = 1.0 / 256.0;

/// Diffuse colour the game writes into every model vertex (ARGB, opaque white;
/// `part_get_unpacked` @0x432840 stores the constant 0xffffffff). Specular is 0.
pub const VERTEX_DIFFUSE: u32 = 0xffff_ffff;

/// Groups with more vertices than this store u16 indices; up to this, u8 (exe 0x432aa1).
pub const MAX_BYTE_INDEX_VERTS: u16 = 255;

/// A vertex as stored: position and rect-relative UV, all in 1/256 units.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vertex {
    pub pos: [i16; 3],
    pub uv: [i16; 2],
}

impl Vertex {
    /// Position in game units (raw / 256), in the file's own axes.
    pub fn position(&self) -> [f32; 3] {
        self.pos.map(|c| f32::from(c) * FIXED_SCALE)
    }
}

/// One textured triangle list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    /// Global texture-rectangle id (record id in `tex*.rom`).
    pub rect_id: u16,
    pub vertices: Vec<Vertex>,
    /// Triangle-list indices into `vertices`, widened to u16.
    pub indices: Vec<u16>,
}

impl Group {
    /// The group's UVs mapped into its texture page via `rect` (see [`TexRect::map_uv`]).
    pub fn page_uvs(&self, rect: &TexRect) -> Vec<[f32; 2]> {
        self.vertices.iter().map(|v| rect.map_uv(v.uv)).collect()
    }

    /// Triangles as index triples. Any trailing indices that don't fill a triangle are ignored,
    /// as D3D does for a triangle list.
    pub fn triangles(&self) -> impl Iterator<Item = [u16; 3]> + '_ {
        self.indices.as_chunks::<3>().0.iter().copied()
    }
}

/// A parsed model part.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Part {
    pub groups: Vec<Group>,
}

impl Part {
    /// Parses an unpacked part. The stream must be consumed exactly.
    pub fn parse(data: &[u8]) -> Result<Self> {
        let (part, used) = Self::parse_prefix(data)?;
        if used != data.len() {
            return Err(FormatError::invalid(
                WHAT,
                format!(
                    "{} trailing bytes after offset {used:#x}",
                    data.len() - used
                ),
            ));
        }
        Ok(part)
    }

    /// Parses an unpacked part from the start of `data`; returns it with the bytes consumed.
    /// Rejects indices that point past the group's vertices.
    pub fn parse_prefix(data: &[u8]) -> Result<(Self, usize)> {
        let b = Bytes::new(data, WHAT);
        let n_groups = b.u16(0)?;
        let mut pos = 2;
        let mut groups = Vec::with_capacity(usize::from(n_groups));
        for g in 0..n_groups {
            let rect_id = b.u16(pos)?;
            let n_verts = b.u16(pos + 2)?;
            pos += 4;
            let vert_bytes = b.slice(pos, usize::from(n_verts) * 10)?;
            let vertices = vert_bytes
                .as_chunks::<10>()
                .0
                .iter()
                .map(|c| {
                    let w = |i: usize| i16::from_le_bytes([c[i], c[i + 1]]);
                    Vertex {
                        pos: [w(0), w(2), w(4)],
                        uv: [w(6), w(8)],
                    }
                })
                .collect();
            pos += vert_bytes.len();
            let n_idx = usize::from(b.u16(pos)?);
            pos += 2;
            let indices: Vec<u16> = if n_verts <= MAX_BYTE_INDEX_VERTS {
                let s = b.slice(pos, n_idx)?;
                pos += n_idx;
                s.iter().map(|&i| u16::from(i)).collect()
            } else {
                let s = b.slice(pos, n_idx * 2)?;
                pos += n_idx * 2;
                s.as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&c| u16::from_le_bytes(c))
                    .collect()
            };
            if let Some(&bad) = indices.iter().find(|&&i| i >= n_verts) {
                return Err(FormatError::invalid(
                    WHAT,
                    format!("group {g}: index {bad} out of range for {n_verts} vertices"),
                ));
            }
            groups.push(Group {
                rect_id,
                vertices,
                indices,
            });
        }
        Ok((Self { groups }, pos))
    }

    /// Parses a raw `parts*.rom` entry (u16 part id + packed block). Returns the id and the part.
    pub fn parse_entry(entry: &[u8]) -> Result<(u16, Self)> {
        let b = Bytes::new(entry, "parts entry");
        let id = b.u16(0)?;
        let unpacked = crate::lz::unpack_block(&entry[2..])?;
        Ok((id, Self::parse(&unpacked)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push16(out: &mut Vec<u8>, v: u16) {
        out.extend_from_slice(&v.to_le_bytes());
    }

    fn group_bytes(rect: u16, verts: &[[i16; 5]], idx: &[u16]) -> Vec<u8> {
        let mut out = Vec::new();
        push16(&mut out, rect);
        push16(&mut out, verts.len() as u16);
        for v in verts {
            for c in v {
                out.extend_from_slice(&c.to_le_bytes());
            }
        }
        push16(&mut out, idx.len() as u16);
        for &i in idx {
            if verts.len() <= 255 {
                out.push(i as u8);
            } else {
                push16(&mut out, i);
            }
        }
        out
    }

    #[test]
    fn parses_two_groups_with_odd_byte_index_count() {
        let mut data = vec![];
        push16(&mut data, 2);
        data.extend(group_bytes(
            7,
            &[[256, -512, 0, 0, 256], [0, 0, 0, 128, 0], [-1, 2, 3, 4, 5]],
            &[0, 1, 2],
        ));
        // Odd byte count above, so this group starts unaligned: no padding in the stream.
        data.extend(group_bytes(9, &[[1, 2, 3, 4, 5]; 3], &[2, 1, 0, 0, 1, 2]));
        let part = Part::parse(&data).unwrap();
        assert_eq!(part.groups.len(), 2);
        let g = &part.groups[0];
        assert_eq!(g.rect_id, 7);
        assert_eq!(g.vertices[0].position(), [1.0, -2.0, 0.0]);
        assert_eq!(g.vertices[2].uv, [4, 5]);
        assert_eq!(g.indices, [0, 1, 2]);
        assert_eq!(part.groups[1].triangles().count(), 2);
    }

    #[test]
    fn wide_indices_above_255_vertices() {
        let verts = vec![[0i16; 5]; 256];
        let mut data = vec![1, 0];
        data.extend(group_bytes(0, &verts, &[255, 0, 1]));
        let part = Part::parse(&data).unwrap();
        assert_eq!(part.groups[0].indices, [255, 0, 1]);
        // At exactly 255 vertices indices are still bytes.
        let mut data = vec![1, 0];
        data.extend(group_bytes(0, &vec![[0i16; 5]; 255], &[254, 0, 1]));
        assert_eq!(data.len(), 2 + 4 + 2550 + 2 + 3);
        assert!(Part::parse(&data).is_ok());
    }

    #[test]
    fn rejects_trailing_truncated_and_out_of_range() {
        let mut data = vec![1, 0];
        data.extend(group_bytes(0, &[[0; 5]; 3], &[0, 1, 2]));
        let mut trailing = data.clone();
        trailing.push(0);
        assert!(matches!(
            Part::parse(&trailing),
            Err(FormatError::Invalid { .. })
        ));
        assert!(matches!(
            Part::parse(&data[..data.len() - 1]),
            Err(FormatError::Truncated { .. })
        ));
        let last = data.len() - 1;
        data[last] = 3;
        assert!(matches!(
            Part::parse(&data),
            Err(FormatError::Invalid { .. })
        ));
        assert_eq!(Part::parse(&[0, 0]).unwrap(), Part::default());
    }

    #[test]
    fn uv_mapping_matches_game_formula() {
        let rect = TexRect {
            id: 0,
            page: 3,
            u: 64,
            v: 32,
            w: 127,
            h: 63,
        };
        // Full rect corner: u spans w+1 texels, v spans h texels.
        assert_eq!(rect.map_uv([256, 256]), [192.0 / 256.0, 95.0 / 256.0]);
        assert_eq!(rect.map_uv([0, 0]), [64.0 / 256.0, 32.0 / 256.0]);
        assert_eq!(rect.map_uv([128, 0]), [128.0 / 256.0, 32.0 / 256.0]);
    }
}

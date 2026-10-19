//! Read-only access to the original executable's initialised data by virtual address.
//!
//! Several tables the game needs (2D palettes, stage tables, bindings) live in the exe's
//! `.rdata`/`.data`. CLAUDE.md asks to read them from the user's exe at runtime instead of
//! hardcoding them, so this maps VAs to file bytes using the PE section table.

use crate::error::{FormatError, Result};
use crate::reader::Bytes;

/// One mapped section: VA range and backing file bytes (raw size may be < virtual size).
#[derive(Debug, Clone)]
struct Section {
    name: String,
    va: u32,
    virtual_size: u32,
    raw_offset: u32,
    raw_size: u32,
}

/// A parsed PE image borrowing the file bytes (32-bit; a 64-bit PE32+ image is mapped at base
/// 0).
#[derive(Debug, Clone)]
pub struct Exe<'a> {
    data: &'a [u8],
    image_base: u32,
    sections: Vec<Section>,
    /// RVA and size of the resource directory (data directory 2), if present.
    resource_dir: Option<(u32, u32)>,
}

impl<'a> Exe<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let b = Bytes::new(data, "PE");
        if b.array::<2>(0)? != *b"MZ" {
            return Err(FormatError::BadMagic {
                what: "PE (MZ)",
                found: b.array::<2>(0)?.to_vec(),
            });
        }
        let pe = b.usize32(0x3c)?;
        if b.array::<4>(pe)? != *b"PE\0\0" {
            return Err(FormatError::BadMagic {
                what: "PE signature",
                found: b.array::<4>(pe)?.to_vec(),
            });
        }
        let section_count = b.u16(pe + 6)? as usize;
        let optional_size = b.u16(pe + 20)? as usize;
        let optional = pe + 24;
        // PE32 keeps its base. A PE32+ (64-bit) base does not fit this reader's 32-bit VAs, so
        // such an image is mapped at 0: its VAs are RVAs (enough for its resources).
        let (image_base, dirs) = match b.u16(optional)? {
            0x10b => (b.u32(optional + 28)?, 92),
            0x20b => (0, 108),
            _ => {
                return Err(FormatError::invalid(
                    "PE",
                    "not a PE32 or PE32+ optional header",
                ));
            }
        };
        // Data directories follow NumberOfRvaAndSizes; entry 2 is resources.
        let dir_count = b.u32(optional + dirs)?;
        let first = dirs + 4;
        let resource_dir = if dir_count > 2 && optional_size >= first + 3 * 8 {
            let rva = b.u32(optional + first + 2 * 8)?;
            let size = b.u32(optional + first + 2 * 8 + 4)?;
            (rva != 0 && size != 0).then_some((rva, size))
        } else {
            None
        };
        let table = optional + optional_size;
        let mut sections = Vec::with_capacity(section_count);
        for i in 0..section_count {
            let s = table + i * 40;
            let name = b.slice(s, 8)?;
            let name = String::from_utf8_lossy(name)
                .trim_end_matches('\0')
                .to_owned();
            sections.push(Section {
                name,
                virtual_size: b.u32(s + 8)?,
                va: image_base.wrapping_add(b.u32(s + 12)?),
                raw_size: b.u32(s + 16)?,
                raw_offset: b.u32(s + 20)?,
            });
        }
        Ok(Self {
            data,
            image_base,
            sections,
            resource_dir,
        })
    }

    pub fn image_base(&self) -> u32 {
        self.image_base
    }

    /// Names of the sections, in file order.
    pub fn section_names(&self) -> impl Iterator<Item = &str> {
        self.sections.iter().map(|s| s.name.as_str())
    }

    /// `len` bytes of initialised data at virtual address `va`. Fails if the range is not fully
    /// backed by file bytes (e.g. it reaches into zero-initialised memory).
    pub fn bytes_at(&self, va: u32, len: usize) -> Result<&'a [u8]> {
        let section = self
            .sections
            .iter()
            .find(|s| va >= s.va && va - s.va < s.virtual_size.max(s.raw_size))
            .ok_or_else(|| FormatError::invalid("PE", format!("VA {va:#x} not in any section")))?;
        let rel = (va - section.va) as usize;
        if rel + len > section.raw_size as usize {
            return Err(FormatError::invalid(
                "PE",
                format!(
                    "VA {va:#x}+{len:#x} not backed by file data in {}",
                    section.name
                ),
            ));
        }
        Bytes::new(self.data, "PE").slice(section.raw_offset as usize + rel, len)
    }

    /// A [`Bytes`] reader over `len` bytes at `va`.
    pub fn reader_at(&self, va: u32, len: usize, what: &'static str) -> Result<Bytes<'a>> {
        Ok(Bytes::new(self.bytes_at(va, len)?, what))
    }

    pub fn u32_at(&self, va: u32) -> Result<u32> {
        self.reader_at(va, 4, "PE")?.u32(0)
    }

    /// Every leaf of the resource tree (type / name / language), in directory order. Empty when
    /// the image has no resource directory.
    pub fn resources(&self) -> Result<Vec<Resource<'a>>> {
        let Some((rva, size)) = self.resource_dir else {
            return Ok(Vec::new());
        };
        let tree = self.reader_at(self.image_base.wrapping_add(rva), size as usize, "PE rsrc")?;
        let mut out = Vec::new();
        for (kind, types) in res_entries(&tree, 0)? {
            let ResEntry::Dir(names) = types else {
                continue;
            };
            for (name, langs) in res_entries(&tree, names)? {
                let ResEntry::Dir(langs) = langs else {
                    continue;
                };
                for (lang, leaf) in res_entries(&tree, langs)? {
                    let ResEntry::Data(entry) = leaf else {
                        continue;
                    };
                    let data_rva = tree.u32(entry)?;
                    let data_size = tree.usize32(entry + 4)?;
                    let data = self.bytes_at(self.image_base.wrapping_add(data_rva), data_size)?;
                    let lang = match lang {
                        ResId::Id(id) => id,
                        ResId::Name(_) => 0,
                    };
                    out.push(Resource {
                        kind: kind.clone(),
                        name: name.clone(),
                        lang,
                        data,
                    });
                }
            }
        }
        Ok(out)
    }

    /// The resource of type `kind` with numeric `id` (first language).
    pub fn resource(&self, kind: u16, id: u16) -> Result<Option<&'a [u8]>> {
        Ok(self
            .resources()?
            .into_iter()
            .find(|r| r.kind == ResId::Id(kind) && r.name == ResId::Id(id))
            .map(|r| r.data))
    }
}

/// `RT_ICON`: one icon image (a DIB or a PNG).
pub const RT_ICON: u16 = 3;
/// `RT_GROUP_ICON`: a directory of `RT_ICON` images.
pub const RT_GROUP_ICON: u16 = 14;

/// A resource type, name or language: numeric or a UTF-16 string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResId {
    Id(u16),
    Name(String),
}

/// One resource leaf.
#[derive(Debug, Clone)]
pub struct Resource<'a> {
    pub kind: ResId,
    pub name: ResId,
    pub lang: u16,
    pub data: &'a [u8],
}

enum ResEntry {
    /// Offset of a subdirectory within the resource section.
    Dir(usize),
    /// Offset of an `IMAGE_RESOURCE_DATA_ENTRY`.
    Data(usize),
}

/// Depth guard: a resource tree has exactly three levels, so offsets must not loop.
const MAX_RES_ENTRIES: usize = 4096;

/// The entries of the `IMAGE_RESOURCE_DIRECTORY` at `dir` (16-byte header, then named and
/// numbered 8-byte entries).
fn res_entries(tree: &Bytes<'_>, dir: usize) -> Result<Vec<(ResId, ResEntry)>> {
    let count = tree.u16(dir + 12)? as usize + tree.u16(dir + 14)? as usize;
    if count > MAX_RES_ENTRIES {
        return Err(FormatError::invalid(
            "PE rsrc",
            "too many directory entries",
        ));
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let e = dir + 16 + i * 8;
        let name = tree.u32(e)?;
        let target = tree.u32(e + 4)?;
        let id = if name & 0x8000_0000 != 0 {
            let at = (name & 0x7fff_ffff) as usize;
            let len = tree.u16(at)? as usize;
            let units: Vec<u16> = tree
                .slice(at + 2, len * 2)?
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            ResId::Name(String::from_utf16_lossy(&units))
        } else {
            ResId::Id(name as u16)
        };
        let off = (target & 0x7fff_ffff) as usize;
        let entry = if target & 0x8000_0000 != 0 {
            if off <= dir {
                return Err(FormatError::invalid(
                    "PE rsrc",
                    "directory points backwards",
                ));
            }
            ResEntry::Dir(off)
        } else {
            ResEntry::Data(off)
        };
        out.push((id, entry));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal PE32 with one section `.data` at RVA 0x1000, 0x10 raw bytes at file 0x200.
    fn tiny_pe() -> Vec<u8> {
        let mut f = vec![0u8; 0x210];
        f[0..2].copy_from_slice(b"MZ");
        f[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        f[0x40..0x44].copy_from_slice(b"PE\0\0");
        f[0x46..0x48].copy_from_slice(&1u16.to_le_bytes()); // sections
        f[0x54..0x56].copy_from_slice(&0xe0u16.to_le_bytes()); // optional header size
        let opt = 0x58;
        f[opt..opt + 2].copy_from_slice(&0x10bu16.to_le_bytes());
        f[opt + 28..opt + 32].copy_from_slice(&0x400000u32.to_le_bytes());
        let s = opt + 0xe0;
        f[s..s + 5].copy_from_slice(b".data");
        f[s + 8..s + 12].copy_from_slice(&0x100u32.to_le_bytes()); // virtual size
        f[s + 12..s + 16].copy_from_slice(&0x1000u32.to_le_bytes()); // RVA
        f[s + 16..s + 20].copy_from_slice(&0x10u32.to_le_bytes()); // raw size
        f[s + 20..s + 24].copy_from_slice(&0x200u32.to_le_bytes()); // raw offset
        f[0x200..0x204].copy_from_slice(&0xdead_beefu32.to_le_bytes());
        f
    }

    #[test]
    fn pe32_plus_maps_at_zero() {
        // The same image as a PE32+: a 0xf0-byte optional header and a 64-bit base.
        let mut f = vec![0u8; 0x220];
        f[0..2].copy_from_slice(b"MZ");
        f[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        f[0x40..0x44].copy_from_slice(b"PE\0\0");
        f[0x46..0x48].copy_from_slice(&1u16.to_le_bytes());
        f[0x54..0x56].copy_from_slice(&0xf0u16.to_le_bytes());
        let opt = 0x58;
        f[opt..opt + 2].copy_from_slice(&0x20bu16.to_le_bytes());
        f[opt + 24..opt + 32].copy_from_slice(&0x1_4000_0000u64.to_le_bytes());
        let s = opt + 0xf0;
        f[s..s + 5].copy_from_slice(b".data");
        f[s + 8..s + 12].copy_from_slice(&0x100u32.to_le_bytes());
        f[s + 12..s + 16].copy_from_slice(&0x1000u32.to_le_bytes());
        f[s + 16..s + 20].copy_from_slice(&0x10u32.to_le_bytes());
        f[s + 20..s + 24].copy_from_slice(&0x210u32.to_le_bytes());
        f[0x210..0x214].copy_from_slice(&0xdead_beefu32.to_le_bytes());
        let exe = Exe::parse(&f).unwrap();
        assert_eq!(exe.image_base(), 0);
        assert_eq!(exe.u32_at(0x1000).unwrap(), 0xdead_beef);
        assert!(exe.resources().unwrap().is_empty());
    }

    #[test]
    fn maps_va_to_file() {
        let f = tiny_pe();
        let exe = Exe::parse(&f).unwrap();
        assert_eq!(exe.u32_at(0x401000).unwrap(), 0xdead_beef);
        assert!(exe.bytes_at(0x40100c, 8).is_err()); // runs past raw data (BSS)
        assert!(exe.bytes_at(0x500000, 1).is_err());
        assert_eq!(exe.section_names().collect::<Vec<_>>(), [".data"]);
    }
}

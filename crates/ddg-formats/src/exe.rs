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

/// A parsed 32-bit PE image borrowing the file bytes.
#[derive(Debug, Clone)]
pub struct Exe<'a> {
    data: &'a [u8],
    image_base: u32,
    sections: Vec<Section>,
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
        if b.u16(optional)? != 0x10b {
            return Err(FormatError::invalid("PE", "not a PE32 optional header"));
        }
        let image_base = b.u32(optional + 28)?;
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
    fn maps_va_to_file() {
        let f = tiny_pe();
        let exe = Exe::parse(&f).unwrap();
        assert_eq!(exe.u32_at(0x401000).unwrap(), 0xdead_beef);
        assert!(exe.bytes_at(0x40100c, 8).is_err()); // runs past raw data (BSS)
        assert!(exe.bytes_at(0x500000, 1).is_err());
        assert_eq!(exe.section_names().collect::<Vec<_>>(), [".data"]);
    }
}

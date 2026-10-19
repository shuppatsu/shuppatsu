//! `ROM2` container: `"ROM2"`, u32 count, then count × (u32 offset, u32 size).
//!
//! Offsets are from the start of the file. Used by `tbl.rom`, `bgm.rom`, `effect.rom` and
//! `parts*.rom`. Evidence: docs/re/loaders.md (relocation at 0x42fef0), confirmed against every
//! container on the 3000-bandai disc.

use crate::error::{FormatError, Result};
use crate::reader::Bytes;

pub const MAGIC: &[u8; 4] = b"ROM2";

/// A parsed `ROM2` container borrowing the file bytes.
#[derive(Debug, Clone)]
pub struct Rom2<'a> {
    data: &'a [u8],
    entries: Vec<(usize, usize)>,
}

impl<'a> Rom2<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let b = Bytes::new(data, "ROM2");
        let magic = b.array::<4>(0)?;
        if &magic != MAGIC {
            return Err(FormatError::BadMagic {
                what: "ROM2",
                found: magic.to_vec(),
            });
        }
        let count = b.usize32(4)?;
        // Reject absurd counts before allocating: the table alone must fit in the file.
        if count > data.len().saturating_sub(8) / 8 {
            return Err(FormatError::invalid(
                "ROM2",
                format!("count {count} too large"),
            ));
        }
        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            let offset = b.usize32(8 + i * 8)?;
            let size = b.usize32(12 + i * 8)?;
            b.slice(offset, size)?;
            entries.push((offset, size));
        }
        Ok(Self { data, entries })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Bytes of entry `index`.
    pub fn get(&self, index: usize) -> Option<&'a [u8]> {
        let &(offset, size) = self.entries.get(index)?;
        Some(&self.data[offset..offset + size])
    }

    /// (offset, size) of entry `index` within the file.
    pub fn span(&self, index: usize) -> Option<(usize, usize)> {
        self.entries.get(index).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = &'a [u8]> + '_ {
        (0..self.len()).filter_map(|i| self.get(i))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(entries: &[&[u8]]) -> Vec<u8> {
        let mut out = b"ROM2".to_vec();
        out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        let mut offset = 8 + 8 * entries.len();
        for e in entries {
            out.extend_from_slice(&(offset as u32).to_le_bytes());
            out.extend_from_slice(&(e.len() as u32).to_le_bytes());
            offset += e.len();
        }
        for e in entries {
            out.extend_from_slice(e);
        }
        out
    }

    #[test]
    fn roundtrip() {
        let file = build(&[b"abc", b"", b"hello"]);
        let rom = Rom2::parse(&file).unwrap();
        assert_eq!(rom.len(), 3);
        assert_eq!(rom.get(0).unwrap(), b"abc");
        assert_eq!(rom.get(1).unwrap(), b"");
        assert_eq!(rom.get(2).unwrap(), b"hello");
        assert!(rom.get(3).is_none());
    }

    #[test]
    fn rejects_bad_input() {
        assert!(matches!(
            Rom2::parse(b"ROM3\0\0\0\0"),
            Err(FormatError::BadMagic { .. })
        ));
        let mut file = build(&[b"abc"]);
        file.truncate(file.len() - 1);
        assert!(Rom2::parse(&file).is_err());
        let mut huge = b"ROM2".to_vec();
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(Rom2::parse(&huge).is_err());
    }
}

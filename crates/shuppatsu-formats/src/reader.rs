//! Bounds-checked little-endian reads at explicit offsets.

use crate::error::{FormatError, Result};

/// A view over input bytes with a label used in error messages.
#[derive(Clone, Copy)]
pub struct Bytes<'a> {
    data: &'a [u8],
    what: &'static str,
}

impl<'a> Bytes<'a> {
    pub fn new(data: &'a [u8], what: &'static str) -> Self {
        Self { data, what }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// `len` bytes starting at `offset`.
    pub fn slice(&self, offset: usize, len: usize) -> Result<&'a [u8]> {
        offset
            .checked_add(len)
            .and_then(|end| self.data.get(offset..end))
            .ok_or(FormatError::Truncated {
                what: self.what,
                offset,
                need: len,
                len: self.data.len(),
            })
    }

    pub fn array<const N: usize>(&self, offset: usize) -> Result<[u8; N]> {
        let s = self.slice(offset, N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(s);
        Ok(out)
    }

    pub fn u8(&self, offset: usize) -> Result<u8> {
        Ok(self.array::<1>(offset)?[0])
    }

    pub fn i8(&self, offset: usize) -> Result<i8> {
        Ok(self.u8(offset)? as i8)
    }

    pub fn u16(&self, offset: usize) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array(offset)?))
    }

    pub fn i16(&self, offset: usize) -> Result<i16> {
        Ok(i16::from_le_bytes(self.array(offset)?))
    }

    pub fn u32(&self, offset: usize) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array(offset)?))
    }

    pub fn i32(&self, offset: usize) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array(offset)?))
    }

    pub fn f32(&self, offset: usize) -> Result<f32> {
        Ok(f32::from_le_bytes(self.array(offset)?))
    }

    /// Reads a u32 and converts it to `usize` (offsets and counts).
    pub fn usize32(&self, offset: usize) -> Result<usize> {
        Ok(self.u32(offset)? as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_bounds() {
        let b = Bytes::new(&[1, 2, 3, 4, 0xff], "test");
        assert_eq!(b.u16(0).unwrap(), 0x0201);
        assert_eq!(b.u32(0).unwrap(), 0x0403_0201);
        assert_eq!(b.i8(4).unwrap(), -1);
        assert!(matches!(b.u32(2), Err(FormatError::Truncated { .. })));
        assert!(b.slice(usize::MAX, 2).is_err());
    }
}

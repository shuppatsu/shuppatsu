//! MSB-first bit reader shared by the MPEG-1 video and audio decoders.
//!
//! Reads past the end of the input return zero bits and set an overrun flag instead of
//! panicking; callers check [`BitReader::overrun`] at sync points and turn it into an error.

use crate::{FormatError, Result};

#[derive(Clone)]
pub(crate) struct BitReader<'a> {
    data: &'a [u8],
    /// Position in bits from the start of `data`.
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub(crate) fn at_byte(data: &'a [u8], byte: usize) -> Self {
        Self {
            data,
            pos: byte.saturating_mul(8),
        }
    }

    pub(crate) fn set_byte_pos(&mut self, byte: usize) {
        self.pos = byte.saturating_mul(8);
    }

    /// Byte position, rounded down.
    pub(crate) fn byte_pos(&self) -> usize {
        self.pos / 8
    }

    pub(crate) fn len_bits(&self) -> usize {
        self.data.len() * 8
    }

    /// True once a read went beyond the end of the input.
    pub(crate) fn overrun(&self) -> bool {
        self.pos > self.len_bits()
    }

    /// Returns an error naming `what` if a read went past the end.
    pub(crate) fn check(&self, what: &'static str) -> Result<()> {
        if self.overrun() {
            Err(FormatError::Truncated {
                what,
                offset: self.data.len(),
                need: self.pos.div_ceil(8) - self.data.len(),
                len: self.data.len(),
            })
        } else {
            Ok(())
        }
    }

    /// The next `n` bits (0..=32) without consuming them; bits past the end read as zero.
    #[inline]
    pub(crate) fn peek(&self, n: u32) -> u32 {
        debug_assert!(n <= 32);
        if n == 0 {
            return 0;
        }
        let byte = self.pos >> 3;
        let mut word: u64 = 0;
        for i in 0..5 {
            let b = self.data.get(byte + i).copied().unwrap_or(0);
            word = (word << 8) | u64::from(b);
        }
        // `word` holds 40 bits starting at `byte`; drop the bits already consumed.
        let shift = 40 - (self.pos & 7) as u32 - n;
        ((word >> shift) & ((1u64 << n) - 1)) as u32
    }

    #[inline]
    pub(crate) fn skip(&mut self, n: u32) {
        self.pos = self.pos.saturating_add(n as usize);
    }

    #[inline]
    pub(crate) fn read(&mut self, n: u32) -> u32 {
        let v = self.peek(n);
        self.skip(n);
        v
    }

    #[inline]
    pub(crate) fn bit(&mut self) -> bool {
        self.read(1) != 0
    }

    pub(crate) fn align(&mut self) {
        self.pos = self.pos.div_ceil(8) * 8;
    }

    /// Aligns, then advances to the next `00 00 01 xx` and returns `xx`, leaving the reader just
    /// after it. `None` at the end of the input.
    pub(crate) fn next_start_code(&mut self) -> Option<u8> {
        self.align();
        let start = self.byte_pos();
        let found = find_start_code(self.data, start)?;
        self.pos = (found + 4) * 8;
        Some(self.data[found + 3])
    }
}

/// Offset of the next `00 00 01 xx` at or after `from` (with `xx` present).
pub(crate) fn find_start_code(data: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 3 < data.len() {
        // Skip quickly: a start code needs data[i + 2] == 1 and two zeros before it.
        let b2 = data[i + 2];
        if b2 > 1 {
            i += 3;
        } else if b2 == 1 && data[i] == 0 && data[i + 1] == 0 {
            return Some(i);
        } else {
            i += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_msb_first_across_bytes() {
        let data = [0b1010_1100, 0b0101_0011, 0xff];
        let mut r = BitReader::new(&data);
        assert_eq!(r.read(3), 0b101);
        assert_eq!(r.read(7), 0b011_0001);
        assert_eq!(r.peek(6), 0b01_0011);
        assert_eq!(r.read(14), 0b01_0011_1111_1111);
        assert!(!r.overrun());
        assert_eq!(r.read(4), 0);
        assert!(r.overrun());
        assert!(r.check("t").is_err());
    }

    #[test]
    fn peeks_32_bits_unaligned() {
        let data = [0x12, 0x34, 0x56, 0x78, 0x9a];
        let mut r = BitReader::new(&data);
        r.skip(4);
        assert_eq!(r.peek(32), 0x2345_6789);
    }

    #[test]
    fn finds_start_codes() {
        let data = [0xff, 0, 0, 1, 0xb3, 9, 0, 0, 0, 1, 0xb8, 0];
        let mut r = BitReader::new(&data);
        assert_eq!(r.next_start_code(), Some(0xb3));
        assert_eq!(r.byte_pos(), 5);
        assert_eq!(r.next_start_code(), Some(0xb8));
        assert_eq!(r.next_start_code(), None);
        assert_eq!(find_start_code(&[0, 0, 1], 0), None);
    }
}

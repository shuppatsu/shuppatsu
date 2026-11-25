//! Raw DEFLATE decompression (RFC 1951), for the InstallShield cabinet on the disc
//! ([`crate::installshield`]). Hand-written to stay within the dependency allowlist.
//!
//! Only decompression of a complete stream held in memory: [`inflate`] decodes blocks until
//! the final one and appends the output to a vector. Huffman codes are decoded through a
//! [`FAST_BITS`]-bit lookup table, with a canonical walk for the rare longer codes.

use crate::error::{FormatError, Result};

const WHAT: &str = "deflate";

/// Lookahead of the decode table; longer codes take the slow path.
const FAST_BITS: u32 = 10;
const MAX_BITS: usize = 15;

/// Base lengths and extra bits of length codes 257..=285.
const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
/// Base distances and extra bits of distance codes 0..=29.
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
/// The order code-length code lengths are sent in (RFC 1951 §3.2.7).
const CLEN_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

fn bad(detail: &str) -> FormatError {
    FormatError::invalid(WHAT, detail)
}

/// Decompresses one raw DEFLATE stream (no zlib or gzip header) from the start of `input`,
/// appending to `out`. Returns the number of input bytes the stream used, rounded up to
/// whole bytes; anything after it is ignored.
pub fn inflate(input: &[u8], out: &mut Vec<u8>) -> Result<usize> {
    let mut bits = Bits::new(input);
    loop {
        let last = bits.take(1)? == 1;
        match bits.take(2)? {
            0 => stored(&mut bits, out)?,
            1 => {
                let (lit, dist) = fixed_codes();
                codes(&mut bits, out, &lit, &dist)?;
            }
            2 => {
                let (lit, dist) = dynamic_codes(&mut bits)?;
                codes(&mut bits, out, &lit, &dist)?;
            }
            _ => return Err(bad("reserved block type 3")),
        }
        if last {
            return Ok(bits.consumed());
        }
    }
}

/// LSB-first bit reader.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    buf: u64,
    count: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            buf: 0,
            count: 0,
        }
    }

    /// Tops the buffer up to at least 32 bits while input lasts.
    #[inline]
    fn refill(&mut self) {
        while self.count <= 56 {
            let Some(&b) = self.data.get(self.pos) else {
                return;
            };
            self.buf |= u64::from(b) << self.count;
            self.pos += 1;
            self.count += 8;
        }
    }

    /// The next `n` (≤ 32) bits without consuming them; missing input reads as zero.
    #[inline]
    fn peek(&mut self, n: u32) -> u32 {
        if self.count < n {
            self.refill();
        }
        (self.buf & ((1u64 << n) - 1)) as u32
    }

    #[inline]
    fn skip(&mut self, n: u32) -> Result<()> {
        if self.count < n {
            self.refill();
            if self.count < n {
                return Err(FormatError::Truncated {
                    what: WHAT,
                    offset: self.data.len(),
                    need: 1,
                    len: self.data.len(),
                });
            }
        }
        self.buf >>= n;
        self.count -= n;
        Ok(())
    }

    #[inline]
    fn take(&mut self, n: u32) -> Result<u32> {
        let v = self.peek(n);
        self.skip(n)?;
        Ok(v)
    }

    /// Drops the bits up to the next byte boundary.
    fn align(&mut self) {
        let r = self.count % 8;
        self.buf >>= r;
        self.count -= r;
    }

    /// Input bytes used so far (whole bytes, buffered bits given back).
    fn consumed(&self) -> usize {
        self.pos - (self.count / 8) as usize
    }
}

/// A canonical Huffman code: a fast table for codes up to [`FAST_BITS`] long, and the
/// per-length counts and sorted symbols for the rest.
struct Huffman {
    /// Indexed by the next `FAST_BITS` input bits: `symbol << 4 | length`, 0 = slow path.
    fast: Vec<u16>,
    counts: [u16; MAX_BITS + 1],
    symbols: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Result<Self> {
        let mut counts = [0u16; MAX_BITS + 1];
        for &l in lengths {
            counts[usize::from(l)] += 1;
        }
        counts[0] = 0;
        // Over-subscribed sets are invalid; incomplete ones are allowed (single-code trees).
        let mut left: i32 = 1;
        for &c in &counts[1..] {
            left = (left << 1) - i32::from(c);
            if left < 0 {
                return Err(bad("over-subscribed Huffman code"));
            }
        }
        let mut offsets = [0u16; MAX_BITS + 2];
        for len in 1..=MAX_BITS {
            offsets[len + 1] = offsets[len] + counts[len];
        }
        let mut symbols = vec![0u16; usize::from(offsets[MAX_BITS + 1])];
        let mut next = offsets;
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[usize::from(next[usize::from(l)])] = sym as u16;
                next[usize::from(l)] += 1;
            }
        }

        // Canonical codes, bit-reversed for the LSB-first table.
        let mut fast = vec![0u16; 1 << FAST_BITS];
        let mut code: u32 = 0;
        let mut index = 0usize;
        for len in 1..=MAX_BITS as u32 {
            for _ in 0..counts[len as usize] {
                if len <= FAST_BITS {
                    let rev = code.reverse_bits() >> (32 - len);
                    let entry = (symbols[index] << 4) | len as u16;
                    let mut i = rev as usize;
                    while i < fast.len() {
                        fast[i] = entry;
                        i += 1 << len;
                    }
                }
                code += 1;
                index += 1;
            }
            code <<= 1;
        }
        Ok(Self {
            fast,
            counts,
            symbols,
        })
    }

    #[inline]
    fn decode(&self, bits: &mut Bits) -> Result<u16> {
        let e = self.fast[bits.peek(FAST_BITS) as usize];
        if e != 0 {
            bits.skip(u32::from(e & 15))?;
            return Ok(e >> 4);
        }
        // Slow path: walk the canonical code bit by bit (RFC 1951 §3.2.2).
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..=MAX_BITS {
            code |= bits.take(1)? as i32;
            let count = i32::from(self.counts[len]);
            if code - first < count {
                return Ok(self.symbols[(index + code - first) as usize]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(bad("invalid Huffman code"))
    }
}

fn stored(bits: &mut Bits, out: &mut Vec<u8>) -> Result<()> {
    bits.align();
    let len = bits.take(16)?;
    let nlen = bits.take(16)?;
    if len != !nlen & 0xffff {
        return Err(bad("stored block length check failed"));
    }
    for _ in 0..len {
        out.push(bits.take(8)? as u8);
    }
    Ok(())
}

fn fixed_codes() -> (Huffman, Huffman) {
    let mut lit = [0u8; 288];
    lit[..144].fill(8);
    lit[144..256].fill(9);
    lit[256..280].fill(7);
    lit[280..].fill(8);
    let dist = [5u8; 30];
    // The fixed lengths are complete codes; construction cannot fail.
    (
        Huffman::new(&lit).expect("fixed literal code"),
        Huffman::new(&dist).expect("fixed distance code"),
    )
}

fn dynamic_codes(bits: &mut Bits) -> Result<(Huffman, Huffman)> {
    let nlit = bits.take(5)? as usize + 257;
    let ndist = bits.take(5)? as usize + 1;
    let nclen = bits.take(4)? as usize + 4;
    if nlit > 286 || ndist > 30 {
        return Err(bad("too many length or distance codes"));
    }
    let mut clen = [0u8; 19];
    for &i in &CLEN_ORDER[..nclen] {
        clen[i] = bits.take(3)? as u8;
    }
    let clen = Huffman::new(&clen)?;
    let mut lengths = vec![0u8; nlit + ndist];
    let mut i = 0;
    while i < lengths.len() {
        let sym = clen.decode(bits)?;
        let (value, repeat) = match sym {
            0..=15 => (sym as u8, 1),
            16 => {
                let prev = *lengths[..i]
                    .last()
                    .ok_or_else(|| bad("repeat with no previous length"))?;
                (prev, 3 + bits.take(2)? as usize)
            }
            17 => (0, 3 + bits.take(3)? as usize),
            _ => (0, 11 + bits.take(7)? as usize),
        };
        if i + repeat > lengths.len() {
            return Err(bad("code lengths overrun"));
        }
        lengths[i..i + repeat].fill(value);
        i += repeat;
    }
    if lengths[256] == 0 {
        return Err(bad("no end-of-block code"));
    }
    Ok((
        Huffman::new(&lengths[..nlit])?,
        Huffman::new(&lengths[nlit..])?,
    ))
}

fn codes(bits: &mut Bits, out: &mut Vec<u8>, lit: &Huffman, dist: &Huffman) -> Result<()> {
    loop {
        let sym = lit.decode(bits)?;
        match sym {
            0..=255 => out.push(sym as u8),
            256 => return Ok(()),
            257..=285 => {
                let i = usize::from(sym - 257);
                let len = usize::from(LEN_BASE[i]) + bits.take(u32::from(LEN_EXTRA[i]))? as usize;
                let d = usize::from(dist.decode(bits)?);
                if d >= 30 {
                    return Err(bad("invalid distance code"));
                }
                let distance =
                    usize::from(DIST_BASE[d]) + bits.take(u32::from(DIST_EXTRA[d]))? as usize;
                if distance > out.len() {
                    return Err(bad("distance before the start of the output"));
                }
                let start = out.len() - distance;
                if distance >= len {
                    out.extend_from_within(start..start + len);
                } else {
                    // Overlapping copy: byte by byte repeats the pattern.
                    for k in 0..len {
                        out.push(out[start + k]);
                    }
                }
            }
            _ => return Err(bad("invalid literal/length code")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        inflate(input, &mut out).unwrap();
        out
    }

    #[test]
    fn stored_block() {
        // BFINAL=1, BTYPE=00, LEN=5, NLEN=!5, "hello".
        let input = [0x01, 0x05, 0x00, 0xfa, 0xff, b'h', b'e', b'l', b'l', b'o'];
        assert_eq!(run(&input), b"hello");
    }

    #[test]
    fn fixed_block_with_back_reference() {
        // zlib (Z_FIXED) raw deflate of "abcabcabcabc": three literals and one match.
        let input = [0x4b, 0x4c, 0x4a, 0x4e, 0x84, 0x21, 0x00];
        assert_eq!(run(&input), b"abcabcabcabc");
    }

    #[test]
    fn empty_fixed_block() {
        // BFINAL=1, BTYPE=01, then end-of-block (seven zero bits).
        assert_eq!(run(&[0x03, 0x00]), b"");
    }

    #[test]
    fn dynamic_block() {
        // zlib (level 9) raw deflate of 300 words picked by an LCG: one dynamic block.
        let words = [
            "signal",
            "brake",
            "notch",
            "station",
            "limit",
            "departure",
            "arrival",
            "0",
            "1",
            "2",
        ];
        let mut text = String::new();
        let mut x: u64 = 7;
        for _ in 0..300 {
            x = (x * 1_103_515_245 + 12_345) % (1 << 31);
            text.push_str(words[((x >> 16) % 10) as usize]);
            text.push(' ');
        }
        let input = DYNAMIC;
        assert_eq!(input[0] >> 1 & 3, 2, "vector is a dynamic block");
        assert_eq!(run(&input), text.as_bytes());
    }

    #[test]
    fn reports_bytes_used() {
        let mut input = vec![0x01, 0x01, 0x00, 0xfe, 0xff, b'x'];
        input.extend_from_slice(b"trailing");
        let mut out = Vec::new();
        assert_eq!(inflate(&input, &mut out).unwrap(), 6);
        assert_eq!(out, b"x");
    }

    #[test]
    fn rejects_garbage() {
        let mut out = Vec::new();
        assert!(inflate(&[0x07], &mut out).is_err()); // BTYPE=11
        assert!(inflate(&[0x01, 0x05, 0x00, 0x00, 0x00], &mut out).is_err()); // NLEN mismatch
    }

    /// See [`dynamic_block`].
    const DYNAMIC: [u8; 337] = [
        0x85, 0x54, 0x5b, 0x4e, 0xc4, 0x30, 0x0c, 0xbc, 0x4a, 0x8e, 0x60, 0xe7, 0x46, 0x85, 0xad,
        0xa0, 0xa2, 0x74, 0x51, 0x37, 0x70, 0x7e, 0x56, 0xb8, 0x8e, 0x3d, 0x76, 0x22, 0x7e, 0xf2,
        0x92, 0x1f, 0xe3, 0x19, 0xc7, 0xfb, 0xf6, 0xb9, 0xb5, 0xb2, 0x9c, 0xe7, 0xf6, 0xb3, 0xec,
        0x85, 0xcb, 0xfe, 0x77, 0x7f, 0x39, 0x97, 0x8f, 0xb5, 0xdc, 0xd6, 0xaf, 0xe5, 0x6c, 0xdf,
        0xe7, 0x7a, 0xbd, 0x3e, 0xb6, 0xb7, 0xe3, 0x69, 0x64, 0xcf, 0x8f, 0xb6, 0xb4, 0xed, 0x7e,
        0xe4, 0x5d, 0x0c, 0xc5, 0xab, 0x96, 0xe3, 0xde, 0x5e, 0xdf, 0x67, 0xd1, 0xae, 0x4d, 0x6c,
        0xe8, 0x69, 0xcd, 0x57, 0x76, 0xf5, 0xd6, 0xa0, 0xe8, 0x85, 0x99, 0xd8, 0x0e, 0xe4, 0x12,
        0x49, 0x50, 0x71, 0x64, 0xf7, 0xce, 0x70, 0xd3, 0xe2, 0xe3, 0x3e, 0x4c, 0x48, 0x0e, 0x51,
        0xc4, 0xa6, 0x8e, 0x8e, 0x20, 0x43, 0xe5, 0x89, 0x95, 0xb3, 0xac, 0x94, 0xf2, 0x72, 0x08,
        0x4b, 0xc8, 0x91, 0x3c, 0xa6, 0x1c, 0xc0, 0x39, 0xf5, 0x10, 0x66, 0x47, 0x83, 0x92, 0x23,
        0x04, 0xc9, 0x50, 0x01, 0xac, 0x46, 0x92, 0x5b, 0x75, 0xb1, 0x29, 0xd4, 0xcd, 0xfd, 0x74,
        0x81, 0xa9, 0x76, 0x30, 0xae, 0xba, 0x56, 0xd4, 0xa5, 0xa6, 0xd4, 0x22, 0x50, 0x8d, 0x1a,
        0x31, 0x12, 0x91, 0x65, 0x30, 0x00, 0x15, 0x98, 0xce, 0x85, 0x6b, 0x31, 0xa1, 0x8f, 0xcc,
        0x50, 0x0c, 0x4c, 0x38, 0x9a, 0xf1, 0x4e, 0x0e, 0x87, 0xcf, 0x29, 0xe7, 0xa8, 0x2d, 0x22,
        0xd7, 0xdd, 0x50, 0xab, 0x00, 0x48, 0xfd, 0x95, 0xaa, 0x83, 0x96, 0x2b, 0x8a, 0x68, 0x31,
        0xb8, 0x7f, 0x1d, 0x75, 0x40, 0x2c, 0x8e, 0x7e, 0xab, 0x66, 0x74, 0x62, 0x20, 0x94, 0x8d,
        0xff, 0x00, 0x60, 0x22, 0x5e, 0x9e, 0x13, 0xae, 0x41, 0x46, 0x93, 0xe2, 0xdf, 0x38, 0xd5,
        0x45, 0xa0, 0xfe, 0xaf, 0x69, 0x9e, 0x10, 0x0b, 0xa7, 0xe9, 0xc0, 0x4a, 0x39, 0x9d, 0x0b,
        0x3e, 0x8c, 0xfe, 0x04, 0x8a, 0x2a, 0x37, 0xb4, 0x8b, 0x82, 0xe6, 0xdf, 0xdb, 0x19, 0xf2,
        0x1f, 0x6c, 0x3c, 0x8c, 0x83, 0x8e, 0x89, 0x35, 0xc0, 0x21, 0x6b, 0x6c, 0xc3, 0x9a, 0x86,
        0xa4, 0x01, 0xf0, 0x7e, 0xb5, 0x77, 0x7f, 0x9c, 0xde, 0x53, 0xc6, 0xaa, 0xb5, 0x49, 0x18,
        0xa4, 0x80, 0x8b, 0x62, 0x19, 0x49, 0xc3, 0xe1, 0x18, 0x56, 0xe7, 0x51, 0xbb, 0x6a, 0x3f,
        0xd8, 0x78, 0x42, 0x99, 0xb8, 0xfc, 0x02,
    ];
}

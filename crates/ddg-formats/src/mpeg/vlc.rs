//! Variable-length code tables of ISO/IEC 11172-2 Annex B, decoded through one flat lookup
//! table per code (indexed by the next `max_len` bits).

use super::bits::BitReader;

/// A decoded VLC entry: `len == 0` marks an invalid code.
#[derive(Clone, Copy, Default)]
pub(crate) struct Entry<T: Copy> {
    pub value: T,
    pub len: u8,
}

pub(crate) struct Vlc<T: Copy + Default> {
    max_len: u32,
    table: Vec<Entry<T>>,
}

impl<T: Copy + Default> Vlc<T> {
    /// Builds a table from `(code, value)` with codes written as strings of '0'/'1' (spaces
    /// ignored). Panics on overlapping codes: the tables are constants checked by tests.
    pub(crate) fn new(codes: &[(&str, T)]) -> Self {
        let parsed: Vec<(u32, u32, T)> = codes
            .iter()
            .map(|&(s, v)| {
                let mut code = 0;
                let mut len = 0;
                for c in s.chars().filter(|c| !c.is_whitespace()) {
                    code = code << 1 | u32::from(c == '1');
                    len += 1;
                }
                (code, len, v)
            })
            .collect();
        let max_len = parsed.iter().map(|p| p.1).max().unwrap_or(1);
        let mut table = vec![Entry::default(); 1 << max_len];
        for &(code, len, value) in &parsed {
            let shift = max_len - len;
            let first = (code << shift) as usize;
            for e in &mut table[first..first + (1 << shift)] {
                assert!(e.len == 0, "overlapping VLC code");
                *e = Entry {
                    value,
                    len: len as u8,
                };
            }
        }
        Self { max_len, table }
    }

    /// Decodes one code; `None` for a bit pattern that is not in the table.
    #[inline]
    pub(crate) fn read(&self, r: &mut BitReader<'_>) -> Option<T> {
        let e = self.table[r.peek(self.max_len) as usize];
        if e.len == 0 {
            return None;
        }
        r.skip(u32::from(e.len));
        Some(e.value)
    }

    /// Kraft sum of the table's codes, in units of 2^-max_len (for completeness tests).
    #[cfg(test)]
    pub(crate) fn coverage(&self) -> (usize, usize) {
        (
            self.table.iter().filter(|e| e.len != 0).count(),
            self.table.len(),
        )
    }
}

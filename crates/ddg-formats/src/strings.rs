//! Scanning binary blobs for embedded ASCII and Shift-JIS text.

use encoding_rs::SHIFT_JIS;

/// A run of text found in a binary blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundString {
    pub offset: usize,
    pub text: String,
    /// True if the run contains at least one double-byte Shift-JIS character.
    pub japanese: bool,
}

fn is_sjis_lead(b: u8) -> bool {
    matches!(b, 0x81..=0x9F | 0xE0..=0xEF)
}

fn is_sjis_trail(b: u8) -> bool {
    matches!(b, 0x40..=0x7E | 0x80..=0xFC)
}

fn is_text_byte(b: u8) -> bool {
    matches!(b, 0x20..=0x7E | b'\t' | b'\r' | b'\n')
}

/// Finds runs of printable ASCII and/or Shift-JIS (JIS X 0208 range) text.
///
/// A run qualifies if it has at least `min_chars` characters. Half-width katakana
/// (0xA1..=0xDF) is accepted only inside runs that also contain double-byte
/// characters, because on its own it is mostly noise in binary data.
pub fn scan(data: &[u8], min_chars: usize) -> Vec<FoundString> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let start = i;
        let mut chars = 0;
        let mut double = 0;
        let mut kana = 0;
        while i < data.len() {
            let b = data[i];
            if is_text_byte(b) {
                i += 1;
            } else if is_sjis_lead(b) && i + 1 < data.len() && is_sjis_trail(data[i + 1]) {
                i += 2;
                double += 1;
            } else if (0xA1..=0xDF).contains(&b) {
                i += 1;
                kana += 1;
            } else {
                break;
            }
            chars += 1;
        }
        if i == start {
            i += 1;
            continue;
        }
        let plausible = kana == 0 || double > 0;
        if chars >= min_chars && plausible {
            let (text, _, had_errors) = SHIFT_JIS.decode(&data[start..i]);
            if !had_errors {
                out.push(FoundString {
                    offset: start,
                    text: text.trim_end().to_owned(),
                    japanese: double > 0,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_ascii_and_sjis() {
        // "電車" in Shift-JIS is 93 64 8E D4.
        let blob = b"\x00\x01hello\x00\x93\x64\x8e\xd4GO\x00\xff";
        let found = scan(blob, 4);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].text, "hello");
        assert!(!found[0].japanese);
        assert_eq!(found[1].text, "電車GO");
        assert_eq!(found[1].offset, 8);
        assert!(found[1].japanese);
    }
}

//! Constant tables of ISO/IEC 11172-2: the Annex B variable-length codes, the zigzag scan and
//! the default quantiser matrices.

use std::sync::OnceLock;

use super::vlc::Vlc;

/// Zigzag scan position → natural (row-major) coefficient index (11172-2, 2.4.4.1).
pub(crate) const ZIGZAG: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// Default intra quantiser matrix, natural order (11172-2, 2.4.3.2).
pub(crate) const DEFAULT_INTRA: [u8; 64] = [
    8, 16, 19, 22, 26, 27, 29, 34, //
    16, 16, 22, 24, 27, 29, 34, 37, //
    19, 22, 26, 27, 29, 34, 34, 38, //
    22, 22, 26, 27, 29, 34, 37, 40, //
    22, 26, 27, 29, 32, 35, 40, 48, //
    26, 27, 29, 32, 35, 40, 48, 58, //
    26, 27, 29, 34, 38, 46, 56, 69, //
    27, 29, 35, 38, 46, 56, 69, 83, //
];

/// Default non-intra matrix: flat 16.
pub(crate) const DEFAULT_NON_INTRA: [u8; 64] = [16; 64];

/// macroblock_address_increment values beyond 1..=33.
pub(crate) const MBA_STUFFING: u8 = 34;
pub(crate) const MBA_ESCAPE: u8 = 35;

/// Macroblock type flags.
pub(crate) const MB_QUANT: u8 = 1;
pub(crate) const MB_FWD: u8 = 2;
pub(crate) const MB_BWD: u8 = 4;
pub(crate) const MB_PATTERN: u8 = 8;
pub(crate) const MB_INTRA: u8 = 16;

/// dct_coeff values beyond `run << 8 | level`.
pub(crate) const DCT_EOB: u16 = 0xffff;
pub(crate) const DCT_ESCAPE: u16 = 0xfffe;

/// Table B.1, macroblock_address_increment.
pub(crate) const MBA: &[(&str, u8)] = &[
    ("1", 1),
    ("011", 2),
    ("010", 3),
    ("0011", 4),
    ("0010", 5),
    ("00011", 6),
    ("00010", 7),
    ("0000111", 8),
    ("0000110", 9),
    ("00001011", 10),
    ("00001010", 11),
    ("00001001", 12),
    ("00001000", 13),
    ("00000111", 14),
    ("00000110", 15),
    ("0000010111", 16),
    ("0000010110", 17),
    ("0000010101", 18),
    ("0000010100", 19),
    ("0000010011", 20),
    ("0000010010", 21),
    ("00000100011", 22),
    ("00000100010", 23),
    ("00000100001", 24),
    ("00000100000", 25),
    ("00000011111", 26),
    ("00000011110", 27),
    ("00000011101", 28),
    ("00000011100", 29),
    ("00000011011", 30),
    ("00000011010", 31),
    ("00000011001", 32),
    ("00000011000", 33),
    ("00000001111", MBA_STUFFING),
    ("00000001000", MBA_ESCAPE),
];

/// Table B.2a–d, macroblock_type for I, P, B and D pictures.
pub(crate) const MB_TYPE_I: &[(&str, u8)] = &[("1", MB_INTRA), ("01", MB_INTRA | MB_QUANT)];
pub(crate) const MB_TYPE_P: &[(&str, u8)] = &[
    ("1", MB_FWD | MB_PATTERN),
    ("01", MB_PATTERN),
    ("001", MB_FWD),
    ("00011", MB_INTRA),
    ("00010", MB_QUANT | MB_FWD | MB_PATTERN),
    ("00001", MB_QUANT | MB_PATTERN),
    ("000001", MB_QUANT | MB_INTRA),
];
pub(crate) const MB_TYPE_B: &[(&str, u8)] = &[
    ("10", MB_FWD | MB_BWD),
    ("11", MB_FWD | MB_BWD | MB_PATTERN),
    ("010", MB_BWD),
    ("011", MB_BWD | MB_PATTERN),
    ("0010", MB_FWD),
    ("0011", MB_FWD | MB_PATTERN),
    ("00011", MB_INTRA),
    ("00010", MB_QUANT | MB_FWD | MB_BWD | MB_PATTERN),
    ("000011", MB_QUANT | MB_FWD | MB_PATTERN),
    ("000010", MB_QUANT | MB_BWD | MB_PATTERN),
    ("000001", MB_QUANT | MB_INTRA),
];
pub(crate) const MB_TYPE_D: &[(&str, u8)] = &[("1", MB_INTRA)];

/// Table B.3, coded_block_pattern.
pub(crate) const CBP: &[(&str, u8)] = &[
    ("111", 60),
    ("1101", 4),
    ("1100", 8),
    ("1011", 16),
    ("1010", 32),
    ("10011", 12),
    ("10010", 48),
    ("10001", 20),
    ("10000", 40),
    ("01111", 28),
    ("01110", 44),
    ("01101", 52),
    ("01100", 56),
    ("01011", 1),
    ("01010", 61),
    ("01001", 2),
    ("01000", 62),
    ("001111", 24),
    ("001110", 36),
    ("001101", 3),
    ("001100", 63),
    ("0010111", 5),
    ("0010110", 9),
    ("0010101", 17),
    ("0010100", 33),
    ("0010011", 6),
    ("0010010", 10),
    ("0010001", 18),
    ("0010000", 34),
    ("00011111", 7),
    ("00011110", 11),
    ("00011101", 19),
    ("00011100", 35),
    ("00011011", 13),
    ("00011010", 49),
    ("00011001", 21),
    ("00011000", 41),
    ("00010111", 14),
    ("00010110", 50),
    ("00010101", 22),
    ("00010100", 42),
    ("00010011", 15),
    ("00010010", 51),
    ("00010001", 23),
    ("00010000", 43),
    ("00001111", 25),
    ("00001110", 37),
    ("00001101", 26),
    ("00001100", 38),
    ("00001011", 29),
    ("00001010", 45),
    ("00001001", 53),
    ("00001000", 57),
    ("00000111", 30),
    ("00000110", 46),
    ("00000101", 54),
    ("00000100", 58),
    ("000000111", 31),
    ("000000110", 47),
    ("000000101", 55),
    ("000000100", 59),
    ("000000011", 27),
    ("000000010", 39),
];

/// Table B.4, motion_code magnitudes; each code is followed by a sign bit (1 = negative),
/// except for 0.
pub(crate) const MOTION: &[(&str, i8)] = &[
    ("01", 1),
    ("001", 2),
    ("0001", 3),
    ("000011", 4),
    ("0000101", 5),
    ("0000100", 6),
    ("0000011", 7),
    ("000001011", 8),
    ("000001010", 9),
    ("000001001", 10),
    ("0000010001", 11),
    ("0000010000", 12),
    ("0000001111", 13),
    ("0000001110", 14),
    ("0000001101", 15),
    ("0000001100", 16),
];

/// Table B.5a/b, dct_dc_size_luminance and dct_dc_size_chrominance.
pub(crate) const DC_LUMA: &[(&str, u8)] = &[
    ("100", 0),
    ("00", 1),
    ("01", 2),
    ("101", 3),
    ("110", 4),
    ("1110", 5),
    ("11110", 6),
    ("111110", 7),
    ("1111110", 8),
];
pub(crate) const DC_CHROMA: &[(&str, u8)] = &[
    ("00", 0),
    ("01", 1),
    ("10", 2),
    ("110", 3),
    ("1110", 4),
    ("11110", 5),
    ("111110", 6),
    ("1111110", 7),
    ("11111110", 8),
];

/// Table B.5c/d, dct_coeff_next as `(code without the sign bit, run, level)`. The variant for
/// the first coefficient of a non-intra block ("1s" = run 0, level 1) is handled by the caller.
pub(crate) const DCT: &[(&str, u8, u8)] = &[
    ("11", 0, 1),
    ("011", 1, 1),
    ("0100", 0, 2),
    ("0101", 2, 1),
    ("00101", 0, 3),
    ("00111", 3, 1),
    ("00110", 4, 1),
    ("000110", 1, 2),
    ("000111", 5, 1),
    ("000101", 6, 1),
    ("000100", 7, 1),
    ("0000110", 0, 4),
    ("0000100", 2, 2),
    ("0000111", 8, 1),
    ("0000101", 9, 1),
    ("00100110", 0, 5),
    ("00100001", 0, 6),
    ("00100101", 1, 3),
    ("00100100", 3, 2),
    ("00100111", 10, 1),
    ("00100011", 11, 1),
    ("00100010", 12, 1),
    ("00100000", 13, 1),
    ("0000001010", 0, 7),
    ("0000001100", 1, 4),
    ("0000001011", 2, 3),
    ("0000001111", 4, 2),
    ("0000001001", 5, 2),
    ("0000001110", 14, 1),
    ("0000001101", 15, 1),
    ("0000001000", 16, 1),
    ("000000011101", 0, 8),
    ("000000011000", 0, 9),
    ("000000010011", 0, 10),
    ("000000010000", 0, 11),
    ("000000011011", 1, 5),
    ("000000010100", 2, 4),
    ("000000011100", 3, 3),
    ("000000010010", 4, 3),
    ("000000011110", 6, 2),
    ("000000010101", 7, 2),
    ("000000010001", 8, 2),
    ("000000011111", 17, 1),
    ("000000011010", 18, 1),
    ("000000011001", 19, 1),
    ("000000010111", 20, 1),
    ("000000010110", 21, 1),
    ("0000000011010", 0, 12),
    ("0000000011001", 0, 13),
    ("0000000011000", 0, 14),
    ("0000000010111", 0, 15),
    ("0000000010110", 1, 6),
    ("0000000010101", 1, 7),
    ("0000000010100", 2, 5),
    ("0000000010011", 3, 4),
    ("0000000010010", 5, 3),
    ("0000000010001", 9, 2),
    ("0000000010000", 10, 2),
    ("0000000011111", 22, 1),
    ("0000000011110", 23, 1),
    ("0000000011101", 24, 1),
    ("0000000011100", 25, 1),
    ("0000000011011", 26, 1),
    ("00000000011111", 0, 16),
    ("00000000011110", 0, 17),
    ("00000000011101", 0, 18),
    ("00000000011100", 0, 19),
    ("00000000011011", 0, 20),
    ("00000000011010", 0, 21),
    ("00000000011001", 0, 22),
    ("00000000011000", 0, 23),
    ("00000000010111", 0, 24),
    ("00000000010110", 0, 25),
    ("00000000010101", 0, 26),
    ("00000000010100", 0, 27),
    ("00000000010011", 0, 28),
    ("00000000010010", 0, 29),
    ("00000000010001", 0, 30),
    ("00000000010000", 0, 31),
    ("000000000011000", 0, 32),
    ("000000000010111", 0, 33),
    ("000000000010110", 0, 34),
    ("000000000010101", 0, 35),
    ("000000000010100", 0, 36),
    ("000000000010011", 0, 37),
    ("000000000010010", 0, 38),
    ("000000000010001", 0, 39),
    ("000000000010000", 0, 40),
    ("000000000011111", 1, 8),
    ("000000000011110", 1, 9),
    ("000000000011101", 1, 10),
    ("000000000011100", 1, 11),
    ("000000000011011", 1, 12),
    ("000000000011010", 1, 13),
    ("000000000011001", 1, 14),
    ("0000000000010011", 1, 15),
    ("0000000000010010", 1, 16),
    ("0000000000010001", 1, 17),
    ("0000000000010000", 1, 18),
    ("0000000000010100", 6, 3),
    ("0000000000011010", 11, 2),
    ("0000000000011001", 12, 2),
    ("0000000000011000", 13, 2),
    ("0000000000010111", 14, 2),
    ("0000000000010110", 15, 2),
    ("0000000000010101", 16, 2),
    ("0000000000011111", 27, 1),
    ("0000000000011110", 28, 1),
    ("0000000000011101", 29, 1),
    ("0000000000011100", 30, 1),
    ("0000000000011011", 31, 1),
];

pub(crate) struct Tables {
    pub mba: Vlc<u8>,
    pub mb_type: [Vlc<u8>; 4],
    pub cbp: Vlc<u8>,
    pub motion: Vlc<i8>,
    pub dc_luma: Vlc<u8>,
    pub dc_chroma: Vlc<u8>,
    pub dct: Vlc<u16>,
}

pub(crate) fn tables() -> &'static Tables {
    static T: OnceLock<Tables> = OnceLock::new();
    T.get_or_init(|| {
        let motion_codes: Vec<String> = MOTION
            .iter()
            .flat_map(|&(c, _)| [format!("{c}0"), format!("{c}1")])
            .collect();
        let mut motion: Vec<(&str, i8)> = vec![("1", 0)];
        for (i, &(_, m)) in MOTION.iter().enumerate() {
            motion.push((&motion_codes[2 * i], m));
            motion.push((&motion_codes[2 * i + 1], -m));
        }
        let mut dct: Vec<(&str, u16)> = DCT
            .iter()
            .map(|&(c, run, level)| (c, u16::from(run) << 8 | u16::from(level)))
            .collect();
        dct.push(("10", DCT_EOB));
        dct.push(("000001", DCT_ESCAPE));
        Tables {
            mba: Vlc::new(MBA),
            mb_type: [
                Vlc::new(MB_TYPE_I),
                Vlc::new(MB_TYPE_P),
                Vlc::new(MB_TYPE_B),
                Vlc::new(MB_TYPE_D),
            ],
            cbp: Vlc::new(CBP),
            motion: Vlc::new(&motion),
            dc_luma: Vlc::new(DC_LUMA),
            dc_chroma: Vlc::new(DC_CHROMA),
            dct: Vlc::new(&dct),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tables build without overlapping codes, and each covers the code space except the
    /// prefixes the standard leaves unused (those that could emulate a start code, and a few
    /// reserved ones).
    #[test]
    fn tables_are_prefix_free_with_the_expected_coverage() {
        let t = tables();
        let frac = |(used, all): (usize, usize)| used as f64 / all as f64;
        // MBA: 0000 0000, 0000 0010 and 0000 0001 001..110 are unused.
        assert_eq!(frac(t.mba.coverage()), 1.0 - 2.0 / 256.0 - 6.0 / 2048.0);
        assert_eq!(frac(t.mb_type[1].coverage()), 1.0 - 1.0 / 64.0);
        assert_eq!(frac(t.mb_type[2].coverage()), 1.0 - 1.0 / 64.0);
        assert_eq!(frac(t.cbp.coverage()), 1.0 - 2.0 / 512.0);
        assert_eq!(frac(t.motion.coverage()), 1.0 - 3.0 / 256.0);
        assert_eq!(frac(t.dc_luma.coverage()), 1.0 - 1.0 / 128.0);
        assert_eq!(frac(t.dc_chroma.coverage()), 1.0 - 1.0 / 256.0);
        // dct_coeff_next, codes listed without the sign bit.
        assert_eq!(DCT.len(), 111);
        // Each entry stands for two codes (sign 0 and 1) one bit longer, so it covers 2^-len
        // either way; with EOB ("10") and escape ("000001") only the 12-zero prefix is unused.
        let entries = DCT
            .iter()
            .map(|e| 2f64.powi(-(e.0.len() as i32)))
            .sum::<f64>();
        assert_eq!(frac(t.dct.coverage()), entries + 0.25 + 1.0 / 64.0);
        assert_eq!(entries + 0.25 + 1.0 / 64.0, 1.0 - 2f64.powi(-12));
    }

    #[test]
    fn zigzag_is_a_permutation() {
        let mut seen = [false; 64];
        for &z in &ZIGZAG {
            assert!(!seen[z as usize]);
            seen[z as usize] = true;
        }
    }
}

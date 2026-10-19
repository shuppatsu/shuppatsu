//! Windows icon resources: `RT_GROUP_ICON` directories and the `RT_ICON` images they name.
//!
//! An icon image is a DIB without a file header: a `BITMAPINFOHEADER` whose height counts both
//! halves, the palette (≤ 8 bpp), the XOR (colour) bitmap and the 1-bpp AND (transparency) mask,
//! both bottom-up with rows padded to 32 bits. Vista-era icons may instead hold a whole PNG file,
//! which this crate does not decode (no inflate); such entries are reported as
//! [`IconData::Png`]. `dengo3k.exe` has one group (#101) with two classic 4-bpp DIBs
//! (docs/recon.md §3).

use crate::error::{FormatError, Result};
use crate::exe::{Exe, RT_GROUP_ICON, RT_ICON, ResId};
use crate::reader::Bytes;

/// One `GRPICONDIRENTRY` (14 bytes): the advertised format and the `RT_ICON` id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupEntry {
    /// Width in pixels (0 in the file means 256).
    pub width: u32,
    pub height: u32,
    /// Palette size (0 = none or ≥ 256).
    pub color_count: u8,
    pub planes: u16,
    pub bit_count: u16,
    pub bytes: u32,
    /// The `RT_ICON` resource id.
    pub id: u16,
}

/// Parses a `GRPICONDIR`: reserved 0, type 1, count, then `count` 14-byte entries.
pub fn parse_group(data: &[u8]) -> Result<Vec<GroupEntry>> {
    let b = Bytes::new(data, "icon group");
    if b.u16(0)? != 0 || b.u16(2)? != 1 {
        return Err(FormatError::invalid("icon group", "not a GRPICONDIR"));
    }
    let count = b.u16(4)? as usize;
    (0..count)
        .map(|i| {
            let e = 6 + i * 14;
            let dim = |v: u8| if v == 0 { 256 } else { u32::from(v) };
            Ok(GroupEntry {
                width: dim(b.u8(e)?),
                height: dim(b.u8(e + 1)?),
                color_count: b.u8(e + 2)?,
                planes: b.u16(e + 4)?,
                bit_count: b.u16(e + 6)?,
                bytes: b.u32(e + 8)?,
                id: b.u16(e + 12)?,
            })
        })
        .collect()
}

/// A decoded icon image: straight (non-premultiplied) RGBA8, top row first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconImage {
    pub width: u32,
    pub height: u32,
    /// Bits per pixel of the source DIB.
    pub bit_count: u16,
    pub rgba: Vec<u8>,
}

/// The payload of one `RT_ICON`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconData<'a> {
    Dib(IconImage),
    /// A complete PNG file (left undecoded).
    Png(&'a [u8]),
}

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

/// Decodes one `RT_ICON` payload.
pub fn decode_image(data: &[u8]) -> Result<IconData<'_>> {
    if data.starts_with(&PNG_SIGNATURE) {
        return Ok(IconData::Png(data));
    }
    decode_dib(data).map(IconData::Dib)
}

fn decode_dib(data: &[u8]) -> Result<IconImage> {
    const WHAT: &str = "icon DIB";
    let b = Bytes::new(data, WHAT);
    let header = b.usize32(0)?;
    if header < 40 {
        return Err(FormatError::invalid(WHAT, format!("header size {header}")));
    }
    let width = b.i32(4)?;
    let total_height = b.i32(8)?;
    let bit_count = b.u16(14)?;
    let compression = b.u32(16)?;
    let colors_used = b.u32(32)?;
    // BI_RGB, or BI_BITFIELDS with the standard 8-8-8-8 masks some 32-bpp icons carry.
    if compression != 0 && !(compression == 3 && bit_count == 32) {
        return Err(FormatError::invalid(
            WHAT,
            format!("compression {compression}"),
        ));
    }
    if !(1..=256).contains(&width) || total_height == 0 || total_height.unsigned_abs() > 512 {
        return Err(FormatError::invalid(
            WHAT,
            format!("size {width}x{total_height}"),
        ));
    }
    // The height covers XOR + AND; a negative height (rare) means top-down rows.
    let top_down = total_height < 0;
    let w = width as usize;
    let h = (total_height.unsigned_abs() / 2).max(1) as usize;

    let mut at = header;
    if compression == 3 {
        at += 12; // three DWORD masks follow a 40-byte header
    }
    let palette: Vec<[u8; 3]> = if bit_count <= 8 {
        let n = if colors_used == 0 {
            1usize << bit_count
        } else {
            (colors_used as usize).min(256)
        };
        let p = b.slice(at, n * 4)?;
        at += n * 4;
        p.as_chunks::<4>()
            .0
            .iter()
            .map(|c| [c[2], c[1], c[0]])
            .collect()
    } else {
        Vec::new()
    };
    if !matches!(bit_count, 1 | 4 | 8 | 16 | 24 | 32) {
        return Err(FormatError::invalid(WHAT, format!("{bit_count} bpp")));
    }
    let xor_stride = (w * bit_count as usize).div_ceil(32) * 4;
    let xor = b.slice(at, xor_stride * h)?;
    at += xor_stride * h;
    let and_stride = w.div_ceil(32) * 4;
    // 32-bpp icons sometimes omit the mask; treat a missing mask as fully opaque.
    let and = b.slice(at, and_stride * h).ok();

    let row_of = |y: usize| if top_down { y } else { h - 1 - y };
    let mut rgba = vec![0u8; w * h * 4];
    let mut any_alpha = false;
    for y in 0..h {
        let src = &xor[row_of(y) * xor_stride..][..xor_stride];
        for x in 0..w {
            let px = match bit_count {
                1 | 4 | 8 => {
                    let bits = bit_count as usize;
                    let bit = x * bits;
                    let byte = src[bit / 8];
                    let index = (byte >> (8 - bits - bit % 8)) & ((1u16 << bits) - 1) as u8;
                    let [r, g, bl] = palette.get(index as usize).copied().unwrap_or([0, 0, 0]);
                    [r, g, bl, 255]
                }
                16 => {
                    // BI_RGB 16 bpp is X1R5G5B5.
                    let v = u16::from_le_bytes([src[x * 2], src[x * 2 + 1]]);
                    let c = |s: u16| {
                        let v = ((v >> s) & 31) as u8;
                        (v << 3) | (v >> 2)
                    };
                    [c(10), c(5), c(0), 255]
                }
                24 => [src[x * 3 + 2], src[x * 3 + 1], src[x * 3], 255],
                _ => {
                    let a = src[x * 4 + 3];
                    any_alpha |= a != 0;
                    [src[x * 4 + 2], src[x * 4 + 1], src[x * 4], a]
                }
            };
            rgba[(y * w + x) * 4..][..4].copy_from_slice(&px);
        }
    }
    // The AND mask decides transparency unless a 32-bpp image carries real alpha.
    let use_mask = !(bit_count == 32 && any_alpha);
    for y in 0..h {
        for x in 0..w {
            let transparent = match and {
                Some(and) => {
                    let row = &and[row_of(y) * and_stride..];
                    row[x / 8] & (0x80 >> (x % 8)) != 0
                }
                None => false,
            };
            let p = &mut rgba[(y * w + x) * 4..][..4];
            if use_mask {
                p[3] = if transparent { 0 } else { 255 };
            }
            if p[3] == 0 {
                p[..3].fill(0);
            }
        }
    }
    Ok(IconImage {
        width: w as u32,
        height: h as u32,
        bit_count,
        rgba,
    })
}

/// One icon of an exe's first icon group.
#[derive(Debug, Clone)]
pub struct ExeIcon<'a> {
    pub group_id: ResId,
    pub entry: GroupEntry,
    pub data: IconData<'a>,
}

/// All images of the exe's first `RT_GROUP_ICON` (the one Explorer shows), in group order.
pub fn exe_icons<'a>(exe: &Exe<'a>) -> Result<Vec<ExeIcon<'a>>> {
    let resources = exe.resources()?;
    let Some(group) = resources
        .iter()
        .find(|r| r.kind == ResId::Id(RT_GROUP_ICON))
    else {
        return Ok(Vec::new());
    };
    parse_group(group.data)?
        .into_iter()
        .map(|entry| {
            let image = resources
                .iter()
                .find(|r| r.kind == ResId::Id(RT_ICON) && r.name == ResId::Id(entry.id))
                .ok_or_else(|| {
                    FormatError::invalid("icon group", format!("RT_ICON #{} missing", entry.id))
                })?;
            Ok(ExeIcon {
                group_id: group.name.clone(),
                entry,
                data: decode_image(image.data)?,
            })
        })
        .collect()
}

/// The largest, then deepest, decodable image (PNG entries are skipped).
pub fn best<'a, 'b>(icons: &'b [ExeIcon<'a>]) -> Option<&'b IconImage> {
    icons
        .iter()
        .filter_map(|i| match &i.data {
            IconData::Dib(img) => Some(img),
            IconData::Png(_) => None,
        })
        .max_by_key(|img| (img.width * img.height, img.bit_count))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2×2 4-bpp DIB: palette 0 black, 1 red, 2 green; top row [1,2], bottom row [2,0] with
    /// the bottom-right pixel masked out.
    fn tiny_dib_4bpp() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend(40u32.to_le_bytes());
        d.extend(2i32.to_le_bytes());
        d.extend(4i32.to_le_bytes()); // XOR + AND
        d.extend(1u16.to_le_bytes());
        d.extend(4u16.to_le_bytes());
        d.extend([0u8; 24]); // compression .. important colours
        let mut pal = [0u8; 64];
        pal[4..8].copy_from_slice(&[0, 0, 255, 0]); // red (BGRX)
        pal[8..12].copy_from_slice(&[0, 255, 0, 0]); // green
        d.extend(pal);
        // XOR rows, bottom-up, 4-byte stride.
        d.extend([0x20, 0, 0, 0]); // bottom: 2, 0
        d.extend([0x12, 0, 0, 0]); // top: 1, 2
        // AND rows, bottom-up.
        d.extend([0x40, 0, 0, 0]); // bottom-right transparent
        d.extend([0x00, 0, 0, 0]);
        d
    }

    #[test]
    fn decodes_4bpp_with_mask() {
        let IconData::Dib(img) = decode_image(&tiny_dib_4bpp()).unwrap() else {
            panic!("expected a DIB");
        };
        assert_eq!((img.width, img.height, img.bit_count), (2, 2, 4));
        assert_eq!(
            img.rgba,
            [
                255, 0, 0, 255, 0, 255, 0, 255, // top
                0, 255, 0, 255, 0, 0, 0, 0, // bottom, right masked
            ]
        );
    }

    #[test]
    fn decodes_32bpp_alpha_and_1bpp() {
        // 1×1 32 bpp with alpha 0x80: the alpha wins over the (opaque) mask.
        let mut d = Vec::new();
        d.extend(40u32.to_le_bytes());
        d.extend(1i32.to_le_bytes());
        d.extend(2i32.to_le_bytes());
        d.extend(1u16.to_le_bytes());
        d.extend(32u16.to_le_bytes());
        d.extend([0u8; 24]);
        d.extend([10, 20, 30, 0x80]);
        d.extend([0u8; 4]);
        let IconData::Dib(img) = decode_image(&d).unwrap() else {
            panic!()
        };
        assert_eq!(img.rgba, [30, 20, 10, 0x80]);

        // 1×1 1 bpp: palette white at index 1.
        let mut d = Vec::new();
        d.extend(40u32.to_le_bytes());
        d.extend(1i32.to_le_bytes());
        d.extend(2i32.to_le_bytes());
        d.extend(1u16.to_le_bytes());
        d.extend(1u16.to_le_bytes());
        d.extend([0u8; 24]);
        d.extend([0, 0, 0, 0, 255, 255, 255, 0]);
        d.extend([0x80, 0, 0, 0]);
        d.extend([0x00, 0, 0, 0]);
        let IconData::Dib(img) = decode_image(&d).unwrap() else {
            panic!()
        };
        assert_eq!(img.rgba, [255, 255, 255, 255]);
    }

    #[test]
    fn png_entries_are_reported() {
        let mut d = PNG_SIGNATURE.to_vec();
        d.extend([0u8; 8]);
        assert!(matches!(decode_image(&d).unwrap(), IconData::Png(_)));
    }

    #[test]
    fn parses_group_and_picks_best() {
        let mut g = vec![0, 0, 1, 0, 2, 0];
        g.extend([16, 16, 16, 0, 1, 0, 4, 0, 40, 1, 0, 0, 2, 0]);
        g.extend([0, 0, 0, 0, 1, 0, 32, 0, 0, 0, 1, 0, 1, 0]);
        let entries = parse_group(&g).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!((entries[0].width, entries[0].id), (16, 2));
        assert_eq!((entries[1].width, entries[1].bit_count), (256, 32));

        let img = |w, bit_count| IconImage {
            width: w,
            height: w,
            bit_count,
            rgba: Vec::new(),
        };
        let icon = |data| ExeIcon {
            group_id: ResId::Id(1),
            entry: entries[0],
            data,
        };
        let icons = [
            icon(IconData::Dib(img(32, 4))),
            icon(IconData::Dib(img(32, 8))),
            icon(IconData::Dib(img(16, 32))),
            icon(IconData::Png(&[])),
        ];
        let b = best(&icons).unwrap();
        assert_eq!((b.width, b.bit_count), (32, 8));
    }

    /// The real exe's icon group (skips without `SHUPPATSU_DATA_DIR`).
    #[test]
    fn real_exe_icon() {
        let Some(root) = std::env::var_os("SHUPPATSU_DATA_DIR") else {
            eprintln!("skipped: SHUPPATSU_DATA_DIR not set");
            return;
        };
        let path =
            std::path::Path::new(&root).join("install/Program_Executable_Files/BIN/dengo3k.exe");
        let Ok(bytes) = std::fs::read(&path) else {
            eprintln!("skipped: {} not found", path.display());
            return;
        };
        let exe = Exe::parse(&bytes).unwrap();
        let icons = exe_icons(&exe).unwrap();
        for i in &icons {
            eprintln!("{:?} {:?}", i.group_id, i.entry);
        }
        assert_eq!(icons.len(), 2);
        assert!(icons.iter().all(|i| matches!(i.data, IconData::Dib(_))));
        let b = best(&icons).unwrap();
        assert_eq!((b.width, b.height, b.bit_count), (32, 32, 4));
        // Some pixels are opaque, and the mask makes some transparent.
        assert!(b.rgba.as_chunks::<4>().0.iter().any(|p| p[3] == 255));
        assert!(b.rgba.as_chunks::<4>().0.iter().any(|p| p[3] == 0));
    }
}

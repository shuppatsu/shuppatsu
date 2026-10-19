//! Enhancement `texture_pack`: replacement images for the 3D texture pages, read from a pack
//! directory (`shuppatsu_formats::texpack`, docs/texture-packs.md). Off (no pack) = the original
//! textures. A page the pack lacks, or whose image is unreadable or not a whole multiple of
//! the original's size, keeps the original texture (reported once on stderr).

use shuppatsu_formats::texpack::{decode_qoi, page_path, scale_of};
use shuppatsu_render::world::PageImage;
use std::path::Path;

/// Replaces `pages` (the pages of the texture file `tex_name`, in file order) with the pack's
/// images where it has them. Returns how many were replaced.
pub fn apply(dir: &Path, tex_name: &str, pages: &mut [PageImage]) -> usize {
    let mut replaced = 0;
    for (i, page) in pages.iter_mut().enumerate() {
        let rel = page_path(tex_name, i);
        let path = dir.join(&rel);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                eprintln!("warning: texture pack: {}: {e}", path.display());
                continue;
            }
        };
        let img = match decode_qoi(&bytes) {
            Ok(img) => img,
            Err(e) => {
                eprintln!("warning: texture pack: {}: {e}", path.display());
                continue;
            }
        };
        if scale_of((page.width, page.height), (img.width, img.height)).is_none() {
            eprintln!(
                "warning: texture pack: {rel} is {}×{}, not a whole multiple of the original \
                 {}×{}; keeping the original",
                img.width, img.height, page.width, page.height
            );
            continue;
        }
        *page = PageImage {
            width: img.width,
            height: img.height,
            rgba: img.rgba,
        };
        replaced += 1;
    }
    replaced
}

#[cfg(test)]
mod tests {
    use super::*;
    use shuppatsu_formats::tex::{TexFile, TexPage, TexRect};
    use shuppatsu_formats::texpack::{RgbaImage, encode_qoi};
    use shuppatsu_render::PartLibrary;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let d =
            std::env::temp_dir().join(format!("shuppatsu-texpack-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn pack_pages_replace_the_originals_and_keep_uvs() {
        // texR9.rom with two 2×1 pages and a rect over the right half of page 1.
        let tex = TexFile {
            rects: vec![TexRect {
                id: 1,
                page: 1,
                u: 128,
                v: 0,
                w: 127,
                h: 255,
            }],
            pages: vec![
                TexPage {
                    width: 2,
                    height: 1,
                    pixels: vec![0xffff, 0],
                },
                TexPage {
                    width: 2,
                    height: 1,
                    pixels: vec![0xf800, 0x001f],
                },
            ],
        };
        let mut lib = PartLibrary::default();
        lib.add_textures(&tex);
        let uv_before = lib.rect(1).unwrap().1.map_uv([256, 0]);

        let dir = temp_dir("replace");
        // Page 1 at ×3 (6×3); page 0 has no file; a wrong-size page 0 of another file.
        let big = RgbaImage {
            width: 6,
            height: 3,
            rgba: [1, 2, 3, 255].repeat(18),
        };
        std::fs::create_dir_all(dir.join("texr9")).unwrap();
        std::fs::write(dir.join(page_path("texR9.rom", 1)), encode_qoi(&big)).unwrap();
        assert_eq!(apply(&dir, "texR9.rom", &mut lib.pages), 1);
        assert_eq!((lib.pages[1].width, lib.pages[1].height), (6, 3));
        assert_eq!(lib.pages[1].rgba, big.rgba);
        assert_eq!((lib.pages[0].width, lib.pages[0].rgba[3]), (2, 255), "kept");
        // UVs are page-normalised: the rect maps to the same place on the larger page.
        assert_eq!(lib.rect(1).unwrap().1.map_uv([256, 0]), uv_before);

        let odd = RgbaImage {
            width: 5,
            height: 3,
            rgba: vec![0; 60],
        };
        std::fs::write(dir.join(page_path("texR9.rom", 0)), encode_qoi(&odd)).unwrap();
        std::fs::write(dir.join("texr9/page01.qoi"), b"not qoi").unwrap();
        let mut fresh = PartLibrary::default();
        fresh.add_textures(&tex);
        assert_eq!(apply(&dir, "texR9.rom", &mut fresh.pages), 0);
        assert_eq!(fresh.pages[0].width, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

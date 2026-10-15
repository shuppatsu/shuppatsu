//! Tests against the user's own game data. They skip unless `DDG_DATA_DIR` points at the
//! extracted disc (the default layout from `scripts/extract-disc.sh`).

use ddg_formats::{lz, rom2::Rom2};
use std::path::PathBuf;

fn data_dir() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("DDG_DATA_DIR")?);
    let dir = root.join("install/Program_Executable_Files/DATA");
    dir.is_dir().then_some(dir)
}

macro_rules! require_data {
    () => {
        match data_dir() {
            Some(d) => d,
            None => {
                eprintln!("skipped: DDG_DATA_DIR not set");
                return;
            }
        }
    };
}

fn parts_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let n = p
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_ascii_lowercase();
            n.starts_with("parts") && n.ends_with(".rom")
        })
        .collect();
    files.sort();
    files
}

#[test]
fn every_part_unpacks_to_declared_size() {
    let dir = require_data!();
    let files = parts_files(&dir);
    assert_eq!(files.len(), 15);
    let (mut packed, mut raw) = (0, 0);
    for path in files {
        let data = std::fs::read(&path).unwrap();
        let rom = Rom2::parse(&data).unwrap();
        for (i, entry) in rom.iter().enumerate() {
            // Entry = u16 part id + block.
            let block = &entry[2..];
            let out = lz::unpack_block(block)
                .unwrap_or_else(|e| panic!("{} entry {i}: {e}", path.display()));
            let declared = u32::from_le_bytes(block[..4].try_into().unwrap()) as usize;
            assert_eq!(out.len(), declared);
            if block[5] == 0 { raw += 1 } else { packed += 1 }
        }
    }
    // Counts from docs/re/loaders.md §4a.
    assert_eq!((packed, raw), (9877, 3210));
}

#[test]
fn every_part_parses_exactly() {
    use ddg_formats::part::{MAX_BYTE_INDEX_VERTS, Part};
    let dir = require_data!();
    let (mut parts, mut groups, mut verts, mut tris, mut wide) = (0, 0, 0, 0, 0);
    for path in parts_files(&dir) {
        let data = std::fs::read(&path).unwrap();
        let rom = Rom2::parse(&data).unwrap();
        for (i, entry) in rom.iter().enumerate() {
            // Part::parse rejects trailing bytes and out-of-range indices.
            let (_, part) = Part::parse_entry(entry)
                .unwrap_or_else(|e| panic!("{} entry {i}: {e}", path.display()));
            parts += 1;
            for g in &part.groups {
                assert_eq!(g.indices.len() % 3, 0, "{} entry {i}", path.display());
                groups += 1;
                verts += g.vertices.len();
                tris += g.indices.len() / 3;
                if g.vertices.len() > usize::from(MAX_BYTE_INDEX_VERTS) {
                    wide += 1;
                }
            }
        }
    }
    // Totals from docs/re/models.md.
    assert_eq!(
        (parts, groups, verts, tris, wide),
        (13087, 37736, 497336, 314511, 7)
    );
}

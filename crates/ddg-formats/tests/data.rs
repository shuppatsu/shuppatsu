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

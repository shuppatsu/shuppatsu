//! Data tests for textures, 2D graphics and audio. Skip unless `DDG_DATA_DIR` is set.

use ddg_formats::{audio, cg, exe::Exe, tex::TexFile};
use std::path::{Path, PathBuf};

fn install_dir() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("DDG_DATA_DIR")?);
    let dir = root.join("install/Program_Executable_Files");
    dir.is_dir().then_some(dir)
}

macro_rules! require_data {
    () => {
        match install_dir() {
            Some(d) => d,
            None => {
                eprintln!("skipped: DDG_DATA_DIR not set");
                return;
            }
        }
    };
}

fn read(dir: &Path, name: &str) -> Vec<u8> {
    std::fs::read(dir.join("DATA").join(name)).unwrap()
}

#[test]
fn every_texture_file_parses_to_eof() {
    let dir = require_data!();
    let mut names: Vec<String> = std::fs::read_dir(dir.join("DATA"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.to_ascii_lowercase().starts_with("tex"))
        .collect();
    names.sort();
    assert_eq!(names.len(), 15);
    for name in names {
        let t = TexFile::parse(&read(&dir, &name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(!t.pages.is_empty(), "{name}");
        for r in &t.rects {
            let p = &t.pages[r.page as usize];
            assert!(
                r.u as u16 + r.width <= p.width.max(256)
                    && r.v as u16 + r.height <= p.height.max(256),
                "{name}: rect {} outside page",
                r.id
            );
        }
    }
}

#[test]
fn every_cg_tile_and_tilemap_decodes() {
    let dir = require_data!();
    let cg_data = read(&dir, "cg.rom");
    let cg = cg::CgFile::parse(&cg_data).unwrap();
    assert_eq!(cg.len(), 44306);
    for i in 0..cg.len() {
        cg.tile(i).unwrap_or_else(|e| panic!("tile {i}: {e}"));
    }
    let maps = cg::parse_tbl(&read(&dir, "tbl.rom")).unwrap();
    assert_eq!(maps.len(), 1365);
    for (i, m) in maps.iter().enumerate() {
        assert!(
            m.tiles.iter().all(|&t| (t as usize) < cg.len()),
            "tilemap {i}"
        );
    }
}

#[test]
fn palettes_read_from_exe() {
    let dir = require_data!();
    let exe_data = std::fs::read(dir.join("BIN/dengo3k.exe")).unwrap();
    let exe = Exe::parse(&exe_data).unwrap();
    // Palette 1 source blob, from GAME_INIT (0x40e910): set_palettes(1, 1, 0x4636c0).
    let p = cg::Palette::parse(exe.bytes_at(0x4636c0, cg::Palette::BYTES).unwrap()).unwrap();
    assert!(p.0.iter().any(|&c| c != 0));
}

#[test]
fn audio_banks_parse() {
    let dir = require_data!();
    let fx_data = read(&dir, "effect.rom");
    let fx = audio::parse_effects(&fx_data).unwrap();
    assert_eq!(fx.len(), 407);
    // docs/re/loaders.md §6: 295 entries are 10 kHz voice clips.
    assert_eq!(fx.iter().filter(|e| e.sample_rate == 10_000).count(), 295);
    let bgm_data = read(&dir, "bgm.rom");
    let bgm = audio::parse_bgm(&bgm_data).unwrap();
    assert_eq!(bgm.len(), 18);
    assert_eq!(bgm.iter().filter(|t| t.looping).count(), 1);
}

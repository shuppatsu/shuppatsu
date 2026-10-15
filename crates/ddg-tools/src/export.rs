//! `ddg-tools export`: writes inspectable PNG/WAV files derived from the game data into `out/`
//! (gitignored). Nothing written here may be committed.

use anyhow::{Context, Result, bail};
use ddg_formats::{audio, cg, exe::Exe, tex::TexFile};
use std::path::{Path, PathBuf};

/// Game install directories under `$DDG_DATA_DIR`.
pub struct GameDirs {
    pub data: PathBuf,
    pub bin: PathBuf,
}

impl GameDirs {
    pub fn from_env() -> Result<Self> {
        let root = std::env::var_os("DDG_DATA_DIR")
            .map(PathBuf::from)
            .context("DDG_DATA_DIR is not set (run inside `nix develop`)")?;
        let install = root.join("install/Program_Executable_Files");
        let dirs = Self {
            data: install.join("DATA"),
            bin: install.join("BIN"),
        };
        if !dirs.data.is_dir() {
            bail!(
                "{} not found; run scripts/extract-disc.sh",
                dirs.data.display()
            );
        }
        Ok(dirs)
    }

    /// Reads a data file, matching the name case-insensitively (the exe uses lowercase names,
    /// the disc uppercase for some).
    pub fn read_data(&self, name: &str) -> Result<Vec<u8>> {
        let path = std::fs::read_dir(&self.data)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name))
            })
            .with_context(|| format!("{name} not found in {}", self.data.display()))?;
        Ok(std::fs::read(path)?)
    }

    pub fn data_names(&self, prefix: &str) -> Result<Vec<String>> {
        let mut names: Vec<String> = std::fs::read_dir(&self.data)?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.to_ascii_lowercase().starts_with(prefix))
            .collect();
        names.sort_by_key(|n| n.to_ascii_lowercase());
        Ok(names)
    }

    pub fn read_exe(&self) -> Result<Vec<u8>> {
        Ok(std::fs::read(self.bin.join("dengo3k.exe"))?)
    }
}

fn save_png(path: &Path, width: u32, height: u32, rgba: Vec<u8>) -> Result<()> {
    let img = image::RgbaImage::from_raw(width, height, rgba).context("image size mismatch")?;
    img.save(path)
        .with_context(|| format!("writing {}", path.display()))
}

fn stem(name: &str) -> String {
    name.rsplit_once('.')
        .map_or(name, |(s, _)| s)
        .to_ascii_lowercase()
}

pub fn textures(dirs: &GameDirs, out: &Path) -> Result<usize> {
    let mut written = 0;
    for name in dirs.data_names("tex")? {
        let t = TexFile::parse(&dirs.read_data(&name)?).with_context(|| name.clone())?;
        let dir = out.join("tex").join(stem(&name));
        std::fs::create_dir_all(&dir)?;
        for (i, page) in t.pages.iter().enumerate() {
            save_png(
                &dir.join(format!("page{i:02}.png")),
                page.width.into(),
                page.height.into(),
                page.to_rgba8(),
            )?;
            written += 1;
        }
        let mut csv = String::from("rect_id,page,u,v,width,height\n");
        for r in &t.rects {
            csv += &format!(
                "{},{},{},{},{},{}\n",
                r.id, r.page, r.u, r.v, r.width, r.height
            );
        }
        std::fs::write(dir.join("rects.csv"), csv)?;
    }
    Ok(written)
}

/// Palette blobs set by GAME_INIT (0x40e910): (palette slot, source VA). Small verified
/// constants; the colours themselves are read from the user's exe.
const INIT_PALETTES: &[(u8, u32)] = &[
    (1, 0x4636c0),
    (4, 0x488290),
    (5, 0x489e90),
    (6, 0x48a290),
    (7, 0x489290),
    (8, 0x487e90),
    (9, 0x487a90),
    (10, 0x48aa90),
    (11, 0x488e90),
    (12, 0x487690),
    (13, 0x489690),
    (14, 0x489a90),
    (15, 0x488690),
    (16, 0x488a90),
    (17, 0x48a690),
    (18, 0x463740),
    (24, 0x48ae90),
    (25, 0x48b290),
    (26, 0x48b690),
];

pub fn init_palettes(exe: &Exe<'_>) -> Result<Vec<(u8, cg::Palette)>> {
    INIT_PALETTES
        .iter()
        .map(|&(slot, va)| {
            Ok((
                slot,
                cg::Palette::parse(exe.bytes_at(va, cg::Palette::BYTES)?)?,
            ))
        })
        .collect()
}

/// Tilemaps rendered with one palette slot (default 4, the one most UI tasks use; the true
/// palette per tilemap is chosen at runtime and is not recorded in the data).
pub fn tilemaps(dirs: &GameDirs, out: &Path, palette_slot: u8) -> Result<usize> {
    let exe_data = dirs.read_exe()?;
    let exe = Exe::parse(&exe_data)?;
    let palettes = init_palettes(&exe)?;
    let palette = palettes
        .iter()
        .find(|(s, _)| *s == palette_slot)
        .map(|(_, p)| p)
        .with_context(|| format!("palette slot {palette_slot} is not set at init"))?;

    // Swatches of every init palette, 16×16 cells of 8 px.
    let pal_dir = out.join("palettes");
    std::fs::create_dir_all(&pal_dir)?;
    for (slot, p) in &palettes {
        let mut rgba = vec![0u8; 128 * 128 * 4];
        for (i, px) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (x, y) = (i % 128 / 8, i / 128 / 8);
            let c = p.0[y * 16 + x].to_le_bytes();
            *px = [c[1], c[2], c[0], 255];
        }
        save_png(
            &pal_dir.join(format!("palette{slot:03}.png")),
            128,
            128,
            rgba,
        )?;
    }

    let cg_data = dirs.read_data("cg.rom")?;
    let cg = cg::CgFile::parse(&cg_data)?;
    let maps = cg::parse_tbl(&dirs.read_data("tbl.rom")?)?;
    let dir = out.join(format!("tbl-pal{palette_slot:03}"));
    std::fs::create_dir_all(&dir)?;
    let mut written = 0;
    for (i, m) in maps.iter().enumerate() {
        if m.width == 0 || m.height == 0 {
            continue;
        }
        let indices = m.render_indices(&cg)?;
        save_png(
            &dir.join(format!("{i:04}.png")),
            m.width as u32 * 16,
            m.height as u32 * 16,
            palette.indices_to_rgba8(&indices),
        )?;
        written += 1;
    }
    Ok(written)
}

pub fn audio(dirs: &GameDirs, out: &Path) -> Result<usize> {
    let mut written = 0;
    let sfx_dir = out.join("sfx");
    std::fs::create_dir_all(&sfx_dir)?;
    let fx_data = dirs.read_data("effect.rom")?;
    for (i, fx) in audio::parse_effects(&fx_data)?.iter().enumerate() {
        let code = i + 1;
        let looped = if fx.loop_range.is_some() { "_loop" } else { "" };
        std::fs::write(
            sfx_dir.join(format!("{code:03x}{looped}.wav")),
            audio::wav(fx.pcm_u8, fx.sample_rate, 1, 8),
        )?;
        written += 1;
    }
    let bgm_dir = out.join("bgm");
    std::fs::create_dir_all(&bgm_dir)?;
    let bgm_data = dirs.read_data("bgm.rom")?;
    for t in audio::parse_bgm(&bgm_data)? {
        let looped = if t.looping { "_loop" } else { "" };
        std::fs::write(
            bgm_dir.join(format!("{:03x}{looped}.wav", t.code)),
            audio::wav(t.pcm_s16le, audio::BGM_SAMPLE_RATE, 2, 16),
        )?;
        written += 1;
    }
    Ok(written)
}

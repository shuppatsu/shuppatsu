//! `shuppatsu-tools export`: writes inspectable PNG/WAV files derived from the game data into `out/`
//! (gitignored). Nothing written here may be committed.

use anyhow::{Context, Result, bail};
use shuppatsu_formats::{audio, cg, exe::Exe, tex::TexFile};
use std::path::{Path, PathBuf};

/// Game install directories under `$SHUPPATSU_DATA_DIR`.
pub struct GameDirs {
    pub data: PathBuf,
    pub bin: PathBuf,
}

impl GameDirs {
    pub fn from_env() -> Result<Self> {
        let root = std::env::var_os("SHUPPATSU_DATA_DIR")
            .map(PathBuf::from)
            .context("SHUPPATSU_DATA_DIR is not set (run inside `nix develop`)")?;
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
                page.to_rgba8_upright(),
            )?;
            written += 1;
        }
        let mut csv = String::from("rect_id,page,u,v,w_minus_1,h_minus_1 (1/256 page units)\n");
        for r in &t.rects {
            csv += &format!("{},{},{},{},{},{}\n", r.id, r.page, r.u, r.v, r.w, r.h);
        }
        std::fs::write(dir.join("rects.csv"), csv)?;
    }
    Ok(written)
}

/// Tilemaps rendered with one palette slot (default 4, the one most UI tasks use; the true
/// palette per tilemap is chosen at runtime and is not recorded in the data).
pub fn tilemaps(dirs: &GameDirs, out: &Path, palette_slot: u8) -> Result<usize> {
    let exe_data = dirs.read_exe()?;
    let exe = Exe::parse(&exe_data)?;
    let palettes = cg::init_palettes(&exe)?;
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

// ---------------------------------------------------------------------------------------------
// 3D: models and routes as glTF.
//
// Conversion from the game's left-handed, Y-up space to glTF's right-handed, Y-up space: negate Z
// and reverse each triangle's winding. UVs: the PNGs are exported upright (tex pages are stored
// bottom-up and the game's UVs address that layout), so V is mirrored (v' = 1 − v).

use crate::gltf::{Gltf, Primitive};
use shuppatsu_formats::{part::Part, rom2::Rom2, route::Route, stages::StageTables, tex::TexRect};
use std::collections::HashMap;

/// Texture rects by global id → (tex file stem, rect). Files later in `tex_names` win, matching
/// the game (rect table 0x825144 is overwritten per file).
fn rect_map(dirs: &GameDirs, tex_names: &[String]) -> Result<HashMap<u16, (String, TexRect)>> {
    let mut map = HashMap::new();
    for name in tex_names {
        let t = TexFile::parse(&dirs.read_data(name)?).with_context(|| name.clone())?;
        for r in t.rects {
            map.insert(r.id, (stem(name), r));
        }
    }
    Ok(map)
}

/// Parts by id from `parts_names`; the first file wins (part table 0x817c7c).
fn part_map(dirs: &GameDirs, parts_names: &[String]) -> Result<HashMap<u16, Part>> {
    let mut map = HashMap::new();
    for name in parts_names {
        let data = dirs.read_data(name)?;
        for entry in Rom2::parse(&data)?.iter() {
            let (id, part) = Part::parse_entry(entry).with_context(|| name.clone())?;
            map.entry(id).or_insert(part);
        }
    }
    Ok(map)
}

/// Converts a part into glTF primitives, one per (tex file, page) material.
fn part_mesh(
    gltf: &mut Gltf,
    materials: &mut HashMap<(String, u16), usize>,
    rects: &HashMap<u16, (String, TexRect)>,
    id: u16,
    part: &Part,
) -> usize {
    let mut prims = Vec::new();
    for g in &part.groups {
        let material = rects.get(&g.rect_id).map(|(tex, r)| {
            *materials.entry((tex.clone(), r.page)).or_insert_with(|| {
                let uri = format!("../tex/{tex}/page{:02}.png", r.page);
                gltf.textured_material(&format!("{tex}/page{:02}", r.page), &uri)
            })
        });
        let positions = g
            .vertices
            .iter()
            .map(|v| {
                let [x, y, z] = v.position();
                [x, y, -z]
            })
            .collect();
        let uvs = rects.get(&g.rect_id).map(|(_, r)| {
            g.page_uvs(r)
                .into_iter()
                .map(|[u, v]| [u, 1.0 - v])
                .collect()
        });
        let indices = g
            .triangles()
            .flat_map(|[a, b, c]| [u32::from(a), u32::from(c), u32::from(b)])
            .collect();
        prims.push(Primitive {
            positions,
            uvs,
            indices: Some(indices),
            material,
            mode: 4,
        });
    }
    gltf.mesh(&format!("part{id}"), prims)
}

fn write_gltf(dir: &Path, name: &str, gltf: Gltf) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let (json, bin) = gltf.finish(&format!("{name}.bin"));
    std::fs::write(dir.join(format!("{name}.gltf")), json)?;
    std::fs::write(dir.join(format!("{name}.bin")), bin)?;
    Ok(())
}

/// The texture set paired with a parts file: `partsTR3.rom` ↔ `texTR3.rom`, plus the common set.
fn tex_for(parts_name: &str) -> Vec<String> {
    let suffix = &stem(parts_name)["parts".len()..];
    let mut names = vec!["texRC.rom".to_owned()];
    if suffix != "rc" {
        names.push(format!("tex{}.rom", suffix.to_ascii_uppercase()));
    }
    names
}

/// One glTF per parts file, every part laid out on a grid for inspection (models need the
/// texture PNGs from `export textures` next to them in `../tex`).
pub fn models(dirs: &GameDirs, out: &Path) -> Result<usize> {
    let dir = out.join("models");
    let mut written = 0;
    for name in dirs.data_names("parts")? {
        let rects = rect_map(dirs, &tex_for(&name))?;
        let data = dirs.read_data(&name)?;
        let mut gltf = Gltf::default();
        let mut materials = HashMap::new();
        for (i, entry) in Rom2::parse(&data)?.iter().enumerate() {
            let (id, part) = Part::parse_entry(entry).with_context(|| name.clone())?;
            let mesh = part_mesh(&mut gltf, &mut materials, &rects, id, &part);
            let (col, row) = ((i % 40) as f32, (i / 40) as f32);
            gltf.node(
                &format!("part{id}"),
                Some(mesh),
                [col * 6.0, 0.0, row * 6.0],
                0.0,
            );
        }
        write_gltf(&dir, &stem(&name), gltf)?;
        written += 1;
    }
    Ok(written)
}

/// One glTF per `.COM`: every track segment as a line strip and every placement with its model.
pub fn routes(dirs: &GameDirs, out: &Path) -> Result<usize> {
    let exe_data = dirs.read_exe()?;
    let tables = StageTables::read(&Exe::parse(&exe_data)?)?;
    let dir = out.join("routes");
    let mut written = 0;
    for (com, file) in tables.com_files.iter().enumerate() {
        let Some(file) = file else { continue };
        // The area comes from any stage that uses this COM.
        let area = tables
            .stages
            .iter()
            .find(|s| s.segments.iter().any(|seg| seg.com as usize == com))
            .map(|s| s.area)
            .with_context(|| format!("no stage uses {file}"))?;
        let route = Route::parse(&dirs.read_data(file)?).with_context(|| file.clone())?;
        let parts = part_map(dirs, &["partsRC.rom".into(), format!("partsR{area}.rom")])?;
        let rects = rect_map(dirs, &["texRC.rom".into(), format!("texR{area}.rom")])?;

        let mut gltf = Gltf::default();
        let track = gltf.colour_material("track", [1.0, 0.2, 0.1]);
        for (si, seg) in route.segments.iter().enumerate() {
            let positions: Vec<[f32; 3]> = route.slots[seg.points.clone()]
                .iter()
                .map(|p| [p.x, p.y(), -p.z])
                .collect();
            if positions.len() < 2 {
                continue;
            }
            let mesh = gltf.mesh(
                &format!("segment{si}"),
                vec![Primitive {
                    positions,
                    uvs: None,
                    indices: None,
                    material: Some(track),
                    mode: 3,
                }],
            );
            gltf.node(&format!("segment{si}"), Some(mesh), [0.0; 3], 0.0);
        }
        let mut materials = HashMap::new();
        let mut meshes: HashMap<u32, Option<usize>> = HashMap::new();
        for (i, p) in route.placements.iter().enumerate().skip(1) {
            let mesh = *meshes.entry(p.part_id).or_insert_with(|| {
                let id = u16::try_from(p.part_id).ok()?;
                let part = parts.get(&id)?;
                Some(part_mesh(&mut gltf, &mut materials, &rects, id, part))
            });
            // Yaw sign flips with the Z mirror (guess; verify visually against the original).
            gltf.node(
                &format!("place{i}_part{}", p.part_id),
                mesh,
                [p.x, p.y, -p.z],
                -p.yaw,
            );
        }
        write_gltf(&dir, &stem(file), gltf)?;
        written += 1;
    }
    Ok(written)
}

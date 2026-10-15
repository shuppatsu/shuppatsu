//! Loads a route's world from `$DDG_DATA_DIR`: textures and parts for its area, scenery
//! placements and track lines.

use anyhow::{Context, Result};
use ddg_formats::{
    exe::Exe, part::Part, rom2::Rom2, route::Route, stages::StageTables, tex::TexFile,
};
use ddg_render::world::{World, WorldBuilder, placement_transform};
use glam::Vec3;
use std::path::PathBuf;

pub struct GameData {
    data: PathBuf,
    bin: PathBuf,
}

impl GameData {
    pub fn from_env() -> Result<Self> {
        let root = std::env::var_os("DDG_DATA_DIR")
            .map(PathBuf::from)
            .context("DDG_DATA_DIR is not set (run inside `nix develop`)")?;
        let install = root.join("install/Program_Executable_Files");
        Ok(Self {
            data: install.join("DATA"),
            bin: install.join("BIN"),
        })
    }

    /// Reads a data file by case-insensitive name (the exe uses lowercase, the disc some uppercase).
    pub fn read(&self, name: &str) -> Result<Vec<u8>> {
        let path = std::fs::read_dir(&self.data)
            .with_context(|| format!("reading {}", self.data.display()))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name))
            })
            .with_context(|| format!("{name} not found in {}", self.data.display()))?;
        Ok(std::fs::read(path)?)
    }

    pub fn exe(&self) -> Result<Vec<u8>> {
        Ok(std::fs::read(self.bin.join("dengo3k.exe"))?)
    }
}

pub struct RouteScene {
    pub world: World,
    pub main_line: Vec<Vec3>,
    pub area: u32,
    pub placements: usize,
    pub missing_parts: usize,
}

/// Builds the scene for `com_name` (e.g. "yamate1a.com").
pub fn load_route(data: &GameData, com_name: &str) -> Result<RouteScene> {
    let exe_bytes = data.exe()?;
    let tables = StageTables::read(&Exe::parse(&exe_bytes)?)?;
    let com = tables
        .com_files
        .iter()
        .position(|f| {
            f.as_deref()
                .is_some_and(|f| f.eq_ignore_ascii_case(com_name))
        })
        .with_context(|| format!("{com_name} is not a route file listed in the exe"))?;
    let area = tables
        .stages
        .iter()
        .find(|s| s.segments.iter().any(|seg| seg.com as usize == com))
        .map(|s| s.area)
        .with_context(|| format!("no stage uses {com_name}"))?;

    let mut builder = WorldBuilder::default();
    // Common set first, then the area: later rects win, earlier parts win (as the game).
    for tex in ["texRC.rom".to_owned(), format!("texR{area}.rom")] {
        builder.add_textures(&TexFile::parse(&data.read(&tex)?).with_context(|| tex.clone())?);
    }
    for parts in ["partsRC.rom".to_owned(), format!("partsR{area}.rom")] {
        let bytes = data.read(&parts)?;
        let entries = Rom2::parse(&bytes)?
            .iter()
            .map(Part::parse_entry)
            .collect::<ddg_formats::Result<Vec<_>>>()
            .with_context(|| parts.clone())?;
        builder.add_parts(entries);
    }

    let route = Route::parse(&data.read(com_name)?).with_context(|| com_name.to_owned())?;
    let mut placements = 0;
    for p in route.placements.iter().skip(1) {
        let Ok(id) = u16::try_from(p.part_id) else {
            continue;
        };
        if builder.add_instance(id, placement_transform(Vec3::new(p.x, p.y, p.z), p.yaw)) {
            placements += 1;
        }
    }
    let to_vec = |p: &ddg_formats::route::TrackPoint| Vec3::new(p.x, p.y(), p.z);
    for (i, seg) in route.segments.iter().enumerate() {
        let pts: Vec<Vec3> = route.slots[seg.points.clone()].iter().map(to_vec).collect();
        let colour = if i == 0 {
            [1.0, 0.1, 0.1, 1.0]
        } else {
            [1.0, 0.6, 0.1, 1.0]
        };
        builder.add_line_strip(&pts, colour);
    }
    let main_line = route.main_line().iter().map(to_vec).collect();
    let missing_parts = builder.missing_parts;
    Ok(RouteScene {
        world: builder.finish(),
        main_line,
        area,
        placements,
        missing_parts,
    })
}

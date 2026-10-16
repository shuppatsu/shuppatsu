//! AI trains in the cab view: the stage's TR part/texture sets and the cars of
//! [`ddg_engine::drive::Drive::traffic_poses`] as part instances (docs/re/traffic.md §4).

use crate::scene::{GameData, RouteScene};
use anyhow::{Context, Result};
use ddg_engine::traffic::{CarPose, lod_for_view, view_xz};
use ddg_formats::exe::Exe;
use ddg_formats::scene::{self, TrackPos};
use ddg_formats::stages::StageTables;
use ddg_formats::{part::Part, rom2::Rom2, tex::TexFile};
use ddg_render::FrameGeometry;
use glam::Mat4;

/// Loads stage `stage`'s TR sets after the area's sets, as `stage_assets_load` (0x40ed90)
/// does: TR *k* is `texTR(k+1)` then `partsTR(k+1)` (rects: last loaded wins; parts: first).
pub fn load_stage_trains(scene: &mut RouteScene, data: &GameData, stage: usize) -> Result<()> {
    let exe_bytes = data.exe()?;
    let stages = StageTables::read(&Exe::parse(&exe_bytes)?)?;
    let Some(st) = stages.stages.get(stage) else {
        return Ok(());
    };
    for &k in &st.train_sets {
        let tex = format!("texTR{}.rom", k + 1);
        scene
            .library
            .add_textures(&TexFile::parse(&data.read(&tex)?).with_context(|| tex.clone())?);
        let parts = format!("partsTR{}.rom", k + 1);
        let bytes = data.read(&parts)?;
        let entries = Rom2::parse(&bytes)?
            .iter()
            .map(Part::parse_entry)
            .collect::<ddg_formats::Result<Vec<_>>>()
            .with_context(|| parts.clone())?;
        scene.library.add_parts(entries);
    }
    Ok(())
}

/// Adds the cars seen from the cab camera at `pos`, each with the part of its distance band
/// (0x405640: up to 150 m ahead, 25 m behind; no frustum test).
pub fn add_cars(scene: &RouteScene, frame: &mut FrameGeometry, pos: TrackPos, cars: &[CarPose]) {
    let camera = scene::cab_camera(&scene.route, pos, scene.eye_height, 0.0);
    for car in cars {
        let (x, z) = view_xz(&camera, car.origin());
        let Some(lod) = lod_for_view(x, z) else {
            continue;
        };
        let (part, m) = car.instance(lod);
        frame.add_instance(
            &scene.library,
            part,
            &Mat4::from_cols_array(&m.to_cols_array()),
        );
        if let Some((part, m)) = car.extra {
            frame.add_instance(
                &scene.library,
                part,
                &Mat4::from_cols_array(&m.to_cols_array()),
            );
        }
    }
}

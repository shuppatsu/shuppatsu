//! AI trains in the cab view: the stage's TR part/texture sets and the cars of
//! [`ddg_engine::drive::Drive::traffic_poses`] as part instances (docs/re/traffic.md §4).

use crate::scene::{GameData, RouteScene};
use anyhow::{Context, Result};
use ddg_engine::traffic::{CarPose, LOD_DEPTHS, lod_for_view, view_xz};
use ddg_formats::exe::Exe;
use ddg_formats::scene::{self, TrackPos};
use ddg_formats::stages::StageTables;
use ddg_formats::{part::Part, rom2::Rom2};
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
        data.add_textures(&mut scene.library, &tex)?;
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
/// (0x405640: up to 150 m ahead, 25 m behind; no frustum test). With `far_traffic`
/// ([`RouteScene::traffic_reach`]) cars beyond 150 m and up to the reach are drawn in the
/// farthest band; only drawing changes (the sounds use the original bands in the engine).
pub fn add_cars(scene: &RouteScene, frame: &mut FrameGeometry, pos: TrackPos, cars: &[CarPose]) {
    let camera = scene::cab_camera(&scene.route, pos, scene.eye_height, 0.0);
    for car in cars {
        let (x, z) = view_xz(&camera, car.origin());
        let Some(lod) = lod_for_view(x, far_z(z, scene.traffic_reach)) else {
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

/// The view depth the distance bands see: with a reach, a car between the original's 150 m
/// (`LOD_DEPTHS[0]`, 0x458c30) and the reach counts as at the edge of the farthest band.
fn far_z(z: f64, reach: Option<f64>) -> f64 {
    match reach {
        Some(r) if z >= -r => z.max(LOD_DEPTHS[0]),
        _ => z,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn far_traffic_uses_the_farthest_band() {
        let reach = Some(f64::from(ddg_render::camera::FAR));
        // Inside 150 m nothing changes.
        for z in [2.0, -1.0, -7.5, -14.9, -15.0] {
            assert_eq!(far_z(z, reach), z);
        }
        assert_eq!(lod_for_view(0.0, far_z(-20.0, None)), None);
        assert_eq!(lod_for_view(0.0, far_z(-20.0, reach)), Some(4));
        assert_eq!(lod_for_view(0.0, far_z(-31.9, reach)), Some(4));
        // The lateral bands still apply, and nothing beyond the reach.
        assert_eq!(lod_for_view(20.0, far_z(-20.0, reach)), None);
        assert_eq!(lod_for_view(0.0, far_z(-32.5, reach)), None);
    }
}

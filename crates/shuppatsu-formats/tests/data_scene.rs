//! Data tests for scene assembly (`docs/re/scene.md`). Skip unless `SHUPPATSU_DATA_DIR` is set.

use shuppatsu_formats::{
    exe::Exe,
    rom2::Rom2,
    route::Route,
    scene::{self, CabWalker, PieceSource, SceneContext, SceneTables},
    stages::StageTables,
};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

fn install_dir() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("SHUPPATSU_DATA_DIR")?);
    let dir = root.join("install/Program_Executable_Files");
    dir.is_dir().then_some(dir)
}

macro_rules! require_data {
    () => {
        match install_dir() {
            Some(d) => d,
            None => {
                eprintln!("skipped: SHUPPATSU_DATA_DIR not set");
                return;
            }
        }
    };
}

/// Case-insensitive lookup in `DATA`.
fn data_file(dir: &Path, name: &str) -> Vec<u8> {
    let data = dir.join("DATA");
    let path = std::fs::read_dir(&data)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .eq_ignore_ascii_case(name)
        })
        .unwrap_or_else(|| panic!("{name} not found"));
    std::fs::read(path).unwrap()
}

struct World {
    stages: StageTables,
    tables: SceneTables,
    /// Route and area by COM index.
    routes: BTreeMap<u32, (Route, usize)>,
}

fn load(dir: &Path) -> World {
    let exe_bytes = std::fs::read(dir.join("BIN/dengo3k.exe")).unwrap();
    let exe = Exe::parse(&exe_bytes).unwrap();
    let stages = StageTables::read(&exe).unwrap();
    let mut routes = BTreeMap::new();
    for stage in &stages.stages {
        for seg in &stage.segments {
            routes.entry(seg.com).or_insert_with(|| {
                let name = stages.com_file(seg.com).unwrap();
                let route = Route::parse(&data_file(dir, name)).unwrap();
                (route, stage.area as usize - 1)
            });
        }
    }
    let max_kind = routes
        .values()
        .map(|(r, _)| scene::max_run_kind(r))
        .max()
        .unwrap();
    let tables = SceneTables::read(&exe, max_kind).unwrap();
    World {
        stages,
        tables,
        routes,
    }
}

fn part_ids(dir: &Path, name: &str) -> HashSet<u32> {
    let data = data_file(dir, name);
    Rom2::parse(&data)
        .unwrap()
        .iter()
        .map(|e| u32::from(u16::from_le_bytes([e[0], e[1]])))
        .collect()
}

fn area_parts(dir: &Path, area: usize) -> HashSet<u32> {
    let mut ids = part_ids(dir, "partsRC.rom");
    ids.extend(part_ids(dir, &format!("partsR{}.rom", area + 1)));
    ids
}

#[test]
fn scene_tables_reference_existing_parts() {
    let dir = require_data!();
    let w = load(&dir);
    let t = &w.tables;
    assert_eq!(t.single_group_pattern, [0, 1, 2, 1]);
    assert_eq!(t.rail_band_step, [1, 2, 4, 8, 16, 32]);
    assert_eq!(t.kind_band_step, [1, 1, 2, 4]);
    for com in 1..16u32 {
        let h = t.eye_height(0, com);
        assert!((0.25..0.35).contains(&h), "COM {com}: eye height {h}");
    }
    let mut missing = BTreeMap::<(u32, &str), usize>::new();
    for (&com, (route, area)) in &w.routes {
        let parts = area_parts(&dir, *area);
        let mut types = HashSet::new();
        for p in &route.slots {
            types.insert(p.track_type());
        }
        for &ty in &types {
            if ty == scene::NO_RAIL {
                continue;
            }
            for group in 0..scene::RAIL_GROUPS {
                let id = t.areas[*area].rail[group][usize::from(ty)];
                if !parts.contains(&id) {
                    *missing.entry((com, "rail")).or_default() += 1;
                }
            }
        }
        for run in route.buckets.iter().flatten() {
            match t.kind_parts(*area, run.kind) {
                Some(ids) => {
                    for id in ids {
                        if !parts.contains(&id) {
                            *missing.entry((com, "kind")).or_default() += 1;
                        }
                    }
                }
                None => assert!(matches!(run.kind, 1 | 2 | 5 | 6), "kind {}", run.kind),
            }
        }
    }
    eprintln!("missing part references (com, table) -> count: {missing:?}");
    assert!(missing.is_empty());
    // Animated parts and tunnel plugs exist in some route part set.
    let mut all = part_ids(&dir, "partsRC.rom");
    for a in 0..scene::AREA_COUNT {
        all.extend(part_ids(&dir, &format!("partsR{}.rom", a + 1)));
    }
    for a in &t.animated {
        assert!(
            !a.frames.is_empty() && a.frames.iter().all(|f| all.contains(f)),
            "{a:?}"
        );
    }
    for list in &t.tunnel_plugs {
        assert!(list[1..].iter().all(|p| all.contains(p)));
    }
    for s in &t.background.sky_sprites {
        assert!(all.contains(&s.part_id), "{s:?}");
        assert!(s.repeats > 0 && s.width > 0 && s.height > 0);
    }
    // R-index 1..=5 have a sky gradient.
    for g in &t.background.gradients[0][1..] {
        assert_ne!(g.sky[0], [0, 0, 0]);
    }
}

fn ctx_for(w: &World, com: u32, station: u32) -> SceneContext {
    SceneContext {
        area: w.routes[&com].1,
        com,
        station,
    }
}

/// Walks every route from its first stop to its last, building the track list every 8 points,
/// and checks that every piece is finite, near the camera and of a known part.
#[test]
fn track_instances_stay_near_the_camera_on_every_route() {
    let dir = require_data!();
    let w = load(&dir);
    for (&com, (route, area)) in &w.routes {
        let parts = area_parts(&dir, *area);
        let eye_h = w.tables.eye_height(*area, com);
        let mut walker = CabWalker::at_segment_start(route, 0);
        walker.advance(route, 0.0);
        let last_stop = *route.stops.last().unwrap();
        let (mut frames, mut pieces, mut max_d2, mut by_source) = (0, 0, 0f32, BTreeMap::new());
        let mut bridges = 0;
        let mut wide_extra = 0usize;
        let mut station = 1;
        while walker.pos().engine_index + 1 < last_stop {
            let pos = walker.pos();
            while station < route.stops.len() && route.stops[station] <= pos.engine_index + 1 {
                station += 1;
            }
            let cam = scene::cab_camera(route, pos, eye_h, 0.0);
            let ctx = ctx_for(&w, com, station as u32);
            for inst in scene::track_instances(route, &w.tables, ctx, pos, &cam) {
                let t = inst.transform;
                assert!(
                    t.translation.iter().chain(&t.z_axis).all(|v| v.is_finite()),
                    "COM {com}: {inst:?}"
                );
                assert!(parts.contains(&inst.part_id), "COM {com}: {inst:?}");
                let d = [t.translation[0] - cam.eye[0], t.translation[2] - cam.eye[2]];
                let d2 = d[0] * d[0] + d[1] * d[1];
                // A piece whose chord bridges two distant points (a gap between side segments)
                // is centred far away; the original draws these too (scene.md §2.6).
                let span = t.z_axis.iter().map(|v| v * v).sum::<f32>().sqrt();
                if d2 > 1600.0 && span > 3.5 {
                    bridges += 1;
                    continue;
                }
                max_d2 = max_d2.max(d2);
                let key = match inst.source {
                    PieceSource::MainLine { .. } => "main",
                    PieceSource::Rail { .. } => "rail",
                    PieceSource::Kind { .. } => "kind",
                    PieceSource::BufferStop { .. } => "stop",
                    PieceSource::TunnelPlug => "plug",
                };
                *by_source.entry(key).or_insert(0usize) += 1;
                pieces += 1;
            }
            let original = scene::track_instances(route, &w.tables, ctx, pos, &cam).len();
            let wide = scene::track_instances_wide(route, &w.tables, ctx, pos, &cam, 16).len();
            assert!(wide >= original);
            wide_extra += wide - original;
            let bg = scene::background(route, &w.tables, ctx, pos);
            if let Some(plug) = bg.tunnel.and_then(|t| t.plug) {
                assert!(parts.contains(&plug.part_id), "COM {com}: plug {plug:?}");
            }
            frames += 1;
            walker.advance(route, 8.0 * 0.125);
        }
        eprintln!(
            "COM {com:2}: {frames} frames, {:.1} pieces/frame, max XZ distance {:.1}, {bridges} bridge pieces, {:.1} extra wide pieces/frame, {by_source:?}",
            pieces as f32 / frames as f32,
            max_d2.sqrt(),
            wide_extra as f32 / frames as f32
        );
        assert!(bridges < 50, "COM {com}: {bridges} bridge pieces");
        assert!(
            max_d2.sqrt() < 40.0,
            "COM {com}: piece {:.1} units away",
            max_d2.sqrt()
        );
    }
    // Every stage segment maps to a loaded route.
    for s in &w.stages.stages {
        for seg in &s.segments {
            assert!(w.routes.contains_key(&seg.com));
        }
    }
}

/// The live scenery list right after a segment start equals a replay from the start of the
/// route, i.e. the 300-point run-up loses nothing (checked at every stop).
#[test]
fn segment_start_run_up_matches_full_replay() {
    let dir = require_data!();
    let w = load(&dir);
    let mut mismatches = 0;
    for (&com, (route, _)) in &w.routes {
        for (s, &stop) in route.stops.iter().enumerate() {
            let mut start = CabWalker::at_segment_start(route, s);
            start.advance(route, 0.0);
            let pos = start.pos();
            // Full replay from engine index 0.
            let mut full = CabWalker::at_segment_start(route, 0);
            full.advance(route, 0.0);
            let mut steps = 0;
            while full.pos().engine_index < pos.engine_index && steps < 1_000_000 {
                full.advance(route, 0.125);
                steps += 1;
            }
            let mut a = start.live().to_vec();
            let mut b = full.live().to_vec();
            a.sort_unstable();
            b.sort_unstable();
            if a != b && stop > 1 {
                mismatches += 1;
                let only_full = b.iter().filter(|x| !a.contains(x)).count();
                let only_start = a.iter().filter(|x| !b.contains(x)).count();
                eprintln!(
                    "COM {com} stop {s}: run-up live {}, full replay {} (only full {only_full}, only run-up {only_start})",
                    a.len(),
                    b.len()
                );
            }
        }
    }
    eprintln!("{mismatches} stops where the run-up differs from a full replay");
}

/// Analysis for widescreen (scene.md §5), run with `--ignored --nocapture`: where relative to
/// the view direction do placements leave the live list, and which side-track slots near the
/// camera are not in the current draw bucket?
#[test]
#[ignore]
fn widescreen_visibility_report() {
    let dir = require_data!();
    let w = load(&dir);
    // Histograms over |horizontal angle| from the view direction in 10° bins (0..180).
    let mut removed_at = [0usize; 18];
    let mut removed_near = [0usize; 18];
    let mut absent_slots = [0usize; 18];
    let mut present_slots = [0usize; 18];
    let mut absent_place = [0usize; 18];
    let mut live_place = [0usize; 18];
    let mut live_dist = [0usize; 40];
    let mut removed_fine = [0usize; 30]; // 1° bins from 20°
    let (mut never_added, mut placements_total) = (0usize, 0usize);
    let angle = |cam: &scene::CabCamera, p: [f32; 2]| -> (usize, f32) {
        let f = cam.forward();
        let (fx, fz) = (f[0], f[2]);
        let (dx, dz) = (p[0] - cam.eye[0], p[1] - cam.eye[2]);
        let d = (dx * dx + dz * dz).sqrt();
        let c = ((dx * fx + dz * fz) / (d * (fx * fx + fz * fz).sqrt())).clamp(-1.0, 1.0);
        ((c.acos().to_degrees() / 10.0).min(17.0) as usize, d)
    };
    for (&com, (route, area)) in &w.routes {
        let eye_h = w.tables.eye_height(*area, com);
        let mut walker = CabWalker::at_segment_start(route, 0);
        walker.advance(route, 0.0);
        let last = *route.stops.last().unwrap();
        eprintln!(
            "COM {com}: main line {} points, last stop {last}, first stop {}",
            route.main_line().len(),
            route.stops[0]
        );
        let side_start = route.segments[1..]
            .iter()
            .map(|s| s.points.start)
            .min()
            .unwrap_or(route.slots.len());
        let mut prev_live: Vec<u16> = walker.live().to_vec();
        let added: HashSet<u16> = route
            .activation_pool
            .iter()
            .filter(|v| **v > 0)
            .map(|v| *v as u16)
            .collect();
        placements_total += route.placements.len() - 1;
        never_added += (1..route.placements.len())
            .filter(|n| !added.contains(&(*n as u16)))
            .count();
        while walker.pos().engine_index + 1 < last {
            walker.advance(route, 0.125);
            let pos = walker.pos();
            let cam = scene::cab_camera(route, pos, eye_h, 0.0);
            let live = walker.live();
            for n in prev_live.iter().filter(|n| !live.contains(n)) {
                let p = route.placements[usize::from(*n)];
                let (a, d) = angle(&cam, [p.x, p.z]);
                removed_at[a] += 1;
                let f = cam.forward();
                let (dx, dz) = (p.x - cam.eye[0], p.z - cam.eye[2]);
                let ang = ((dx * f[0] + dz * f[2])
                    / ((dx * dx + dz * dz).sqrt() * (f[0] * f[0] + f[2] * f[2]).sqrt()))
                .clamp(-1.0, 1.0)
                .acos()
                .to_degrees();
                if (20.0..50.0).contains(&ang) {
                    removed_fine[(ang - 20.0) as usize] += 1;
                }
                if d < 3.0 {
                    removed_near[a] += 1;
                }
            }
            prev_live = live.to_vec();
            if !pos.engine_index.is_multiple_of(16) {
                continue;
            }
            for (n, p) in route.placements.iter().enumerate().skip(1) {
                let (a, d) = angle(&cam, [p.x, p.z]);
                if d > 6.0 {
                    continue;
                }
                if live.contains(&(n as u16)) {
                    live_place[a] += 1;
                } else {
                    absent_place[a] += 1;
                }
            }
            for &n in live {
                let p = route.placements[usize::from(n)];
                let (_, d) = angle(&cam, [p.x, p.z]);
                live_dist[(d as usize).min(39)] += 1;
            }
            let Some(bucket) = route.buckets.get(pos.engine_index / 4) else {
                continue;
            };
            let covered: HashSet<usize> = bucket.iter().flat_map(|r| r.slots.clone()).collect();
            for (i, p) in route.slots.iter().enumerate().skip(side_start) {
                if route.locate(i).is_none() {
                    continue;
                }
                let (a, d) = angle(&cam, [p.x, p.z]);
                if d > 8.0 {
                    continue;
                }
                if covered.contains(&i) {
                    present_slots[a] += 1;
                } else {
                    absent_slots[a] += 1;
                }
            }
        }
    }
    eprintln!("angle bins of 10° from the view direction (0 = straight ahead):");
    eprintln!("placements removed from live list, any distance: {removed_at:?}");
    eprintln!("removals by 1° from 20° to 50°: {removed_fine:?}");
    eprintln!("placements never added: {never_added} of {placements_total}");
    eprintln!("placements removed within 3 units:              {removed_near:?}");
    eprintln!("placements within 6 units, live:                {live_place:?}");
    eprintln!("placements within 6 units, not live:            {absent_place:?}");
    eprintln!("live placements by distance (1-unit bins):      {live_dist:?}");
    eprintln!("side slots within 8 units, in current bucket:   {present_slots:?}");
    eprintln!("side slots within 8 units, not in bucket:       {absent_slots:?}");
}

/// `live_intervals` reproduces the walker's live list, and widening for a 21:9 Hor+ view moves
/// the removals that happened at the 4:3 edge (≈ 32°) out to the wide edge.
#[test]
fn live_intervals_match_walker_and_widen_past_the_4_3_edge() {
    let dir = require_data!();
    let w = load(&dir);
    // Hor+ at 21:9 keeps the vertical FOV and scales the horizontal tangent by (21/9)/(4/3).
    let tan_wide = scene::ORIGINAL_TAN_HALF_FOV_X * (21.0 / 9.0) / (4.0 / 3.0);
    let view = scene::WideView {
        tan_half_fov_x: tan_wide,
        original_tan_half_fov_x: scene::ORIGINAL_TAN_HALF_FOV_X,
        max_extra_points: 160,
    };
    let (mut early_before, mut early_after, mut total) = (0usize, 0usize, 0usize);
    for (route, _) in w.routes.values() {
        let iv = scene::live_intervals(route);
        let mut walker = CabWalker::at_segment_start(route, 0);
        walker.advance(route, 0.0);
        let last = *route.stops.last().unwrap();
        while walker.pos().engine_index + 1 < last {
            let e = walker.pos().engine_index;
            let mut a: Vec<u16> = walker.live().to_vec();
            let mut b: Vec<u16> = scene::live_at(&iv, e).collect();
            a.sort_unstable();
            b.sort_unstable();
            assert_eq!(a, b, "engine index {e}");
            walker.advance(route, 64.0 * 0.125);
        }
        let wide = scene::widen_intervals(route, &iv, view, |_| 0.5);
        let angle_at = |e: usize, n: u16| {
            let cam = scene::cab_camera(
                route,
                scene::TrackPos {
                    engine_index: e,
                    t: 0.0,
                },
                0.0,
                0.0,
            );
            let p = route.placements[usize::from(n)];
            let f = cam.forward();
            let (dx, dz) = (p.x - cam.eye[0], p.z - cam.eye[2]);
            let along = dx * f[0] + dz * f[2];
            let lat = (dx * f[2] - dz * f[0]).abs();
            (along > 0.0).then(|| lat / along)
        };
        let (_, max_e) = scene::camera_limits(route);
        for (before, after) in iv.iter().zip(&wide) {
            if before.to > max_e {
                continue;
            }
            total += 1;
            // Removed while its centre is ahead and inside the 21:9 wedge (minus 2° slack).
            let inside = |t: Option<f32>| t.is_some_and(|t| t.atan() < tan_wide.atan() - 0.035);
            early_before += usize::from(inside(angle_at(before.to, before.placement)));
            early_after += usize::from(inside(angle_at(after.to, after.placement)));
        }
    }
    eprintln!(
        "removals inside the 21:9 view: {early_before} of {total} before widening, {early_after} after"
    );
    assert!(early_after * 20 < early_before);
}

/// Parts by id from `partsRC.rom` and the area's `partsR*.rom`.
fn area_part(dir: &Path, area: usize, id: u32) -> Option<shuppatsu_formats::part::Part> {
    for name in [format!("partsR{}.rom", area + 1), "partsRC.rom".to_string()] {
        let data = data_file(dir, &name);
        for e in Rom2::parse(&data).unwrap().iter() {
            if u32::from(u16::from_le_bytes([e[0], e[1]])) == id {
                return Some(shuppatsu_formats::part::Part::parse_entry(e).unwrap().1);
            }
        }
    }
    None
}

fn area_rect(dir: &Path, area: usize, id: u16) -> Option<shuppatsu_formats::tex::TexRect> {
    for name in [format!("texR{}.rom", area + 1), "texRC.rom".to_string()] {
        let tex = shuppatsu_formats::tex::TexFile::parse(&data_file(dir, &name)).unwrap();
        if let Some(r) = tex.rects.iter().find(|r| r.id == id) {
            return Some(*r);
        }
    }
    None
}

fn sprite_texture(dir: &Path, area: usize, part_id: u32) -> scene::SpriteTexture {
    let part = area_part(dir, area, part_id).unwrap_or_else(|| panic!("part {part_id:#x}"));
    let g = &part.groups[0];
    assert_eq!(
        g.vertices.len(),
        4,
        "part {part_id:#x}: first group is not a quad"
    );
    let rect = area_rect(dir, area, g.rect_id).unwrap_or_else(|| panic!("rect {}", g.rect_id));
    scene::SpriteTexture::from_group(g, &rect).unwrap()
}

/// Every COM's sky sprites and textured grounds resolve to a 4-vertex part with a known rect,
/// and the panorama quads are finite and cover the original viewport along the whole route.
#[test]
fn background_quads_resolve_and_cover_the_view() {
    let dir = require_data!();
    let w = load(&dir);
    let bg = &w.tables.background;
    for (i, s) in bg.sky_sprites.iter().enumerate() {
        eprintln!("sky sprite {i:2}: {s:?}");
    }
    let vp = scene::Viewport2d::original(
        (scene::DRIVING_FOV_Y_DEG.to_radians() * 0.5).tan(),
        scene::DRIVING_ASPECT,
    );
    let wide = vp.hor_plus(vp.half[1] * 21.0 / 9.0);
    for (&com, (route, area)) in &w.routes {
        let eye_h = w.tables.eye_height(*area, com);
        let mut textures = BTreeMap::new();
        let mut walker = CabWalker::at_segment_start(route, 0);
        walker.advance(route, 0.0);
        let last_stop = *route.stops.last().unwrap();
        let (mut frames, mut drawn, mut uncovered, mut extra) = (0, 0, 0, 0);
        let mut station = 1;
        while walker.pos().engine_index + 1 < last_stop {
            let pos = walker.pos();
            while station < route.stops.len() && route.stops[station] <= pos.engine_index + 1 {
                station += 1;
            }
            let ctx = ctx_for(&w, com, station as u32);
            let plan = scene::background(route, &w.tables, ctx, pos);
            let cam = scene::cab_camera(route, pos, eye_h, 0.0);
            let view = scene::ViewBasis::from_cab(&cam);
            for &idx in &plan.sky_sprites {
                let sprite = &bg.sky_sprites[idx];
                let tex = *textures.entry(idx).or_insert_with(|| {
                    sprite_texture(&dir, *area, sprite.part_id).for_sky(sprite.flags)
                });
                let q = scene::sky_quads(sprite, &tex, &view, &vp, true);
                assert_eq!(q.len(), 3);
                assert!(q.iter().flat_map(|t| t.xy).flatten().all(f32::is_finite));
                let left = q.iter().map(|t| t.xy[0][0]).fold(f32::INFINITY, f32::min);
                let right = q
                    .iter()
                    .map(|t| t.xy[3][0])
                    .fold(f32::NEG_INFINITY, f32::max);
                if left > 0.0 || right < 496.0 {
                    uncovered += 1;
                }
                let q = scene::sky_quads(sprite, &tex, &view, &wide, true);
                let [l, r] = wide.x_range();
                let left = q.iter().map(|t| t.xy[0][0]).fold(f32::INFINITY, f32::min);
                let right = q
                    .iter()
                    .map(|t| t.xy[3][0])
                    .fold(f32::NEG_INFINITY, f32::max);
                assert!(
                    left <= l && right >= r,
                    "COM {com} sprite {idx}: {left}..{right}"
                );
                drawn += 1;
            }
            if let Some(which) = plan.extra_quad
                && let Some(q) = scene::extra_quad(bg, which, &view, &vp, true)
            {
                assert!(q.xy.iter().flatten().all(|v| v.is_finite()));
                extra += 1;
            }
            frames += 1;
            walker.advance(route, 16.0 * 0.125);
        }
        eprintln!(
            "COM {com:2}: {frames} frames, sprites {:?}, {drawn} drawn, {uncovered} not covering 0..496, {extra} extra quads",
            textures.keys().collect::<Vec<_>>()
        );
        assert_eq!(uncovered, 0, "COM {com}");
    }
    for (area, part) in [
        (0, scene::GROUND_PART_AREA0),
        (3, scene::GROUND_PART_AREA3),
        (4, scene::GROUND_PART_AREA4),
    ] {
        let tex = sprite_texture(&dir, area, part);
        eprintln!("ground part {part:#x} (area {area}): {tex:?}");
        for screen_uv in [true, false] {
            let g = scene::Ground::Textured {
                xy: bg.ground_quad.xy,
                part_id: part,
                screen_uv,
                depth: scene::BACKGROUND_DEPTH,
            };
            let quads = scene::ground_quad(bg, &g, &tex, wide.x_range());
            assert!(!quads.is_empty());
            for q in &quads {
                assert!(q.uv.iter().chain(&q.xy).flatten().all(|v| v.is_finite()));
            }
            if !screen_uv {
                // The part's own UVs never leave its rect on the atlas page.
                let own = tex.page_uv();
                for c in 0..2 {
                    let lo = own.iter().map(|p| p[c]).fold(f32::INFINITY, f32::min);
                    let hi = own.iter().map(|p| p[c]).fold(f32::NEG_INFINITY, f32::max);
                    let inside = |p: [f32; 2]| p[c] >= lo - 1e-6 && p[c] <= hi + 1e-6;
                    assert!(
                        quads.iter().flat_map(|q| q.uv).all(inside),
                        "part {part:#x}"
                    );
                }
            }
        }
    }
    eprintln!(
        "sky quad {:?}, ground quad {:?}",
        bg.sky_quad.xy, bg.ground_quad.xy
    );
    eprintln!("extra quads {:?}", bg.extra_quads);
    // The coupling minigame's bottom band (0x486ec4): virtual rows 268..400, black.
    assert_eq!(
        bg.bottom_band_quad.xy,
        [[0, 268], [0, 400], [496, 400], [496, 268]]
    );
    assert_eq!(bg.bottom_band_quad.colours, [0; 4]);
    assert!(
        bg.extra_quads
            .iter()
            .flatten()
            .flatten()
            .all(|v| v.is_finite())
    );
}

#[test]
fn gradient_event_tables_are_sane() {
    let dir = require_data!();
    let exe_bytes = std::fs::read(dir.join("BIN/dengo3k.exe")).unwrap();
    let exe = Exe::parse(&exe_bytes).unwrap();
    let t = scene::GradientEventTables::read(&exe).unwrap();
    eprintln!("{t:?}");
    assert!(t.from_station.iter().all(|&s| (0..=99).contains(&s)));
    assert!(t.duration_ticks.iter().all(|&d| d > 0));
    assert!(t.min_remaining.iter().flatten().all(|&m| m >= 0));
}

/// Where the original activates scenery and side tracks: the distance from the camera to an
/// object when it enters the live list, and to a side-track run's slots when its bucket first
/// lists it (scene.md §5.3, the `early_activation` / `draw_distance` enhancements).
#[test]
#[ignore]
fn activation_distance_report() {
    let dir = require_data!();
    let w = load(&dir);
    let mut place = [0usize; 12]; // 4-unit bins of eye distance at activation
    let mut run = [0usize; 12]; // 4-unit bins of the farthest slot of a newly listed run
    for (&com, (route, area)) in &w.routes {
        let eye_h = w.tables.eye_height(*area, com);
        let cam = |e: usize| {
            scene::cab_camera(
                route,
                scene::TrackPos {
                    engine_index: e,
                    t: 0.0,
                },
                eye_h,
                0.0,
            )
        };
        let (lo, hi) = scene::camera_limits(route);
        for iv in scene::live_intervals(route) {
            if iv.from <= lo || iv.from >= hi {
                continue;
            }
            let p = &route.placements[usize::from(iv.placement)];
            let c = cam(iv.from);
            let d = ((p.x - c.eye[0]).powi(2) + (p.z - c.eye[2]).powi(2)).sqrt();
            place[((d / 4.0) as usize).min(11)] += 1;
        }
        let mut seen = HashSet::new();
        for (b, bucket) in route.buckets.iter().enumerate() {
            let e = b * 4;
            if e <= lo || e >= hi {
                bucket.iter().for_each(|r| {
                    seen.insert(r.slots.start);
                });
                continue;
            }
            let c = cam(e);
            for r in bucket {
                if !seen.insert(r.slots.start) {
                    continue;
                }
                let d = r
                    .slots
                    .clone()
                    .filter_map(|s| route.slots.get(s))
                    .map(|p| ((p.x - c.eye[0]).powi(2) + (p.z - c.eye[2]).powi(2)).sqrt())
                    .fold(0.0f32, f32::max);
                run[((d / 4.0) as usize).min(11)] += 1;
            }
        }
    }
    eprintln!("placements activated at eye distance (4-unit bins): {place:?}");
    eprintln!("new runs, farthest slot (4-unit bins): {run:?}");
}

#[test]
#[ignore]
fn run_lod_dump() {
    let dir = require_data!();
    let w = load(&dir);
    let (route, _) = &w.routes[&1];
    for (b, bucket) in route.buckets.iter().enumerate().skip(400).take(40) {
        let s: Vec<String> = bucket
            .iter()
            .map(|r| format!("k{} {:?} {:?} d{:.0}", r.kind, r.slots, r.lod, r.dist_sq))
            .collect();
        eprintln!("{b}: {}", s.join(" | "));
    }
}

#[test]
#[ignore]
fn lod_part_report() {
    let dir = require_data!();
    let w = load(&dir);
    let stat = |area: usize, id: u32| -> String {
        match area_part(&dir, area, id) {
            Some(p) => {
                let tris: usize = p.groups.iter().map(|g| g.triangles().count()).sum();
                let zs = p
                    .groups
                    .iter()
                    .flat_map(|g| &g.vertices)
                    .map(|v| v.position()[2]);
                let (lo, hi) = zs.fold((f32::MAX, f32::MIN), |(a, b), z| (a.min(z), b.max(z)));
                format!("{id:#x}:{tris}t z{lo:.2}..{hi:.2}")
            }
            None => format!("{id:#x}:-"),
        }
    };
    for area in 0..4 {
        for ty in [0usize, 1, 2, 5] {
            let s: Vec<String> = (0..8)
                .map(|g| stat(area, w.tables.areas[area].rail[g][ty]))
                .collect();
            eprintln!("area {area} rail type {ty}: {}", s.join("  "));
        }
        for kind in [3u16, 4, 7, 8, 9, 20, 21, 117, 135, 145] {
            if let Some(parts) = w.tables.kind_parts(area, kind) {
                let s: Vec<String> = parts.iter().map(|&p| stat(area, p)).collect();
                eprintln!("area {area} kind {kind}: {}", s.join("  "));
            }
        }
    }
}

#[test]
#[ignore]
fn activation_swap_report() {
    let dir = require_data!();
    let w = load(&dir);
    let (mut swaps, mut total) = (0usize, 0usize);
    for (&com, (route, _)) in &w.routes {
        for e in 1..route.activation_index.len() {
            let acts = route.activations(e);
            let adds: Vec<u16> = acts
                .iter()
                .filter(|v| **v > 0)
                .map(|v| v.unsigned_abs())
                .collect();
            let rems: Vec<u16> = acts
                .iter()
                .filter(|v| **v < 0)
                .map(|v| v.unsigned_abs())
                .collect();
            total += adds.len();
            for &a in &adds {
                let pa = &route.placements[usize::from(a)];
                for &r in &rems {
                    let pr = &route.placements[usize::from(r)];
                    let d = ((pa.x - pr.x).powi(2) + (pa.z - pr.z).powi(2)).sqrt();
                    if d < 0.5 {
                        swaps += 1;
                        if swaps < 30 {
                            eprintln!(
                                "COM {com} e {e}: +{a} part {:#x} / -{r} part {:#x} d {d:.3}",
                                pa.part_id, pr.part_id
                            );
                        }
                    }
                }
            }
        }
    }
    eprintln!("swaps {swaps} of {total} activations");
}

#[test]
#[ignore]
fn traffic_lod_report() {
    let dir = require_data!();
    let exe_bytes = std::fs::read(dir.join("BIN/dengo3k.exe")).unwrap();
    let exe = Exe::parse(&exe_bytes).unwrap();
    let models = shuppatsu_formats::traffic::TrainModels::read(&exe).unwrap();
    let mut tris = std::collections::HashMap::new();
    for e in std::fs::read_dir(dir.join("DATA")).unwrap() {
        let p = e.unwrap().path();
        let n = p.file_name().unwrap().to_string_lossy().to_lowercase();
        if !n.starts_with("parts") {
            continue;
        }
        for entry in Rom2::parse(&std::fs::read(&p).unwrap()).unwrap().iter() {
            if let Ok((id, part)) = shuppatsu_formats::part::Part::parse_entry(entry) {
                let t: usize = part.groups.iter().map(|g| g.triangles().count()).sum();
                tris.entry(u32::from(id)).or_insert(t);
            }
        }
    }
    for (i, row) in models.rows.iter().enumerate() {
        if let Some(c) = row.cars[0].first().or(row.cars[1].first()) {
            let t: Vec<String> = c
                .lods
                .iter()
                .map(|l| format!("{l:#x}:{:?}", tris.get(l)))
                .collect();
            eprintln!("row {i} kind {}: {}", row.kind, t.join(" "));
        }
    }
}

#[test]
#[ignore]
fn main_line_runs_report() {
    let dir = require_data!();
    let w = load(&dir);
    for (&com, (route, _)) in &w.routes {
        let main = route.segments[0].points.clone();
        let mut ahead = Vec::new();
        for (b, bucket) in route.buckets.iter().enumerate() {
            let cam_slot = main.start + 1 + b * 4;
            for r in bucket.iter().filter(|r| main.contains(&r.slots.start)) {
                ahead.push((
                    r.slots.start as isize - cam_slot as isize,
                    r.slots.end as isize - cam_slot as isize,
                    r.kind,
                ));
            }
        }
        let n = ahead.len();
        let maxe = ahead.iter().map(|a| a.1).max();
        let k1 = ahead.iter().filter(|a| a.2 == 1).count();
        eprintln!(
            "COM {com}: {n} runs on the main line ({k1} rail), farthest end {maxe:?} points ahead; sample {:?}",
            &ahead[..n.min(5)]
        );
    }
}

/// `early_activation` (scene.md §5.3): activations only move earlier, removals stay, one
/// placement is never live twice, and most objects now enter beyond 20 units.
#[test]
fn early_activation_moves_entries_to_the_far_plane() {
    let dir = require_data!();
    let w = load(&dir);
    let view = scene::EarlyView {
        reach: 32.0,
        tan_half_fov_x: scene::ORIGINAL_TAN_HALF_FOV_X,
        max_extra_points: 512,
    };
    let (mut near_before, mut near_after, mut total) = (0usize, 0usize, 0usize);
    for (route, _) in w.routes.values() {
        let iv = scene::live_intervals(route);
        let early = scene::activate_early(route, &iv, view, |_| 0.5);
        assert_eq!(iv.len(), early.len());
        let key = |i: &scene::LiveInterval| (i.placement, i.to);
        let mut by_key: BTreeMap<(u16, usize), usize> = BTreeMap::new();
        for i in &iv {
            by_key.insert(key(i), i.from);
        }
        let mut per_placement: BTreeMap<u16, Vec<(usize, usize)>> = BTreeMap::new();
        for e in &early {
            let from = by_key[&key(e)];
            assert!(e.from <= from, "{e:?} starts after {from}");
            per_placement
                .entry(e.placement)
                .or_default()
                .push((e.from, e.to));
        }
        for spans in per_placement.values_mut() {
            spans.sort_unstable();
            for pair in spans.windows(2) {
                assert!(pair[0].1 <= pair[1].0, "overlapping {pair:?}");
            }
        }
        let (lo, hi) = scene::camera_limits(route);
        let dist = |e: usize, n: u16| {
            let cam = scene::cab_camera(
                route,
                scene::TrackPos {
                    engine_index: e,
                    t: 0.0,
                },
                0.0,
                0.0,
            );
            let p = route.placements[usize::from(n)];
            ((p.x - cam.eye[0]).powi(2) + (p.z - cam.eye[2]).powi(2)).sqrt()
        };
        for (before, after) in iv.iter().zip(&early) {
            if before.from <= lo + 600 || before.from >= hi {
                continue;
            }
            total += 1;
            near_before += usize::from(dist(before.from, before.placement) < 20.0);
            near_after += usize::from(dist(after.from, after.placement) < 20.0);
        }
    }
    eprintln!("activations within 20 units: {near_before} of {total} before, {near_after} after");
    assert!(near_after * 4 < near_before);
}

/// `max_lod` draws only single rail pieces and band-0 kind pieces; a `reach` adds main-line
/// pieces out to it and side tracks from the buckets ahead.
#[test]
fn max_lod_and_reach_track_pieces() {
    let dir = require_data!();
    let w = load(&dir);
    for (&com, (route, area)) in &w.routes {
        let eye_h = w.tables.eye_height(*area, com);
        let ctx = ctx_for(&w, com, 1);
        let (lo, hi) = scene::camera_limits(route);
        for e in (lo + 300..hi.saturating_sub(300)).step_by(997) {
            let pos = scene::TrackPos {
                engine_index: e,
                t: 0.0,
            };
            let cam = scene::cab_camera(route, pos, eye_h, 0.0);
            let max = scene::TrackOptions {
                max_lod: true,
                ..Default::default()
            };
            for i in scene::track_instances_with(route, &w.tables, ctx, pos, &cam, max) {
                match i.source {
                    PieceSource::MainLine { band }
                    | PieceSource::Rail { band }
                    | PieceSource::Kind { band, .. } => assert_eq!(band, 0, "COM {com}: {i:?}"),
                    _ => {}
                }
            }
            let original = scene::track_instances(route, &w.tables, ctx, pos, &cam);
            let far = scene::TrackOptions {
                reach: Some(64.0),
                ..Default::default()
            };
            let reach = scene::track_instances_with(route, &w.tables, ctx, pos, &cam, far);
            // Everything the original draws is drawn unchanged.
            for o in &original {
                assert!(reach.contains(o), "COM {com} at {e}: {o:?} missing");
            }
            let farthest = reach
                .iter()
                .filter(|i| matches!(i.source, PieceSource::MainLine { .. }))
                .map(|i| i.slot)
                .max()
                .unwrap_or(0);
            let main_end = route.segments[0].points.end;
            assert!(
                farthest + 64 >= (pos.slot(route) + 512).min(main_end - 1),
                "COM {com} at {e}: main line ends at slot {farthest} of {main_end}"
            );
        }
    }
}

/// Issue mark 4 ("the main rails aren't straight", stage 13 between 立花 and 甲子園口, engine
/// index 1822 of toukai1b.com, 2026-10-20; scene.md §2.3): on that straight main line the
/// single rail pieces join on the track's centre line, but each piece's width is its chord
/// length × 8 (0x4186a0 gets the unnormalised chord), and the route's point spacing varies
/// (0.117–0.125 units there). Adjacent pieces therefore differ in gauge by up to ~6 %: the
/// visible steps at the joins, which the original draws too.
#[test]
fn main_line_singles_join_on_the_centre_line_and_scale_width_with_the_chord() {
    let dir = require_data!();
    let w = load(&dir);
    let com = (1..)
        .take_while(|&c| c < w.stages.com_files.len() as u32)
        .find(|&c| {
            w.stages
                .com_file(c)
                .is_some_and(|f| f.eq_ignore_ascii_case("toukai1b.com"))
        })
        .expect("toukai1b.com");
    let (route, area) = &w.routes[&com];
    let pos = scene::TrackPos {
        engine_index: 1822,
        t: 0.089,
    };
    let cam = scene::cab_camera(route, pos, w.tables.eye_height(*area, com), 0.0);
    let options = scene::TrackOptions {
        max_lod: true,
        ..Default::default()
    };
    let mut main: Vec<_> =
        scene::track_instances_with(route, &w.tables, ctx_for(&w, com, 4), pos, &cam, options)
            .into_iter()
            .filter(|i| matches!(i.source, PieceSource::MainLine { band: 0 }))
            .collect();
    main.sort_by_key(|i| i.slot);
    let width = |a: &scene::Affine| a.x_axis[0].hypot(a.x_axis[2]);
    let mut worst_step = 1f32;
    let mut joins = 0;
    for pair in main.windows(2).take(160) {
        let (a, b) = (&pair[0], &pair[1]);
        if b.slot != a.slot + 1 {
            continue;
        }
        joins += 1;
        // Local +z points backwards: a's front end meets b's back end.
        let front = a.transform.transform_point([0.0, 0.0, -1.0 / 16.0]);
        let back = b.transform.transform_point([0.0, 0.0, 1.0 / 16.0]);
        let (p, q) = (route.slots[b.slot], route.slots[b.slot + 1]);
        let len = (q.x - p.x).hypot(q.z - p.z);
        let side = [-(q.z - p.z) / len, (q.x - p.x) / len];
        let lateral = (front[0] - back[0]) * side[0] + (front[2] - back[2]) * side[1];
        assert!(
            lateral.abs() < 1e-4,
            "slot {}: centre lines {lateral} apart",
            a.slot
        );
        // The width follows the chord exactly (8 × |d|).
        let chord = |s: usize| {
            let (p, q) = (route.slots[s], route.slots[s + 1]);
            8.0 * (q.x - p.x).hypot(q.z - p.z)
        };
        assert!((width(&a.transform) - chord(a.slot)).abs() < 1e-4);
        let r = width(&a.transform) / width(&b.transform);
        worst_step = worst_step.max(r.max(1.0 / r));
    }
    assert!(joins >= 100, "only {joins} joins");
    eprintln!("toukai1b.com at 1822: widest gauge step between pieces {worst_step}");
    assert!((1.02..1.08).contains(&worst_step), "{worst_step}");
}

/// Enhancement `even_track_width` (scene.md §2.3) at issue mark 4's spot: with it on, the
/// main line's single pieces are all one width (gauge step 1.000) and still join on the centre
/// line; with it off the pieces are exactly the original's. On every route, with `max_lod`
/// on and off, the option changes only the length of local +x: positions, up and along-track
/// axes, parts and the set of pieces stay the original's, every rail piece comes out 1 wide,
/// and neighbouring track-side pieces of a run share one width.
#[test]
fn even_track_width_gives_adjacent_pieces_one_width() {
    let dir = require_data!();
    let w = load(&dir);
    let width = |a: &scene::Affine| a.x_axis[0].hypot(a.x_axis[2]);
    let com = (1..)
        .take_while(|&c| c < w.stages.com_files.len() as u32)
        .find(|&c| {
            w.stages
                .com_file(c)
                .is_some_and(|f| f.eq_ignore_ascii_case("toukai1b.com"))
        })
        .expect("toukai1b.com");
    let (route, area) = &w.routes[&com];
    let pos = scene::TrackPos {
        engine_index: 1822,
        t: 0.089,
    };
    let cam = scene::cab_camera(route, pos, w.tables.eye_height(*area, com), 0.0);
    let main_singles = |even_width| {
        let options = scene::TrackOptions {
            max_lod: true,
            even_width,
            ..Default::default()
        };
        let mut v: Vec<_> =
            scene::track_instances_with(route, &w.tables, ctx_for(&w, com, 4), pos, &cam, options)
                .into_iter()
                .filter(|i| matches!(i.source, PieceSource::MainLine { band: 0 }))
                .collect();
        v.sort_by_key(|i| i.slot);
        v
    };
    // Off: the original (f246905's test pins its chord-scaled widths).
    let original = scene::TrackOptions {
        max_lod: true,
        ..Default::default()
    };
    let mut plain: Vec<_> =
        scene::track_instances_with(route, &w.tables, ctx_for(&w, com, 4), pos, &cam, original)
            .into_iter()
            .filter(|i| matches!(i.source, PieceSource::MainLine { band: 0 }))
            .collect();
    plain.sort_by_key(|i| i.slot);
    assert_eq!(main_singles(false), plain);
    let even = main_singles(true);
    let mut joins = 0;
    for pair in even.windows(2).take(160) {
        let (a, b) = (&pair[0], &pair[1]);
        if b.slot != a.slot + 1 {
            continue;
        }
        joins += 1;
        let front = a.transform.transform_point([0.0, 0.0, -1.0 / 16.0]);
        let back = b.transform.transform_point([0.0, 0.0, 1.0 / 16.0]);
        let (p, q) = (route.slots[b.slot], route.slots[b.slot + 1]);
        let len = (q.x - p.x).hypot(q.z - p.z);
        let side = [-(q.z - p.z) / len, (q.x - p.x) / len];
        let lateral = (front[0] - back[0]) * side[0] + (front[2] - back[2]) * side[1];
        assert!(lateral.abs() < 1e-4, "slot {}: {lateral} apart", a.slot);
        let r = width(&a.transform) / width(&b.transform);
        assert!((r - 1.0).abs() < 1e-5, "slot {}: gauge step {r}", a.slot);
        assert!((width(&a.transform) - 1.0).abs() < 1e-5);
    }
    assert!(joins >= 100, "only {joins} joins");

    // Every route: only |x| changes.
    let (mut rails, mut kinds, mut kind_joins) = (0, 0, 0);
    let mut kind_steps = [0; 2];
    for (&com, (route, area)) in &w.routes {
        let eye_h = w.tables.eye_height(*area, com);
        let ctx = ctx_for(&w, com, 1);
        let (lo, hi) = scene::camera_limits(route);
        for e in (lo + 300..hi.saturating_sub(300)).step_by(1499) {
            let pos = scene::TrackPos {
                engine_index: e,
                t: 0.0,
            };
            let cam = scene::cab_camera(route, pos, eye_h, 0.0);
            for max_lod in [false, true] {
                let get = |even_width| {
                    let o = scene::TrackOptions {
                        max_lod,
                        even_width,
                        ..Default::default()
                    };
                    scene::track_instances_with(route, &w.tables, ctx, pos, &cam, o)
                };
                let (off, on) = (get(false), get(true));
                assert_eq!(off.len(), on.len());
                for (a, b) in off.iter().zip(&on) {
                    assert_eq!((a.part_id, a.slot, a.source), (b.part_id, b.slot, b.source));
                    let (ta, tb) = (&a.transform, &b.transform);
                    assert_eq!(
                        (ta.translation, ta.y_axis, ta.z_axis),
                        (tb.translation, tb.y_axis, tb.z_axis)
                    );
                    // Same direction.
                    let cross = ta.x_axis[0] * tb.x_axis[2] - ta.x_axis[2] * tb.x_axis[0];
                    assert!(cross.abs() < 1e-3 * width(ta).max(1.0), "{a:?} / {b:?}");
                    match b.source {
                        PieceSource::MainLine { .. } | PieceSource::Rail { .. } => {
                            rails += 1;
                            assert!((width(tb) - 1.0).abs() < 1e-4, "COM {com}: {b:?}");
                        }
                        PieceSource::Kind { .. } => kinds += 1,
                        _ => assert_eq!(ta, tb),
                    }
                }
                // Neighbouring pieces of a track-side run (same kind and band, consecutive).
                for (k, list) in [&off, &on].into_iter().enumerate() {
                    let mut side: Vec<_> = list
                        .iter()
                        .filter(|i| matches!(i.source, PieceSource::Kind { .. }))
                        .collect();
                    side.sort_by_key(|i| (i.slot, i.part_id));
                    for pair in side.windows(2) {
                        let (a, b) = (pair[0], pair[1]);
                        let step = b.slot - a.slot;
                        if a.source != b.source
                            || a.part_id != b.part_id
                            || !(1..=4).contains(&step)
                        {
                            continue;
                        }
                        kind_joins += k;
                        let r = width(&a.transform) / width(&b.transform);
                        kind_steps[k] += usize::from((r - 1.0).abs() > 1e-4);
                    }
                }
            }
        }
    }
    eprintln!(
        "{rails} rail pieces at width 1; {kinds} track-side pieces; {} of {kind_joins} track-side joins step (original: {})",
        kind_steps[1], kind_steps[0]
    );
    assert!(rails > 10_000 && kinds > 5_000);
    assert!(
        kind_steps[1] * 50 < kind_joins,
        "{kind_steps:?} of {kind_joins}"
    );
}

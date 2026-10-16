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

// ---- Route .COM files and exe stage tables (docs/re/routes.md) ----

fn com_files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("com")))
        .collect();
    files.sort();
    files
}

/// Case-insensitive lookup of a file in the DATA directory.
fn find_file(dir: &std::path::Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name))
        })
}

fn exe_bytes(dir: &std::path::Path) -> Vec<u8> {
    std::fs::read(dir.join("../BIN/dengo3k.exe")).unwrap()
}

#[test]
fn every_route_parses_completely() {
    use ddg_formats::route::Route;
    let dir = require_data!();
    let files = com_files(&dir);
    assert_eq!(files.len(), 15);
    for path in files {
        let data = std::fs::read(&path).unwrap();
        let name = path.display();
        let r = Route::parse(&data).unwrap_or_else(|e| panic!("{name}: {e}"));
        // Everything is accounted for except ≤3-byte alignment padding.
        for (offset, len) in r.unparsed(data.len()) {
            assert!(len < 4, "{name}: {len} unparsed bytes at {offset:#x}");
            assert!(data[offset..offset + len].iter().all(|&b| b == 0));
        }
        let seg0 = &r.segments[0];
        assert!(seg0.head_marker && seg0.tail_marker, "{name}");
        let main = r.main_line();
        assert_eq!(r.activation_index.len(), main.len());
        assert_eq!(r.buckets.len(), main.len() / 4, "{name}");
        assert!(!r.stops.is_empty() && r.stops.windows(2).all(|w| w[0] < w[1]));
        assert!(r.segments.iter().all(|s| s.reserved == [0, 0]));
        assert_eq!(r.placements[0].part_id, 0);
        // Part ids index the 13087-entry part table (docs/re/loaders.md §5).
        assert!(r.placements.iter().all(|p| p.part_id < 13087), "{name}");
        // Consecutive main-line points are ~0.125 world units apart.
        let gaps: Vec<f32> = main
            .windows(2)
            .map(|w| ((w[1].x - w[0].x).powi(2) + (w[1].z - w[0].z).powi(2)).sqrt())
            .collect();
        let mean = gaps.iter().sum::<f32>() / gaps.len() as f32;
        assert!((0.115..0.13).contains(&mean), "{name}: mean spacing {mean}");
    }
}

#[test]
fn stage_tables_read_and_agree_with_routes() {
    use ddg_formats::{exe::Exe, route, route::Route, stages::StageTables};
    let dir = require_data!();
    let exe_data = exe_bytes(&dir);
    let exe = Exe::parse(&exe_data).unwrap();
    let tables = StageTables::read(&exe).unwrap();
    assert_eq!(tables.stages.len(), 20);
    assert_eq!(tables.com_files.iter().flatten().count(), 15);

    let mut routes = std::collections::HashMap::new();
    for (com, name) in tables.com_files.iter().enumerate() {
        if let Some(name) = name {
            let path = find_file(&dir, name).unwrap_or_else(|| panic!("{name} missing"));
            routes.insert(
                com as u32,
                Route::parse(&std::fs::read(path).unwrap()).unwrap(),
            );
        }
    }

    let (mut checked, mut worst) = (0, 0.0f32);
    for (s, stage) in tables.stages.iter().enumerate() {
        assert!((1..=5).contains(&stage.area), "stage {s}");
        let table = &stage.timetables[0];
        assert!(!table.is_empty(), "stage {s}");
        assert_eq!(stage.station_names.len(), table.len());
        assert!(stage.station_names.iter().all(|n| !n.is_empty()));
        for (station, row) in table.iter().enumerate().take(table.len() - 1) {
            // Station-to-station distance in the timetable vs. stop points × 1.25 m.
            let (seg, local) = stage.segment_for_station(station as u32).unwrap();
            let stops = &routes[&seg.com].stops;
            if let (Some(a), Some(b)) = (stops.get(local as usize), stops.get(local as usize + 1)) {
                let metres = (b - a) as f32 * route::METRES_PER_POINT;
                worst = worst.max((metres - row.to_next_m as f32).abs());
                checked += 1;
            }
        }
        for ev in &stage.traffic_events {
            let (seg, _) = stage.segment_for_station(ev.station as u32).unwrap();
            let spawn = routes[&seg.com].traffic.get(ev.spawn as usize);
            assert!(spawn.is_some(), "stage {s}: spawn {}", ev.spawn);
            // The model row: the override, or the spawn's own type − 1 (0x404f10).
            let row = if ev.args[3] == -1 {
                i32::from(spawn.unwrap().train_type) - 1
            } else {
                i32::from(ev.args[3])
            };
            let model = tables.train_models.row(row as usize);
            assert!(model.is_some(), "stage {s}: model row {row}");
        }
        assert!(stage.course_events.iter().all(|e| e.station >= 0));
        assert!(stage.limit_events.iter().all(|e| (4..=6).contains(&e.kind)));
    }
    eprintln!("timetable vs .COM stop spacing: {checked} pairs, worst error {worst} m");
    assert!(checked > 100);
    assert!(worst <= 5.0, "worst {worst} m");
}

#[test]
fn traffic_model_table() {
    use ddg_formats::{exe::Exe, traffic::TrainModels};
    let dir = require_data!();
    let exe_data = exe_bytes(&dir);
    let models = TrainModels::read(&Exe::parse(&exe_data).unwrap()).unwrap();
    assert_eq!(models.rows.len(), 79);
    let mut lists = 0;
    for (row, m) in models.rows.iter().enumerate() {
        assert!((0..=8).contains(&m.kind), "row {row}: kind {}", m.kind);
        // Road vehicles (kind 0) are rows 68..=78 and use area parts; trains use TR parts.
        assert_eq!(m.kind == 0, row >= 68, "row {row}");
        for cars in &m.cars {
            if cars.is_empty() {
                continue;
            }
            lists += 1;
            // The head is at offset 0 and each offset is the previous one plus the spacing.
            assert_eq!((cars[0].offset, cars[0].spacing), (0.0, 0.0), "row {row}");
            for w in cars.windows(2) {
                assert!((w[0].offset + w[1].spacing - w[1].offset).abs() < 1e-3);
            }
            for c in cars {
                let tr = (11762..=13086).contains(&c.lods[0]);
                assert_eq!(tr, m.kind != 0, "row {row}: part {:#x}", c.lods[0]);
            }
        }
    }
    assert!(lists > 140, "{lists} car lists");
}

// ---- Driving-HUD tables (docs/re/hud.md) ----

#[test]
fn hud_tables_read_and_point_at_tilemaps() {
    use ddg_formats::{cg, exe::Exe, hud::HudTables};
    let dir = require_data!();
    let exe_data = exe_bytes(&dir);
    let exe = Exe::parse(&exe_data).unwrap();
    let t = HudTables::read(&exe).unwrap();
    assert_eq!(t.mode_sizes[1], (640, 480));
    // 43 static slot placements; the first is the current-time label (hud.md §2).
    assert_eq!(t.layout.len(), 43);
    let first = t.layout[0].at_640();
    assert_eq!((first.slot, first.x, first.y), (10, 66, 56));
    let dist = t.slot(0x5a).unwrap().at_640();
    assert_eq!((dist.x, dist.y, dist.layer), (496, 408, 3));
    // Every sprite id must name a tbl.rom tilemap.
    let maps = cg::parse_tbl(&std::fs::read(dir.join("tbl.rom")).unwrap()).unwrap();
    let ids = t
        .time_digits
        .iter()
        .chain(&t.clock_digits)
        .chain(&t.clock_colon_digits)
        .chain(&t.distance_digits)
        .chain(&t.brake_sprites)
        .chain(&t.mascon_sprites)
        .copied()
        .chain(
            t.station_names[5..]
                .iter()
                .flat_map(|s| [s.plain, s.highlighted]),
        );
    for id in ids {
        assert!((id as usize) < maps.len(), "sprite {id:#x}");
    }
    assert_eq!(t.brake_sprites[9], t.brake_sprites[10]);
    assert_eq!(t.stage_station_ids.len(), 20);
    assert!(t.stage_station_ids.iter().all(|s| s.len() >= 2));
    // Seven-segment "8" lights all segments.
    assert_eq!(t.segment_masks[8], 0x7f);
}

#[test]
fn cab_hud_tables_match_the_decompiled_code() {
    use ddg_formats::{cab_hud::CabTables, exe::Exe};
    let dir = require_data!();
    let exe_data = exe_bytes(&dir);
    let exe = Exe::parse(&exe_data).unwrap();
    let t = CabTables::read(&exe).unwrap();
    // Star offsets built on the stack by 0x4124f0: row 0 (stage 10 first half) starts
    // 0x28, 0x48, …; stage 0 (row 1) is 0x40 then 0x318 (unused, off screen).
    assert_eq!(
        t.star_offsets[0][..8],
        [0x28, 0x48, 0x68, 0x88, 0xa8, 200, 0xe8, 0x108]
    );
    assert_eq!(t.star_offsets[1][..2], [0x40, 0x318]);
    assert_eq!(
        t.star_offsets[3][..6],
        [0x30, 0x78, 0xb0, 0xd8, 0xf8, 0x118]
    );
    assert_eq!(t.star_offsets[20][..4], [0x70, 0x70, 0xb8, 0xf0]);
    let from = t.plates[0];
    assert_eq!((from.slot, from.x, from.y), (0x83, 493, 8));
    assert_eq!(t.caption_pos, (80, 112));
    assert_eq!(t.lamp_pos, (80, 144));
    assert_eq!(t.digital_pos, (17, 432));
    assert_eq!(t.dial_pos, (16, 345));
    assert_eq!(t.origin, (0, 0));
    assert_eq!(t.caption_words[..2], [0x6a, 0x11c]);
    assert_eq!(t.banner_station, 0x71);
    assert_eq!(t.atc_bar[0], 0x4a);
    assert_eq!(t.atc_bar[10], 0x40);
    assert_eq!(t.landmarks[0].sprite, 0x222);
    assert_eq!(t.route_strips[0][..2], [0x327, 0x32a]);
    assert_eq!(t.vehicles[0].name, 0x268);
    assert_eq!(t.speed_mode[0], 1);
}

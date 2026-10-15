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
            let spawns = routes[&seg.com].traffic.len();
            assert!(
                (ev.spawn as usize) < spawns,
                "stage {s}: spawn {}",
                ev.spawn
            );
        }
        assert!(stage.course_events.iter().all(|e| e.station >= 0));
        assert!(stage.limit_events.iter().all(|e| (4..=6).contains(&e.kind)));
    }
    eprintln!("timetable vs .COM stop spacing: {checked} pairs, worst error {worst} m");
    assert!(checked > 100);
    assert!(worst <= 5.0, "worst {worst} m");
}

//! Per-stage tables in the exe's `.rdata`/`.data`, read from the user's `dengo3k.exe` at runtime.
//!
//! A *stage* is one selectable run (route + train + timetable); there are 20. Each stage has an
//! area (scenery set), train-part sets, a list of `.COM` route segments, a timetable per
//! difficulty, station names, a track-type → sound table and three event lists keyed by
//! (station, distance to the stop). The event lists carry the signals and speed limits: they are
//! not in the `.COM` files. Addresses, layouts and evidence: `docs/re/routes.md` §8–9.
//!
//! Only the table addresses and counts below are hardcoded (3000-bandai exe); every value is
//! read from the exe.

use crate::error::{FormatError, Result};
use crate::exe::Exe;

const WHAT: &str = "stage tables";

/// Number of stages. The per-stage pointer arrays at 0x485610, 0x485660, 0x485800, 0x485850
/// and 0x4858a0 are 20 entries each (each array ends where the next begins), confirmed.
pub const STAGE_COUNT: usize = 20;
/// Timetables are per stage × difficulty (`0x4856b0[stage*4 + difficulty]`, 0x40d040).
pub const DIFFICULTY_COUNT: usize = 4;
/// `.COM` filename table has 16 entries; index 0 is "none" (0x435320 indexes it by COM id).
pub const COM_TABLE_LEN: usize = 16;

/// u32 area R-index per stage (0x41cb00 / area_assets_load).
const AREA_TABLE: u32 = 0x463c18;
/// u32 per stage: row of [`TRAIN_SET_ROWS`].
const TRAIN_ROW_TABLE: u32 = 0x463d14;
/// Rows of 4 × i32 TR-set indices (−1 = none).
const TRAIN_SET_ROWS: u32 = 0x463c74;
/// char* per COM index.
const COM_NAME_TABLE: u32 = 0x47ca70;
/// ptr per stage → {i32 com, i32 firstStation} pairs, −1 terminated (0x408010).
const SEGMENT_LISTS: u32 = 0x485610;
/// ptr per stage → 32 × i8 (0x407bc0 → 0x407c10).
const TRACK_SOUND_TABLES: u32 = 0x485660;
/// ptr per stage × difficulty → 36-byte rows, terminated by time −1 (0x40d040).
const TIMETABLES: u32 = 0x4856b0;
/// ptr per stage → 6 × i16 records (0x40acd0).
const COURSE_EVENTS: u32 = 0x485800;
/// ptr per stage → 6 × i16 records (0x40b4a0).
const LIMIT_EVENTS: u32 = 0x485850;
/// ptr per stage → 8 × i16 records (0x40b710).
const TRAFFIC_EVENTS: u32 = 0x4858a0;
/// ptr per stage → char*[station count].
const STATION_NAMES: u32 = 0x485b30;

/// Event lists end with a record whose station is 99 (and distance 9999); the game only ever
/// compares the station field, so 99 is effectively "never". Observed in every list.
const EVENT_END_STATION: i16 = 99;
/// Sanity bounds so corrupt pointers can't make us walk the whole image.
const MAX_LIST: usize = 4096;
const MAX_NAME: usize = 64;

/// A `.COM` route segment used by a stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageSegment {
    /// COM index (1..=15), see [`StageTables::com_files`].
    pub com: u32,
    /// The stage-wide station index of this segment's first station.
    pub first_station: u32,
}

/// One timetable row (9 × i32). Row *n* describes arrival at station *n*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimetableRow {
    /// Scheduled time, in 1/30 s ticks since midnight (likely).
    pub time_ticks: i32,
    /// Cumulative distance along the stage, metres (confirmed against `.COM` stop points).
    pub position_m: i32,
    /// Distance to the next station, metres (0 for the last).
    pub to_next_m: i32,
    /// 1 = the train stops here, 0 = passes through (likely, 0x40dad0).
    pub stops: i32,
    /// Fields 4..=8, raw (scoring/time thresholds; see routes.md §9).
    pub params: [i32; 5],
}

/// A course or speed-limit event: fires when the distance to station `station`'s stop point
/// drops to `distance_m`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CourseEvent {
    pub station: i16,
    pub distance_m: i16,
    /// 0 = signal aspect, 1 = sound, 2 = announcement, 3 = …, 4/5/6 = speed limit
    /// start/change/end (likely/guess, routes.md §9).
    pub kind: i16,
    pub args: [i16; 3],
}

/// An AI-train event: spawns the train at a `.COM` traffic spawn point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrafficEvent {
    pub station: i16,
    pub distance_m: i16,
    /// Fires only if bit `n` is set, where `n` is a game-state value (guess).
    pub mask: i16,
    /// Index into the `.COM` traffic spawn table.
    pub spawn: i16,
    /// Remaining four arguments passed to the spawner (0x404f10); the last is a model override,
    /// −1 = use the spawn's own type.
    pub args: [i16; 4],
}

/// Everything the exe knows about one stage.
#[derive(Debug, Clone, PartialEq)]
pub struct Stage {
    /// Area (scenery set) R-index: 1 Yamanote, 2 Akita, 3 Tōkaidō/Kōbe, 4 Hokuhoku, 5 Keihin.
    pub area: u32,
    /// TR part/texture set indices (TR *k* loads `partsTR(k+1)`); −1 entries dropped.
    pub train_sets: Vec<u32>,
    pub segments: Vec<StageSegment>,
    /// Track type (point flags & 0x1f) → ambient sound class (0 none, 1–3 sound ids 0x2c/0x2e/0x29).
    pub track_sounds: [i8; 32],
    /// One timetable per difficulty.
    pub timetables: [Vec<TimetableRow>; DIFFICULTY_COUNT],
    /// Romanised station names, one per station of the difficulty-0 timetable.
    pub station_names: Vec<String>,
    pub course_events: Vec<CourseEvent>,
    pub limit_events: Vec<CourseEvent>,
    pub traffic_events: Vec<TrafficEvent>,
}

impl Stage {
    /// The segment holding stage-wide station `station`, and the station's index inside that
    /// segment's `.COM` stop list (the game's `stage_select_segment`, 0x408010).
    pub fn segment_for_station(&self, station: u32) -> Option<(StageSegment, u32)> {
        let seg = self
            .segments
            .iter()
            .rev()
            .find(|s| s.first_station <= station)?;
        Some((*seg, station - seg.first_station))
    }
}

/// All stage tables.
#[derive(Debug, Clone, PartialEq)]
pub struct StageTables {
    pub stages: Vec<Stage>,
    /// Lower-case `.COM` file names by COM index; index 0 is `None`. Match case-insensitively
    /// (the disc has upper-case names).
    pub com_files: Vec<Option<String>>,
    /// The AI-train model table (0x457358), shared by all stages.
    pub train_models: crate::traffic::TrainModels,
}

impl StageTables {
    /// Reads every table from the exe image.
    pub fn read(exe: &Exe<'_>) -> Result<Self> {
        let mut com_files = Vec::with_capacity(COM_TABLE_LEN);
        for i in 0..COM_TABLE_LEN {
            let p = exe.u32_at(COM_NAME_TABLE + 4 * i as u32)?;
            com_files.push(if p == 0 {
                None
            } else {
                Some(c_string(exe, p)?)
            });
        }
        let stages = (0..STAGE_COUNT)
            .map(|s| read_stage(exe, s as u32, com_files.len()))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            stages,
            com_files,
            train_models: crate::traffic::TrainModels::read(exe)?,
        })
    }

    /// The filename for a COM index, if any.
    pub fn com_file(&self, com: u32) -> Option<&str> {
        self.com_files.get(com as usize)?.as_deref()
    }
}

fn i32_at(exe: &Exe<'_>, va: u32) -> Result<i32> {
    Ok(exe.u32_at(va)? as i32)
}

fn i16s<const N: usize>(exe: &Exe<'_>, va: u32) -> Result<[i16; N]> {
    let r = exe.reader_at(va, 2 * N, WHAT)?;
    let mut out = [0i16; N];
    for (i, v) in out.iter_mut().enumerate() {
        *v = r.i16(2 * i)?;
    }
    Ok(out)
}

fn ptr(exe: &Exe<'_>, table: u32, index: usize) -> Result<u32> {
    exe.u32_at(table + 4 * index as u32)
}

fn read_stage(exe: &Exe<'_>, stage: u32, com_count: usize) -> Result<Stage> {
    let s = stage as usize;
    let area = exe.u32_at(AREA_TABLE + 4 * stage)?;

    let row = exe.u32_at(TRAIN_ROW_TABLE + 4 * stage)?;
    let mut train_sets = Vec::new();
    for k in 0..4 {
        let v = i32_at(exe, TRAIN_SET_ROWS + 16 * row + 4 * k)?;
        if v >= 0 {
            train_sets.push(v as u32);
        }
    }

    let mut segments = Vec::new();
    let mut p = ptr(exe, SEGMENT_LISTS, s)?;
    loop {
        let com = i32_at(exe, p)?;
        if com == -1 {
            break;
        }
        let first_station = i32_at(exe, p + 4)?;
        let ok = com > 0 && (com as usize) < com_count && first_station >= 0;
        let ordered = segments
            .last()
            .is_none_or(|l: &StageSegment| (first_station as u32) > l.first_station);
        if !ok || !ordered || segments.len() >= MAX_LIST {
            return Err(FormatError::invalid(
                WHAT,
                format!("stage {stage}: bad segment {com}@{first_station}"),
            ));
        }
        segments.push(StageSegment {
            com: com as u32,
            first_station: first_station as u32,
        });
        p += 8;
    }
    if segments.is_empty() {
        return Err(FormatError::invalid(
            WHAT,
            format!("stage {stage}: no segments"),
        ));
    }

    let mut track_sounds = [0i8; 32];
    let sounds = exe.bytes_at(ptr(exe, TRACK_SOUND_TABLES, s)?, 32)?;
    for (d, &v) in track_sounds.iter_mut().zip(sounds) {
        *d = v as i8;
    }

    let mut timetables: [Vec<TimetableRow>; DIFFICULTY_COUNT] = Default::default();
    for (d, t) in timetables.iter_mut().enumerate() {
        *t = read_timetable(exe, ptr(exe, TIMETABLES, s * DIFFICULTY_COUNT + d)?)?;
    }

    let names_table = ptr(exe, STATION_NAMES, s)?;
    let station_names = (0..timetables[0].len())
        .map(|i| {
            let name = c_string(exe, exe.u32_at(names_table + 4 * i as u32)?)?;
            Ok(name.trim_end().to_owned())
        })
        .collect::<Result<Vec<_>>>()?;

    let course_events = read_events(exe, ptr(exe, COURSE_EVENTS, s)?, |r: [i16; 6]| {
        CourseEvent {
            station: r[0],
            distance_m: r[1],
            kind: r[2],
            args: [r[3], r[4], r[5]],
        }
    })?;
    let limit_events = read_events(exe, ptr(exe, LIMIT_EVENTS, s)?, |r: [i16; 6]| CourseEvent {
        station: r[0],
        distance_m: r[1],
        kind: r[2],
        args: [r[3], r[4], r[5]],
    })?;
    let traffic_events = read_events(exe, ptr(exe, TRAFFIC_EVENTS, s)?, |r: [i16; 8]| {
        TrafficEvent {
            station: r[0],
            distance_m: r[1],
            mask: r[2],
            spawn: r[3],
            args: [r[4], r[5], r[6], r[7]],
        }
    })?;

    Ok(Stage {
        area,
        train_sets,
        segments,
        track_sounds,
        timetables,
        station_names,
        course_events,
        limit_events,
        traffic_events,
    })
}

fn read_timetable(exe: &Exe<'_>, va: u32) -> Result<Vec<TimetableRow>> {
    let mut rows = Vec::new();
    for i in 0..MAX_LIST as u32 {
        let r = exe.reader_at(va + 36 * i, 36, WHAT)?;
        let time_ticks = r.i32(0)?;
        if time_ticks == -1 {
            return Ok(rows);
        }
        let mut params = [0i32; 5];
        for (k, p) in params.iter_mut().enumerate() {
            *p = r.i32(16 + 4 * k)?;
        }
        rows.push(TimetableRow {
            time_ticks,
            position_m: r.i32(4)?,
            to_next_m: r.i32(8)?,
            stops: r.i32(12)?,
            params,
        });
    }
    Err(FormatError::invalid(
        WHAT,
        format!("timetable at {va:#x} has no terminator"),
    ))
}

/// A course-event list (6 × i16 records, terminated by station 99) at
/// `va`; for lists outside the stage tables, such as the attract lesson demo's at 0x459338.
pub fn read_course_events(exe: &Exe<'_>, va: u32) -> Result<Vec<CourseEvent>> {
    read_events(exe, va, |r: [i16; 6]| CourseEvent {
        station: r[0],
        distance_m: r[1],
        kind: r[2],
        args: [r[3], r[4], r[5]],
    })
}

fn read_events<const N: usize, T>(
    exe: &Exe<'_>,
    va: u32,
    make: impl Fn([i16; N]) -> T,
) -> Result<Vec<T>> {
    let mut out = Vec::new();
    for i in 0..MAX_LIST as u32 {
        let r = i16s::<N>(exe, va + (2 * N) as u32 * i)?;
        if r[0] == EVENT_END_STATION {
            return Ok(out);
        }
        out.push(make(r));
    }
    Err(FormatError::invalid(
        WHAT,
        format!("event list at {va:#x} has no terminator"),
    ))
}

/// A NUL-terminated string at `va`, decoded as Shift-JIS (the names in practice are ASCII).
fn c_string(exe: &Exe<'_>, va: u32) -> Result<String> {
    let mut bytes = Vec::new();
    for i in 0..MAX_NAME as u32 {
        let b = exe.bytes_at(va + i, 1)?[0];
        if b == 0 {
            let (text, _, _) = encoding_rs::SHIFT_JIS.decode(&bytes);
            return Ok(text.into_owned());
        }
        bytes.push(b);
    }
    Err(FormatError::invalid(
        WHAT,
        format!("string at {va:#x} too long"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal PE32 whose single section covers 0x457000..0x486000, so the real table
    /// addresses can be used.
    struct Image {
        file: Vec<u8>,
    }

    const SECTION_VA: u32 = 0x457000;
    const SECTION_RAW: usize = 0x400;
    const SECTION_LEN: usize = 0x2f000;

    impl Image {
        fn new() -> Self {
            let mut f = vec![0u8; SECTION_RAW + SECTION_LEN];
            f[0..2].copy_from_slice(b"MZ");
            f[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
            f[0x40..0x44].copy_from_slice(b"PE\0\0");
            f[0x46..0x48].copy_from_slice(&1u16.to_le_bytes());
            f[0x54..0x56].copy_from_slice(&0xe0u16.to_le_bytes());
            let opt = 0x58;
            f[opt..opt + 2].copy_from_slice(&0x10bu16.to_le_bytes());
            f[opt + 28..opt + 32].copy_from_slice(&0x400000u32.to_le_bytes());
            let s = opt + 0xe0;
            f[s..s + 5].copy_from_slice(b".data");
            let len = SECTION_LEN as u32;
            f[s + 8..s + 12].copy_from_slice(&len.to_le_bytes());
            f[s + 12..s + 16].copy_from_slice(&(SECTION_VA - 0x400000).to_le_bytes());
            f[s + 16..s + 20].copy_from_slice(&len.to_le_bytes());
            f[s + 20..s + 24].copy_from_slice(&(SECTION_RAW as u32).to_le_bytes());
            Self { file: f }
        }
        fn put(&mut self, va: u32, bytes: &[u8]) {
            let at = SECTION_RAW + (va - SECTION_VA) as usize;
            self.file[at..at + bytes.len()].copy_from_slice(bytes);
        }
        fn u32(&mut self, va: u32, v: u32) {
            self.put(va, &v.to_le_bytes());
        }
        fn i32(&mut self, va: u32, v: i32) {
            self.put(va, &v.to_le_bytes());
        }
        fn i16s(&mut self, va: u32, v: &[i16]) {
            for (i, x) in v.iter().enumerate() {
                self.put(va + 2 * i as u32, &x.to_le_bytes());
            }
        }
    }

    /// Every stage points at the same small data block in free space at 0x470000.
    fn sample() -> Vec<u8> {
        let mut img = Image::new();
        let free = 0x470000u32;
        let (names, segs, sounds, table, course, traffic, stations) = (
            free,
            free + 0x100,
            free + 0x200,
            free + 0x300,
            free + 0x400,
            free + 0x500,
            free + 0x600,
        );
        img.put(names, b"keihin1a.com\0");
        img.put(names + 0x10, b"keihin1b.com\0");
        img.u32(COM_NAME_TABLE + 4, names);
        img.u32(COM_NAME_TABLE + 8, names + 0x10);
        for (i, v) in [1, 0, 2, 3, -1].into_iter().enumerate() {
            img.i32(segs + 4 * i as u32, v);
        }
        img.put(sounds, &[0, 1, 2, 3]);
        // Timetable: two stations and the terminator.
        img.i32(table, 54_030 * 30);
        img.i32(table + 4, 100);
        img.i32(table + 8, 1179);
        img.i32(table + 12, 1);
        img.i32(table + 36 + 4, 1279);
        img.i32(table + 36 + 12, 1);
        img.i32(table + 72, -1);
        img.i16s(course, &[0, 320, 4, 75, 320, 0, 99, 9999, -1, -1, -1, -1]);
        img.i16s(
            traffic,
            &[
                0, 1100, 1, 2, 1, 80, 0, -1, 99, 9999, -1, -1, -1, -1, -1, -1,
            ],
        );
        img.put(stations + 0x10, b"YOKOHAMA   \0");
        img.put(stations + 0x20, b"TURUMI     \0");
        img.u32(stations, stations + 0x10);
        img.u32(stations + 4, stations + 0x20);
        img.i32(TRAIN_SET_ROWS + 16, 0);
        img.i32(TRAIN_SET_ROWS + 20, 5);
        img.i32(TRAIN_SET_ROWS + 24, -1);
        img.i32(TRAIN_SET_ROWS + 28, -1);
        for s in 0..STAGE_COUNT as u32 {
            img.u32(AREA_TABLE + 4 * s, 5);
            img.u32(TRAIN_ROW_TABLE + 4 * s, 1);
            img.u32(SEGMENT_LISTS + 4 * s, segs);
            img.u32(TRACK_SOUND_TABLES + 4 * s, sounds);
            img.u32(COURSE_EVENTS + 4 * s, course);
            img.u32(LIMIT_EVENTS + 4 * s, course);
            img.u32(TRAFFIC_EVENTS + 4 * s, traffic);
            img.u32(STATION_NAMES + 4 * s, stations);
            for d in 0..DIFFICULTY_COUNT as u32 {
                img.u32(TIMETABLES + 16 * s + 4 * d, table);
            }
        }
        img.file
    }

    #[test]
    fn reads_sample_tables() {
        let file = sample();
        let exe = Exe::parse(&file).unwrap();
        let t = StageTables::read(&exe).unwrap();
        assert_eq!(t.stages.len(), STAGE_COUNT);
        assert_eq!(t.com_file(0), None);
        assert_eq!(t.com_file(2), Some("keihin1b.com"));
        let st = &t.stages[10];
        assert_eq!(st.area, 5);
        assert_eq!(st.train_sets, [0, 5]);
        assert_eq!(
            st.segments,
            [
                StageSegment {
                    com: 1,
                    first_station: 0
                },
                StageSegment {
                    com: 2,
                    first_station: 3
                }
            ]
        );
        assert_eq!(st.segment_for_station(4).unwrap().1, 1);
        assert_eq!(st.segment_for_station(2).unwrap().0.com, 1);
        assert_eq!(st.track_sounds[..4], [0, 1, 2, 3]);
        assert_eq!(st.timetables[3].len(), 2);
        assert_eq!(st.timetables[0][0].to_next_m, 1179);
        assert_eq!(st.station_names, ["YOKOHAMA", "TURUMI"]);
        assert_eq!(st.course_events.len(), 1);
        assert_eq!(st.course_events[0].args, [75, 320, 0]);
        assert_eq!(st.traffic_events[0].spawn, 2);
        assert_eq!(st.traffic_events[0].args[3], -1);
    }

    #[test]
    fn rejects_unordered_segments() {
        let mut file = sample();
        // Make the second segment start at station 0 as well.
        let at = SECTION_RAW + (0x470100 + 12 - SECTION_VA) as usize;
        file[at..at + 4].copy_from_slice(&0i32.to_le_bytes());
        let exe = Exe::parse(&file).unwrap();
        assert!(StageTables::read(&exe).is_err());
    }
}

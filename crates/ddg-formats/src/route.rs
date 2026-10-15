//! Route `.COM` files (`"RAIL DATA VER7.0"`): track geometry, scenery placement, draw buckets,
//! traffic spawn points and station stop points for one route segment.
//!
//! Format, units and evidence: `docs/re/routes.md`. In short: a 0x30-byte header of eight
//! file-relative offsets plus the tag, then sections whose internal pointers are also
//! file-relative (the game relocates them in place, 0x435380).
//!
//! Track points live in two contiguous regions of 12-byte *slots*: the main line, and all side
//! tracks back to back. Some slots are segment markers rather than points. The game walks these
//! slots linearly (draw runs may cross from one side segment into the next), so this parser keeps
//! every slot in [`Route::slots`] and resolves every point pointer to a slot index.
//!
//! Speed limits, signals and the timetable are **not** in these files; they come from the exe
//! (see [`crate::stages`]).

use std::ops::Range;

use crate::error::{FormatError, Result};
use crate::reader::Bytes;

const WHAT: &str = "route .COM";

pub const MAGIC: [u8; 16] = *b"RAIL DATA VER7.0";
const MAGIC_OFFSET: usize = 0x20;
const HEADER_LEN: usize = 0x30;
const POINT_LEN: usize = 12;
const SEGMENT_LEN: usize = 24;
const TRAFFIC_LEN: usize = 8;
const PLACEMENT_LEN: usize = 20;
const RUN_LEN: usize = 20;
const STOP_LEN: usize = 8;

/// Track-position units advanced per point (float 0.125 at 0x458b80, used by the camera walk
/// 0x41d160 and the stop distance in 0x41c7b0).
pub const UNITS_PER_POINT: f32 = 0.125;
/// Metres per track-position unit: distance-to-stop = units × 10.0 (double at 0x458b88, 0x40dad0).
pub const METRES_PER_UNIT: f32 = 10.0;
/// Metres per track point (= [`UNITS_PER_POINT`] × [`METRES_PER_UNIT`]). Cross-checked against
/// the exe timetable's station-to-station distances; see routes.md §3.
pub const METRES_PER_POINT: f32 = UNITS_PER_POINT * METRES_PER_UNIT;
/// Height scale for [`TrackPoint::height_raw`] (double 1/4096 at 0x458c38).
pub const HEIGHT_SCALE: f32 = 1.0 / 4096.0;

/// One 12-byte track slot: `f32 x, f32 z, i16 height, u16 flags`. World units (≈10 m).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackPoint {
    pub x: f32,
    pub z: f32,
    /// Height in 1/4096 world units (before the per-route base height the exe adds). In a
    /// marker slot this is the segment index instead.
    pub height_raw: i16,
    pub flags: u16,
}

impl TrackPoint {
    fn read(b: &Bytes<'_>, offset: usize) -> Result<Self> {
        Ok(Self {
            x: b.f32(offset)?,
            z: b.f32(offset + 4)?,
            height_raw: b.i16(offset + 8)?,
            flags: b.u16(offset + 10)?,
        })
    }

    /// Height in world units.
    pub fn y(&self) -> f32 {
        f32::from(self.height_raw) * HEIGHT_SCALE
    }

    /// Bits 0–4: track type. Selects the rail model (per-area table) and the ambient sound class
    /// (per-stage table); 31 = no rail drawn.
    pub fn track_type(&self) -> u8 {
        (self.flags & 0x1f) as u8
    }

    /// `flags & 0xe000` is 0x8000 or 0xa000: a turnout. The game plays the points-clatter sound
    /// when the train passes one.
    pub fn is_turnout(&self) -> bool {
        matches!(self.flags & 0xe000, 0x8000 | 0xa000)
    }

    /// Bits 5–12 on turnout points: the low 8 bits of the index of the segment that joins here.
    pub fn joining_segment_low8(&self) -> u8 {
        (self.flags >> 5) as u8
    }

    fn is_marker(&self, high: u16, segment: usize) -> bool {
        self.flags & 0xf000 == high
            && self.x == 0.0
            && self.z == 0.0
            && usize::try_from(self.height_raw).is_ok_and(|s| s == segment)
    }
}

/// One polyline of track. Segment 0 is the line the player drives.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    /// The segment's points (marker slots excluded) as a range of [`Route::slots`]. May be empty.
    pub points: Range<usize>,
    /// A marker slot (`flags 0xf000`, x = z = 0, height = segment index) precedes the points.
    pub head_marker: bool,
    /// A marker slot (`flags 0xe000`, x = z = 0, height = segment index) follows the points.
    pub tail_marker: bool,
    /// Slot where a walk continues past the start (`linkA`).
    pub link_prev: Option<usize>,
    /// Slot where a walk continues past the end (`linkB`).
    pub link_next: Option<usize>,
    /// Two u32 at +16/+20 of the table entry; 0 in every shipped file.
    pub reserved: [u32; 2],
}

/// Header slot 1: a spawn point for an AI train, referenced by index from the exe's per-stage
/// traffic event lists.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrafficSpawn {
    /// 1-based train type (`type - 1` indexes the exe's train-model table at 0x457358).
    pub train_type: u16,
    /// Bit 0: direction (1 = walks towards the end of the segment, 0 = towards the start).
    pub flags: u16,
    /// Slot index.
    pub slot: Option<usize>,
}

/// Header slot 2: a scenery object. Index 0 is an all-zero dummy so indices match the game.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub part_id: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    /// Rotation about the vertical axis, radians.
    pub yaw: f32,
}

/// One 20-byte record of a draw bucket: a run of track slots drawn with one model set.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackRun {
    /// Model set: 1 = rail (per-area table by point track type), 3/4/7/8 = fixed exe sets,
    /// ≥ 9 = per-area table of 5 parts per kind. Stored as `|i16|`; the sign marks the last
    /// record of a bucket.
    pub kind: u16,
    /// u16 at +2. Unknown.
    pub unknown: u16,
    /// Bytes +4..+7: level-of-detail band counts (see routes.md §6 for how each kind reads them).
    pub lod: [u8; 4],
    /// Slots from the start pointer to the end pointer. Can run on into the next side segment.
    pub slots: Range<usize>,
    /// f32 at +16: ≈ squared horizontal distance from the bucket's camera point to the run (likely).
    pub dist_sq: f32,
}

/// A byte range of the file and what it holds (for coverage checks and tools).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Section {
    pub name: &'static str,
    pub offset: usize,
    pub len: usize,
}

/// A fully parsed `.COM` route file.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    /// Raw header offsets (slot 0 is relocated by the game but never read; it equals slot 1).
    pub header: [u32; 8],
    /// Every 12-byte track slot: the main-line region, then the side-track region.
    pub slots: Vec<TrackPoint>,
    pub segments: Vec<Segment>,
    pub traffic: Vec<TrafficSpawn>,
    pub placements: Vec<Placement>,
    /// One bucket per 4 main-line points: the track runs drawn while the camera is there.
    pub buckets: Vec<Vec<TrackRun>>,
    /// Station stop points as main-line point indices, in station order.
    pub stops: Vec<usize>,
    /// Per main-line point (engine index, see [`Route::activations`]): index into
    /// `activation_pool`, 0 = nothing changes there.
    pub activation_index: Vec<u16>,
    /// Zero-terminated lists of signed placement indices.
    pub activation_pool: Vec<i16>,
    /// The byte ranges parsed, sorted by offset.
    pub sections: Vec<Section>,
}

/// A contiguous run of slots in the file.
struct Region {
    offset: usize,
    len: usize, // in slots
    base: usize,
}

struct SlotMap {
    regions: Vec<Region>,
}

impl SlotMap {
    /// Slot index of file offset `offset`; `allow_end` also accepts one past a region's end.
    fn resolve(&self, offset: usize, allow_end: bool) -> Option<usize> {
        self.regions.iter().find_map(|r| {
            let rel = offset.checked_sub(r.offset)?;
            let k = rel / POINT_LEN;
            let inside = k < r.len || (allow_end && k == r.len);
            (rel.is_multiple_of(POINT_LEN) && inside).then_some(r.base + k)
        })
    }

    fn require(&self, offset: usize, allow_end: bool, what: &str) -> Result<usize> {
        self.resolve(offset, allow_end).ok_or_else(|| {
            FormatError::invalid(WHAT, format!("{what}: {offset:#x} is not a track slot"))
        })
    }

    fn link(&self, offset: usize, what: &str) -> Result<Option<usize>> {
        if offset == 0 {
            return Ok(None);
        }
        self.require(offset, false, what).map(Some)
    }
}

impl Route {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let b = Bytes::new(data, WHAT);
        let tag = b.array::<16>(MAGIC_OFFSET)?;
        if tag != MAGIC {
            return Err(FormatError::BadMagic {
                what: WHAT,
                found: tag.to_vec(),
            });
        }
        let mut header = [0u32; 8];
        for (i, h) in header.iter_mut().enumerate() {
            *h = b.u32(i * 4)?;
        }
        let off = |slot: usize| header[slot] as usize;
        let mut sections = vec![Section {
            name: "header",
            offset: 0,
            len: HEADER_LEN,
        }];

        // Slot 3: segment table, terminated by a u32 −1.
        let (raw, table_len) = read_segment_table(&b, off(3))?;
        sections.push(Section {
            name: "segment table",
            offset: off(3),
            len: table_len,
        });
        let (slots, mut segments, map) = read_track(&b, &raw, &mut sections)?;
        for (seg, r) in segments.iter_mut().zip(&raw) {
            seg.link_prev = map.link(r.link_a, "segment linkA")?;
            seg.link_next = map.link(r.link_b, "segment linkB")?;
        }
        let main = segments[0].points.clone();

        // Slot 1: traffic spawns, terminated by type 0xffff (4-byte terminator).
        let mut traffic = Vec::new();
        let mut o = off(1);
        while b.u16(o)? != 0xffff {
            traffic.push(TrafficSpawn {
                train_type: b.u16(o)?,
                flags: b.u16(o + 2)?,
                slot: map.link(b.usize32(o + 4)?, "traffic spawn")?,
            });
            o += TRAFFIC_LEN;
        }
        sections.push(Section {
            name: "traffic spawns",
            offset: off(1),
            len: o + 4 - off(1),
        });

        // Slot 2: placements, terminated by part id 0xffffffff.
        let mut placements = Vec::new();
        let mut o = off(2);
        while b.u32(o)? != u32::MAX {
            placements.push(Placement {
                part_id: b.u32(o)?,
                x: b.f32(o + 4)?,
                y: b.f32(o + 8)?,
                z: b.f32(o + 12)?,
                yaw: b.f32(o + 16)?,
            });
            o += PLACEMENT_LEN;
        }
        sections.push(Section {
            name: "placements",
            offset: off(2),
            len: o + 4 - off(2),
        });

        // Slot 6: one u16 per main-line point; slot 7: the lists they index.
        let activation_index = (0..main.len())
            .map(|i| b.u16(off(6) + 2 * i))
            .collect::<Result<Vec<_>>>()?;
        sections.push(Section {
            name: "activation index",
            offset: off(6),
            len: 2 * main.len(),
        });
        let activation_pool =
            read_activation_pool(&b, off(7), &activation_index, placements.len())?;
        sections.push(Section {
            name: "activation lists",
            offset: off(7),
            len: 2 * activation_pool.len(),
        });

        // Slot 4: bucket pointer table, then the run records.
        let (buckets, records) = read_buckets(&b, off(4), &map)?;
        sections.push(Section {
            name: "bucket table",
            offset: off(4),
            len: 4 * buckets.len(),
        });
        sections.extend(records);

        // Slot 5: stop points {ptr, u32 index}, terminated by ptr 0 (8-byte terminator).
        let mut stops = Vec::new();
        let mut o = off(5);
        loop {
            let ptr = b.usize32(o)?;
            if ptr == 0 {
                break;
            }
            let slot = map.require(ptr, false, "stop point")?;
            let index = b.usize32(o + 4)?;
            if slot.checked_sub(main.start) != Some(index) || index >= main.len() {
                return Err(FormatError::invalid(
                    WHAT,
                    format!(
                        "stop {} at {ptr:#x} disagrees with index {index}",
                        stops.len()
                    ),
                ));
            }
            stops.push(index);
            o += STOP_LEN;
        }
        sections.push(Section {
            name: "stops",
            offset: off(5),
            len: o + STOP_LEN - off(5),
        });

        sections.sort_by_key(|s| s.offset);
        Ok(Self {
            header,
            slots,
            segments,
            traffic,
            placements,
            buckets,
            stops,
            activation_index,
            activation_pool,
            sections,
        })
    }

    /// The points of segment `index` (markers excluded).
    pub fn points(&self, index: usize) -> &[TrackPoint] {
        &self.slots[self.segments[index].points.clone()]
    }

    /// The driven line (segment 0).
    pub fn main_line(&self) -> &[TrackPoint] {
        self.points(0)
    }

    /// The segment and point index of a slot, or `None` for a marker slot.
    pub fn locate(&self, slot: usize) -> Option<(usize, usize)> {
        self.segments
            .iter()
            .position(|s| s.points.contains(&slot))
            .map(|i| (i, slot - self.segments[i].points.start))
    }

    /// The activation list consulted when the camera moves forward onto main-line point
    /// `engine_index + 1` (the game counts from the second point). Positive values show
    /// placement *n*, negative values hide placement −*n*; moving backwards the game reads entry
    /// `engine_index + 1` with the signs swapped.
    pub fn activations(&self, engine_index: usize) -> &[i16] {
        let start = match self.activation_index.get(engine_index) {
            Some(&s) if s != 0 => s as usize,
            _ => return &[],
        };
        let list = &self.activation_pool[start..];
        let end = list.iter().position(|&v| v == 0).unwrap_or(list.len());
        &list[..end]
    }

    /// Byte ranges of the file not covered by any parsed section.
    pub fn unparsed(&self, file_len: usize) -> Vec<(usize, usize)> {
        let mut gaps = Vec::new();
        let mut cursor = 0;
        for s in &self.sections {
            if s.offset > cursor {
                gaps.push((cursor, s.offset - cursor));
            }
            cursor = cursor.max(s.offset + s.len);
        }
        if cursor < file_len {
            gaps.push((cursor, file_len - cursor));
        }
        gaps
    }
}

struct RawSegment {
    start: usize,
    end: usize,
    link_a: usize,
    link_b: usize,
    reserved: [u32; 2],
}

fn read_segment_table(b: &Bytes<'_>, offset: usize) -> Result<(Vec<RawSegment>, usize)> {
    let mut out = Vec::new();
    let mut o = offset;
    while b.i32(o)? != -1 {
        out.push(RawSegment {
            start: b.usize32(o)?,
            end: b.usize32(o + 4)?,
            link_a: b.usize32(o + 8)?,
            link_b: b.usize32(o + 12)?,
            reserved: [b.u32(o + 16)?, b.u32(o + 20)?],
        });
        o += SEGMENT_LEN;
    }
    if out.is_empty() {
        return Err(FormatError::invalid(WHAT, "no track segments"));
    }
    Ok((out, o + 4 - offset))
}

/// Reads both slot regions: the main line (segment 0 with its markers) and the side segments,
/// which must follow each other without gaps in table order.
fn read_track(
    b: &Bytes<'_>,
    raw: &[RawSegment],
    sections: &mut Vec<Section>,
) -> Result<(Vec<TrackPoint>, Vec<Segment>, SlotMap)> {
    let mut slots = Vec::new();
    let mut segments = Vec::with_capacity(raw.len());
    let mut regions: Vec<Region> = Vec::new();
    for (i, r) in raw.iter().enumerate() {
        if r.end < r.start || !(r.end - r.start).is_multiple_of(POINT_LEN) {
            return Err(FormatError::invalid(
                WHAT,
                format!("segment {i}: bad range {:#x}..{:#x}", r.start, r.end),
            ));
        }
        let head_marker = r.start >= HEADER_LEN + POINT_LEN
            && TrackPoint::read(b, r.start - POINT_LEN)?.is_marker(0xf000, i);
        let first = if head_marker {
            r.start - POINT_LEN
        } else {
            r.start
        };
        // Segment 0 starts the main region and segment 1 the side region; every later segment
        // must continue the current region.
        let continues = regions
            .last()
            .is_some_and(|g| g.offset + g.len * POINT_LEN == first);
        if i <= 1 || !continues {
            if i > 1 {
                return Err(FormatError::invalid(
                    WHAT,
                    format!(
                        "segment {i} at {first:#x} does not follow segment {}",
                        i - 1
                    ),
                ));
            }
            regions.push(Region {
                offset: first,
                len: 0,
                base: slots.len(),
            });
        }
        let count = (r.end - first) / POINT_LEN;
        let base = slots.len();
        for k in 0..count {
            slots.push(TrackPoint::read(b, first + k * POINT_LEN)?);
        }
        if let Some(g) = regions.last_mut() {
            g.len += count;
        }
        let mut points = base + usize::from(head_marker)..base + count;
        let tail_marker = !points.is_empty() && slots[points.end - 1].is_marker(0xe000, i);
        if tail_marker {
            points.end -= 1;
        }
        segments.push(Segment {
            points,
            head_marker,
            tail_marker,
            link_prev: None,
            link_next: None,
            reserved: r.reserved,
        });
    }
    for (g, name) in regions.iter().zip(["main line slots", "side track slots"]) {
        sections.push(Section {
            name,
            offset: g.offset,
            len: g.len * POINT_LEN,
        });
    }
    Ok((slots, segments, SlotMap { regions }))
}

fn read_activation_pool(
    b: &Bytes<'_>,
    offset: usize,
    index: &[u16],
    placement_count: usize,
) -> Result<Vec<i16>> {
    let mut end = 0usize; // pool length in entries
    for &start in index.iter().filter(|&&s| s != 0) {
        let mut k = start as usize;
        loop {
            let v = b.i16(offset + 2 * k)?;
            if v == 0 {
                break;
            }
            if v.unsigned_abs() as usize >= placement_count {
                return Err(FormatError::invalid(
                    WHAT,
                    format!("activation list {start}: placement {v} out of range"),
                ));
            }
            k += 1;
        }
        end = end.max(k + 1);
    }
    (0..end).map(|k| b.i16(offset + 2 * k)).collect()
}

/// The bucket count is the distance to the first non-null pointer, as the game's relocation
/// loop computes it.
fn read_buckets(
    b: &Bytes<'_>,
    offset: usize,
    map: &SlotMap,
) -> Result<(Vec<Vec<TrackRun>>, Option<Section>)> {
    let mut k = 0;
    let first = loop {
        let p = b.usize32(offset + 4 * k)?;
        if p != 0 {
            break p;
        }
        k += 1;
    };
    if first <= offset || !(first - offset).is_multiple_of(4) {
        return Err(FormatError::invalid(
            WHAT,
            "bucket table does not end at its first record",
        ));
    }
    let count = (first - offset) / 4;
    let (mut lo, mut hi) = (usize::MAX, 0);
    let mut buckets = Vec::with_capacity(count);
    for i in 0..count {
        let mut o = b.usize32(offset + 4 * i)?;
        let mut runs = Vec::new();
        if o != 0 {
            lo = lo.min(o);
            loop {
                let kind = b.i16(o)?;
                let start = map.require(b.usize32(o + 8)?, false, "track run start")?;
                let end = map.require(b.usize32(o + 12)?, true, "track run end")?;
                if end < start {
                    return Err(FormatError::invalid(
                        WHAT,
                        format!("bucket {i}: run ends before it starts"),
                    ));
                }
                runs.push(TrackRun {
                    kind: kind.unsigned_abs(),
                    unknown: b.u16(o + 2)?,
                    lod: b.array(o + 4)?,
                    slots: start..end,
                    dist_sq: b.f32(o + 16)?,
                });
                o += RUN_LEN;
                if kind < 0 {
                    break;
                }
            }
            hi = hi.max(o);
        }
        buckets.push(runs);
    }
    let records = (lo < hi).then(|| Section {
        name: "bucket records",
        offset: lo,
        len: hi - lo,
    });
    Ok((buckets, records))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes little-endian values at explicit offsets into a growing buffer.
    struct Buf(Vec<u8>);

    impl Buf {
        fn put(&mut self, at: usize, bytes: &[u8]) {
            if self.0.len() < at + bytes.len() {
                self.0.resize(at + bytes.len(), 0);
            }
            self.0[at..at + bytes.len()].copy_from_slice(bytes);
        }
        fn u32(&mut self, at: usize, v: u32) {
            self.put(at, &v.to_le_bytes());
        }
        fn u16(&mut self, at: usize, v: u16) {
            self.put(at, &v.to_le_bytes());
        }
        fn f32(&mut self, at: usize, v: f32) {
            self.put(at, &v.to_le_bytes());
        }
        fn point(&mut self, at: usize, x: f32, z: f32, h: i16, flags: u16) {
            self.f32(at, x);
            self.f32(at + 4, z);
            self.put(at + 8, &h.to_le_bytes());
            self.u16(at + 10, flags);
        }
    }

    /// A tiny route: main line of 8 points (with head/tail markers); two side segments of 3 and
    /// 2 points back to back, the first joining main-line point 2; one placement; two buckets;
    /// one stop.
    fn sample() -> Vec<u8> {
        let mut f = Buf(Vec::new());
        f.put(0x20, &MAGIC);
        let seg_table = 0x30; // 3 entries + terminator
        let traffic = seg_table + 3 * 24 + 4; // 1 entry + terminator
        let main_head = traffic + 8 + 4;
        let main = main_head + 12;
        let main_end = main + 9 * 12; // 8 points + tail marker
        let act_index = main_end; // 8 × u16
        let act_pool = act_index + 16; // [0, 1, 0, -1, 0] + 2 bytes padding
        let buckets = act_pool + 12; // 2 pointers
        let runs = buckets + 8;
        let placements = runs + 2 * 20; // dummy + 1 + terminator
        let side = placements + 2 * 20 + 4;
        let side2 = side + 3 * 12;
        let side_end = side2 + 2 * 12;
        let stops = side_end;
        let header = [
            traffic, traffic, placements, seg_table, buckets, stops, act_index, act_pool,
        ];
        for (slot, v) in header.into_iter().enumerate() {
            f.u32(slot * 4, v as u32);
        }
        // Segment table.
        f.u32(seg_table, main as u32);
        f.u32(seg_table + 4, main_end as u32);
        f.u32(seg_table + 24, side as u32);
        f.u32(seg_table + 28, side2 as u32);
        f.u32(seg_table + 36, (main + 2 * 12) as u32); // side linkB → main point 2
        f.u32(seg_table + 48, side2 as u32);
        f.u32(seg_table + 52, side_end as u32);
        f.u32(seg_table + 72, u32::MAX);
        // Traffic: type 3, forward, at main point 1.
        f.u16(traffic, 3);
        f.u16(traffic + 2, 1);
        f.u32(traffic + 4, (main + 12) as u32);
        f.u16(traffic + 8, 0xffff);
        // Main line.
        f.point(main_head, 0.0, 0.0, 0, 0xf000);
        for i in 0..8 {
            let flags = if i == 2 { 0x8000 | (1 << 5) | 1 } else { 1 };
            f.point(main + i * 12, 0.0, -0.125 * i as f32, 4096, flags);
        }
        f.point(main + 8 * 12, 0.0, 0.0, 0, 0xe000);
        // Side segments: no markers; the last point of each carries 0xe000.
        for i in 0..5 {
            let flags = if i == 2 || i == 4 { 0xe000 } else { 0x1f };
            f.point(side + i * 12, 1.0, -0.125 * i as f32, 0, flags);
        }
        // Activation: index 1 shows placement 1, index 3 hides it.
        f.u16(act_index + 2, 1);
        f.u16(act_index + 6, 3);
        f.put(act_pool, &[0, 0, 1, 0, 0, 0, 0xff, 0xff, 0, 0]);
        // Bucket 0: a rail run over side slots 1..5 (crossing into segment 2).
        // Bucket 1: kind −9 (last) over the first side segment.
        f.u32(buckets, runs as u32);
        f.u32(buckets + 4, (runs + 20) as u32);
        f.u16(runs, 0xffff); // kind −1
        f.put(runs + 4, &[4, 0x10, 0, 0]);
        f.u32(runs + 8, (side + 12) as u32);
        f.u32(runs + 12, side_end as u32);
        f.f32(runs + 16, 0.5);
        f.u16(runs + 20, (-9i16) as u16);
        f.u32(runs + 28, side as u32);
        f.u32(runs + 32, side2 as u32);
        // Placements: dummy, then part 42.
        f.u32(placements + 20, 42);
        f.f32(placements + 24, 1.5);
        f.f32(placements + 36, std::f32::consts::FRAC_PI_2);
        f.u32(placements + 40, u32::MAX);
        // Stops: main point 5, then the 8-byte terminator.
        f.u32(stops, (main + 5 * 12) as u32);
        f.u32(stops + 4, 5);
        f.put(stops + 8, &[0; 8]);
        f.0
    }

    #[test]
    fn parses_sample_completely() {
        let data = sample();
        let r = Route::parse(&data).unwrap();
        assert_eq!(r.segments.len(), 3);
        assert_eq!(r.slots.len(), 10 + 5);
        let main = &r.segments[0];
        assert!(main.head_marker && main.tail_marker);
        assert_eq!(main.points, 1..9);
        let line = r.main_line();
        assert_eq!(line.len(), 8);
        assert_eq!(line[3].y(), 1.0);
        assert_eq!(line[3].track_type(), 1);
        assert!(line[2].is_turnout());
        assert_eq!(line[2].joining_segment_low8(), 1);
        let side = &r.segments[1];
        assert!(!side.head_marker && !side.tail_marker);
        assert_eq!(side.points, 10..13);
        assert_eq!(r.segments[2].points, 13..15);
        assert_eq!(side.link_next, Some(3));
        assert_eq!(r.locate(3), Some((0, 2)));
        assert_eq!(r.locate(0), None); // head marker
        assert_eq!(side.link_prev, None);
        assert_eq!(r.traffic[0].slot, Some(2));
        assert_eq!(r.placements.len(), 2);
        assert_eq!(r.placements[1].part_id, 42);
        assert_eq!(r.activations(1), &[1]);
        assert_eq!(r.activations(3), &[-1]);
        assert!(r.activations(0).is_empty());
        assert_eq!(r.buckets.len(), 2);
        assert_eq!(r.buckets[0][0].kind, 1);
        assert_eq!(r.buckets[0][0].slots, 11..15);
        assert_eq!(r.buckets[1][0].kind, 9);
        assert_eq!(r.buckets[1][0].slots, 10..13);
        assert_eq!(r.stops, [5]);
        // Only the 2-byte alignment pad after the activation pool is left.
        let gaps = r.unparsed(data.len());
        assert_eq!(gaps.len(), 1);
        assert!(gaps[0].1 < 4);
    }

    #[test]
    fn rejects_bad_tag_and_bad_pointers() {
        let mut data = sample();
        data[0x20] = b'X';
        assert!(matches!(
            Route::parse(&data),
            Err(FormatError::BadMagic { .. })
        ));

        let mut data = sample();
        let stops = u32::from_le_bytes(data[20..24].try_into().unwrap()) as usize;
        data[stops + 4] = 4; // index disagrees with the pointer
        assert!(Route::parse(&data).is_err());

        let mut data = sample();
        data[0x30 + 36] += 1; // misaligned link
        assert!(Route::parse(&data).is_err());
    }

    #[test]
    fn truncated_input_errors() {
        let data = sample();
        for cut in [0, 0x2f, 0x40, data.len() - 5] {
            assert!(Route::parse(&data[..cut]).is_err(), "cut {cut}");
        }
    }
}

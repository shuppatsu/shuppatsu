//! The pre-departure HUD (docs/re/hud.md §3.7, §5): the segment set-up 0x4124f0 places the
//! station plates, the route strip with the stations served so far and the star marks, and the
//! departure task 0x413370 animates them, then removes them and shows the departure banner.
//! The departure task's sounds, signals and hints come from the rules and the sound module.
//!
//! The strip's arrows are palette-animated: palette 4 entries `0x50 + i` are green
//! (0x466b70) for the segments already run and olive (0x466b10) for the rest; the current one
//! blinks between the two every 10 updates for the first 60.

use crate::slots;
use ddg_formats::cab_hud::{CabTables, ROUTE_STRIP_ENTRIES, STAR_SLOTS, STRIP_COLOURS};
use ddg_render::sprites::SpriteLayer;

const SLOT_STRIP: usize = 0x7a;
const SLOT_DIAGRAM: usize = 0x7b;
const SLOT_STARS: usize = 0x3f;
/// Strip and diagram at `origin + (0x90, 0x88)` and `+ (0x90, 0xb8)`; stars at
/// `origin + (0x90 + offset, 0x90)` (0x4124f0).
const STRIP_X: i16 = 0x90;
const STRIP_Y: i16 = 0x88;
const DIAGRAM_Y: i16 = 0xb8;
const STARS_Y: i16 = 0x90;
/// Palette 4 entry of the first strip arrow.
const STRIP_ENTRY: usize = 0x50;
/// The current segment blinks for 60 updates, 10 per phase (0x413370).
const BLINK_UNTIL: i32 = 0x3c;
const BLINK_PHASE: i32 = 10;
/// Star marks animate for 91 updates, 5 per frame, 6 frames: mark 1 silver 0x38f.., mark 2
/// gold 0x395.. (0x413370).
const STARS_UNTIL: i32 = 0x5b;
const STAR_FRAME_TICKS: i32 = 5;
const STAR_FRAMES: i32 = 6;
const STAR_SILVER: i16 = 0x38f;
const STAR_GOLD: i16 = 0x395;
/// Departure-task milestones after its offset: strip removed at +100, doors close (banner,
/// cab-signal lamp) at +200.
const REMOVE_AT: i32 = 100;
const DOORS_AT: i32 = 200;
/// Stage 10 changes route diagrams from its 8th station (0x4124f0).
const STAGE10_SECOND_HALF_FROM: usize = 8;

/// What the segment set-up reads.
#[derive(Clone, Copy)]
pub struct Segment<'a> {
    pub stage: usize,
    pub station: usize,
    /// Timetable "stops" flags by station.
    pub stops: &'a [bool],
    /// Marks by station (`0x856fe0[station]`: 1 clean stop, 2 clean on-time stop).
    pub mark_at: &'a dyn Fn(usize) -> u8,
    /// Station-name ids of the stage.
    pub name_ids: &'a [u16],
}

/// What the HUD needs to do at the doors-close tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DoorsClosed {
    /// Station-name id for the banner (the segment's start station).
    pub name_id: Option<usize>,
}

/// The departure HUD state for one segment.
#[derive(Debug, Clone, PartialEq)]
pub struct DepartureHud {
    olive: [u32; STRIP_COLOURS],
    green: [u32; STRIP_COLOURS],
    extra: [u32; STRIP_COLOURS],
    strips: Vec<[i16; ROUTE_STRIP_ENTRIES]>,
    strip_stage10: [i16; ROUTE_STRIP_ENTRIES],
    star_offsets: Vec<[i32; STAR_SLOTS]>,
    origin: (i16, i16),
    /// Stopping stations served so far (`task+0x30`).
    served: usize,
    /// Marks per star slot (`task+0x50..`).
    marks: [u8; STAR_SLOTS],
    name_id: Option<usize>,
    /// Plates: start station and next stop (`0x856d4c`, `0x856c74`).
    pub plates: (Option<usize>, Option<usize>),
    /// Station-name ids from the segment's start station on (the diagram's columns).
    window: Vec<usize>,
}

impl DepartureHud {
    pub fn new(cab: &CabTables) -> Self {
        Self {
            olive: cab.strip_olive,
            green: cab.strip_green,
            extra: cab.strip_extra,
            strips: cab.route_strips.clone(),
            strip_stage10: cab.route_strip_stage10,
            star_offsets: cab.star_offsets.to_vec(),
            origin: cab.origin,
            served: 0,
            marks: [0; STAR_SLOTS],
            name_id: None,
            plates: (None, None),
            window: Vec::new(),
        }
    }

    /// Every diagram tilemap the strip tables name (for [`crate::board_en::BoardNames`]).
    pub fn diagram_tilemaps(&self) -> impl Iterator<Item = u16> + '_ {
        self.strips
            .iter()
            .chain(std::iter::once(&self.strip_stage10))
            .flat_map(|s| s[1..].iter())
            .filter_map(|&t| u16::try_from(t).ok())
    }

    /// Enhancement `board_names`: the romaji under the diagram's columns while it shows.
    pub fn draw_names(&self, names: &crate::board_en::BoardNames, layer: &mut SpriteLayer) {
        let slot = layer.slots[SLOT_DIAGRAM];
        if slot.tilemap < 0 {
            return;
        }
        let anchor = layer.anchors[SLOT_DIAGRAM];
        layer.anchored(anchor, |layer| {
            names.draw(
                layer,
                slot.tilemap,
                (slot.x, slot.y),
                (slot.layer, slot.palette),
                &|i| self.window.get(i).copied(),
            );
        });
    }

    /// Hud_Create 0x40f7e0: the whole strip palette is olive again.
    pub fn stage_start(&self, layer: &mut SpriteLayer) {
        slots::palette4(layer, STRIP_ENTRY, &self.olive);
    }

    /// The segment set-up 0x4124f0.
    pub fn segment_setup(&mut self, s: &Segment<'_>, layer: &mut SpriteLayer) {
        let stage10_late = s.stage == 10 && s.station >= STAGE10_SECOND_HALF_FROM;
        let row = if s.stage == 10 && !stage10_late {
            0
        } else {
            s.stage + 1
        };
        let offsets = self
            .star_offsets
            .get(row)
            .copied()
            .unwrap_or([0; STAR_SLOTS]);
        for (i, off) in offsets.iter().enumerate() {
            let x = (i32::from(self.origin.0) + i32::from(STRIP_X) + off).clamp(-0x8000, 0x7fff);
            slots::place(
                layer,
                SLOT_STARS + i,
                x as i16,
                self.origin.1 + STARS_Y,
                3,
                4,
            );
        }
        // Stopping stations up to this one, with their marks.
        self.marks = [0; STAR_SLOTS];
        self.served = 0;
        for st in 0..=s.station.min(s.stops.len().saturating_sub(1)) {
            if s.stops[st] {
                if let Some(m) = self.marks.get_mut(self.served) {
                    *m = (s.mark_at)(st);
                }
                self.served += 1;
            }
        }
        // The origin station never has a mark.
        self.marks[0] = 0;
        let next_stop = (s.station + 1..s.stops.len()).find(|&st| s.stops[st]);
        let id = |st: usize| s.name_ids.get(st).map(|&i| usize::from(i));
        let (here, next) = (id(s.station), next_stop.and_then(id));
        self.name_id = here;
        self.plates = (here, next);
        self.window = s
            .name_ids
            .iter()
            .skip(s.station)
            .map(|&i| usize::from(i))
            .collect();

        let strip = if stage10_late {
            &self.strip_stage10
        } else {
            match self.strips.get(s.stage) {
                Some(s) => s,
                None => return,
            }
        };
        let (x, y) = (self.origin.0 + STRIP_X, self.origin.1);
        slots::place(layer, SLOT_STRIP, x, y + STRIP_Y, 3, 4);
        slots::place(layer, SLOT_DIAGRAM, x, y + DIAGRAM_Y, 3, 4);
        slots::image(layer, SLOT_STRIP, strip[0]);
        slots::image(
            layer,
            SLOT_DIAGRAM,
            strip.get(self.served).copied().unwrap_or(-1),
        );
        let n = self.served.min(STRIP_COLOURS);
        slots::palette4(layer, STRIP_ENTRY, &self.green[..n]);
        let extra = |n: usize| &self.extra[..n.min(STRIP_COLOURS)];
        match s.stage {
            0 | 1 => slots::palette4(layer, 0x52, extra(6)),
            2 => slots::palette4(layer, 0x56, extra(1)),
            5 => slots::palette4(layer, 0x54, extra(11)),
            17..=19 => slots::palette4(layer, 0x55, extra(1)),
            _ => {}
        }
    }

    /// One update of the departure task with its counter `c` and offset `offset`. Returns
    /// the doors-close event on `offset + 200`.
    pub fn update(&mut self, layer: &mut SpriteLayer, c: i32, offset: i32) -> Option<DoorsClosed> {
        if c < BLINK_UNTIL {
            let colour = if (c / BLINK_PHASE) % 2 == 0 {
                self.olive[0]
            } else {
                self.green[0]
            };
            slots::palette4(layer, STRIP_ENTRY + self.served, &[colour]);
        }
        if c < STARS_UNTIL {
            let frame = ((c / STAR_FRAME_TICKS) % STAR_FRAMES) as i16;
            for (i, &m) in self.marks.iter().enumerate().skip(1) {
                match m {
                    1 => slots::image(layer, SLOT_STARS + i, STAR_SILVER + frame),
                    2 => slots::image(layer, SLOT_STARS + i, STAR_GOLD + frame),
                    _ => {}
                }
            }
        }
        if c == offset + REMOVE_AT {
            slots::image(layer, SLOT_STRIP, -1);
            slots::image(layer, SLOT_DIAGRAM, -1);
            for i in 0..STAR_SLOTS {
                slots::clear(layer, SLOT_STARS + i);
            }
        }
        (c == offset + DOORS_AT).then_some(DoorsClosed {
            name_id: self.name_id,
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ddg_formats::cab_hud::{CabTables, Place, STAR_ROWS, VehicleRow, WideSprite};
    use ddg_formats::cg::Palette;

    pub(crate) fn cab() -> CabTables {
        CabTables {
            plates: [Place {
                slot: 0x83,
                x: 0,
                y: 0,
                layer: 2,
                palette: 4,
            }; 3],
            station_plates: [-1; ddg_formats::hud::STATION_NAME_COUNT],
            vehicles: [VehicleRow {
                label: 0x261,
                name: 0x268,
                picture: 0x68,
            }; ddg_formats::cab_hud::VEHICLE_ROWS],
            speed_mode: [1; 20],
            atc_bar: [
                0x4a, 0x49, 0x48, 0x47, 0x46, 0x45, 0x44, 0x43, 0x42, 0x41, 0x40,
            ],
            caption_words: [0x6a, 0x11c, 0x11e, 0x120, 0x6c, 0x6e, -1],
            caption_station: 0x70,
            banner_words: [0x6b, 0x11d, 0x11f, 0x121, 0x6d, 0x6f, -1],
            banner_station: 0x71,
            landmarks: [WideSprite {
                sprite: 0x222,
                width: 0xb0,
            }; ddg_formats::cab_hud::LANDMARK_COUNT],
            route_strips: vec![[0x327; ROUTE_STRIP_ENTRIES]; 20],
            route_strip_stage10: [0x318; ROUTE_STRIP_ENTRIES],
            strip_olive: [0x0080_8000; STRIP_COLOURS],
            strip_green: [0x00f0_0000; STRIP_COLOURS],
            strip_extra: [0; STRIP_COLOURS],
            star_offsets: [[0x40; STAR_SLOTS]; STAR_ROWS],
            caption_pos: (80, 112),
            time_up_advice_pos: (304, 344),
            dial_pos: (16, 345),
            digital_pos: (17, 432),
            lamp_pos: (80, 144),
            origin: (0, 0),
        }
    }

    fn layer() -> SpriteLayer {
        let mut l = SpriteLayer::default();
        l.palettes[4] = Some(Palette([0; 256]));
        l
    }

    fn entry(l: &SpriteLayer, i: usize) -> u32 {
        l.palettes[4].as_ref().unwrap().0[i]
    }

    #[test]
    fn strip_marks_served_segments_and_blinks_the_current_one() {
        let mut d = DepartureHud::new(&cab());
        let mut l = layer();
        d.stage_start(&mut l);
        let stops = [true, false, true, true, false, true];
        let marks = |st: usize| if st == 2 { 2 } else { 1 };
        d.segment_setup(
            &Segment {
                stage: 3,
                station: 3,
                stops: &stops,
                mark_at: &marks,
                name_ids: &[10, 11, 12, 13, 14, 15],
            },
            &mut l,
        );
        // Stations 0, 2, 3 stop: three served, entries 0x50..0x53 green, the rest olive.
        assert_eq!(entry(&l, 0x52), 0x00f0_0000);
        assert_eq!(entry(&l, 0x53), 0x0080_8000);
        assert_eq!(d.plates, (Some(13), Some(15)));
        assert_eq!(d.marks[..3], [0, 2, 1]);
        // Blink of entry 0x53: olive for counter 0..9, green 10..19, … until 60.
        d.update(&mut l, 5, 0);
        assert_eq!(entry(&l, 0x53), 0x0080_8000);
        d.update(&mut l, 15, 0);
        assert_eq!(entry(&l, 0x53), 0x00f0_0000);
        d.update(&mut l, 60, 0);
        assert_eq!(entry(&l, 0x53), 0x00f0_0000);
        // Stars: gold for mark 2 (slot 0x40), silver for mark 1 (slot 0x41), frame (c/5)%6.
        d.update(&mut l, 12, 0);
        assert_eq!(l.slots[0x40].tilemap, STAR_GOLD + 2);
        assert_eq!(l.slots[0x41].tilemap, STAR_SILVER + 2);
        assert_eq!(l.slots[0x3f].tilemap, -1);
    }

    #[test]
    fn strip_goes_at_offset_plus_100_and_doors_close_at_200() {
        let mut d = DepartureHud::new(&cab());
        let mut l = layer();
        let marks = |_| 0;
        d.segment_setup(
            &Segment {
                stage: 0,
                station: 0,
                stops: &[true, true],
                mark_at: &marks,
                name_ids: &[5, 6],
            },
            &mut l,
        );
        assert_eq!(l.slots[SLOT_STRIP].tilemap, 0x327);
        assert_eq!((l.slots[SLOT_STRIP].x, l.slots[SLOT_STRIP].y), (144, 136));
        assert_eq!(d.update(&mut l, 279, 180), None);
        assert_eq!(l.slots[SLOT_STRIP].tilemap, 0x327);
        assert_eq!(d.update(&mut l, 280, 180), None);
        assert_eq!(l.slots[SLOT_STRIP].tilemap, -1);
        assert_eq!(l.slots[SLOT_DIAGRAM].tilemap, -1);
        assert_eq!(
            d.update(&mut l, 380, 180),
            Some(DoorsClosed { name_id: Some(5) })
        );
    }
}

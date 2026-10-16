//! The driving HUD as sprite slots (docs/re/hud.md). Each tick the game fills a [`HudState`]
//! from the simulation and [`Hud::update`] sets the slots the way `Hud_Update` (0x40ff20),
//! `Clock_Draw` (0x417140), `Hud_Distance` (0x4162c0), `Hud_HighSpeed` (0x416d10) and
//! `Cab_Overlay` (0x434fd0) do. The timed elements (signals, signs, hints, captions, the
//! departure strip, popups) are in [`crate::cab`] and the modules it drives.

use crate::slots;
use anyhow::Result;
use ddg_formats::cab_hud::{CabTables, Place, VehicleRow};
use ddg_formats::cg;
use ddg_formats::exe::Exe;
use ddg_formats::hud::{HudTables, digits_blank_leading};
use ddg_render::sprites::{Anchor, Slot, SpriteBank, SpriteLayer};
use plates_en::EnglishPlates;
use std::collections::HashMap;

mod plates_en;

// Slots (hud.md §2).
const SLOT_CLOCK_LABEL: usize = 0x0a;
const SLOT_CLOCK: usize = 0x0b;
const SLOT_SCHED_LABEL: usize = 0x11;
const SLOT_SCHED: usize = 0x12;
const SLOT_REMAINING: usize = 0x18;
const SLOT_SECONDS: usize = 0x1b;
const SLOT_REMAINING_LABEL: usize = 0x1c;
const SLOT_VEHICLE_NAME: usize = 0x1d;
const SLOT_VEHICLE_PICTURE: usize = 0x1e;
const SLOT_BRAKE: usize = 0x1f;
const SLOT_DIST_HEADER: usize = 0x5a;
const SLOT_HIGH_SPEED: usize = 0x75;
const SLOT_ATC_SPEED: usize = 0x76;
const SLOT_ATC_LABEL: usize = 0x77;
const SLOT_ATC_BAR: usize = 0x78;
const SLOT_MASCON: usize = 0x7e;
const SLOT_SPEEDO: usize = 0x81;
const SLOT_LAMP: usize = 0x82;
const SLOT_PLATE_FROM: usize = 0x83;
const SLOT_PLATE_ARROW: usize = 0x84;
const SLOT_PLATE_NEXT: usize = 0x85;
const SLOT_GRADE_SIGN: usize = 0x5e;
const SLOT_SIGNAL: usize = 0x60;
/// Distance digits, most significant first (hud.md §3.4).
const SLOTS_DIST_CM: [usize; 7] = [0x7c, 0x7d, 0x20, 0x21, 0x22, 0x23, 0x24];

// Tilemaps (hud.md §3).
const TBL_CLOCK_LABEL: i16 = 0x206;
const TBL_NEXT_ARRIVAL: i16 = 0x207;
const TBL_NEXT_PASSING: i16 = 0x208;
const TBL_SECONDS: i16 = 0x40e;
const TBL_TRANSFER_MASKS: [i16; 2] = [0x40b, 0x40c];
const TBL_PROFESSIONAL: i16 = 0x2c7;
const TBL_DIAL: i16 = 0x54a;
/// The "888 km/h" seven-segment panel (digital speedometer, high-speed readout).
const TBL_SEVEN_SEGMENT: i16 = 0x53;
const TBL_ATC_LABEL: i16 = 0x3f;
/// ATC speed readouts `0x4b + index` (0x416d10).
const TBL_ATC_SPEED0: i16 = 0x4b;
const TBL_PLATE_ARROW: i16 = 0x4b8;
/// Cab-signal lamp dark / lit (0x434fd0).
const TBL_LAMP: [i16; 2] = [0x54e, 0x54f];
/// Distance headers, metres / centimetres: passing, before, within (short), within (over),
/// overrun (hud.md §3.4).
const TBL_DIST_HEADERS: [[i16; 2]; 5] = [
    [0x59, 0x551],
    [0x58, 0x4fe],
    [0x5a, 0x4fc],
    [0x5c, 0x4fd],
    [0x5e, 0x507],
];
/// Per-stage label rows (`0x46594c + 16·stage`, first word = 持ち時間 label).
const STAGE_LABELS_VA: u32 = 0x46594c;

// Palette slots. The same slot gives different colours on different art (hud.md §3), so they
// are named by number.
const PAL_4: u8 = 4;
const PAL_5: u8 = 5;
const PAL_6: u8 = 6;

// Two-handle positions (hud.md §2, 640×480 column of 0x466cf8.. tables).
const BRAKE_POS: (i16, i16) = (590, 16);
const MASCON_POS: (i16, i16) = (11, 16);

// Analog speedometer (hud.md §3.5).
const NEEDLE_PIVOT: (f64, f64) = (80.0, 409.0);
const NEEDLE_MODEL: [(f64, f64); 3] = [(4.0, -20.0), (0.0, -58.0), (-4.0, -20.0)];
const DIAL_MAX_KMH: f64 = 120.0;

/// Seven-segment digits: palette 4 entries 0xdb.. (3 digits × 7 segments), lit 0xff0000
/// (`0x555555 + 0xa9aaab`, green in the B, R, G byte order) or unlit 0x555555 (0x416d10,
/// 0x4350b0).
const SEGMENT_ENTRY: usize = 0xdb;
const SEGMENT_LIT: u32 = 0x00ff_0000;
const SEGMENT_UNLIT: u32 = 0x0055_5555;
/// High-speed readout hysteresis `0x856c84`: on at 120 km/h, off below 100 (0x416d10).
const HIGH_SPEED_ON_KMH: i32 = 0x78;
const HIGH_SPEED_OFF_KMH: i32 = 100;
/// ATC speeds by index `0x856cf4` (stack table in 0x416d10), km/h.
const ATC_SPEEDS: [i32; 8] = [0, 0x1e, 0x46, 0x6e, 0xa0, 0xd2, 0xf5, 0x113];
const ATC_BAR_MAX: i32 = 10;
/// Speedometer mode `0x856d34`: 2 = ATC section.
pub const SPEED_MODE_ATC: i32 = 2;
/// The cab-signal lamp stays 30 more frames after the wait (`0x829d14 = 0x1e`, 0x434fd0).
const LAMP_AFTER_TICKS: i32 = 0x1e;

/// Ticks per second of the stage clock.
const TICKS: i32 = 30;

/// Settings that change the HUD (eeprom options, menus.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DistanceReadout {
    Off,
    Centimetres,
    Metres,
}

/// The speedometer option `0x856354` (eeprom +0x334, settings item 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Speedometer {
    None,
    Analog,
    Digital,
}

/// What the HUD shows this tick, filled from the simulation.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HudState {
    pub power: u8,
    pub brake: u8,
    pub remaining_s: i32,
    pub pending_s: i32,
    /// The HUD task counter driving the ±1 s transfer rhythm (`task+0x34`).
    pub hud_counter: u32,
    pub clock: i32,
    pub scheduled: i32,
    pub clocks_visible: bool,
    /// The clocks are not redrawn this tick (`0x856c6c > 0` after a station is passed).
    pub clocks_frozen: bool,
    pub remaining_visible: bool,
    /// The driving panel (distance readout) is active (`0x856d0c`).
    pub panel_active: bool,
    /// The panel shows the distance (1) or the "Professional" plate (0) (`0x856c70`).
    pub panel_on: bool,
    pub next_is_stop: bool,
    /// Distance to the stop, m (`0x85708c`), and its rounded value (`0x85722c`).
    pub dist_m: f32,
    pub dist_round_m: i32,
    pub tolerance_m: i32,
    pub speed_kmh: f32,
    /// Station plates "from" and "next stop" (`0x856d4c`, `0x856c74`), tbl ids.
    pub plates: [Option<i16>; 2],
    /// Vehicle name and picture (`0x856cfe`, `0x856cfc`), shown while `0x856d5c == 1`.
    pub vehicle: Option<(i16, i16)>,
    /// Speedometer mode `0x856d34`: 0 none, 1 high-speed readout, 2 ATC; ATC speed index
    /// `0x856cf4`.
    pub speed_mode: i32,
    pub atc_index: i32,
    /// A navigator hint is up (`0x856d44 ≠ 0`).
    pub hint_up: bool,
    /// The departure wait (`0x855ff8 & 0x800`) and the driving flag (`& 0x200`).
    pub waiting: bool,
    pub train_driving: bool,
    /// The cab-signal lamp is lit (`0x829d16`).
    pub lamp_lit: bool,
}

pub struct Hud {
    tables: HudTables,
    vehicles: Vec<VehicleRow>,
    dial_pos: (i16, i16),
    digital_pos: (i16, i16),
    lamp_pos: (i16, i16),
    atc_bar: [i16; 11],
    plate_places: [Place; 3],
    station_plates: Vec<i16>,
    remaining_label: i16,
    pub distance: DistanceReadout,
    pub speedometer: Speedometer,
    /// A stage run is active (`0x855ff8 & 2`, set only by `Stage_Run` 0x4080d0 at 0x4081dd /
    /// 0x408b48). The attract demos set 0x204 instead (0x40e530, 0x40e75d), so `Cab_Overlay`
    /// never draws the speedometer there, whatever the option (menus.md §2.4).
    pub stage_run: bool,
    /// High-speed readout latch `0x856c84`.
    high_speed: bool,
    /// Cab_Overlay: the last draw was during the wait (`0x829d18`), and the lamp's frames left
    /// after it (`0x829d14`).
    overlay_waiting: bool,
    lamp_ticks: i32,
    /// Enhancement `wide_plates` ([`plates_en`]); `None` draws the original plates.
    english_plates: Option<EnglishPlates>,
    /// The vehicle picture's place in the layout table (moved left for wider plates).
    picture_pos: (i16, i16),
}

/// Enhancement `wide_hud` (docs/re/hud.md §6.4): the edge each driving-HUD slot keeps to on a
/// wide canvas. Left: handles in one-handle mode, clocks, speedometer, cab-signal lamp, ATC
/// panel and high-speed readout. Right: vehicle picture, station plates, brake in two-handle
/// mode, signal, signs and the distance panel. Everything else (remaining time, vehicle name,
/// captions, banners, messages, navigator, popups) stays centred. On a 4:3 canvas every
/// anchor is the same, so this changes nothing there.
pub fn anchor_slots(layer: &mut SpriteLayer) {
    const LEFT: [std::ops::RangeInclusive<usize>; 3] = [
        SLOT_CLOCK_LABEL..=SLOT_SCHED + 5,
        SLOT_HIGH_SPEED..=SLOT_ATC_BAR,
        SLOT_MASCON..=SLOT_LAMP,
    ];
    const RIGHT: [std::ops::RangeInclusive<usize>; 4] = [
        SLOT_VEHICLE_PICTURE..=SLOT_VEHICLE_PICTURE,
        SLOT_PLATE_FROM..=SLOT_PLATE_NEXT,
        SLOT_DIST_HEADER..=SLOT_DIST_HEADER,
        SLOT_GRADE_SIGN..=SLOT_SIGNAL,
    ];
    for (ranges, anchor) in [(&LEFT[..], Anchor::Left), (&RIGHT[..], Anchor::Right)] {
        for r in ranges {
            for i in r.clone() {
                layer.anchors[i] = anchor;
            }
        }
    }
    for i in SLOTS_DIST_CM {
        layer.anchors[i] = Anchor::Right;
    }
    // The brake gauge sits right in two-handle mode and left in one-handle mode.
    layer.anchors[SLOT_BRAKE] = if layer.slots[SLOT_BRAKE].x < 320 {
        Anchor::Left
    } else {
        Anchor::Right
    };
}

impl Hud {
    pub fn new(exe: &Exe<'_>, stage: usize) -> Result<Self> {
        let label = exe.reader_at(STAGE_LABELS_VA + 16 * stage as u32, 2, "HUD labels")?;
        let cab = CabTables::read(exe)?;
        Ok(Self::with_tables(
            HudTables::read(exe)?,
            &cab,
            label.u16(0)? as i16,
        ))
    }

    fn with_tables(tables: HudTables, cab: &CabTables, remaining_label: i16) -> Self {
        let picture_pos = tables
            .layout
            .iter()
            .map(|r| r.at_640())
            .find(|p| usize::try_from(p.slot) == Ok(SLOT_VEHICLE_PICTURE))
            .map_or((411, 16), |p| (p.x, p.y));
        Self {
            picture_pos,
            tables,
            vehicles: cab.vehicles.to_vec(),
            dial_pos: cab.dial_pos,
            digital_pos: cab.digital_pos,
            lamp_pos: cab.lamp_pos,
            atc_bar: cab.atc_bar,
            station_plates: cab.station_plates.to_vec(),
            plate_places: cab.plates,
            remaining_label,
            distance: DistanceReadout::Metres,
            speedometer: Speedometer::Analog,
            stage_run: true,
            high_speed: false,
            overlay_waiting: false,
            lamp_ticks: 0,
            english_plates: None,
        }
    }

    /// Switches the station plates to the horizontal layout ([`plates_en`]; a translation in
    /// the enhanced HUD layout), with `romaji` names by plate tilemap id (empty: kanji only). Call
    /// before [`Hud::create`]. Returns false (and keeps the original plates) when the art has no
    /// usable plates. Names whose kanji will be clipped at `ui_scale` are listed on stderr.
    pub fn use_horizontal_plates(
        &mut self,
        bank: &SpriteBank,
        romaji: &HashMap<u16, String>,
        ui_scale: usize,
    ) -> bool {
        let palette = self.plate_places[0].palette as u8;
        self.english_plates =
            EnglishPlates::new(bank, &self.tables, &self.station_plates, palette, romaji);
        if let Some(e) = &self.english_plates {
            let wide = e.too_wide(ui_scale);
            if !wide.is_empty() {
                eprintln!(
                    "station plates: kanji clipped at ui_scale {ui_scale} (plate, px): {wide:x?}"
                );
            }
        }
        self.english_plates.is_some()
    }

    pub fn tables(&self) -> &HudTables {
        &self.tables
    }

    /// Palettes set at init (GAME_INIT 0x40e910).
    pub fn install_palettes(exe: &Exe<'_>, layer: &mut SpriteLayer) -> Result<()> {
        for (slot, p) in cg::init_palettes(exe)? {
            layer.palettes[usize::from(slot)] = Some(p);
        }
        Ok(())
    }

    /// Places the static slots (`Hud_Create` 0x40f7e0), with no image yet.
    pub fn create(&self, layer: &mut SpriteLayer) {
        for row in &self.tables.layout {
            let p = row.at_640();
            let Some(slot) = usize::try_from(p.slot)
                .ok()
                .and_then(|s| layer.slots.get_mut(s))
            else {
                continue;
            };
            *slot = Slot::new(-1, p.x, p.y, p.palette as u8, p.layer as u8, true);
        }
        slots::image(layer, SLOT_REMAINING_LABEL, self.remaining_label);
        // Station plates from the 0x464c18 / 0x464c40 / 0x464c68 rows.
        for p in self.plate_places {
            if let Ok(slot) = usize::try_from(p.slot) {
                slots::clear(layer, slot);
                slots::place(layer, slot, p.x, p.y, p.layer as u8, p.palette as u8);
                if self.english_plates.is_some() {
                    // Parked off screen: the slots keep their state, the boxes show it.
                    let off = Slot::CLEAR;
                    slots::move_to(layer, slot, off.x, off.y);
                }
            }
        }
    }

    /// The vehicle row for a stage, or for `Vehicle_Set(0x1000 | n)` (row 20 + n, ATC mode)
    /// (0x410320): `(name, picture)`.
    pub fn vehicle(&self, code: u32) -> Option<(i16, i16)> {
        let row = if code & 0x1000 != 0 {
            20 + (code & 0xfff) as usize
        } else {
            code as usize
        };
        self.vehicles.get(row).map(|v| (v.name, v.picture))
    }

    /// The plate tilemap of a station-name id (`0x46645c[id]`, −1 = none).
    pub fn plate(&self, name_id: usize) -> Option<i16> {
        self.station_plates
            .get(name_id)
            .copied()
            .filter(|&t| t >= 0)
    }

    /// One HUD tick: `Hud_Update` then `Cab_Overlay`.
    pub fn update(&mut self, s: &HudState, layer: &mut SpriteLayer) {
        self.handles(s, layer);
        if !s.clocks_frozen {
            self.clocks(s, layer);
        }
        self.plates(s, layer);
        self.remaining(s, layer);
        self.vehicle_plate(s, layer);
        self.distance(s, layer);
        self.high_speed(s, layer);
        self.cab_overlay(s, layer);
    }

    fn set(layer: &mut SpriteLayer, slot: usize, tbl: i16, palette: u8) {
        let s = &mut layer.slots[slot];
        s.tilemap = tbl;
        s.palette = palette;
        s.keep = true;
    }

    fn handles(&self, s: &HudState, layer: &mut SpriteLayer) {
        let brake = self.tables.brake_sprites[usize::from(s.brake.min(10))];
        let mascon = self.tables.mascon_sprites[usize::from(s.power.min(5))];
        layer.slots[SLOT_BRAKE] = Slot::new(brake as i16, BRAKE_POS.0, BRAKE_POS.1, 4, 2, true);
        layer.slots[SLOT_MASCON] = Slot::new(mascon as i16, MASCON_POS.0, MASCON_POS.1, 4, 2, true);
    }

    /// 0x40f780: the plates drawn every tick (a plate id of −1 leaves the slot as it is).
    fn plates(&self, s: &HudState, layer: &mut SpriteLayer) {
        if let Some(t) = s.plates[0] {
            slots::image(layer, SLOT_PLATE_FROM, t);
        }
        slots::image(layer, SLOT_PLATE_ARROW, TBL_PLATE_ARROW);
        if let Some(t) = s.plates[1] {
            slots::image(layer, SLOT_PLATE_NEXT, t);
        }
        if let Some(e) = &self.english_plates {
            let shown = |slot: usize| Some(layer.slots[slot].tilemap).filter(|&t| t >= 0);
            let (from, next) = (shown(SLOT_PLATE_FROM), shown(SLOT_PLATE_NEXT));
            let l = layer.slots[SLOT_PLATE_ARROW].layer;
            let extra = plates_en::extra_width(layer);
            layer.anchored(Anchor::Right, |layer| e.draw(layer, l, from, next, extra));
        }
    }

    fn remaining(&self, s: &HudState, layer: &mut SpriteLayer) {
        if !s.remaining_visible {
            for i in 0..4 {
                layer.slots[SLOT_REMAINING + i].tilemap = -1;
            }
            layer.slots[SLOT_REMAINING_LABEL].tilemap = -1;
            return;
        }
        let palette = if s.remaining_s > 14 { PAL_5 } else { PAL_4 };
        let v = s.remaining_s.clamp(0, 999) as u32;
        let digits = [v / 100, v / 10 % 10, v % 10];
        let phase = s.hud_counter % 3;
        for (i, d) in digits.iter().enumerate() {
            let tbl = if s.pending_s != 0 && phase < 2 {
                TBL_TRANSFER_MASKS[phase as usize]
            } else {
                self.tables.time_digits[*d as usize] as i16
            };
            Self::set(layer, SLOT_REMAINING + i, tbl, palette);
        }
        Self::set(layer, SLOT_SECONDS, TBL_SECONDS, palette);
        Self::set(layer, SLOT_REMAINING_LABEL, self.remaining_label, PAL_4);
    }

    /// 0x40ff20: name and picture while `0x856d5c == 1`; the picture's palette from
    /// `0x431dc0`.
    fn vehicle_plate(&self, s: &HudState, layer: &mut SpriteLayer) {
        match s.vehicle {
            Some((name, picture)) => {
                slots::image(layer, SLOT_VEHICLE_NAME, name);
                if self.english_plates.is_some() {
                    // Room for the wider plates on its right (the same group, hud.md §6.4).
                    let (x, y) = self.picture_pos;
                    let extra = plates_en::extra_width(layer) as i16;
                    slots::move_to(layer, SLOT_VEHICLE_PICTURE, x - extra, y);
                }
                slots::image_palette(
                    layer,
                    SLOT_VEHICLE_PICTURE,
                    picture,
                    picture_palette(picture),
                );
            }
            None => {
                slots::image(layer, SLOT_VEHICLE_NAME, -1);
                slots::image(layer, SLOT_VEHICLE_PICTURE, -1);
            }
        }
    }

    fn clock_digits(&self, ticks: i32) -> [i16; 6] {
        let t = ticks.max(0);
        let (h, m, sec) = (t / (TICKS * 3600), t / (TICKS * 60) % 60, t / TICKS % 60);
        let plain = |d: i32| self.tables.clock_digits[d as usize % 10] as i16;
        let colon = |d: i32| self.tables.clock_colon_digits[d as usize % 10] as i16;
        [
            plain(h / 10 % 10),
            plain(h % 10),
            colon(m / 10),
            plain(m % 10),
            colon(sec / 10),
            plain(sec % 10),
        ]
    }

    fn clocks(&self, s: &HudState, layer: &mut SpriteLayer) {
        let all = [SLOT_CLOCK_LABEL, SLOT_SCHED_LABEL]
            .into_iter()
            .chain(SLOT_CLOCK..SLOT_CLOCK + 6)
            .chain(SLOT_SCHED..SLOT_SCHED + 6);
        if !s.clocks_visible {
            for slot in all {
                layer.slots[slot].tilemap = -1;
            }
            return;
        }
        let late = s.clock >= s.scheduled + TICKS;
        let clock_pal = if late { PAL_4 } else { PAL_5 };
        Self::set(layer, SLOT_CLOCK_LABEL, TBL_CLOCK_LABEL, PAL_4);
        for (i, tbl) in self.clock_digits(s.clock).into_iter().enumerate() {
            Self::set(layer, SLOT_CLOCK + i, tbl, clock_pal);
        }
        let (label, sched_pal) = if s.next_is_stop {
            (TBL_NEXT_ARRIVAL, PAL_5)
        } else {
            (TBL_NEXT_PASSING, PAL_6)
        };
        Self::set(layer, SLOT_SCHED_LABEL, label, PAL_4);
        for (i, tbl) in self.clock_digits(s.scheduled).into_iter().enumerate() {
            Self::set(layer, SLOT_SCHED + i, tbl, sched_pal);
        }
    }

    fn distance(&self, s: &HudState, layer: &mut SpriteLayer) {
        for slot in SLOTS_DIST_CM {
            layer.slots[slot].tilemap = -1;
        }
        if !s.panel_active {
            layer.slots[SLOT_DIST_HEADER].tilemap = -1;
            return;
        }
        let header = &mut layer.slots[SLOT_DIST_HEADER];
        header.keep = true;
        header.y = 408;
        if self.distance == DistanceReadout::Off || !s.panel_on {
            header.x = 496;
            header.tilemap = TBL_PROFESSIONAL;
            return;
        }
        let cm = self.distance == DistanceReadout::Centimetres;
        let (value, tol, over_tol): (i64, i64, i64) = if cm {
            let tol = i64::from(s.tolerance_m);
            (
                ((f64::from(s.dist_m) * 100.0) as i64).max(-99_999),
                tol * 100 + 50,
                tol * 100 + 50,
            )
        } else {
            let tol = i64::from(s.tolerance_m);
            (i64::from(s.dist_round_m).max(-999), tol, tol)
        };
        let state = if !s.next_is_stop {
            0
        } else if value > tol {
            1
        } else if value >= 0 {
            2
        } else if value >= -over_tol {
            3
        } else {
            4
        };
        let digit_pal = match state {
            0 => PAL_6,
            4 => PAL_5,
            _ => PAL_4,
        };
        let header = &mut layer.slots[SLOT_DIST_HEADER];
        header.x = if cm { 472 } else { 496 };
        header.tilemap = TBL_DIST_HEADERS[state][usize::from(cm)];
        header.palette = PAL_4;
        let magnitude = value.unsigned_abs() as u32;
        let (slots, xs): (&[usize], &[i16]) = if cm {
            (&SLOTS_DIST_CM, &[496, 504, 520, 536, 552, 568, 584])
        } else {
            (&SLOTS_DIST_CM[2..], &[522, 536, 552, 568, 584])
        };
        let digits: Vec<Option<u8>> = if cm {
            digits_blank_leading::<7>(magnitude).to_vec()
        } else {
            digits_blank_leading::<5>(magnitude).to_vec()
        };
        for ((&slot, &x), d) in slots.iter().zip(xs).zip(digits) {
            let tbl = d.map_or(-1, |d| self.tables.distance_digits[usize::from(d)] as i16);
            layer.slots[slot] = Slot::new(tbl, x, 440, digit_pal, 2, true);
        }
    }

    /// Palette 4 entries 0xdb..0xef from the three digits of `kmh` (0x416d10 / 0x4350b0).
    pub fn segments(&self, kmh: i32, layer: &mut SpriteLayer) {
        let v = kmh.clamp(0, 999);
        let digits = [v / 100, v / 10 % 10, v % 10];
        let mut colours = [0u32; 21];
        for (d, digit) in digits.iter().enumerate() {
            let mask = self.tables.segment_masks[*digit as usize];
            for k in 0..7 {
                colours[d * 7 + k] = if mask & (1 << k) != 0 {
                    SEGMENT_LIT
                } else {
                    SEGMENT_UNLIT
                };
            }
        }
        slots::palette4(layer, SEGMENT_ENTRY, &colours);
    }

    /// `Hud_HighSpeed` 0x416d10: the digital readout beside the dial above 120 km/h on fast
    /// stages, and the ATC panel in an ATC section.
    fn high_speed(&mut self, s: &HudState, layer: &mut SpriteLayer) {
        let v = s.speed_kmh as i32;
        if self.speedometer == Speedometer::None || s.speed_mode == 0 {
            return;
        }
        if !self.high_speed {
            if v >= HIGH_SPEED_ON_KMH {
                self.high_speed = true;
            }
        } else if v < HIGH_SPEED_OFF_KMH {
            self.high_speed = false;
        }
        if self.speedometer == Speedometer::Digital {
            self.high_speed = false;
        }
        if s.speed_mode == SPEED_MODE_ATC {
            self.high_speed = true;
        }
        let shown = s.panel_active && !s.hint_up;
        self.segments(v, layer);
        let tbl = if shown && self.high_speed {
            TBL_SEVEN_SEGMENT
        } else {
            -1
        };
        slots::image(layer, SLOT_HIGH_SPEED, tbl);
        if shown && s.speed_mode == SPEED_MODE_ATC {
            let index = s.atc_index.clamp(0, ATC_SPEEDS.len() as i32 - 1);
            slots::image(layer, SLOT_ATC_SPEED, TBL_ATC_SPEED0 + index as i16);
            slots::image(layer, SLOT_ATC_LABEL, TBL_ATC_LABEL);
            let margin = (ATC_SPEEDS[index as usize] - v).clamp(0, ATC_BAR_MAX);
            slots::image(layer, SLOT_ATC_BAR, self.atc_bar[margin as usize]);
        } else {
            for slot in [SLOT_ATC_SPEED, SLOT_ATC_LABEL, SLOT_ATC_BAR] {
                slots::image(layer, slot, -1);
            }
        }
    }

    /// `Cab_Overlay` 0x434fd0 (once per drawn frame in the original, once per tick here): the
    /// speedometer while waiting or driving outside ATC sections, and the cab-signal lamp
    /// while waiting and 30 frames after.
    fn cab_overlay(&mut self, s: &HudState, layer: &mut SpriteLayer) {
        let active = self.stage_run && (s.waiting || s.train_driving);
        if !active {
            if self.overlay_waiting {
                self.speedometer_draw(s, layer);
                self.overlay_waiting = false;
            } else {
                slots::clear(layer, SLOT_SPEEDO);
            }
        } else if s.speed_mode != SPEED_MODE_ATC {
            self.speedometer_draw(s, layer);
            self.overlay_waiting = s.waiting;
        } else {
            // ATC sections draw the digital readout instead; the dial is not drawn.
            slots::clear(layer, SLOT_SPEEDO);
        }
        if s.waiting {
            self.lamp_ticks = LAMP_AFTER_TICKS;
        } else if self.lamp_ticks > 0 {
            self.lamp_ticks -= 1;
        }
        if s.waiting || self.lamp_ticks > 0 {
            let tbl = TBL_LAMP[usize::from(s.lamp_lit)];
            slots::set(layer, SLOT_LAMP, tbl, self.lamp_pos, PAL_4, 3);
        } else {
            slots::clear(layer, SLOT_LAMP);
        }
    }

    /// 0x4350b0.
    fn speedometer_draw(&self, s: &HudState, layer: &mut SpriteLayer) {
        match self.speedometer {
            Speedometer::None => {}
            Speedometer::Analog => {
                slots::set(layer, SLOT_SPEEDO, TBL_DIAL, self.dial_pos, PAL_4, 3);
                let points = needle(f64::from(s.speed_kmh));
                layer.anchored(Anchor::Left, |l| l.add_fan(&points, 0xff00_0000));
            }
            Speedometer::Digital => {
                self.segments(s.speed_kmh as i32, layer);
                slots::set(
                    layer,
                    SLOT_SPEEDO,
                    TBL_SEVEN_SEGMENT,
                    self.digital_pos,
                    PAL_4,
                    3,
                );
            }
        }
    }
}

/// `0x431dc0`: palette of a vehicle picture: 4 for 0x69, 10 for pictures 0–8, 0x67, 0x68 and
/// 0x184, else 9.
fn picture_palette(picture: i16) -> u8 {
    match picture {
        0x69 => 4,
        0..=8 | 0x67 | 0x68 | 0x184 => 10,
        _ => 9,
    }
}

/// The speedometer needle triangle for `kmh` (hud.md §3.5), in canvas pixels.
fn needle(kmh: f64) -> [[i32; 2]; 3] {
    let v = kmh.clamp(0.0, DIAL_MAX_KMH);
    let theta = v * std::f64::consts::PI * 0.0125 - std::f64::consts::PI * 0.75;
    let (sin, cos) = theta.sin_cos();
    NEEDLE_MODEL.map(|(x, y)| {
        [
            (x * cos - y * sin + NEEDLE_PIVOT.0) as i32,
            (x * sin + y * cos + NEEDLE_PIVOT.1) as i32,
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ddg_formats::cg::Palette;

    #[test]
    fn hud_groups_anchor_to_the_edges() {
        let mut l = SpriteLayer::default();
        l.slots[SLOT_BRAKE].x = BRAKE_POS.0;
        anchor_slots(&mut l);
        for s in [
            SLOT_CLOCK,
            SLOT_SCHED + 5,
            SLOT_MASCON,
            SLOT_SPEEDO,
            SLOT_ATC_BAR,
        ] {
            assert_eq!(l.anchors[s], Anchor::Left, "{s:#x}");
        }
        for s in [
            SLOT_PLATE_FROM,
            SLOT_VEHICLE_PICTURE,
            SLOT_SIGNAL,
            0x7c,
            0x24,
            SLOT_BRAKE,
        ] {
            assert_eq!(l.anchors[s], Anchor::Right, "{s:#x}");
        }
        // Remaining time, vehicle name, captions, messages, navigator: centred.
        for s in [
            SLOT_REMAINING,
            SLOT_VEHICLE_NAME,
            0x28,
            0x34,
            0x36,
            0x39,
            0x61,
        ] {
            assert_eq!(l.anchors[s], Anchor::Centre, "{s:#x}");
        }
        // One-handle mode puts the brake gauge on the left.
        l.slots[SLOT_BRAKE].x = 11;
        anchor_slots(&mut l);
        assert_eq!(l.anchors[SLOT_BRAKE], Anchor::Left);
    }

    #[test]
    fn needle_sweeps_from_down_left_to_down_right() {
        // 0 km/h: tip rotated by −135°, i.e. down-left of the pivot (y grows downward).
        let [_, tip0, _] = needle(0.0);
        assert!(tip0[0] < 80 && tip0[1] > 409);
        // 60 km/h: straight up.
        let [_, tip60, _] = needle(60.0);
        assert_eq!(tip60, [80, 351]);
        let [_, tip120, _] = needle(200.0);
        assert!(tip120[0] > 80 && tip120[1] > 409);
    }

    fn hud() -> Hud {
        let tables = HudTables {
            mode_sizes: [(640, 480); 4],
            layout: Vec::new(),
            time_digits: [0; 10],
            clock_digits: [0; 10],
            clock_colon_digits: [0; 10],
            distance_digits: [0; 10],
            brake_sprites: [0; 11],
            mascon_sprites: [0; 6],
            station_names: Vec::new(),
            stage_station_ids: Vec::new(),
            segment_masks: [0x77, 0x24, 0x5d, 0x6d, 0x2e, 0x6b, 0x7b, 0x25, 0x7f, 0x6f],
        };
        Hud::with_tables(tables, &crate::departure_hud::tests::cab(), 0x261)
    }

    fn layer() -> SpriteLayer {
        let mut l = SpriteLayer::default();
        l.palettes[4] = Some(Palette([0; 256]));
        l
    }

    fn state(kmh: f32) -> HudState {
        HudState {
            speed_kmh: kmh,
            speed_mode: 1,
            panel_active: true,
            train_driving: true,
            ..Default::default()
        }
    }

    #[test]
    fn high_speed_readout_has_hysteresis() {
        let mut h = hud();
        let mut l = layer();
        let mut shown = |h: &mut Hud, kmh: f32| {
            h.update(&state(kmh), &mut l);
            l.slots[SLOT_HIGH_SPEED].tilemap == TBL_SEVEN_SEGMENT
        };
        assert!(!shown(&mut h, 119.9));
        assert!(shown(&mut h, 120.0));
        assert!(shown(&mut h, 100.0));
        assert!(!shown(&mut h, 99.9));
        assert!(!shown(&mut h, 110.0));
    }

    #[test]
    fn segments_light_from_the_masks() {
        let h = hud();
        let mut l = layer();
        h.segments(128, &mut l);
        let p = l.palettes[4].as_ref().unwrap();
        // Digit 1 (mask 0x24): segments 2 and 5 lit.
        let first: Vec<bool> = (0..7)
            .map(|k| p.0[SEGMENT_ENTRY + k] == SEGMENT_LIT)
            .collect();
        assert_eq!(first, [false, false, true, false, false, true, false]);
        // Digit 8: all seven lit.
        assert!((0..7).all(|k| p.0[SEGMENT_ENTRY + 14 + k] == SEGMENT_LIT));
        assert_eq!(p.0[SEGMENT_ENTRY], SEGMENT_UNLIT);
    }

    #[test]
    fn no_speedometer_outside_a_stage_run() {
        // The attract demos never set `0x855ff8 & 2`: Cab_Overlay clears slot 0x81.
        let mut h = hud();
        h.stage_run = false;
        let mut l = layer();
        h.update(&state(80.0), &mut l);
        assert_eq!(l.slots[SLOT_SPEEDO].tilemap, -1);
        h.stage_run = true;
        h.update(&state(80.0), &mut l);
        assert_eq!(l.slots[SLOT_SPEEDO].tilemap, TBL_DIAL);
    }

    #[test]
    fn digital_speedometer_and_hint_hides_high_speed() {
        let mut h = hud();
        h.speedometer = Speedometer::Digital;
        let mut l = layer();
        let s = HudState {
            hint_up: true,
            ..state(130.0)
        };
        h.update(&s, &mut l);
        assert_eq!(l.slots[SLOT_SPEEDO].tilemap, TBL_SEVEN_SEGMENT);
        assert_eq!((l.slots[SLOT_SPEEDO].x, l.slots[SLOT_SPEEDO].y), (17, 432));
        assert_eq!(l.slots[SLOT_HIGH_SPEED].tilemap, -1);
    }

    #[test]
    fn atc_panel_shows_speed_and_margin_bar() {
        let mut h = hud();
        let mut l = layer();
        let s = HudState {
            speed_mode: SPEED_MODE_ATC,
            atc_index: 4,
            ..state(155.0)
        };
        h.update(&s, &mut l);
        assert_eq!(l.slots[SLOT_HIGH_SPEED].tilemap, TBL_SEVEN_SEGMENT);
        assert_eq!(l.slots[SLOT_ATC_SPEED].tilemap, 0x4b + 4);
        assert_eq!(l.slots[SLOT_ATC_LABEL].tilemap, TBL_ATC_LABEL);
        // 160 − 155 = 5 km/h margin.
        assert_eq!(l.slots[SLOT_ATC_BAR].tilemap, 0x4a - 5);
        // No dial in an ATC section.
        assert_eq!(l.slots[SLOT_SPEEDO].tilemap, -1);
    }

    #[test]
    fn lamp_shows_while_waiting_and_thirty_ticks_after() {
        let mut h = hud();
        let mut l = layer();
        let waiting = HudState {
            waiting: true,
            train_driving: false,
            ..state(0.0)
        };
        h.update(&waiting, &mut l);
        assert_eq!(l.slots[SLOT_LAMP].tilemap, TBL_LAMP[0]);
        h.update(
            &HudState {
                lamp_lit: true,
                ..waiting
            },
            &mut l,
        );
        assert_eq!(l.slots[SLOT_LAMP].tilemap, TBL_LAMP[1]);
        let after = HudState {
            lamp_lit: true,
            ..state(0.0)
        };
        let shown = (0..40)
            .filter(|_| {
                h.update(&after, &mut l);
                l.slots[SLOT_LAMP].tilemap != -1
            })
            .count();
        assert_eq!(shown, 29);
    }

    #[test]
    fn frozen_clocks_keep_their_digits() {
        let mut h = hud();
        let mut l = layer();
        h.tables.clock_digits = [
            0x1f2, 0x1f4, 0x1f6, 0x1f8, 0x1fa, 0x1fc, 0x1fe, 0x200, 0x202, 0x204,
        ];
        let s = HudState {
            clocks_visible: true,
            clock: 5 * TICKS,
            ..state(0.0)
        };
        h.update(&s, &mut l);
        assert_eq!(l.slots[SLOT_CLOCK + 5].tilemap, 0x1fc);
        h.update(
            &HudState {
                clock: 6 * TICKS,
                clocks_frozen: true,
                ..s
            },
            &mut l,
        );
        assert_eq!(l.slots[SLOT_CLOCK + 5].tilemap, 0x1fc);
    }

    #[test]
    fn vehicle_rows_and_picture_palettes() {
        let h = hud();
        assert_eq!(h.vehicle(0), Some((0x268, 0x68)));
        assert_eq!(h.vehicle(0x1001), Some((0x268, 0x68)));
        assert_eq!(picture_palette(0x69), 4);
        assert_eq!(picture_palette(0x68), 10);
        assert_eq!(picture_palette(0x129), 9);
    }
}

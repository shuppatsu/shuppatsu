//! The driving HUD as sprite slots (docs/re/hud.md). Each tick the game fills a [`HudState`]
//! from the simulation and [`Hud::update`] sets the slots the way `Hud_Update` (0x40ff20),
//! `Clock_Draw` (0x417140), `Hud_Distance` (0x4162c0) and `Cab_Overlay` (0x434fd0) do.

use anyhow::Result;
use ddg_formats::cg;
use ddg_formats::exe::Exe;
use ddg_formats::hud::{HudTables, digits_blank_leading};
use ddg_render::sprites::{Slot, SpriteLayer};

// Slots (hud.md §2).
const SLOT_CLOCK_LABEL: usize = 0x0a;
const SLOT_CLOCK: usize = 0x0b;
const SLOT_SCHED_LABEL: usize = 0x11;
const SLOT_SCHED: usize = 0x12;
const SLOT_REMAINING: usize = 0x18;
const SLOT_SECONDS: usize = 0x1b;
const SLOT_REMAINING_LABEL: usize = 0x1c;
const SLOT_BRAKE: usize = 0x1f;
const SLOT_DIST_HEADER: usize = 0x5a;
const SLOT_MASCON: usize = 0x7e;
const SLOT_SPEEDO: usize = 0x81;
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
const DIAL_POS: (i16, i16) = (16, 345);
const NEEDLE_PIVOT: (f64, f64) = (80.0, 409.0);
const NEEDLE_MODEL: [(f64, f64); 3] = [(4.0, -20.0), (0.0, -58.0), (-4.0, -20.0)];
const DIAL_MAX_KMH: f64 = 120.0;

/// Ticks per second of the stage clock.
const TICKS: i32 = 30;

/// Settings that change the HUD (eeprom options, menus.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DistanceReadout {
    Off,
    Centimetres,
    Metres,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // set from the options screen later
pub enum Speedometer {
    None,
    Analog,
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
}

pub struct Hud {
    tables: HudTables,
    remaining_label: i16,
    pub distance: DistanceReadout,
    pub speedometer: Speedometer,
}

impl Hud {
    pub fn new(exe: &Exe<'_>, stage: usize) -> Result<Self> {
        let label = exe.reader_at(STAGE_LABELS_VA + 16 * stage as u32, 2, "HUD labels")?;
        Ok(Self {
            tables: HudTables::read(exe)?,
            remaining_label: label.u16(0)? as i16,
            distance: DistanceReadout::Metres,
            speedometer: Speedometer::Analog,
        })
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
    }

    /// One HUD tick.
    pub fn update(&self, s: &HudState, layer: &mut SpriteLayer) {
        self.handles(s, layer);
        self.remaining(s, layer);
        self.clocks(s, layer);
        self.distance(s, layer);
        self.speedometer(s, layer);
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

    fn speedometer(&self, s: &HudState, layer: &mut SpriteLayer) {
        if self.speedometer != Speedometer::Analog {
            layer.slots[SLOT_SPEEDO].tilemap = -1;
            return;
        }
        layer.slots[SLOT_SPEEDO] = Slot::new(TBL_DIAL, DIAL_POS.0, DIAL_POS.1, 4, 3, true);
        layer.add_fan(&needle(f64::from(s.speed_kmh)), 0xff00_0000);
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
}

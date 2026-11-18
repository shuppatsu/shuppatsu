//! The coupling minigame's screen (`Coupling_Run` 0x4093c0, docs/re/rules.md §7.5): the two
//! captions, the speed and distance readouts, the distance bar, the result plate and the timer
//! on the remaining-time digits. The state comes from [`shuppatsu_engine::coupling::Coupling`]; the
//! positions and tilemap ids are read from the exe for resolution mode 1 (640×480).

use anyhow::Result;
use shuppatsu_engine::coupling::{
    CAPTION_TICK, Coupling, CouplingEvent, CouplingPhase, CouplingResult, SETUP_TICK,
};
use shuppatsu_engine::rules::RuleEvent;
use shuppatsu_formats::exe::Exe;
use shuppatsu_formats::hud::{MODE_640, RES_MODES};
use shuppatsu_render::sprites::{Slot, SpriteLayer};

use crate::slots;

const WHAT: &str = "coupling tables";

/// Slots 0x3f..=0x4b: `{x, y}` × 4 resolution modes each (0x458fd0, i32).
const SLOT_POSITIONS: u32 = 0x458fd0;
const FIRST_SLOT: usize = 0x3f;
const SLOT_COUNT: usize = 13;
/// The distance unit sprite's position when it is moved (0x459170, same shape).
const UNIT_POSITION: u32 = 0x459170;
/// Caption positions (0x459190 for tbl 0x2d8, 0x4591b0 for 0x2d9).
const CAPTION_POSITIONS: [u32; 2] = [0x459190, 0x4591b0];
/// Speed digits (0x458f50) and distance digits (0x458f78), 10 × i32 tilemap ids.
const SPEED_DIGITS: u32 = 0x458f50;
const DIST_DIGITS: u32 = 0x458f78;
/// The HUD's remaining-time digits (0x4653d4), used by the timer routine 0x417040.
const TIME_DIGITS: u32 = 0x4653d4;

/// Captions: ボーナスゲーム / 時間内に衝撃なく列車を連結せよ, `0x417de0(0x33, tbl, x, y, 4,
/// ticks, 10, 0)`.
const SLOT_CAPTION: usize = 0x33;
const CAPTIONS: [(i16, i32); 2] = [(0x2d8, 0x46), (0x2d9, 0x50)];
const CAPTION_DELAY: i32 = 10;
/// Static pictures of slots 0x45..=0x4a set at the setup (the meter frame and bar).
const STATIC_PICTURES: [(usize, i16); 6] = [
    (0x45, 0x17c),
    (0x46, 0x17d),
    (0x47, 0x17e),
    (0x48, 0x181),
    (0x49, 0x182),
    (0x4a, 0x183),
];
/// Speed readout slots (tens, units), distance digits, distance unit, the lamp and the bar.
const SLOT_SPEED: [usize; 2] = [0x3f, 0x40];
const SLOT_DIST: [usize; 3] = [0x41, 0x42, 0x43];
const SLOT_UNIT: usize = 0x44;
const SLOT_LAMP: usize = 0x47;
const SLOT_BAR: usize = 0x49;
const SLOT_RESULT: usize = 0x4b;
const TBL_UNIT: i16 = 0x171;
/// The lamp: steady at 10 m and more, blinking red closer; green / red with the result.
const TBL_LAMP_FAR: i16 = 0x17e;
const TBL_LAMP_NEAR: i16 = 0x180;
const TBL_LAMP_OK: i16 = 0x17f;
const TBL_SUCCESS: i16 = 0x2dc;
const TBL_FAILURE: i16 = 0x2dd;
/// The unit sprite moves right by this much from 10 m (`+ 0x28`).
const UNIT_SHIFT: i16 = 0x28;
/// The distance readout is hidden beyond 17 m (f64 at 0x459460).
const READOUT_MAX_M: f32 = 17.0;
/// Bar: `x = base − 272 + min(272, trunc(16 · dist))` (f64 16 at 0x458be0, 272 at 0x459458).
const BAR_SCALE: f64 = 16.0;
const BAR_SPAN: i32 = 0x110;
/// The success plate goes at outro tick 120.
const SUCCESS_PLATE_UNTIL: u32 = 120;
/// Timer slots 0x18–0x1a, masks and the 秒 tail (0x417040).
const SLOT_TIMER: usize = 0x18;
const TBL_TIMER_MASKS: [i16; 2] = [0x40b, 0x40c];
const TBL_TIMER_TAIL: i16 = 0x40e;
const LAYER: u8 = 3;
const PALETTE: u8 = 4;
const TIMER_PALETTE: u8 = 5;

#[derive(Debug, Clone, PartialEq)]
pub struct CouplingTables {
    slots: [(i16, i16); SLOT_COUNT],
    unit: (i16, i16),
    captions: [(i16, i16); 2],
    speed_digits: [i16; 10],
    dist_digits: [i16; 10],
    time_digits: [i16; 10],
}

impl CouplingTables {
    pub fn read(exe: &Exe<'_>) -> Result<Self> {
        let pos = |va: u32| -> Result<(i16, i16)> {
            let r = exe.reader_at(va, 8 * RES_MODES, WHAT)?;
            Ok((r.i32(8 * MODE_640)? as i16, r.i32(8 * MODE_640 + 4)? as i16))
        };
        let ids = |va: u32| -> Result<[i16; 10]> {
            let r = exe.reader_at(va, 40, WHAT)?;
            let mut out = [0; 10];
            for (i, v) in out.iter_mut().enumerate() {
                *v = r.i32(4 * i)? as i16;
            }
            Ok(out)
        };
        let mut slots = [(0, 0); SLOT_COUNT];
        for (i, s) in slots.iter_mut().enumerate() {
            *s = pos(SLOT_POSITIONS + 0x20 * i as u32)?;
        }
        Ok(Self {
            slots,
            unit: pos(UNIT_POSITION)?,
            captions: [pos(CAPTION_POSITIONS[0])?, pos(CAPTION_POSITIONS[1])?],
            speed_digits: ids(SPEED_DIGITS)?,
            dist_digits: ids(DIST_DIGITS)?,
            time_digits: ids(TIME_DIGITS)?,
        })
    }

    fn slot(&self, slot: usize) -> (i16, i16) {
        self.slots[slot - FIRST_SLOT]
    }
}

/// A `0x417de0` popup: `delay` updates hidden, then shown for `ticks` more.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Caption {
    tbl: i16,
    delay: i32,
    ticks: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CouplingHud {
    t: CouplingTables,
    caption: Option<Caption>,
    /// The minigame's sprites are placed (from intro tick 10 to the cleanup).
    active: bool,
    /// The 3D view's black bottom band is up (`0x84c0f8`, see [`Self::bottom_band`]).
    band: bool,
    result: Option<CouplingResult>,
}

impl CouplingHud {
    pub fn new(t: CouplingTables) -> Self {
        Self {
            t,
            caption: None,
            active: false,
            band: false,
            result: None,
        }
    }

    /// `0x84c0f8 = 1`: from the slot setup (intro tick 10) to the cleanup, or until a too-weak
    /// coupling hides the meter, the render task draws the depth-0 black band 0x486ec4
    /// (virtual rows 268..400) over the 3D under the readout panel, and `Cab_Overlay` hides
    /// the speedometer.
    pub fn bottom_band(&self) -> bool {
        self.band
    }

    /// Whether the HUD's own remaining-time readout is off (`0x856d1c = 0` from the start of
    /// the minigame until the success bonus at outro tick 120).
    pub fn hides_remaining(&self, c: Option<&Coupling>) -> bool {
        match c.map(|c| c.phase) {
            Some(CouplingPhase::Intro(_) | CouplingPhase::Running) => true,
            Some(CouplingPhase::Outro(CouplingResult::Coupled, t)) => t <= SUCCESS_PLATE_UNTIL,
            Some(CouplingPhase::Outro(..)) => true,
            _ => false,
        }
    }

    /// One tick, after the HUD task drew (the minigame's sprites and its timer go on top).
    /// `c` is the minigame while the rules are in it.
    pub fn update(&mut self, c: Option<&Coupling>, events: &[RuleEvent], layer: &mut SpriteLayer) {
        for e in events {
            match e {
                RuleEvent::CouplingStarted => {
                    self.result = None;
                    self.show_caption(layer, 0);
                }
                RuleEvent::Coupling(CouplingEvent::Result(r)) => self.result = Some(*r),
                _ => {}
            }
        }
        self.caption_step(layer);
        let Some(c) = c else {
            if self.active {
                self.clear(layer);
            }
            return;
        };
        match c.phase {
            CouplingPhase::Intro(t) => {
                // The phase already counts this tick.
                let done = t.saturating_sub(1);
                if done == SETUP_TICK {
                    self.setup(layer);
                }
                if done == CAPTION_TICK {
                    self.show_caption(layer, 1);
                }
                self.timer(layer, c.timer);
            }
            CouplingPhase::Running => {
                self.readouts(c, layer);
                self.timer(layer, c.timer);
            }
            CouplingPhase::Outro(r, t) => {
                if t == 1 {
                    self.readouts(c, layer);
                    let (lamp, plate) = if r == CouplingResult::Coupled {
                        (TBL_LAMP_OK, TBL_SUCCESS)
                    } else {
                        (TBL_LAMP_NEAR, TBL_FAILURE)
                    };
                    if r != CouplingResult::TimeUp {
                        slots::image(layer, SLOT_LAMP, lamp);
                        slots::image(layer, SLOT_RESULT, plate);
                    }
                    if r == CouplingResult::TooWeak {
                        // 0x4093c0 hides the meter (not the plate) before its 100-tick wait
                        // and clears `0x84c0f8`.
                        self.band = false;
                        for s in (0x45..=0x4a).chain(0x3f..=0x44) {
                            slots::image(layer, s, -1);
                        }
                    }
                }
                if r == CouplingResult::Coupled && t == SUCCESS_PLATE_UNTIL + 1 {
                    slots::image(layer, SLOT_RESULT, -1);
                }
                if t <= SUCCESS_PLATE_UNTIL || r != CouplingResult::Coupled {
                    self.timer(layer, c.timer);
                }
            }
            CouplingPhase::Done(_) => {
                if self.active {
                    self.clear(layer);
                }
            }
        }
        if self.active {
            // The partner train (slot 0x49) runs up to 80 px past the strip's end at x 560;
            // the original's screen cuts it at x 640. On the wide canvas of the enhanced
            // layout the panel stays that 4:3 picture, over `wide_coupling_band` too: the art's
            // own end (x 832 at 17 m) cuts through a car and a wheel, never shown originally.
            layer.frame(FIRST_SLOT..FIRST_SLOT + SLOT_COUNT);
        }
    }

    fn show_caption(&mut self, layer: &mut SpriteLayer, i: usize) {
        let (tbl, ticks) = CAPTIONS[i];
        let (x, y) = self.t.captions[i];
        slots::clear(layer, SLOT_CAPTION);
        slots::place(layer, SLOT_CAPTION, x, y, LAYER, PALETTE);
        self.caption = Some(Caption {
            tbl,
            delay: CAPTION_DELAY,
            ticks,
        });
    }

    fn caption_step(&mut self, layer: &mut SpriteLayer) {
        let Some(p) = self.caption.as_mut() else {
            return;
        };
        if p.delay > 0 {
            p.delay -= 1;
        } else if p.delay == 0 {
            p.delay = -1;
            slots::image(layer, SLOT_CAPTION, p.tbl);
        } else if p.ticks < 0 {
            slots::clear(layer, SLOT_CAPTION);
            self.caption = None;
        } else {
            p.ticks -= 1;
        }
    }

    /// Intro tick 10: the slots are placed (layer 3, palette 4) and the static pictures set.
    fn setup(&mut self, layer: &mut SpriteLayer) {
        for (i, &(x, y)) in self.t.slots.iter().enumerate() {
            slots::place(layer, FIRST_SLOT + i, x, y, LAYER, PALETTE);
        }
        for (slot, tbl) in STATIC_PICTURES {
            slots::image(layer, slot, tbl);
        }
        slots::image(layer, SLOT_SPEED[1], self.t.speed_digits[0]);
        self.active = true;
        self.band = true;
    }

    /// The readouts of one driving iteration (0x4097xx..0x409c4f).
    fn readouts(&self, c: &Coupling, layer: &mut SpriteLayer) {
        let d = c.dist_m;
        if c.timer > 0 {
            let lamp = if d >= 10.0 {
                TBL_LAMP_FAR
            } else if (c.timer / 10) % 2 == 0 {
                -1
            } else {
                TBL_LAMP_NEAR
            };
            slots::image(layer, SLOT_LAMP, lamp);
        }
        let kmh = c.speed_kmh.max(0) as usize;
        let tens = if kmh < 10 {
            -1
        } else {
            self.t.speed_digits[(kmh / 10) % 10]
        };
        slots::image(layer, SLOT_SPEED[0], tens);
        slots::image(layer, SLOT_SPEED[1], self.t.speed_digits[kmh % 10]);
        let dd = |v: f64| self.t.dist_digits[(v as i32).rem_euclid(10) as usize];
        let df = f64::from(d);
        if d >= 10.0 {
            if d > READOUT_MAX_M {
                for s in SLOT_DIST.into_iter().chain([SLOT_UNIT]) {
                    slots::image(layer, s, -1);
                }
            } else {
                let (x, y) = self.t.unit;
                slots::set(
                    layer,
                    SLOT_UNIT,
                    TBL_UNIT,
                    (x + UNIT_SHIFT, y),
                    PALETTE,
                    LAYER,
                );
                slots::image(layer, SLOT_DIST[0], dd(df / 10.0));
                slots::image(layer, SLOT_DIST[1], dd(df));
                slots::image(layer, SLOT_DIST[2], dd(df * 10.0));
            }
        } else {
            slots::set(layer, SLOT_UNIT, TBL_UNIT, self.t.unit, PALETTE, LAYER);
            slots::image(layer, SLOT_DIST[0], dd(df));
            slots::image(layer, SLOT_DIST[1], dd(df * 10.0));
            slots::image(layer, SLOT_DIST[2], dd(df * 100.0));
        }
        let bar = ((df * BAR_SCALE) as i32).clamp(0, BAR_SPAN);
        let (x, y) = self.t.slot(SLOT_BAR);
        slots::move_to(layer, SLOT_BAR, x - BAR_SPAN as i16 + bar as i16, y);
    }

    /// The timer routine 0x417040 on slots 0x18–0x1b.
    fn timer(&self, layer: &mut SpriteLayer, ticks: i32) {
        let t = ticks.max(0);
        let (secs, sub) = (t / 30, t % 30);
        let tbls = if sub < 26 {
            let s = secs.clamp(0, 999) as usize;
            [s / 100, s / 10 % 10, s % 10].map(|d| self.t.time_digits[d])
        } else if sub < 28 {
            [TBL_TIMER_MASKS[0]; 3]
        } else {
            [TBL_TIMER_MASKS[1]; 3]
        };
        for (i, tbl) in tbls.into_iter().enumerate() {
            slots::image_palette(layer, SLOT_TIMER + i, tbl, TIMER_PALETTE);
        }
        slots::image(layer, SLOT_TIMER + 3, TBL_TIMER_TAIL);
    }

    /// The cleanup of 0x4093c0: every minigame slot hidden.
    fn clear(&mut self, layer: &mut SpriteLayer) {
        for s in FIRST_SLOT..FIRST_SLOT + SLOT_COUNT {
            slots::clear(layer, s);
        }
        self.active = false;
        self.band = false;
    }
}

/// The upper edge (640×480 pixels) of the minigame's readout panel while it is up: the
/// highest of its slots 0x3f..=0x4a that shows a picture (the result plate 0x4b is not part
/// of it), or of its caption (slot 0x33) while that is up too, since the caption sits just
/// above the panel. The port's subtitles go above it (subtitles.rs).
pub fn panel_top(layer: &SpriteLayer) -> Option<f32> {
    let top = |s: &Slot| (s.tilemap >= 0 && s.y < Slot::CLEAR.y).then_some(f32::from(s.y));
    let panel = layer.slots[FIRST_SLOT..SLOT_RESULT]
        .iter()
        .filter_map(top)
        .reduce(f32::min)?;
    Some(top(&layer.slots[SLOT_CAPTION]).map_or(panel, |c| c.min(panel)))
}

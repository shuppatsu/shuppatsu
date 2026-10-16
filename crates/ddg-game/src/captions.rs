//! Station captions, station banners and landmark captions (docs/re/hud.md §3.13), and the
//! station-approach state `0x856d68` of 0x40c4e0 that creates them.
//!
//! * `StationCaption` 0x410450 → task 0x410620: small "name 駅 通過/停車", slots 0x36–0x38.
//! * `StationBanner` 0x410720 → task 0x410970: big centred banner, slots 0x39–0x3b.
//! * `Landmark_Show` 0x410a70 → task 0x410bd0: landmark caption, slot 0x3c.
//!
//! One task of each kind at a time; creating one replaces the last (its slots are cleared at
//! once). A task waits 6 updates, then draws every update (hidden while a time message is up)
//! until its tick count runs out or, in distance mode (kind flag 0x10; landmarks always), the
//! odometer has moved `distance` metres past the creation point. Cancelling (`id < 0`) ends it
//! after one more update; arrival or time-up (`0x856d00 = 0`) ends every task at its next
//! update.

use crate::slots;
use ddg_formats::cab_hud::{CabTables, LANDMARK_COUNT, WideSprite};
use ddg_formats::hud::{HudTables, StationNameSprite};
use ddg_render::sprites::SpriteLayer;

const SLOT_CAPTION: usize = 0x36;
const SLOT_BANNER: usize = 0x39;
const SLOT_LANDMARK: usize = 0x3c;
/// Updates before the first draw (`+0x38 = 6`, 0x410450 / 0x410720 / 0x410a70).
const DELAY: i32 = 6;
/// Kind flag: the duration is a distance in metres (0x410450 / 0x410720).
pub const DISTANCE: u32 = 0x10;
/// Caption / banner kinds 4 通過 and 5 停車 (words at 0x465500 / 0x465520).
pub const KIND_PASS: u32 = 4;
pub const KIND_STOP: u32 = 5;
/// Kind 6 has no word and no 駅.
const KIND_NONE: usize = 6;
/// Banner word widths by kind (stack table in 0x410720): 128 for the departure words 0–3,
/// 64 for 通過 / 停車 / none.
const BANNER_WORD_W: [i16; 7] = [0x80, 0x80, 0x80, 0x80, 0x40, 0x40, 0x40];
/// Banner and landmark centring: `x = origin + 0x40 + 0x100 − w / 2`, `y = origin + 0x28 + y`.
const CENTRE_X: i16 = 0x40;
const CENTRE_HALF: i16 = 0x100;
const TOP_Y: i16 = 0x28;
/// Width of 駅 before the word (0x410450 / 0x410720).
const STATION_WORD_W: i16 = 0x20;

/// The approach HUD (0x40c4e0, state 0x856d68): caption at 50 m into the leg (f32 50.0 at
/// 0x4594c8 against 0x84c40c), banner at `f5 + 150` m, caption again at `f5 + 50`, done at
/// `f5 − 10`, where `f5` is the home-signal distance (timetable field 5).
const LEG_M: f32 = 50.0;
const BANNER_AT_M: i32 = 0x96;
const BANNER_FOR_M: i32 = 100;
const CAPTION_AGAIN_AT_M: i32 = 0x32;
const CAPTION_AGAIN_FOR_M: i32 = 0xff;
const DONE_AT_M: i32 = -10;
/// Banner y argument of the approach (0xb0) and of the departure (0xb8), before `+ 0x28`.
const APPROACH_BANNER_Y: i16 = 0xb0;
pub const DEPARTURE_BANNER_Y: i16 = 0xb8;
/// Landmark y argument (course event 17, 0x40acd0).
const LANDMARK_Y: i16 = 0xd0;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Until {
    Ticks(i32),
    /// Odometer at creation (truncated) and distance, m.
    Distance {
        start: i32,
        metres: i32,
    },
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Task {
    delay: i32,
    until: Until,
    /// Images of the task's slots (name, 駅, word); −1 = none.
    images: [i16; 3],
}

impl Task {
    fn new(images: [i16; 3], kind: u32, duration: i32, odometer: f32) -> Self {
        let until = if kind & DISTANCE != 0 {
            Until::Distance {
                start: odometer as i32,
                metres: duration,
            }
        } else {
            Until::Ticks(duration)
        };
        Self {
            delay: DELAY,
            until,
            images,
        }
    }

    /// One update: draws into `slots`, returns false when the task ends.
    fn update(
        &mut self,
        layer: &mut SpriteLayer,
        slots: &[usize],
        hidden: bool,
        allowed: bool,
        odometer: f32,
    ) -> bool {
        if self.delay > 0 {
            self.delay -= 1;
            return true;
        }
        for (i, &slot) in slots.iter().enumerate() {
            slots::image(layer, slot, if hidden { -1 } else { self.images[i] });
        }
        let alive = match &mut self.until {
            Until::Ticks(t) => {
                let left = *t;
                *t -= 1;
                left >= 0
            }
            Until::Distance { start, metres } => odometer - (*start as f32) < *metres as f32,
            Until::Cancelled => false,
        };
        alive && allowed
    }
}

/// The exe tables the captions use.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptionDefs {
    names: Vec<StationNameSprite>,
    caption_words: [i16; 7],
    caption_station: i16,
    banner_words: [i16; 7],
    banner_station: i16,
    landmarks: [WideSprite; LANDMARK_COUNT],
    caption_pos: (i16, i16),
    origin: (i16, i16),
}

impl CaptionDefs {
    pub fn new(hud: &HudTables, cab: &CabTables) -> Self {
        Self {
            names: hud.station_names.clone(),
            caption_words: cab.caption_words,
            caption_station: cab.caption_station,
            banner_words: cab.banner_words,
            banner_station: cab.banner_station,
            landmarks: cab.landmarks,
            caption_pos: cab.caption_pos,
            origin: cab.origin,
        }
    }
}

/// The caption, banner and landmark tasks and the approach state.
#[derive(Debug, Clone, PartialEq)]
pub struct Captions {
    defs: CaptionDefs,
    caption: Option<Task>,
    banner: Option<Task>,
    landmark: Option<Task>,
    /// `0x856d00`: set when one is created, cleared at arrival and time-up.
    allowed: bool,
    /// `0x856d68`.
    approach: u8,
}

/// What the approach state machine reads (0x40c4e0, after 0x40dad0 of the same tick).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Approach {
    /// `0x84c40c` as 0x40c4e0 sees it: the train's leg distance before this tick's physics, 0
    /// when a station was passed this tick.
    pub leg_m: f32,
    /// Station-name id of the target station.
    pub name_id: Option<usize>,
    pub stops: bool,
    pub dist_m: f32,
    pub dist_round_m: i32,
    /// Home-signal distance of the target (timetable field 5).
    pub home_signal_m: i32,
    pub odometer: f32,
}

impl Captions {
    pub fn new(defs: CaptionDefs) -> Self {
        Self {
            defs,
            caption: None,
            banner: None,
            landmark: None,
            allowed: false,
            approach: 0,
        }
    }

    /// 0x40cf40 (segment reset) and 0x40d620 (station passed): the approach starts over.
    pub fn reset_approach(&mut self) {
        self.approach = 0;
    }

    /// Arrival or time-up: `0x856d00 = 0` and every creator called with −1.
    pub fn stop_all(&mut self) {
        self.allowed = false;
        for t in [&mut self.caption, &mut self.banner, &mut self.landmark]
            .into_iter()
            .flatten()
        {
            t.until = Until::Cancelled;
        }
    }

    /// Whether any caption task is alive (for tests and the shot tool).
    #[cfg(test)]
    fn any(&self) -> bool {
        self.caption.is_some() || self.banner.is_some() || self.landmark.is_some()
    }

    /// One step of the approach state (0x40c4e0), in the driving-loop body only.
    pub fn approach_step(&mut self, a: &Approach, layer: &mut SpriteLayer) {
        let kind = DISTANCE | if a.stops { KIND_STOP } else { KIND_PASS };
        let f5 = a.home_signal_m;
        let Some(id) = a.name_id else {
            return;
        };
        match self.approach {
            0 if a.leg_m >= LEG_M => {
                let metres = a.dist_round_m - (f5 + BANNER_AT_M);
                self.caption(layer, id, kind, metres, a.odometer);
                self.approach = 1;
            }
            1 if a.dist_round_m <= f5 + BANNER_AT_M => {
                self.banner(layer, id, kind, BANNER_FOR_M, APPROACH_BANNER_Y, a.odometer);
                self.approach = 2;
            }
            2 if a.dist_m <= (f5 + CAPTION_AGAIN_AT_M) as f32 => {
                self.caption(layer, id, kind, CAPTION_AGAIN_FOR_M, a.odometer);
                self.approach = 3;
            }
            3 if a.dist_m <= (f5 + DONE_AT_M) as f32 => self.approach = 4,
            _ => {}
        }
    }

    /// `StationCaption(id, kind, duration)` 0x410450 at the caption position (0x466ec8).
    pub fn caption(
        &mut self,
        layer: &mut SpriteLayer,
        id: usize,
        kind: u32,
        duration: i32,
        odometer: f32,
    ) {
        let Some(name) = self.defs.names.get(id).copied() else {
            return;
        };
        let k = (kind & 0xf) as usize;
        for s in SLOT_CAPTION..SLOT_CAPTION + 3 {
            slots::clear(layer, s);
        }
        let (x, y) = self.defs.caption_pos;
        let w = name.width as i16;
        slots::place(layer, SLOT_CAPTION, x, y, 3, 4);
        if k != KIND_NONE {
            slots::place(layer, SLOT_CAPTION + 1, x + w, y, 3, 4);
        }
        slots::place(layer, SLOT_CAPTION + 2, x + w + STATION_WORD_W, y, 3, 4);
        let images = [
            name.plain as i16,
            self.defs.caption_station,
            self.defs.caption_words.get(k).copied().unwrap_or(-1),
        ];
        self.caption = Some(Task::new(images, kind, duration, odometer));
        self.allowed = true;
    }

    /// `StationBanner(id, kind, duration, −1, y)` 0x410720, centred.
    pub fn banner(
        &mut self,
        layer: &mut SpriteLayer,
        id: usize,
        kind: u32,
        duration: i32,
        y: i16,
        odometer: f32,
    ) {
        let Some(name) = self.defs.names.get(id).copied() else {
            return;
        };
        let k = (kind & 0xf) as usize;
        for s in SLOT_BANNER..SLOT_BANNER + 3 {
            slots::clear(layer, s);
        }
        let w = name.width as i16;
        let word_w = BANNER_WORD_W.get(k).copied().unwrap_or(0x40);
        // The 駅 width is added unless the kind is 3 (a quirk of 0x410720's test).
        let total = w + word_w + if k != 3 { STATION_WORD_W } else { 0 };
        let origin = self.defs.origin;
        let mut x = origin.0 + CENTRE_X + CENTRE_HALF - total / 2;
        let y = origin.1 + TOP_Y + y;
        slots::place(layer, SLOT_BANNER, x, y, 3, 4);
        x += w;
        if k != KIND_NONE {
            slots::place(layer, SLOT_BANNER + 1, x, y, 3, 4);
            x += STATION_WORD_W;
        }
        slots::place(layer, SLOT_BANNER + 2, x, y, 3, 4);
        let images = [
            name.highlighted as i16,
            self.defs.banner_station,
            self.defs.banner_words.get(k).copied().unwrap_or(-1),
        ];
        self.banner = Some(Task::new(images, kind, duration, odometer));
        self.allowed = true;
    }

    /// `Landmark_Show(id, metres, −1, 0xd0)` 0x410a70 (course event 17): always distance mode.
    pub fn landmark(&mut self, layer: &mut SpriteLayer, id: i32, metres: i32, odometer: f32) {
        if id < 0 {
            if let Some(t) = self.landmark.as_mut() {
                t.until = Until::Cancelled;
            }
            return;
        }
        let Some(l) = usize::try_from(id)
            .ok()
            .and_then(|i| self.defs.landmarks.get(i).copied())
        else {
            return;
        };
        slots::clear(layer, SLOT_LANDMARK);
        slots::clear(layer, SLOT_LANDMARK + 1);
        let origin = self.defs.origin;
        let x = origin.0 + CENTRE_X + CENTRE_HALF - l.width / 2;
        let y = origin.1 + TOP_Y + LANDMARK_Y;
        slots::place(layer, SLOT_LANDMARK, x, y, 3, 4);
        self.landmark = Some(Task::new([l.sprite, -1, -1], DISTANCE, metres, odometer));
        self.allowed = true;
    }

    /// The three task updates. `message_up`: a time message is shown (`0x856d38 ≠ 0`).
    pub fn update(&mut self, layer: &mut SpriteLayer, message_up: bool, odometer: f32) {
        let allowed = self.allowed;
        let groups: [(&mut Option<Task>, &[usize]); 2] = [
            (
                &mut self.caption,
                &[SLOT_CAPTION, SLOT_CAPTION + 1, SLOT_CAPTION + 2],
            ),
            (
                &mut self.banner,
                &[SLOT_BANNER, SLOT_BANNER + 1, SLOT_BANNER + 2],
            ),
        ];
        for (t, group) in groups {
            if let Some(task) = t.as_mut()
                && !task.update(layer, group, message_up, allowed, odometer)
            {
                for &s in group {
                    slots::clear(layer, s);
                }
                *t = None;
            }
        }
        // The landmark task draws slot 0x3c only (and hides 0x3d with it under a message).
        if let Some(task) = self.landmark.as_mut() {
            let drew = task.delay == 0;
            let alive = task.update(layer, &[SLOT_LANDMARK], message_up, allowed, odometer);
            if drew && message_up {
                slots::image(layer, SLOT_LANDMARK + 1, -1);
            }
            if !alive {
                slots::clear(layer, SLOT_LANDMARK);
                slots::clear(layer, SLOT_LANDMARK + 1);
                self.landmark = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defs() -> CaptionDefs {
        CaptionDefs {
            names: vec![
                StationNameSprite {
                    plain: 0x100,
                    highlighted: 0x200,
                    width: 96,
                };
                4
            ],
            caption_words: [0x6a, 0x11c, 0x11e, 0x120, 0x6c, 0x6e, -1],
            caption_station: 0x70,
            banner_words: [0x6b, 0x11d, 0x11f, 0x121, 0x6d, 0x6f, -1],
            banner_station: 0x71,
            landmarks: [WideSprite {
                sprite: 0x222,
                width: 0xb0,
            }; LANDMARK_COUNT],
            caption_pos: (80, 112),
            origin: (0, 0),
        }
    }

    fn approach(leg_m: f32, dist_m: f32) -> Approach {
        Approach {
            leg_m,
            name_id: Some(1),
            stops: true,
            dist_m,
            dist_round_m: dist_m.round() as i32,
            home_signal_m: 300,
            odometer: 0.0,
        }
    }

    #[test]
    fn caption_waits_six_updates_and_ends_by_distance() {
        let mut c = Captions::new(defs());
        let mut layer = SpriteLayer::default();
        c.caption(&mut layer, 1, DISTANCE | KIND_STOP, 20, 100.4);
        for _ in 0..6 {
            c.update(&mut layer, false, 100.4);
            assert_eq!(layer.slots[SLOT_CAPTION].tilemap, -1);
        }
        c.update(&mut layer, false, 100.4);
        assert_eq!(layer.slots[SLOT_CAPTION].tilemap, 0x100);
        assert_eq!(layer.slots[SLOT_CAPTION + 1].tilemap, 0x70);
        assert_eq!(layer.slots[SLOT_CAPTION + 2].tilemap, 0x6e);
        assert_eq!(
            (
                layer.slots[SLOT_CAPTION + 2].x,
                layer.slots[SLOT_CAPTION + 2].y
            ),
            (80 + 96 + 32, 112)
        );
        // Start truncated to 100: shown while less than 20 m past it.
        c.update(&mut layer, false, 119.9);
        assert_eq!(layer.slots[SLOT_CAPTION].tilemap, 0x100);
        c.update(&mut layer, false, 120.0);
        assert_eq!(layer.slots[SLOT_CAPTION].tilemap, -1);
        assert!(!c.any());
    }

    #[test]
    fn tick_mode_shows_ticks_plus_one_updates() {
        let mut c = Captions::new(defs());
        let mut layer = SpriteLayer::default();
        c.banner(&mut layer, 1, 0, 3, DEPARTURE_BANNER_Y, 0.0);
        let shown: Vec<bool> = (0..12)
            .map(|_| {
                c.update(&mut layer, false, 0.0);
                layer.slots[SLOT_BANNER].tilemap != -1
            })
            .collect();
        assert_eq!(shown.iter().filter(|&&s| s).count(), 4);
        assert!(shown[6..10].iter().all(|&s| s));
    }

    #[test]
    fn banner_is_centred_with_its_word() {
        let mut c = Captions::new(defs());
        let mut layer = SpriteLayer::default();
        // Kind 5 (停車): 96 + 64 + 32 = 192 wide → x0 = 64 + 256 − 96 = 224.
        c.banner(&mut layer, 1, DISTANCE | KIND_STOP, 100, 0xb0, 0.0);
        assert_eq!(layer.slots[SLOT_BANNER].x, 224);
        assert_eq!(layer.slots[SLOT_BANNER].y, 216);
        assert_eq!(layer.slots[SLOT_BANNER + 1].x, 224 + 96);
        assert_eq!(layer.slots[SLOT_BANNER + 2].x, 224 + 96 + 32);
        // Kind 3 leaves the 駅 width out of the centring.
        c.banner(&mut layer, 1, 3, 10, 0xb8, 0.0);
        assert_eq!(layer.slots[SLOT_BANNER].x, 64 + 256 - (96 + 128) / 2);
        assert_eq!(layer.slots[SLOT_BANNER].y, 224);
    }

    #[test]
    fn a_time_message_hides_captions_but_time_runs() {
        let mut c = Captions::new(defs());
        let mut layer = SpriteLayer::default();
        c.caption(&mut layer, 1, 0, 10, 0.0);
        for _ in 0..7 {
            c.update(&mut layer, true, 0.0);
        }
        assert_eq!(layer.slots[SLOT_CAPTION].tilemap, -1);
        assert!(c.any());
    }

    #[test]
    fn arrival_ends_every_task() {
        let mut c = Captions::new(defs());
        let mut layer = SpriteLayer::default();
        c.landmark(&mut layer, 0, 500, 0.0);
        for _ in 0..7 {
            c.update(&mut layer, false, 0.0);
        }
        assert_eq!(layer.slots[SLOT_LANDMARK].tilemap, 0x222);
        assert_eq!(layer.slots[SLOT_LANDMARK].x, 64 + 256 - 0x58);
        c.stop_all();
        c.update(&mut layer, false, 0.0);
        assert!(!c.any());
        assert_eq!(layer.slots[SLOT_LANDMARK].tilemap, -1);
    }

    #[test]
    fn approach_creates_caption_banner_caption() {
        let mut c = Captions::new(defs());
        let mut layer = SpriteLayer::default();
        c.approach_step(&approach(49.0, 1000.0), &mut layer);
        assert!(!c.any());
        c.approach_step(&approach(50.0, 1000.0), &mut layer);
        assert_eq!(c.approach, 1);
        // The caption lasts until f5 + 150 = 450 m from the stop: 550 m from here.
        assert_eq!(
            c.caption.unwrap().until,
            Until::Distance {
                start: 0,
                metres: 550
            }
        );
        c.approach_step(&approach(60.0, 451.0), &mut layer);
        assert_eq!(c.approach, 1);
        c.approach_step(&approach(60.0, 450.0), &mut layer);
        assert_eq!(c.approach, 2);
        assert!(c.banner.is_some());
        c.approach_step(&approach(60.0, 350.0), &mut layer);
        assert_eq!(c.approach, 3);
        c.approach_step(&approach(60.0, 290.0), &mut layer);
        assert_eq!(c.approach, 4);
    }
}

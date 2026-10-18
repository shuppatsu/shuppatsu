//! Station captions, station banners and landmark captions (docs/re/hud.md §3.13), and the
//! station-approach state `0x856d68` of 0x40c4e0 that creates them.
//!
//! * `StationCaption` 0x410450 → task 0x410620: small "name 駅 通過/停車", slots 0x36–0x38.
//! * `StationBanner` 0x410720 → task 0x410970: big centred banner, slots 0x39–0x3b.
//! * `Landmark_Show` 0x410a70 → task 0x410bd0: landmark caption, slot 0x3c.
//!
//! With a translation (enhancement `one_line_captions`, docs/localisation.md), a caption or
//! banner is drawn as one line of text over the group's area instead of its three sprites
//! ([`OneLine`]); the tasks, their timing and their slots are unchanged.
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
use ddg_render::sprites::{Anchor, Paint, SpriteBank, SpriteLayer, TextStyle, WidgetKind};
use ddg_render::text::{Align, FontRole};
use std::collections::HashMap;

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
/// Layer and palette of every caption, banner and landmark slot (0x410450 / 0x410720).
const LAYER: u8 = 3;
const PALETTE: u8 = 4;
/// One-line text: the caption's line runs from its x to the same margin on the right; the
/// banner's line is centred, 40 px from each edge.
const BANNER_LINE_W: f32 = 560.0;
/// Latin capital height as a share of the original lettering height (as `ddg_render::text`).
const LATIN_CAP_OF_LETTERING: f32 = 0.75;

/// The colours and size of one caption set, read from its 駅 sprite.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Look {
    /// Lettering colour index; for plain art the commonest colour, outlined in the second;
    /// for art on a box (the banners), the commonest colour that is not the box's.
    letter: u8,
    outline: Option<u8>,
    /// The box colour of boxed art: most of the sprite's edge is opaque.
    backdrop: Option<u8>,
    /// Capital height and sprite height, original pixels.
    cap: f32,
    height: f32,
}

impl Look {
    fn of(bank: &SpriteBank, tbl: i16) -> Option<Self> {
        let (w, h, px) = bank.art(u16::try_from(tbl).ok()?)?;
        let mut counts = [0u32; 256];
        let mut edge = [0u32; 256];
        let mut edge_n = 0u32;
        for y in 0..h {
            for x in 0..w {
                let v = px[y * w + x];
                if v != 0 {
                    counts[usize::from(v)] += 1;
                }
                if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                    edge[usize::from(v)] += 1;
                    edge_n += 1;
                }
            }
        }
        let opaque_edge = edge_n - edge[0];
        let backdrop = (2 * opaque_edge > edge_n)
            .then(|| (1..=255u8).max_by_key(|&i| edge[usize::from(i)]))
            .flatten();
        let mut ranked: Vec<u8> = (1..=255u8)
            .filter(|&i| counts[usize::from(i)] > 0 && Some(i) != backdrop)
            .collect();
        ranked.sort_by_key(|&i| std::cmp::Reverse(counts[usize::from(i)]));
        let letter = *ranked.first()?;
        // The lettering's rows (the box, when there is one, spans them all).
        let (mut top, mut bottom) = (h, 0);
        for y in 0..h {
            if px[y * w..(y + 1) * w].contains(&letter) {
                top = top.min(y);
                bottom = bottom.max(y + 1);
            }
        }
        Some(Self {
            letter,
            outline: if backdrop.is_some() {
                None
            } else {
                ranked.get(1).copied()
            },
            backdrop,
            cap: bottom.saturating_sub(top) as f32 * LATIN_CAP_OF_LETTERING,
            height: h as f32,
        })
    }
}

/// Enhancement `one_line_captions`: a caption or banner as one translated line, e.g.
/// "Higashi-Kanagawa · Stop", at the original lettering's size (smaller only to fit), in the
/// caption's palette colours with its dark outline.
#[derive(Debug, Clone, PartialEq)]
pub struct OneLine {
    /// Station names by station-name id.
    names: HashMap<usize, String>,
    /// Phrases by kind 0–6, `{name}` replaced by the name.
    phrases: [Option<String>; 7],
    /// Caption (plain) and banner (highlighted) sets.
    looks: [Look; 2],
    /// Font roles of the two sets (`[caption]` `font` / `banner_font`).
    fonts: [FontRole; 2],
}

impl OneLine {
    /// `tbl_texts`: translated texts by tilemap id (the names of the name sprites); `phrases`:
    /// by kind; `fonts`: the roles of the captions and the banners. `None` when the 駅 sprites
    /// have no art.
    pub fn new(
        bank: &SpriteBank,
        defs: &CaptionDefs,
        tbl_texts: &HashMap<u16, String>,
        phrases: [Option<String>; 7],
        fonts: [FontRole; 2],
    ) -> Option<Self> {
        let names = defs
            .names
            .iter()
            .enumerate()
            .filter_map(|(id, n)| Some((id, tbl_texts.get(&n.plain)?.clone())))
            .collect();
        Some(Self {
            names,
            phrases,
            looks: [
                Look::of(bank, defs.caption_station)?,
                Look::of(bank, defs.banner_station)?,
            ],
            fonts,
        })
    }

    /// The line for station `id` and kind `k`; `banner` picks the highlighted set.
    fn line(
        &self,
        id: usize,
        k: usize,
        banner: bool,
        rect: [f32; 4],
        align: Align,
    ) -> Option<Line> {
        let name = self.names.get(&id)?;
        let text = self.phrases.get(k)?.as_ref()?.replace("{name}", name);
        let look = self.looks[usize::from(banner)];
        let [x, y, w, _] = rect;
        let paint = |index| Paint {
            palette: PALETTE,
            index,
        };
        // One pixel of margin for an outline; a box keeps the sprite's height.
        let m = if look.outline.is_some() { 1.0 } else { 0.0 };
        Some(Line {
            text,
            rect: [x - m, y - m, w + 2.0 * m, look.height + 2.0 * m],
            style: TextStyle {
                paint: paint(look.letter),
                cap: Some(look.cap),
                outline: look.outline.map(paint),
                backdrop: look.backdrop.map(paint),
                align,
                vertical: false,
                font: self.fonts[usize::from(banner)],
            },
            fill: None,
        })
    }
}

/// Enhancement `landmark_names`: a landmark caption keeps its original kanji plate, and the
/// translated name is drawn on a small line directly under it, in the plate's own colours read
/// from the art (white plate, dark lettering): the plate is extended downwards by the line,
/// and widened when the name is longer than the plate. Same task, so same timing and hiding.
#[derive(Debug, Clone, PartialEq)]
pub struct LandmarkLines {
    /// By landmark sprite tilemap.
    lines: HashMap<u16, LandmarkLine>,
}

#[derive(Debug, Clone, PartialEq)]
struct LandmarkLine {
    text: String,
    /// The plate's opaque columns `[x, w]` and the row under it, art pixels.
    plate: [u16; 2],
    bottom: u16,
    look: Look,
}

/// The name line under a landmark plate: capital height in original pixels, but at least
/// [`LANDMARK_LINE_MIN_CANVAS_CAP`] canvas pixels (so 8 at `ui_scale` 1); the line is
/// [`LANDMARK_LINE_PAD`] taller. The text is centred under the plate and at most
/// [`LANDMARK_LINE_MAX_W`] wide.
const LANDMARK_LINE_CAP: f32 = 6.5;
const LANDMARK_LINE_MIN_CANVAS_CAP: f32 = 8.0;
const LANDMARK_LINE_PAD: f32 = 4.5;
const LANDMARK_LINE_MAX_W: f32 = 300.0;

impl LandmarkLines {
    /// `names`: translated names by landmark sprite tilemap (`[landmark]`). Landmarks without
    /// a name or without art are left as they are.
    pub fn new(bank: &SpriteBank, defs: &CaptionDefs, names: &HashMap<u16, String>) -> Self {
        let mut lines = HashMap::new();
        for l in &defs.landmarks {
            let Ok(tbl) = u16::try_from(l.sprite) else {
                continue;
            };
            let (Some(text), Some((w, h, px)), Some(look)) =
                (names.get(&tbl), bank.art(tbl), Look::of(bank, l.sprite))
            else {
                continue;
            };
            let (mut x0, mut x1, mut y1) = (w, 0, 0);
            for y in 0..h {
                for x in 0..w {
                    if px[y * w + x] != 0 {
                        x0 = x0.min(x);
                        x1 = x1.max(x + 1);
                        y1 = y1.max(y + 1);
                    }
                }
            }
            if x0 >= x1 {
                continue;
            }
            lines.insert(
                tbl,
                LandmarkLine {
                    text: text.clone(),
                    plate: [x0 as u16, (x1 - x0) as u16],
                    bottom: y1 as u16,
                    look,
                },
            );
        }
        Self { lines }
    }

    /// The line under landmark sprite `tbl` drawn at `(x, y)`, on a canvas of `ui_scale` `s`.
    fn line(&self, tbl: i16, x: i16, y: i16, s: usize) -> Option<Line> {
        let cap = LANDMARK_LINE_CAP.max(LANDMARK_LINE_MIN_CANVAS_CAP / s.max(1) as f32);
        let h = cap + LANDMARK_LINE_PAD;
        let l = self.lines.get(&u16::try_from(tbl).ok()?)?;
        let paint = |index| Paint {
            palette: PALETTE,
            index,
        };
        let px = f32::from(x) + f32::from(l.plate[0]);
        let pw = f32::from(l.plate[1]);
        let top = f32::from(y) + f32::from(l.bottom);
        let centre = px + pw / 2.0;
        // No box in the art: the lettering colour with its outline instead.
        let outline = if l.look.backdrop.is_some() {
            None
        } else {
            l.look.outline
        };
        Some(Line {
            text: l.text.clone(),
            rect: [
                centre - LANDMARK_LINE_MAX_W / 2.0,
                top,
                LANDMARK_LINE_MAX_W,
                h,
            ],
            style: TextStyle {
                paint: paint(l.look.letter),
                cap: Some(cap),
                outline: outline.map(paint),
                backdrop: l.look.backdrop.map(paint),
                align: Align::Center,
                vertical: false,
                font: FontRole::Body,
            },
            fill: l.look.backdrop.map(|b| ([px, top, pw, h], paint(b))),
        })
    }
}

/// A one-line caption ready to draw.
#[derive(Debug, Clone, PartialEq)]
struct Line {
    text: String,
    rect: [f32; 4],
    style: TextStyle,
    /// A rectangle filled before the text (the landmark plate's extension).
    fill: Option<([f32; 4], Paint)>,
}

impl Line {
    fn draw(&self, layer: &mut SpriteLayer, anchor: Anchor) {
        layer.anchored(anchor, |layer| {
            if let Some((rect, paint)) = self.fill {
                layer.add_widget(LAYER, rect, WidgetKind::Fill(paint));
            }
            layer.add_styled_text(LAYER, self.rect, &self.text, self.style);
        });
    }
}

/// Enhancement `one_line_captions` outside the caption tasks: the ending's station plate
/// (0x417ab0: the caption set's name, 駅 and word sprites) as the in-game caption's one line,
/// centred. Its owner draws it every update while the plate shows.
#[derive(Debug, Clone, PartialEq)]
pub struct PlateLine(Line);

impl PlateLine {
    /// Draws the line, centred on the 640-wide screen like the plate's sprites.
    pub fn draw(&self, layer: &mut SpriteLayer) {
        self.0.draw(layer, Anchor::Centre);
    }
}

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

#[derive(Debug, Clone, PartialEq)]
struct Task {
    delay: i32,
    until: Until,
    /// Images of the task's slots (name, 駅, word); −1 = none.
    images: [i16; 3],
    /// The one-line text drawn instead of the images ([`OneLine`]).
    line: Option<Line>,
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
            line: None,
        }
    }

    /// The task shows `line` instead of its sprites (which stay empty).
    fn with_line(mut self, line: Option<Line>) -> Self {
        if line.is_some() {
            self.images = [-1; 3];
        }
        self.line = line;
        self
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
        if let Some(l) = self.line.as_ref().filter(|_| !hidden) {
            // Anchored with the task's slots (enhancement `wide_hud`).
            let anchor = slots.first().map_or(Anchor::Centre, |&s| layer.anchors[s]);
            l.draw(layer, anchor);
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
    pub(crate) names: Vec<StationNameSprite>,
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
    /// Enhancement `one_line_captions`; `None` draws the original sprites.
    one_line: Option<OneLine>,
    /// Enhancement `landmark_names`; `None` draws the landmark sprites alone.
    landmark_lines: Option<LandmarkLines>,
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
            one_line: None,
            landmark_lines: None,
        }
    }

    pub fn defs(&self) -> &CaptionDefs {
        &self.defs
    }

    /// Draws captions and banners as one translated line from now on ([`OneLine`]).
    pub fn use_one_line(&mut self, one_line: OneLine) {
        self.one_line = Some(one_line);
    }

    /// The one-line text for a caption-set plate of station `id` and `kind` (4 通過, 5 停車)
    /// with its sprites' top at `y`, centred on x = 320 ([`PlateLine`]). `None` keeps the
    /// sprites: no translation drawn, the setting off, or no phrase for the kind.
    pub fn plate_line(&self, id: usize, kind: u32, y: i16) -> Option<PlateLine> {
        let rect = [
            320.0 - BANNER_LINE_W / 2.0,
            f32::from(y),
            BANNER_LINE_W,
            0.0,
        ];
        let k = (kind & 0xf) as usize;
        self.one_line
            .as_ref()
            .and_then(|o| o.line(id, k, false, rect, Align::Center))
            .map(PlateLine)
    }

    /// Draws the translated landmark names under the landmark captions ([`LandmarkLines`]).
    pub fn use_landmark_lines(&mut self, lines: LandmarkLines) {
        self.landmark_lines = Some(lines);
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
        // Left-aligned from the caption's x, with the same margin on the right.
        let (fx, fy) = (f32::from(x), f32::from(y));
        let rect = [fx, fy, 640.0 - 2.0 * fx, 0.0];
        let line = self
            .one_line
            .as_ref()
            .and_then(|o| o.line(id, k, false, rect, Align::Left));
        self.caption = Some(Task::new(images, kind, duration, odometer).with_line(line));
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
        let centre = origin.0 + CENTRE_X + CENTRE_HALF;
        let mut x = centre - total / 2;
        let y = origin.1 + TOP_Y + y;
        let rect = [
            f32::from(centre) - BANNER_LINE_W / 2.0,
            f32::from(y),
            BANNER_LINE_W,
            0.0,
        ];
        let line = self
            .one_line
            .as_ref()
            .and_then(|o| o.line(id, k, true, rect, Align::Center));
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
        self.banner = Some(Task::new(images, kind, duration, odometer).with_line(line));
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
        let mut task = Task::new([l.sprite, -1, -1], DISTANCE, metres, odometer);
        // The kanji sprite stays; the name line is drawn with it.
        task.line = self
            .landmark_lines
            .as_ref()
            .and_then(|n| n.line(l.sprite, x, y, layer.scale));
        self.landmark = Some(task);
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
    fn landmark_name_goes_under_the_kanji_plate() {
        use ddg_render::sprites::WidgetKind;
        let look = Look {
            letter: 0x02,
            outline: None,
            backdrop: Some(0x0f),
            cap: 18.0,
            height: 32.0,
        };
        let mut c = Captions::new(defs());
        c.use_landmark_lines(LandmarkLines {
            lines: [(
                0x222,
                LandmarkLine {
                    text: "Akita Depot".into(),
                    plate: [8, 160],
                    bottom: 30,
                    look,
                },
            )]
            .into_iter()
            .collect(),
        });
        let mut layer = SpriteLayer {
            scale: 2,
            ..SpriteLayer::default()
        };
        c.landmark(&mut layer, 0, 500, 0.0);
        for _ in 0..6 {
            layer.reset_transient();
            c.update(&mut layer, false, 0.0);
            assert!(layer.widgets.is_empty());
        }
        layer.reset_transient();
        c.update(&mut layer, false, 0.0);
        // The kanji sprite is drawn as in the original.
        assert_eq!(layer.slots[SLOT_LANDMARK].tilemap, 0x222);
        let x = f32::from(64 + 256 - 0x58_i16) + 8.0;
        let y = f32::from(TOP_Y + LANDMARK_Y) + 30.0;
        let h = LANDMARK_LINE_CAP + LANDMARK_LINE_PAD;
        match &layer.widgets[..] {
            [fill, text] => {
                assert_eq!(fill.rect, [x, y, 160.0, h]);
                assert_eq!(
                    fill.kind,
                    WidgetKind::Fill(Paint {
                        palette: PALETTE,
                        index: 0x0f
                    })
                );
                let WidgetKind::Text { text: t, style } = &text.kind else {
                    panic!("{:?}", text.kind);
                };
                assert_eq!(t, "Akita Depot");
                assert_eq!(text.rect[0] + text.rect[2] / 2.0, x + 80.0);
                assert_eq!(style.cap, Some(LANDMARK_LINE_CAP));
                assert_eq!(style.paint.index, 0x02);
                assert_eq!(style.backdrop.map(|p| p.index), Some(0x0f));
            }
            w => panic!("{w:?}"),
        }
        // Hidden with the sprite under a time message.
        layer.reset_transient();
        c.update(&mut layer, true, 0.0);
        assert!(layer.widgets.is_empty());
        assert_eq!(layer.slots[SLOT_LANDMARK].tilemap, -1);
        // At `ui_scale` 1 the capitals stay 8 canvas pixels high.
        let lines = c.landmark_lines.as_ref().unwrap();
        let l = lines.line(0x222, 0, 0, 1).unwrap();
        assert_eq!(l.style.cap, Some(LANDMARK_LINE_MIN_CANVAS_CAP));
        assert!(lines.line(0x223, 0, 0, 2).is_none());
    }

    /// Against the user's data: every landmark has an English name and a white plate with dark
    /// lettering that the line extends.
    #[test]
    fn landmark_lines_from_the_data() {
        if std::env::var_os("DDG_DATA_DIR").is_none() {
            eprintln!("skipped: DDG_DATA_DIR not set");
            return;
        }
        let data = crate::scene::GameData::from_env().unwrap();
        let exe_bytes = data.exe().unwrap();
        let exe = ddg_formats::exe::Exe::parse(&exe_bytes).unwrap();
        let bank = SpriteBank::new(
            &ddg_formats::cg::CgFile::parse(&data.read("cg.rom").unwrap()).unwrap(),
            ddg_formats::cg::parse_tbl(&data.read("tbl.rom").unwrap()).unwrap(),
        );
        let defs = CaptionDefs::new(
            &HudTables::read(&exe).unwrap(),
            &CabTables::read(&exe).unwrap(),
        );
        let en = crate::locale::Catalogue::parse(include_str!("../../../locales/en.toml")).unwrap();
        let names: HashMap<u16, String> = en.landmarks.into_iter().collect();
        let lines = LandmarkLines::new(&bank, &defs, &names);
        assert_eq!(lines.lines.len(), LANDMARK_COUNT);
        for l in lines.lines.values() {
            let b = l.look.backdrop.expect("landmark art on a plate");
            assert_ne!(b, l.look.letter);
            assert!(l.plate[1] > 0 && l.bottom > 0);
        }
    }

    #[test]
    fn one_line_replaces_the_three_sprites() {
        use ddg_render::sprites::WidgetKind;
        let look = Look {
            letter: 0x1c,
            outline: Some(0x1d),
            backdrop: None,
            cap: 21.0,
            height: 32.0,
        };
        let boxed = Look {
            outline: None,
            backdrop: Some(0x30),
            ..look
        };
        let mut phrases: [Option<String>; 7] = Default::default();
        phrases[KIND_STOP as usize] = Some("{name} · Stop".into());
        let mut c = Captions::new(defs());
        c.use_one_line(OneLine {
            names: [(1, "Higashi-Kanagawa".to_owned())].into_iter().collect(),
            phrases,
            looks: [look, boxed],
            fonts: [FontRole::Body, FontRole::Display],
        });
        let mut layer = SpriteLayer::default();
        c.caption(&mut layer, 1, DISTANCE | KIND_STOP, 20, 0.0);
        let text = |l: &SpriteLayer| {
            l.widgets.iter().find_map(|w| match &w.kind {
                WidgetKind::Text { text, style } => Some((text.clone(), w.rect, *style)),
                _ => None,
            })
        };
        // Same timing: nothing for six updates, then the line; the sprites stay empty.
        for _ in 0..6 {
            layer.reset_transient();
            c.update(&mut layer, false, 0.0);
            assert!(text(&layer).is_none());
        }
        layer.reset_transient();
        c.update(&mut layer, false, 0.0);
        let (t, rect, style) = text(&layer).unwrap();
        assert_eq!(t, "Higashi-Kanagawa · Stop");
        assert_eq!(rect, [79.0, 111.0, 482.0, 34.0]);
        assert_eq!(style.align, Align::Left);
        assert_eq!((style.paint.index, style.cap), (0x1c, Some(21.0)));
        assert_eq!(style.outline.map(|p| p.index), Some(0x1d));
        assert_eq!(style.font, FontRole::Body);
        for s in SLOT_CAPTION..SLOT_CAPTION + 3 {
            assert_eq!(layer.slots[s].tilemap, -1);
        }
        // Hidden under a time message.
        layer.reset_transient();
        c.update(&mut layer, true, 0.0);
        assert!(text(&layer).is_none());
        // The banner: centred, on its box; a kind without a phrase keeps the sprites.
        c.banner(&mut layer, 1, DISTANCE | KIND_STOP, 100, 0xb0, 0.0);
        c.banner(&mut layer, 1, DISTANCE | KIND_PASS, 100, 0xb0, 0.0);
        for _ in 0..7 {
            layer.reset_transient();
            c.update(&mut layer, false, 0.0);
        }
        assert_eq!(layer.slots[SLOT_BANNER].tilemap, 0x200);
        c.banner(&mut layer, 1, DISTANCE | KIND_STOP, 100, 0xb0, 0.0);
        c.caption = None;
        for _ in 0..7 {
            layer.reset_transient();
            c.update(&mut layer, false, 0.0);
        }
        let (_, rect, style) = text(&layer).unwrap();
        assert_eq!(rect, [40.0, 216.0, 560.0, 32.0]);
        assert_eq!(style.backdrop.map(|p| p.index), Some(0x30));
        // Each set in its own font role.
        assert_eq!(style.font, FontRole::Display);
        assert_eq!(layer.slots[SLOT_BANNER].tilemap, -1);
    }

    /// The ending's station plate: the in-game caption's line (same text, size, colours and
    /// font), centred on the screen; no line without the enhancement or without a phrase.
    #[test]
    fn plate_line_is_the_caption_line_centred() {
        use ddg_render::sprites::WidgetKind;
        let look = Look {
            letter: 0x1c,
            outline: Some(0x1d),
            backdrop: None,
            cap: 21.0,
            height: 32.0,
        };
        let mut phrases: [Option<String>; 7] = Default::default();
        phrases[KIND_STOP as usize] = Some("{name} · Stop".into());
        phrases[KIND_PASS as usize] = Some("{name} · Pass".into());
        let mut c = Captions::new(defs());
        assert!(c.plate_line(1, KIND_STOP, 0x40).is_none());
        c.use_one_line(OneLine {
            names: [(1, "Ōmagari".to_owned())].into_iter().collect(),
            phrases,
            looks: [look, Look { cap: 9.0, ..look }],
            fonts: [FontRole::Body, FontRole::Display],
        });
        let text = |l: &SpriteLayer| match &l.widgets[..] {
            [w] => match &w.kind {
                WidgetKind::Text { text, style } => (text.clone(), w.rect, *style),
                k => panic!("{k:?}"),
            },
            w => panic!("{w:?}"),
        };
        let mut layer = SpriteLayer::default();
        c.caption(&mut layer, 1, DISTANCE | KIND_STOP, 20, 0.0);
        for _ in 0..7 {
            layer.reset_transient();
            c.update(&mut layer, false, 0.0);
        }
        let (_, _, caption) = text(&layer);
        layer.reset_transient();
        c.plate_line(1, KIND_STOP, 0x40).unwrap().draw(&mut layer);
        let (t, rect, style) = text(&layer);
        assert_eq!(t, "Ōmagari · Stop");
        assert_eq!(rect, [39.0, 63.0, 562.0, 34.0]);
        assert_eq!(rect[0] + rect[2] / 2.0, 320.0);
        assert_eq!(
            style,
            TextStyle {
                align: Align::Center,
                ..caption
            }
        );
        let pass = c.plate_line(1, KIND_PASS, 0x40).unwrap();
        assert_eq!(pass.0.text, "Ōmagari · Pass");
        assert!(c.plate_line(2, KIND_STOP, 0x40).is_none());
        assert!(c.plate_line(1, 3, 0x40).is_none());
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
            c.caption.as_ref().unwrap().until,
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

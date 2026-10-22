//! The ending (`Ending_Run` 0x4040a0, docs/re/menus.md §12.2): after the report card of a
//! cleared stage whose difficulty (`0x4656c4`) is above 3, before the ranking. The full ending
//! (0x404110, task 0x4042b0) tours the stage's stations with a station plate and a
//! scheduled/actual-time panel, then shows an oncoming AI train; the plain one (0x404920, task
//! 0x404a80) runs the camera forward from the first station. Both roll the staff credits
//! (text tables 0x45811c / 0x457a5c) between letterbox masks, and Enter or Esc skips them.
//!
//! The camera is the drive's: [`Drive::ending_place`] (0x408080), [`Drive::ending_tick`] with
//! `0x850208 = v × 0.000925` and [`Drive::ending_kill`] (list 5 killed). The masks are black
//! quads over the 3D band; the credits are sprites on top (text with `one_line_captions`:
//! `play/staff_roll.rs`).

use super::Play;
use super::staff_roll::{Bands, BankMeasure, CreditScreen, Shown, StaffRoll};
use crate::captions::{KIND_PASS, KIND_STOP, PlateLine};
use crate::front::attract::Fade;
use crate::screens::flow::WorldView;
use crate::slots;
use anyhow::Result;
use shuppatsu_engine::traffic::SpawnRequest;
use shuppatsu_formats::cg::Palette;
use shuppatsu_formats::exe::Exe;
use shuppatsu_formats::hud::{MODE_640, RES_MODES};
use shuppatsu_render::effects::EffectGeometry;
use shuppatsu_render::sprites::SpriteLayer;
use std::collections::HashMap;

const WHAT: &str = "ending tables";
/// Text tables: 0x30-byte records, one 12-byte sub-record per resolution mode
/// `{tbl (0 blank, −1 end), x, y, fade kind, n, duration}`.
const TEXT_FULL: u32 = 0x45811c;
const TEXT_PLAIN: u32 = 0x457a5c;
const TEXT_RECORD: u32 = 0x30;
const TEXT_MAX: u32 = 64;
/// Final shot per stage, 9 × i32 at 0x4587dc + 0x24·stage: station, jump m, camera km/h,
/// spawn, speed, param, decel, flags, model.
const FINAL_SHOT: u32 = 0x4587dc;
/// Station plates `{tbl, width}` by name id (0x4665c4), kind words (0x465500), 駅 (0x465518),
/// plate y (0x4676b4) and clock y (0x458aac) per resolution mode, digit sets A / B (0x465d8c /
/// 0x465dd0: digits 0–9, cm, m).
const PLATES: u32 = 0x4665c4;
const KIND_WORDS: u32 = 0x465500;
const STATION_WORD: u32 = 0x465518;
const PLATE_Y: u32 = 0x4676b4;
const CLOCK_Y: u32 = 0x458aac;
const DIGITS: [u32; 2] = [0x465d8c, 0x465dd0];
/// Fade palettes (title palette data): kind 4 → 0x488290, kind 11 → 0x488e90.
const FADE_PALETTES: [(i16, u32); 2] = [(4, 0x488290), (11, 0x488e90)];
const FADE_TICKS: i32 = 24;
pub(super) const PAL_TEXT: u8 = 0x17;
const SLOT_TEXT: usize = 0x6e;
const TEXT_SLOTS: usize = 5;

/// Music: 0x1a2 (full), 0x19f (plain); the plain destroy releases 0x1a2 (a game bug: 0x19f
/// plays on until Stage_Run's stop-all).
const BGM_FULL: u16 = 0x1a2;
const BGM_PLAIN: u16 = 0x19f;
/// Render mode 3 offsets (f32 0.04 / 0.06, 0x4254b0).
const EYE_DROP: f32 = 0.04;
const TARGET_DROP: f32 = 0.06;

/// km/h → track units per tick (f64 0.000925 at 0x458af0).
const KMH_TO_UNITS: f64 = 0.000925;
/// A one-tick camera jump of `m` metres: `v = m × 0.1 × 1081.081` (f64 0x44e8e0, 0x458b28).
const JUMP_SCALE: f64 = 0.1 * 1081.081;
/// Full ending timing (code immediates of 0x404110 / 0x4042b0).
const FULL_T0: i32 = -200;
const PERIOD: i32 = 160;
const FULL_KILL_AT: i32 = 0x10f4;
const FULL_END_AT: i32 = 0x11ee;
const FULL_SKIP_AFTER: i32 = -190;
const FINAL_BRAKE_AT: i32 = 0xfc8;
/// Opening run: +0.3 km/h per tick while 0 ≤ v ≤ 120 (f64 0x458af8, f32 0x458b00).
const OPEN_ACCEL: f32 = 0.3;
const OPEN_TOP: f32 = 120.0;
/// Stop approach: v = 21620/P, a = −21620/P² (f64 0x458b18); pass: v = f5 m per period
/// (0x458b20 = 1/160) and +0.2 per tick (0x458b30).
const STOP_K: f64 = 21620.0;
const PASS_PERIOD: f64 = 0.00625;
const PASS_ACCEL: f32 = 0.2;
/// The approach starts this far before a stop, m.
const STOP_LEAD_M: i32 = 100;
/// Plain ending: t from −160, +0.1 km/h per tick up to 110 (f32 0x458b40).
const PLAIN_T0: i32 = -160;
const PLAIN_ACCEL: f32 = 0.1;
const PLAIN_TOP: f32 = 110.0;
const PLAIN_KILL_AT: i32 = 0xdd4;
const PLAIN_END_AT: i32 = 0xfaa;
const PLAIN_SKIP_AFTER: i32 = -150;
/// Mask edges in the 496×400 virtual space: top 94; bottom 276 (full) / 268 (plain), 288
/// while a station shows; closed 194 / 188. The blank credit closes them by `C·progress`
/// (C = 100 / 90 from bases 288 / 268).
const MASK_TOP: i32 = 94;
const MASK_BOTTOM_FULL: i32 = 276;
const MASK_BOTTOM_PLAIN: i32 = 268;
const MASK_BOTTOM_OPEN: i32 = 288;
const MASK_CLOSED: (i32, i32) = (194, 188);
/// The 3D viewport is virtual y 52..400.
const VIEW_TOP: f32 = 52.0;
const VIEW_H: f32 = 348.0;
/// Panel slots: plate 0x36–0x38; clock 0x50–0x5f and label 0x60.
const SLOT_PLATE: usize = 0x36;
const SLOT_SCHED: usize = 0x50;
const SLOT_ACTUAL: usize = 0x56;
const SLOT_ERR_UNITS: usize = 0x5d;
const SLOT_ERR_TENS: usize = 0x5e;
const SLOT_ERR_GLYPH: usize = 0x5f;
const SLOT_LABEL: usize = 0x60;
const PANEL_SLOTS: std::ops::RangeInclusive<usize> = 0x50..=0x6d;
/// Labels (stop late / on time, pass late / on time).
const LABELS: [[i16; 2]; 2] = [[0x20d, 0x20b], [0x20c, 0x20a]];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TextRec {
    tbl: i16,
    x: i16,
    y: i16,
    kind: i16,
    n: i16,
    dur: i16,
}

#[derive(Debug, Clone)]
pub struct EndingTables {
    full: Vec<TextRec>,
    plain: Vec<TextRec>,
    finals: Vec<[i32; 9]>,
    plates: Vec<(i16, i16)>,
    kind_words: [i16; 7],
    station_word: i16,
    plate_y: i16,
    clock_y: i16,
    digits: [[i16; 12]; 2],
    fades: Vec<(i16, Palette)>,
}

impl EndingTables {
    pub fn read(exe: &Exe<'_>) -> Result<Self> {
        let text = |va: u32| -> Result<Vec<TextRec>> {
            let mut out = Vec::new();
            for i in 0..TEXT_MAX {
                let r = exe.reader_at(va + TEXT_RECORD * i + 12 * MODE_640 as u32, 12, WHAT)?;
                let rec = TextRec {
                    tbl: r.i16(0)?,
                    x: r.i16(2)?,
                    y: r.i16(4)?,
                    kind: r.i16(6)?,
                    n: r.i16(8)?,
                    dur: r.i16(10)?,
                };
                out.push(rec);
                if rec.tbl == -1 {
                    break;
                }
            }
            Ok(out)
        };
        let res = |va: u32| -> Result<i16> {
            Ok(exe.reader_at(va, 4 * RES_MODES, WHAT)?.i32(4 * MODE_640)? as i16)
        };
        let finals = (0..shuppatsu_formats::stages::STAGE_COUNT as u32)
            .map(|s| {
                let r = exe.reader_at(FINAL_SHOT + 0x24 * s, 0x24, WHAT)?;
                let mut row = [0; 9];
                for (k, v) in row.iter_mut().enumerate() {
                    *v = r.i32(4 * k)?;
                }
                Ok(row)
            })
            .collect::<Result<_>>()?;
        let ids = |va: u32, n: usize| -> Result<Vec<i16>> {
            let r = exe.reader_at(va, 4 * n, WHAT)?;
            (0..n).map(|i| Ok(r.i32(4 * i)? as i16)).collect()
        };
        let plates = {
            let n = shuppatsu_formats::hud::STATION_NAME_COUNT;
            let r = exe.reader_at(PLATES, 8 * n, WHAT)?;
            (0..n)
                .map(|i| Ok((r.i32(8 * i)? as i16, r.i32(8 * i + 4)? as i16)))
                .collect::<Result<_>>()?
        };
        let mut digits = [[0; 12]; 2];
        for (d, va) in digits.iter_mut().zip(DIGITS) {
            d.copy_from_slice(&ids(va, 12)?);
        }
        let mut kind_words = [0; 7];
        kind_words.copy_from_slice(&ids(KIND_WORDS, 7)?);
        let fades = FADE_PALETTES
            .iter()
            .map(|&(k, va)| Ok((k, Palette::parse(exe.bytes_at(va, Palette::BYTES)?)?)))
            .collect::<Result<_>>()?;
        Ok(Self {
            full: text(TEXT_FULL)?,
            plain: text(TEXT_PLAIN)?,
            finals,
            plates,
            kind_words,
            station_word: ids(STATION_WORD, 1)?[0],
            plate_y: res(PLATE_Y)?,
            clock_y: res(CLOCK_Y)?,
            digits,
            fades,
        })
    }

    fn fade_palette(&self, kind: i16) -> Option<&Palette> {
        self.fades.iter().find(|(k, _)| *k == kind).map(|(_, p)| p)
    }
}

/// `0x436920`: a palette fading from `data` to black over `total` updates.
#[derive(Debug, Clone)]
struct FadeOut {
    total: i32,
    remaining: i32,
    data: Palette,
}

impl FadeOut {
    fn tick(&mut self, l: &mut SpriteLayer) -> bool {
        if self.remaining < 1 {
            return false;
        }
        let f = f64::from(self.remaining - 1) / f64::from(self.total);
        let scale = |c: u32| (f64::from(c & 0xff) * f) as u32 & 0xff;
        let p = Palette(
            self.data
                .0
                .map(|c| (scale(c >> 16) << 16) | (scale(c >> 8) << 8) | scale(c)),
        );
        l.palettes[usize::from(PAL_TEXT)] = Some(p);
        self.remaining -= 1;
        true
    }
}

/// The ending's task state.
#[derive(Debug, Clone)]
pub(super) struct Ending {
    full: bool,
    /// Text: countdown (+0x30), state (+0x34: 1 show, 0 count, 2 done), record (+0x38).
    cnt: i32,
    state: u8,
    rec: usize,
    /// Mask animation of a blank credit (+0x58) and its countdown base (+0x5c).
    mask_anim: bool,
    mask_base: i32,
    /// Master counter `t` (+0x3c), camera speed `v` km/h (+0x44), deceleration `a` (+0x48),
    /// station index (+0x74), final shot started (+0x78).
    t: i32,
    v: f32,
    a: f32,
    idx: usize,
    final_shot: bool,
    /// The scheduled time of the station on show (`0x8571e8`), −1 past the timetable.
    sched: i32,
    /// Mask edges (virtual y).
    top: i32,
    bottom: i32,
    /// List 5 (camera, scenery, AI trains) is alive.
    world: bool,
    fade_in: Option<Fade>,
    fade_out: Option<FadeOut>,
    /// Enhancement `one_line_captions`: the station plate as one line instead of its sprites.
    plate_line: Option<PlateLine>,
    /// Enhancement `one_line_captions`: the staff credits as text, by the record that starts
    /// each screen, and the screen on show.
    roll: HashMap<usize, CreditScreen>,
    credits: Option<CreditScreen>,
    /// Enter / Esc ended it.
    pub(super) done: bool,
}

impl Play {
    /// `Ending_Run`: whether the stage gets one, by the stage difficulty (`0x4656c4 > 3`).
    pub(super) fn has_ending(&self) -> bool {
        self.drive.rules().stage_rank() > 3
    }

    /// Starts the ending (0x404110 / 0x404920 after 0x4040a0).
    pub(super) fn start_ending(&mut self, variant: Option<bool>) -> Result<()> {
        let exe_bytes = self.data.exe()?;
        let exe = Exe::parse(&exe_bytes)?;
        let tables = EndingTables::read(&exe)?;
        let full = variant.unwrap_or_else(|| self.drive.rules().full_ending());
        let first = if full { &tables.full } else { &tables.plain };
        let cnt = first.first().map_or(0, |r| i32::from(r.dur));
        // The HUD task and the report card are gone (Stage_Run kills lists 2, 3 and 5).
        self.layer
            .slots
            .fill(shuppatsu_render::sprites::Slot::CLEAR);
        self.layer.widgets.clear();
        self.drive.ending_place(0)?;
        if let Some(s) = self.sound.as_mut() {
            s.request(if full { BGM_FULL } else { BGM_PLAIN });
        }
        eprintln!(
            "[play tick {}] ending ({})",
            self.ticks,
            if full { "full" } else { "plain" }
        );
        let roll = self.staff_roll(&tables, full);
        self.ending_tables = Some(tables);
        self.ending = Some(Ending {
            full,
            cnt,
            state: 1,
            rec: 0,
            mask_anim: false,
            mask_base: 1,
            t: if full { FULL_T0 } else { PLAIN_T0 },
            v: 0.0,
            a: 0.0,
            idx: 0,
            final_shot: false,
            sched: 0,
            top: MASK_TOP,
            bottom: if full {
                MASK_BOTTOM_FULL
            } else {
                MASK_BOTTOM_PLAIN
            },
            world: true,
            fade_in: None,
            fade_out: None,
            plate_line: None,
            roll,
            credits: None,
            done: false,
        });
        self.world = WorldView::Cab;
        Ok(())
    }

    /// Enhancement `one_line_captions`: the screens of the staff roll as text, by the record
    /// that starts each; empty (the sprites) without the setting or when the roll does not fit.
    /// The sizes are fitted to both endings, so the roll looks the same in either.
    fn staff_roll(&self, t: &EndingTables, full: bool) -> HashMap<usize, CreditScreen> {
        let Some((parts, font, kanji)) = &self.staff_parts else {
            return HashMap::new();
        };
        let tbls = t
            .full
            .iter()
            .chain(&t.plain)
            .filter_map(|r| u16::try_from(r.tbl).ok());
        let Some(mut roll) = StaffRoll::new(&self.bank, parts, tbls, *font, kanji.as_deref())
        else {
            return HashMap::new();
        };
        let m = BankMeasure(&self.bank, *font);
        let all: Vec<(Vec<Shown>, Bands)> = [true, false]
            .into_iter()
            .flat_map(|f| {
                let bands = credit_bands(f);
                credit_screens(t, f)
                    .into_iter()
                    .map(move |(_, s)| (s, bands))
            })
            .collect();
        if !roll.fit(&m, &all) {
            eprintln!("[ending] staff roll does not fit as text; the sprites stay");
            return HashMap::new();
        }
        credit_screens(t, full)
            .into_iter()
            .filter_map(|(rec, s)| Some((rec, roll.layout(&m, &s, credit_bands(full))?)))
            .collect()
    }

    /// One tick of the ending: the task update, then list 5 (camera and AI trains).
    pub(super) fn ending_tick(&mut self) -> Result<()> {
        self.ticks += 1;
        let (Some(mut e), Some(tables)) = (self.ending.take(), self.ending_tables.take()) else {
            return Ok(());
        };
        let input = self.menu_input;
        if let Some(f) = e.fade_in.as_mut()
            && !f.tick(&mut self.layer)
        {
            e.fade_in = None;
        }
        if let Some(f) = e.fade_out.as_mut()
            && !f.tick(&mut self.layer)
        {
            e.fade_out = None;
        }
        self.ending_text(&mut e, &tables);
        e.t += 1;
        if e.full {
            self.ending_full(&mut e, &tables)?;
        } else if e.v <= PLAIN_TOP {
            e.v += PLAIN_ACCEL;
        }
        let units = (f64::from(e.v) * KMH_TO_UNITS) as f32;
        let (end_at, skip_after, kill_at) = if e.full {
            (FULL_END_AT, FULL_SKIP_AFTER, FULL_KILL_AT)
        } else {
            (PLAIN_END_AT, PLAIN_SKIP_AFTER, PLAIN_KILL_AT)
        };
        let skip = input.confirm || input.cancel;
        if (skip || e.t >= end_at) && e.t > skip_after {
            // Destroy 0x4048a0 / 0x404ca0, then 0x4040a0 kills lists 4, 5, 7.
            self.ending_destroy(&mut e);
            self.drive.ending_kill();
            e.done = true;
            e.world = false;
        } else if e.t == kill_at {
            self.drive.ending_kill();
            e.world = false;
        }
        if e.world {
            self.drive.ending_tick(units);
            self.scene.traffic = self.drive.traffic_poses(1.0);
        } else {
            self.drive.idle_global_tick();
            self.scene.traffic.clear();
        }
        if let Some(s) = self.sound.as_mut() {
            s.traffic(self.drive.traffic_sounds());
            s.menu_frame();
        }
        self.world = if e.world {
            WorldView::Cab
        } else {
            WorldView::Hidden
        };
        if self.drive.com() != self.scene_com {
            self.reload_scene()?;
        }
        self.interpolate = true;
        self.poll();
        // The ending's only widget: the one-line station plate, redrawn every update.
        self.layer.widgets.clear();
        if let Some(line) = &e.plate_line {
            line.draw(&mut self.layer);
        }
        if let Some(c) = &e.credits {
            c.draw(&mut self.layer);
        }
        self.canvas.compose(&self.layer, &self.bank, None);
        let done = e.done;
        self.ending = Some(e);
        self.ending_tables = Some(tables);
        if done {
            self.ending_finished()?;
        }
        Ok(())
    }

    /// The credits (shared by both tasks).
    fn ending_text(&mut self, e: &mut Ending, t: &EndingTables) {
        let recs = if e.full { &t.full } else { &t.plain };
        let (c, base, dy) = if e.full {
            (100.0, MASK_BOTTOM_OPEN, 0)
        } else {
            (90.0, MASK_BOTTOM_PLAIN, -8)
        };
        let l = &mut self.layer;
        if e.state == 1 {
            let first = recs.get(e.rec).copied();
            let n = first.map_or(0, |r| r.n.max(0) as usize);
            for i in 0..TEXT_SLOTS {
                let slot = SLOT_TEXT + i;
                match recs.get(e.rec + i).filter(|_| i < n) {
                    Some(r) if r.tbl == 0 => {
                        slots::clear(l, slot);
                        e.mask_anim = true;
                        e.mask_base = e.cnt.max(1);
                    }
                    Some(r) => {
                        slots::set(l, slot, r.tbl, (r.x, r.y + dy), PAL_TEXT, 3);
                        e.mask_anim = false;
                    }
                    None => slots::clear(l, slot),
                }
            }
            // Enhancement `one_line_captions`: the screen as text instead of its sprites.
            e.credits = e.roll.get(&e.rec).cloned();
            if e.credits.is_some() {
                for i in 0..TEXT_SLOTS {
                    slots::clear(l, SLOT_TEXT + i);
                }
            }
            e.rec += n.saturating_sub(1);
            if let Some(p) = first.and_then(|r| t.fade_palette(r.kind)) {
                e.fade_in = Some(Fade::new(PAL_TEXT, FADE_TICKS, p.clone(), l));
                e.fade_out = None;
            }
            e.state = 0;
        }
        if e.state == 0 {
            if e.mask_anim {
                let h = ((1.0 - f64::from(e.cnt) / f64::from(e.mask_base)) * c) as i32;
                e.top = MASK_TOP + h;
                e.bottom = base - h;
            }
            if e.cnt == FADE_TICKS
                && let Some(p) = recs.get(e.rec).and_then(|r| t.fade_palette(r.kind))
            {
                e.fade_in = None;
                e.fade_out = Some(FadeOut {
                    total: FADE_TICKS,
                    remaining: FADE_TICKS,
                    data: p.clone(),
                });
            }
            e.cnt -= 1;
            if e.cnt <= 0 {
                e.rec += 1;
                e.state = 1;
                e.cnt = recs.get(e.rec).map_or(0, |r| i32::from(r.dur));
                if recs.get(e.rec).is_none_or(|r| r.tbl == -1) {
                    e.state = 2;
                    e.mask_anim = false;
                }
            }
        }
    }

    /// The 3D part of 0x4042b0.
    fn ending_full(&mut self, e: &mut Ending, t: &EndingTables) -> Result<()> {
        if e.t < 0 {
            if (0.0..=OPEN_TOP).contains(&e.v) {
                e.v += OPEN_ACCEL;
            }
            return Ok(());
        }
        let tt = self.drive.rules().timetable().to_vec();
        let row = |i: usize| tt.get(i).copied();
        let time = |i: usize| row(i).map_or(-1, |r| r.time_ticks);
        let ph = e.t % PERIOD;
        if ph == 0 {
            e.sched = time(e.idx);
            if e.sched != -1 {
                loop {
                    e.idx += 1;
                    e.sched = time(e.idx);
                    if e.sched == -1 || row(e.idx).is_some_and(|r| r.params[0] != -1) {
                        break;
                    }
                }
            }
        }
        let stage = self.drive.rules().stage().0;
        if e.sched != -1 {
            let (Some(cur), Some(prev)) = (row(e.idx), e.idx.checked_sub(1).and_then(row)) else {
                return Ok(());
            };
            let stop = cur.stops == 1;
            match ph {
                0 => {
                    (e.top, e.bottom) = MASK_CLOSED;
                    self.drive.ending_kill();
                    e.world = false;
                }
                1 => {
                    self.ending_hide_panels(e);
                    self.drive.ending_place(e.idx - 1)?;
                    // The next placement (as ph 0 picks it), or the final shot's station:
                    // its scene loads while this one shows.
                    let mut j = e.idx;
                    if time(j) != -1 {
                        loop {
                            j += 1;
                            if time(j) == -1 || row(j).is_some_and(|r| r.params[0] != -1) {
                                break;
                            }
                        }
                    }
                    let next = if time(j) != -1 {
                        Some(j - 1)
                    } else {
                        t.finals.get(stage).map(|f| f[0].max(0) as usize)
                    };
                    if let Some(s) = next {
                        self.prefetch_station(s, &[]);
                    }
                    e.world = true;
                    let jump = if stop {
                        prev.to_next_m - STOP_LEAD_M
                    } else {
                        prev.to_next_m - cur.params[1]
                    };
                    e.v = (f64::from(jump) * JUMP_SCALE) as f32;
                }
                2 => {
                    (e.top, e.bottom) = (MASK_TOP, MASK_BOTTOM_OPEN);
                    if stop {
                        e.v = (STOP_K / f64::from(PERIOD)) as f32;
                        e.a = (-STOP_K / f64::from(PERIOD * PERIOD)) as f32;
                    } else {
                        e.v = (f64::from(cur.params[1]) * JUMP_SCALE * PASS_PERIOD) as f32;
                    }
                    self.ending_panels(e, t, stop);
                }
                _ if stop => e.v += e.a,
                _ => e.v += PASS_ACCEL,
            }
            return Ok(());
        }
        let Some(f) = t.finals.get(stage).copied() else {
            return Ok(());
        };
        let [station, jump, cam, spawn, speed, param, decel, flags, model] = f;
        if !e.final_shot {
            match ph {
                0 if e.v >= 0.0 => {
                    (e.top, e.bottom) = MASK_CLOSED;
                    self.drive.ending_kill();
                    e.world = false;
                }
                1 if e.v >= 0.0 => {
                    self.ending_hide_panels(e);
                    self.drive.ending_place(station.max(0) as usize)?;
                    e.world = true;
                    e.v = (f64::from(jump) * JUMP_SCALE) as f32;
                    // Stage 2 after coupling the 0x8000000 set shows model 8.
                    let model = if stage == 2 && self.drive.train().train_type == 0xe {
                        8
                    } else {
                        model
                    };
                    self.drive.ending_spawn(&SpawnRequest {
                        spawn: spawn as i16,
                        flags: flags | 5,
                        speed_kmh: speed,
                        param,
                        model,
                    });
                }
                2 => {
                    (e.top, e.bottom) = (MASK_TOP, MASK_BOTTOM_OPEN);
                    e.final_shot = true;
                    e.v = -(cam as f32);
                }
                _ => {}
            }
        } else if e.t >= FINAL_BRAKE_AT {
            e.v += (f64::from(decel) * 0.1) as f32;
            if e.v > 0.0 {
                e.v = 0.0;
            }
        }
        Ok(())
    }

    /// The station plate (0x417ab0) and the clock panel (0x417410) of station `e.idx`.
    fn ending_panels(&mut self, e: &mut Ending, t: &EndingTables, stop: bool) {
        let k = usize::from(stop);
        let l = &mut self.layer;
        let stage = self.drive.rules().stage().0;
        let ids = &self.cab.hud.tables().stage_station_ids;
        let id = ids.get(stage).and_then(|v| v.get(e.idx)).copied();
        // Enhancement `one_line_captions`: the plate is the caption set (name, 駅, word), so
        // with a translation it is the in-game caption's line, at its size.
        let kind = if stop { KIND_STOP } else { KIND_PASS };
        e.plate_line = id.and_then(|i| {
            self.cab
                .captions()
                .plate_line(usize::from(i), kind, t.plate_y)
        });
        if e.plate_line.is_some() {
            for s in SLOT_PLATE..SLOT_PLATE + 3 {
                slots::clear(l, s);
            }
        } else if let Some(&(name, width)) = id.and_then(|i| t.plates.get(usize::from(i))) {
            let x = 320 - (width + 64 + 32) / 2;
            let y = t.plate_y;
            slots::set(l, SLOT_PLATE, name, (x, y), 4, 3);
            slots::set(l, SLOT_PLATE + 1, t.station_word, (x + width, y), 4, 3);
            slots::set(
                l,
                SLOT_PLATE + 2,
                t.kind_words[k + 4],
                (x + width + 32, y),
                4,
                3,
            );
        }
        // REC[idx] = the arrival record of the leg ending at idx.
        let (time, err) = e
            .idx
            .checked_sub(1)
            .and_then(|i| self.drive.rules().arrivals.get(i).copied())
            .unwrap_or((0, 999_999));
        let late = time >= e.sched + 30;
        let x = 164 - 32 * k as i16;
        let y = t.clock_y;
        slots::set(
            l,
            SLOT_LABEL,
            LABELS[k][usize::from(late)],
            (x + 64, y),
            4,
            3,
        );
        let clock = |ticks: i32| {
            let (h, m, s) = (ticks / 108_000, (ticks / 1800) % 60, (ticks / 30) % 60);
            [h / 10, h % 10, m / 10, m % 10, s / 10, s % 10].map(|d| d.rem_euclid(10) as usize)
        };
        for (i, d) in clock(e.sched).into_iter().enumerate() {
            let xx = x + 0x90 + 16 * i as i16;
            slots::set(l, SLOT_SCHED + i, t.digits[0][d], (xx, y), 4, 3);
        }
        let set = usize::from(!late);
        for (i, d) in clock(time).into_iter().enumerate() {
            let xx = x + 0x90 + 16 * i as i16;
            slots::set(l, SLOT_ACTUAL + i, t.digits[set][d], (xx, y + 16), 4, 3);
        }
        if stop {
            let a = err.unsigned_abs() as i32;
            let (value, glyph) = if a < 100 {
                (a, 10)
            } else {
                (((a + 50) / 100).min(99), 11)
            };
            let set = usize::from(err >= 0);
            let d = &t.digits[set];
            let tens = (value / 10) as usize;
            slots::set(l, SLOT_ERR_TENS, d[tens], (x + 0x100, y + 16), 4, 3);
            slots::set(
                l,
                SLOT_ERR_UNITS,
                d[(value % 10) as usize],
                (x + 0x110, y + 16),
                4,
                3,
            );
            slots::set(l, SLOT_ERR_GLYPH, d[glyph], (x + 0x120, y + 16), 4, 3);
        } else {
            for s in SLOT_ERR_UNITS..=SLOT_ERR_GLYPH {
                slots::clear(l, s);
            }
        }
    }

    fn ending_hide_panels(&mut self, e: &mut Ending) {
        e.plate_line = None;
        for s in PANEL_SLOTS.chain(SLOT_PLATE..SLOT_PLATE + 3) {
            slots::clear(&mut self.layer, s);
        }
    }

    /// Destroy 0x4048a0 / 0x404ca0: panels hidden, the music released, every non-keep slot
    /// cleared.
    fn ending_destroy(&mut self, e: &mut Ending) {
        self.ending_hide_panels(e);
        e.credits = None;
        if let Some(s) = self.sound.as_mut() {
            s.release(BGM_FULL);
        }
        crate::front::clear_all(&mut self.layer);
    }

    /// The letterbox masks over the 3D band (0x486160 top, 0x486ec4 bottom; depth 0, black).
    pub(super) fn ending_masks(ending: Option<&Ending>, fx: &mut EffectGeometry) {
        let Some(e) = ending else {
            return;
        };
        let ndc = |vy: i32| 1.0 - 2.0 * ((vy as f32 - VIEW_TOP) / VIEW_H);
        let quad = |a: f32, b: f32| [[-1.0, a], [1.0, a], [1.0, b], [-1.0, b]];
        fx.add_flat_quad(quad(1.0, ndc(e.top)), [0.0, 0.0, 0.0, 1.0], 0);
        fx.add_flat_quad(quad(ndc(e.bottom), -1.0), [0.0, 0.0, 0.0, 1.0], 0);
    }

    /// Render mode 3 of the full ending (0x425680): the eye 0.04 and the target 0.06 units
    /// lower than the cab camera (`0x84c158` / `0x84c154`, set in 0x4254b0).
    pub(super) fn ending_camera(
        ending: Option<&Ending>,
        camera: &mut shuppatsu_render::camera::Camera,
    ) {
        if ending.is_some_and(|e| e.full) {
            camera.eye.y -= EYE_DROP;
            camera.target.y -= TARGET_DROP;
        }
    }

    /// After 0x4040a0: Stage_Run stops every sound and runs the ranking.
    fn ending_finished(&mut self) -> Result<()> {
        if let Some(s) = self.sound.as_mut() {
            s.stop_all();
        }
        self.ending = None;
        self.ending_tables = None;
        self.world = WorldView::Hidden;
        let mut sfx = Vec::new();
        self.start_ranking_now(true, &mut sfx);
        self.play(&sfx);
        Ok(())
    }
}

/// Design pixels per virtual pixel down the screen (the masks' 400 rows over 480).
const VIRTUAL_TO_DESIGN: f32 = 480.0 / 400.0;

/// The 3D view between the masks while credits show (the open bottom edge of a station shot
/// in the full ending, the plain ending's own), design y.
fn credit_bands(full: bool) -> Bands {
    let bottom = if full {
        MASK_BOTTOM_OPEN
    } else {
        MASK_BOTTOM_PLAIN
    };
    Bands {
        view: (
            MASK_TOP as f32 * VIRTUAL_TO_DESIGN,
            bottom as f32 * VIRTUAL_TO_DESIGN,
        ),
    }
}

/// The screens of a text table as [`Play::ending_text`] shows them: the record that starts
/// each and its credits `(tbl, x, y)` (blank credits and the end mark show none).
fn credit_screens(t: &EndingTables, full: bool) -> Vec<(usize, Vec<Shown>)> {
    let (recs, dy) = if full { (&t.full, 0) } else { (&t.plain, -8) };
    let mut out = Vec::new();
    let mut rec = 0;
    while let Some(first) = recs.get(rec).filter(|r| r.tbl != -1) {
        let n = first.n.max(0) as usize;
        let shown = recs[rec..]
            .iter()
            .take(n)
            .filter_map(|r| Some((u16::try_from(r.tbl).ok().filter(|&t| t > 0)?, r.x, r.y + dy)))
            .collect();
        out.push((rec, shown));
        rec += n.max(1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against the user's data: the ending's plate is the caption set (name sprites, 駅 and
    /// the 通過 / 停車 words), so the in-game caption's one line stands in for it.
    #[test]
    fn plate_is_the_caption_set() {
        if std::env::var_os("SHUPPATSU_DATA_DIR").is_none() {
            eprintln!("skipped: SHUPPATSU_DATA_DIR not set");
            return;
        }
        let data = crate::scene::GameData::from_env().unwrap();
        let exe_bytes = data.exe().unwrap();
        let exe = Exe::parse(&exe_bytes).unwrap();
        let t = EndingTables::read(&exe).unwrap();
        let hud = shuppatsu_formats::hud::HudTables::read(&exe).unwrap();
        let cab = shuppatsu_formats::cab_hud::CabTables::read(&exe).unwrap();
        let names: Vec<(i16, i16)> = hud
            .station_names
            .iter()
            .map(|n| (n.plain as i16, n.width as i16))
            .collect();
        assert_eq!(t.plates, names);
        assert_eq!(t.station_word, cab.caption_station);
        assert_eq!(t.kind_words[4..6], cab.caption_words[4..6]);
    }
}

#[cfg(test)]
mod roll_tests {
    use super::*;

    /// The user's credits with the built-in English and the bundled font.
    pub(super) fn data_roll(
        kanji: Option<&str>,
    ) -> Option<(
        StaffRoll,
        shuppatsu_render::sprites::SpriteBank,
        EndingTables,
    )> {
        if std::env::var_os("SHUPPATSU_DATA_DIR").is_none() {
            eprintln!("skipped: SHUPPATSU_DATA_DIR not set");
            return None;
        }
        let data = crate::scene::GameData::from_env().unwrap();
        let exe_bytes = data.exe().unwrap();
        let exe = Exe::parse(&exe_bytes).unwrap();
        let t = EndingTables::read(&exe).unwrap();
        let mut bank = shuppatsu_render::sprites::SpriteBank::new(
            &shuppatsu_formats::cg::CgFile::parse(&data.read("cg.rom").unwrap()).unwrap(),
            shuppatsu_formats::cg::parse_tbl(&data.read("tbl.rom").unwrap()).unwrap(),
        );
        crate::locale::install_label_font(&mut bank).unwrap();
        let parts = crate::locale::Catalogue::load("en", None)
            .unwrap()
            .tbl_parts();
        let tbls = t
            .full
            .iter()
            .chain(&t.plain)
            .filter_map(|r| u16::try_from(r.tbl).ok());
        let roll = StaffRoll::new(
            &bank,
            &parts,
            tbls,
            shuppatsu_render::text::FontRole::Body,
            kanji,
        )
        .unwrap();
        Some((roll, bank, t))
    }

    /// Against the user's data: with the bundled font both endings' credits fit as text at
    /// one name size close to the kanji's and one role size, every text and logo inside the
    /// 640-wide design (16 px margins) and inside the black bands, none overlapping another,
    /// no role of a person wrapped (only the JR East company and the Planning & Reference
    /// Materials header may be); with the names' kanji after them too (setting `staff_kanji`,
    /// bare or in parentheses), where the names come out a little smaller.
    #[test]
    fn staff_roll_fits_both_endings() {
        let parts = crate::locale::Catalogue::load("en", None)
            .unwrap()
            .tbl_parts();
        let wrappable = [
            "JR East Marketing & Communications, Inc.",
            "Planning & Reference Materials",
        ];
        for (kanji, name_cap) in [
            (None, 24.0),
            (Some("{kanji}"), 23.0),
            (Some("({kanji})"), 22.0),
        ] {
            // Every line of the catalogue as written, and the kanji in the format.
            let mut whole: Vec<String> = Vec::new();
            for p in parts.values().flatten() {
                whole.extend(
                    p.text
                        .split(['\n'])
                        .flat_map(|l| l.split(" · "))
                        .map(str::to_owned),
                );
                if let (Some(f), Some(k)) = (kanji, &p.kanji) {
                    whole.extend(k.split(" · ").map(|k| f.replace("{kanji}", k)));
                }
            }
            let Some((mut roll, bank, t)) = data_roll(kanji) else {
                return;
            };
            let m = BankMeasure(&bank, shuppatsu_render::text::FontRole::Body);
            // The kanji: 32 px names, 15 px one-line roles (0.75 of them for Latin capitals).
            assert_eq!((roll.name_cap, roll.role_cap), (24.0, 11.25));
            let all: Vec<(Vec<Shown>, Bands)> = [true, false]
                .into_iter()
                .flat_map(|f| {
                    credit_screens(&t, f)
                        .into_iter()
                        .map(move |(_, s)| (s, credit_bands(f)))
                })
                .collect();
            assert!(roll.fit(&m, &all), "{kanji:?}");
            eprintln!(
                "staff roll {kanji:?}: names {} px, roles {} px",
                roll.name_cap, roll.role_cap
            );
            assert_eq!(roll.name_cap, name_cap, "{kanji:?}");
            let mut text_screens = 0;
            let mut kanji_texts = 0;
            for (shown, bands) in &all {
                let Some(screen) = roll.layout(&m, shown, *bands) else {
                    // Only the blank credit and the closing logos keep their sprites.
                    assert!(
                        shown.iter().all(|s| [0x304, 0x15e].contains(&s.0)),
                        "{shown:x?}"
                    );
                    continue;
                };
                text_screens += 1;
                let boxes = screen.ink_boxes(&m);
                kanji_texts += boxes
                    .iter()
                    .filter(|(w, _)| w.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)))
                    .count();
                for (what, _) in &boxes {
                    assert!(
                        what.starts_with("art ")
                            || whole.contains(what)
                            || wrappable.iter().any(|w| w.contains(what.as_str())),
                        "{what:?} wrapped {shown:x?} {kanji:?}"
                    );
                }
                for (what, [x0, y0, x1, y1]) in &boxes {
                    assert!(
                        *x0 >= 16.0 - 0.01 && *x1 <= 624.0 + 0.01,
                        "{what} {x0}..{x1} {shown:x?} {kanji:?}"
                    );
                    let clear = *y1 <= bands.view.0 || *y0 >= bands.view.1;
                    assert!(
                        clear && *y0 >= 0.0 && *y1 <= 480.0,
                        "{what} {y0}..{y1} {shown:x?}"
                    );
                }
                for (i, (a, p)) in boxes.iter().enumerate() {
                    for (b, q) in &boxes[i + 1..] {
                        let apart = p[2] <= q[0] || q[2] <= p[0] || p[3] <= q[1] || q[3] <= p[1];
                        assert!(apart, "{a} {p:?} overlaps {b} {q:?} {kanji:?}");
                    }
                }
            }
            assert_eq!(text_screens, 29 + 27);
            // Every person of both endings (34 names in each, Kikuchi twice), and nothing
            // else, has kanji.
            assert_eq!(kanji_texts, if kanji.is_some() { 2 * 34 } else { 0 });
        }
    }

    /// Every credited person in the English catalogue has their kanji, one per name.
    #[test]
    fn every_credited_name_has_kanji() {
        let parts = crate::locale::Catalogue::load("en", None)
            .unwrap()
            .tbl_parts();
        let mut names = 0;
        for (tbl, ps) in &parts {
            for p in ps {
                let is_name = p.shared_group.as_deref() == Some("ending.name")
                    || (p.text.contains(" · ") && (0x3a8..=0x3c8).contains(tbl));
                if !is_name {
                    assert!(p.kanji.is_none(), "{tbl:#x} {:?}", p.text);
                    continue;
                }
                let k = p
                    .kanji
                    .as_deref()
                    .unwrap_or_else(|| panic!("{tbl:#x} {:?}", p.text));
                assert_eq!(k.split(" · ").count(), p.text.split(" · ").count());
                names += k.split(" · ").count();
            }
        }
        assert_eq!(names, 34);
    }

    /// Against the user's data: every credit's lettering is either under a catalogue part
    /// (drawn as text) or kept or dropped art, and every kept box has ink.
    #[test]
    fn kept_art_covers_the_untranslated_ink() {
        let Some((_, bank, t)) = data_roll(None) else {
            return;
        };
        let parts = crate::locale::Catalogue::load("en", None)
            .unwrap()
            .tbl_parts();
        let mut n = 0;
        for tbl in t.full.iter().filter_map(|r| u16::try_from(r.tbl).ok()) {
            let Some(p) = parts.get(&tbl) else {
                continue;
            };
            let (stray, empty) = super::super::staff_roll::unaccounted_ink(&bank, p, tbl);
            assert!(
                stray.is_empty(),
                "{tbl:#x}: {} stray pixels, first {:?}",
                stray.len(),
                &stray[..stray.len().min(4)]
            );
            assert!(
                empty.is_empty(),
                "{tbl:#x}: kept boxes without ink {empty:?}"
            );
            n += 1;
        }
        assert_eq!(n, 32);
    }
}

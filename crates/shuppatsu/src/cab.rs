//! The whole in-cab HUD for one stage: `Hud_Update` / `Cab_Overlay` ([`Hud`]) and the HUD tasks
//! the original keeps in task list 3 — signals, signs, time messages, the ATS lamp and popups
//! ([`Popups`]), navigator hints ([`Hints`]), captions and banners ([`Captions`]) and the
//! pre-departure strip ([`DepartureHud`]) — plus the HUD-only parts of the rule functions:
//! the station-approach captions and gradient signs of 0x40c4e0, the clock freeze of 0x40d620
//! and the time-up popup of 0x4158b0 (docs/re/hud.md).
//!
//! One [`CabHud::tick`] per simulation tick, after the rules and the train task: rule events
//! first (they are calls made earlier in the same tick), then the task updates.

use crate::captions::{Approach, CaptionDefs, Captions, DEPARTURE_BANNER_Y, DISTANCE};
use crate::coupling_hud::{CouplingHud, CouplingTables};
use crate::departure_hud::{DepartureHud, Segment};
use crate::hints::Hints;
use crate::hud::{Hud, HudState, SPEED_MODE_ATC};
use crate::navigator::{
    Effect, NaviState, NaviTables, NaviTick, Navigator, ReadyAnim, ReadyTables,
};
use crate::popups::{Popups, time_up_advice};
use anyhow::Result;
use shuppatsu_engine::coupling::Coupling;
use shuppatsu_engine::drive::{Drive, DrivePhase};
use shuppatsu_engine::rules::{Phase, RuleEvent, STAT_COUNT};
use shuppatsu_formats::cab_hud::CabTables;
use shuppatsu_formats::exe::Exe;
use shuppatsu_render::sprites::SpriteLayer;

/// The clocks freeze for 0x5a HUD updates after a station is passed (0x40d620 → 0x40ff20).
const CLOCK_FREEZE_TICKS: i32 = 0x5a;
/// The stage loop runs 20 `Task_Tick`s after a game over before the time-up popup (0x40857b).
const TIME_UP_AFTER_TICKS: u32 = 0x14;
/// Course events the HUD acts on (0x40acd0): 7 ATC speed index, 17 landmark, 23 the E3
/// coupling-ready animation (0x411240).
const COURSE_ATC: i16 = 7;
const COURSE_LANDMARK: i16 = 17;
const COURSE_E3_READY: i16 = 23;
/// The ATC speed index starts at 2 (0x40f7e0).
const ATC_INDEX_START: i32 = 2;

/// The simulation state the HUD reads, after one tick.
#[derive(Debug, Clone, PartialEq)]
pub struct Snap {
    pub phase: Phase,
    pub drive_phase: DrivePhase,
    pub stage_rank: i32,
    pub station: usize,
    pub clock: i32,
    pub scheduled: i32,
    pub remaining_s: i32,
    pub pending_s: i32,
    pub hud_counter: u32,
    pub target_stops: bool,
    pub final_station: bool,
    pub aspect: i16,
    pub ats: i16,
    pub dist_m: f32,
    pub dist_round_m: i32,
    pub departure_counter: i32,
    pub departure_base: i32,
    pub departure_done: bool,
    pub odometer: f32,
    pub speed_kmh: f32,
    /// Notches the train task applied (task +0x60 / +0x64 after the tick).
    pub power: i32,
    pub brake: i32,
    /// `0x84c3d8` and `0x84c40c` after the tick.
    pub grade_force: f32,
    pub leg_distance: f32,
    pub train_driving: bool,
    pub stats: [i32; STAT_COUNT],
    pub continues: i32,
    pub early_ticks: i32,
    /// Home-signal distance of the target station (timetable field 5).
    pub home_signal_m: i32,
    /// Timetable "stops" flags and marks (`0x856fe1[station]`).
    pub stops: Vec<bool>,
    pub marks: Vec<u8>,
    /// The coupling minigame while the rules run it (rules.md §7.5).
    pub coupling: Option<Coupling>,
    /// The last `Vehicle_Set` code of the run (0x410320), `None` = the stage's own.
    pub vehicle_code: Option<u32>,
    /// The bonus game runs (remaining time and distance panel off: `0x856d1c = 0`,
    /// `0x856d0c = 0`).
    pub bonus: bool,
    /// What the navigator reactions read (navigator.rs).
    pub navi: NaviState,
}

impl Snap {
    pub fn of(drive: &Drive) -> Self {
        let r = drive.rules();
        let t = drive.train();
        Self {
            phase: r.phase,
            drive_phase: drive.phase(),
            stage_rank: r.stage_rank(),
            station: r.station,
            clock: r.clock,
            scheduled: r.scheduled,
            remaining_s: r.remaining_s,
            pending_s: r.pending_s,
            hud_counter: r.hud_counter,
            target_stops: r.target_stops,
            final_station: r.final_station,
            aspect: r.aspect,
            ats: r.ats,
            dist_m: r.dist_m,
            dist_round_m: r.dist_round_m,
            departure_counter: r.departure_counter,
            departure_base: r.departure_base,
            departure_done: r.departure_done,
            odometer: t.odometer,
            speed_kmh: drive.speed_kmh(),
            power: t.prev_power,
            brake: t.prev_brake,
            grade_force: t.grade_force,
            leg_distance: t.leg_distance,
            train_driving: drive.train_driving(),
            stats: r.stats,
            continues: r.continues,
            early_ticks: r.early_ticks,
            home_signal_m: r
                .timetable()
                .get(r.station + 1)
                .map_or(0, |row| row.params[1]),
            stops: r.timetable().iter().map(|row| row.stops == 1).collect(),
            marks: r.marks.clone(),
            coupling: r.coupling.clone().filter(|_| r.phase == Phase::Coupling),
            vehicle_code: r.vehicle_code,
            bonus: drive.bonus().is_some(),
            navi: NaviState::of(drive),
        }
    }

    /// The driving-loop body (0x4084f5) runs in the tick that starts from this state.
    fn runs_loop_body(&self) -> bool {
        self.phase == Phase::Driving || (self.phase == Phase::PreDeparture && self.departure_done)
    }

    /// The departure task (0x413370) updates in the tick that starts from this state.
    fn runs_departure_task(&self) -> bool {
        self.phase == Phase::PreDeparture && !self.departure_done
    }

    /// The departure wait `0x855ff8 & 0x800`.
    fn waiting(&self) -> bool {
        self.phase == Phase::PreDeparture && !self.departure_done
    }
}

/// The inputs of one tick that the HUD reads besides the simulation state.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct TickInput {
    pub segment_started: bool,
    /// The input word this tick ran with: horn held, confirm pressed.
    pub horn_held: bool,
    pub confirm_pressed: bool,
    /// The remaining time the station result shows while it deducts.
    pub remaining_override: Option<i32>,
    pub tolerance_m: i32,
}

/// The translated cab texts of a run ([`CabHud::translations`]).
pub struct Translations {
    landmarks: Option<crate::captions::LandmarkLines>,
    one_line: Option<crate::captions::OneLine>,
    board: Option<std::sync::Arc<crate::board_en::BoardNames>>,
}

pub struct CabHud {
    pub hud: Hud,
    popups: Popups,
    hints: Hints,
    /// Navigator reactions and the emergency-stop signal (`NaviAnim` 0x410cb0).
    navigator: Navigator,
    navi_tables: NaviTables,
    /// The E3 coupling-ready animation (0x411240), sharing the navigator's slot.
    ready: Option<ReadyAnim>,
    ready_tables: ReadyTables,
    captions: Captions,
    departure: DepartureHud,
    stage: usize,
    name_ids: Vec<u16>,
    prev: Option<Snap>,
    /// `0x856c6c`, `0x856d0c`, `0x856c70`, `0x856c80`, `0x829d16`.
    clock_freeze: i32,
    panel_active: bool,
    panel_on: bool,
    clocks_visible: bool,
    lamp_lit: bool,
    /// `0x856d34`, `0x856cf4`, vehicle row.
    speed_mode: i32,
    atc_index: i32,
    vehicle: Option<(i16, i16)>,
    /// Ticks since the game over, and the category of the last statistic added (`0x856d84`).
    game_over_ticks: Option<u32>,
    last_stat: i32,
    /// The coupling minigame's screen, and the `Vehicle_Set` code the cab shows.
    coupling: CouplingHud,
    vehicle_code: Option<u32>,
    stage_speed_mode: i32,
    /// An attract demo drive: its task, not the stage loop, runs (no 0x40c4e0 HUD parts).
    demo: bool,
    /// Enhancement `board_names`; `None` leaves the route board in kanji only.
    board_names: Option<std::sync::Arc<crate::board_en::BoardNames>>,
}

impl CabHud {
    pub fn new(exe: &Exe<'_>, stage: usize, horn_on_button: bool) -> Result<Self> {
        let hud = Hud::new(exe, stage)?;
        let cab = CabTables::read(exe)?;
        let name_ids = hud
            .tables()
            .stage_station_ids
            .get(stage)
            .cloned()
            .unwrap_or_default();
        let captions = Captions::new(CaptionDefs::new(hud.tables(), &cab));
        let vehicle = hud.vehicle(stage as u32);
        let speed_mode = cab.speed_mode.get(stage).copied().unwrap_or(0);
        let coupling = CouplingHud::new(CouplingTables::read(exe)?);
        Ok(Self {
            popups: Popups::new(exe, stage, cab.origin, cab.time_up_advice_pos)?,
            hints: Hints::new(horn_on_button),
            navigator: Navigator::default(),
            navi_tables: NaviTables::read(exe, stage)?,
            ready: None,
            ready_tables: ReadyTables::read(exe)?,
            captions,
            departure: DepartureHud::new(&cab),
            speed_mode,
            hud,
            stage,
            name_ids,
            prev: None,
            clock_freeze: 0,
            panel_active: false,
            panel_on: true,
            clocks_visible: false,
            lamp_lit: false,
            atc_index: ATC_INDEX_START,
            vehicle,
            game_over_ticks: None,
            last_stat: -1,
            coupling,
            vehicle_code: None,
            stage_speed_mode: speed_mode,
            demo: false,
            board_names: None,
        })
    }

    /// The translated cab texts of a run (install them with [`CabHud::use_translations`]);
    /// it only reads the cab, so a run start builds them beside its other work (the board
    /// names take about 9 ms the first time):
    /// - enhancement `landmark_names` (`landmarks`): translated names under the landmark
    ///   captions, by landmark sprite tilemap;
    /// - enhancement `one_line_captions` (`one_line`): captions and banners as one translated
    ///   line (the sprites stay when the art has no caption sprites);
    /// - enhancement `board_names` (`board`, translated texts by tilemap id: the station names
    ///   are their name sprites' texts): romaji under the route board's station names, kept
    ///   for the next run start with the same bank and names (a stage intro and the run after
    ///   it build the same).
    pub fn translations(
        &self,
        bank: &shuppatsu_render::sprites::SpriteBank,
        landmarks: Option<&std::collections::HashMap<u16, String>>,
        one_line: Option<&crate::locale::CaptionPhrases>,
        board: Option<&std::collections::HashMap<u16, String>>,
    ) -> Translations {
        let defs = self.captions.defs();
        Translations {
            landmarks: landmarks.map(|n| crate::captions::LandmarkLines::new(bank, defs, n)),
            one_line: one_line.and_then(|p| {
                crate::captions::OneLine::new(bank, defs, &p.tbl_texts, p.by_kind.clone(), p.fonts)
            }),
            board: board.map(|t| self.board_names(bank, t)),
        }
    }

    /// Installs what [`CabHud::translations`] built.
    pub fn use_translations(&mut self, t: Translations) {
        if let Some(l) = t.landmarks {
            self.captions.use_landmark_lines(l);
        }
        if let Some(o) = t.one_line {
            self.captions.use_one_line(o);
        }
        if let Some(b) = t.board {
            self.board_names = Some(b);
        }
    }

    /// The captions (for the ending's station plate: [`Captions::plate_line`]).
    pub fn captions(&self) -> &Captions {
        &self.captions
    }

    /// Enhancement `board_names` for `tbl_texts`, built or kept from the last start.
    fn board_names(
        &self,
        bank: &shuppatsu_render::sprites::SpriteBank,
        tbl_texts: &std::collections::HashMap<u16, String>,
    ) -> std::sync::Arc<crate::board_en::BoardNames> {
        type Key = (u64, u64, Vec<u16>, Vec<(usize, String)>);
        static KEPT: std::sync::Mutex<Option<(Key, std::sync::Arc<crate::board_en::BoardNames>)>> =
            std::sync::Mutex::new(None);
        let names: std::collections::HashMap<usize, String> = self
            .captions
            .defs()
            .names
            .iter()
            .enumerate()
            .filter_map(|(id, n)| Some((id, tbl_texts.get(&n.plain)?.clone())))
            .collect();
        let tilemaps: Vec<u16> = self.departure.diagram_tilemaps(self.stage).collect();
        let mut sorted: Vec<(usize, String)> =
            names.iter().map(|(&id, n)| (id, n.clone())).collect();
        sorted.sort_unstable();
        let key = (bank.art_id(), bank.generation(), tilemaps.clone(), sorted);
        let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
        match kept.as_ref() {
            Some((k, b)) if *k == key => b.clone(),
            _ => {
                let b =
                    std::sync::Arc::new(crate::board_en::BoardNames::new(bank, tilemaps, &names));
                *kept = Some((key, b.clone()));
                b
            }
        }
    }

    /// `Hud_Create` 0x40f7e0 (stage start and restart): static slots, strip palette.
    pub fn create(&mut self, layer: &mut SpriteLayer) {
        self.hud.create(layer);
        self.departure.stage_start(layer);
        self.navigator = Navigator::default();
        self.ready = None;
        self.prev = None;
        self.clock_freeze = 0;
        self.panel_active = false;
        self.panel_on = true;
        self.clocks_visible = false;
        self.lamp_lit = false;
        self.atc_index = ATC_INDEX_START;
        self.game_over_ticks = None;
        self.last_stat = -1;
    }

    /// The distance panel is active and shows the distance, not the "Professional" plate
    /// (`0x856d0c == 1` and `0x856c70 == 1`).
    /// The time-up popup is still on screen (phase flag 0x400 in the original).
    pub fn time_up_running(&self) -> bool {
        self.popups.time_up_running()
    }

    pub fn panel_shown(&self) -> bool {
        self.panel_active && self.panel_on
    }

    /// The coupling minigame's black band over the bottom of the 3D view is up (`0x84c0f8`).
    pub fn bottom_band(&self) -> bool {
        self.coupling.bottom_band()
    }

    fn name_id(&self, station: usize) -> Option<usize> {
        self.name_ids.get(station).map(|&i| usize::from(i))
    }

    /// One HUD tick after a simulation tick.
    pub fn tick(
        &mut self,
        s: &Snap,
        input: &TickInput,
        events: &[RuleEvent],
        layer: &mut SpriteLayer,
    ) {
        let b = self.prev.take().unwrap_or_else(|| s.clone());
        if input.segment_started {
            self.segment_setup(s, layer);
        }
        self.track_stats(&b, s);
        // The rules ran on the odometer before this tick's physics.
        let rules_odometer = b.odometer;
        let mut station_passed = false;
        let mut ats_bell = false;
        let mut acknowledged = false;
        for e in events {
            self.popups.event(e, rules_odometer, layer);
            match *e {
                RuleEvent::HintShown {
                    id,
                    ticks,
                    distance_m,
                    start_m,
                } => self
                    .hints
                    .show(id, ticks, distance_m, start_m, s.stage_rank),
                RuleEvent::HintHidden => self.hints.hide(),
                RuleEvent::Script { kind, args } if kind == COURSE_ATC => {
                    self.atc_index = i32::from(args[0]);
                }
                RuleEvent::Script { kind, args } if kind == COURSE_LANDMARK => {
                    let (id, metres) = (i32::from(args[0]), i32::from(args[1]));
                    self.captions.landmark(layer, id, metres, rules_odometer);
                }
                RuleEvent::Script { kind, .. } if kind == COURSE_E3_READY => {
                    let (first, colours) = ReadyAnim::palette(&self.ready_tables);
                    crate::slots::palette4(layer, first, colours);
                    self.ready = Some(ReadyAnim::new());
                }
                RuleEvent::StationPassed { .. } => {
                    station_passed = true;
                    self.clock_freeze = CLOCK_FREEZE_TICKS;
                    self.captions.reset_approach();
                }
                RuleEvent::Arrived(_) if s.final_station => self.popups.all_clear(layer),
                RuleEvent::AtsBell => ats_bell = true,
                RuleEvent::AtsAcknowledged => acknowledged = true,
                RuleEvent::GameOver => self.game_over_ticks = Some(0),
                // 0x4093c0: the distance panel goes off (`0x856d0c = 0`).
                RuleEvent::CouplingStarted => self.panel_active = false,
                _ => {}
            }
        }
        if s.ats == 0 && !acknowledged {
            self.popups.ats_cleared();
        }
        if s.vehicle_code != self.vehicle_code {
            // `Vehicle_Set` 0x410320: the vehicle plate and, for `0x1000 | n`, the ATC readout.
            self.vehicle_code = s.vehicle_code;
            let code = s.vehicle_code.unwrap_or(self.stage as u32);
            self.vehicle = self.hud.vehicle(code);
            self.speed_mode = if s.vehicle_code.is_some_and(|c| c & 0x1000 != 0) {
                SPEED_MODE_ATC
            } else {
                self.stage_speed_mode
            };
        }

        // 0x40c4e0 (driving-loop body): panel toggle, approach captions, gradient signs.
        if b.runs_loop_body() && !input.segment_started && !self.demo {
            if input.confirm_pressed && b.ats < 1 && !ats_bell {
                self.panel_on = !self.panel_on;
            }
            let approach = Approach {
                leg_m: if station_passed { 0.0 } else { b.leg_distance },
                name_id: self.name_id(s.station + 1),
                stops: s.target_stops,
                dist_m: s.dist_m,
                dist_round_m: s.dist_round_m,
                home_signal_m: s.home_signal_m,
                odometer: rules_odometer,
            };
            self.captions.approach_step(&approach, layer);
            self.popups.gradient_step(b.grade_force, rules_odometer);
        }

        // Leaving the stage loop after an arrival (0x4080d0): everything timed is cancelled and
        // the clocks and the panel go.
        if matches!(s.phase, Phase::Arrived | Phase::Cleared) && b.phase != s.phase {
            self.captions.stop_all();
            self.hints.hide();
            self.panel_active = false;
            self.clocks_visible = false;
        }

        // The `NaviAnim` calls of this tick (rules, 0x40c4e0, 0x40a0a0, the train task).
        let navi_tick = NaviTick {
            before: b.navi,
            after: s.navi,
            events,
            segment_started: input.segment_started,
            game_over: self.game_over_ticks.is_some(),
            loop_exit: self
                .game_over_ticks
                .is_some_and(|n| n + 1 == TIME_UP_AFTER_TICKS),
        };
        // `NaviAnim` (0x410cb0) does nothing while `0x84c0f8 == 1`: no reactions (nor the
        // obstacle's emergency-stop signal) under the coupling minigame's bottom band.
        let band = self.coupling.bottom_band();
        for call in crate::navigator::calls(&navi_tick, &self.navi_tables) {
            if band && call != crate::navigator::Call::Clear {
                continue;
            }
            self.navigator
                .apply(call, &self.navi_tables, &mut |e| navi_draw(layer, e));
        }

        // List 3 in creation order: departure task, signal/sign/message/lamp/popup tasks, hints,
        // captions, then the HUD task.
        if b.runs_departure_task() && !input.segment_started && !self.popups.time_up_running() {
            self.panel_active = true;
            let c = b.departure_counter;
            if let Some(doors) = self.departure.update(layer, c, b.departure_base) {
                if let Some(id) = doors.name_id {
                    let kind = DISTANCE | u32::try_from(s.aspect).unwrap_or(0);
                    self.captions
                        .banner(layer, id, kind, 10, DEPARTURE_BANNER_Y, rules_odometer);
                }
                self.lamp_lit = true;
            }
        }
        self.popups
            .update(layer, s.odometer, s.clock, input.horn_held);
        // After the signal task, so the emergency-stop signal wins slot 0x60.
        self.navigator
            .update(&self.navi_tables, &mut |e| navi_draw(layer, e));
        self.hints.update(layer, s.odometer);
        self.hints
            .handling_error(layer, s.train_driving, b.power, b.brake);
        self.captions
            .update(layer, self.popups.message_up(), s.odometer);
        if let Some(names) = self.board_names.as_ref() {
            self.departure.draw_names(names, layer);
        }

        let frozen = self.clock_freeze > 0;
        if frozen {
            self.clock_freeze -= 1;
        }
        if s.bonus {
            self.panel_active = false;
        }
        let plates = self.departure.plates;
        let state = HudState {
            power: u8::try_from(s.power).unwrap_or(0),
            brake: u8::try_from(s.brake).unwrap_or(0),
            remaining_s: input.remaining_override.unwrap_or(s.remaining_s),
            pending_s: s.pending_s,
            hud_counter: s.hud_counter,
            clock: s.clock,
            scheduled: s.scheduled,
            clocks_visible: self.clocks_visible,
            clocks_frozen: frozen,
            remaining_visible: s.drive_phase != DrivePhase::Cleared
                && !self.coupling.hides_remaining(s.coupling.as_ref())
                && !s.bonus,
            remaining_label_kept: self.coupling.hides_remaining(s.coupling.as_ref()),
            panel_active: self.panel_active,
            panel_on: self.panel_on,
            next_is_stop: s.target_stops,
            dist_m: s.dist_m,
            dist_round_m: s.dist_round_m,
            tolerance_m: input.tolerance_m,
            speed_kmh: s.speed_kmh,
            plates: [
                plates.0.and_then(|id| self.hud.plate(id)),
                plates.1.and_then(|id| self.hud.plate(id)),
            ],
            vehicle: self.vehicle,
            speed_mode: self.speed_mode,
            atc_index: self.atc_index,
            hint_up: self.hints.up(),
            waiting: s.waiting(),
            train_driving: s.train_driving,
            lamp_lit: self.lamp_lit,
            bottom_band: false,
        };
        self.hud.hud_update(&state, layer);
        // Created by a course event, after the HUD task in list 3.
        if let Some(r) = self.ready.as_mut() {
            let busy = self.navigator.busy();
            if !r.update(&self.ready_tables, busy, &mut |e| navi_draw(layer, e)) {
                self.ready = None;
            }
        }
        self.coupling.update(s.coupling.as_ref(), events, layer);
        // `Cab_Overlay` comes at the end of the frame, after every task (`Frame_End`).
        let state = HudState {
            bottom_band: self.coupling.bottom_band(),
            ..state
        };
        self.hud.cab_overlay(&state, layer);

        // The time-up popup is created after the 20th `Task_Tick` past the game over; its first
        // update is in the next tick.
        if let Some(n) = self.game_over_ticks.as_mut() {
            *n += 1;
            if *n == TIME_UP_AFTER_TICKS {
                self.time_up(s, layer);
            }
        }
        crate::hud::anchor_slots(layer);
        self.prev = Some(s.clone());
    }

    /// The HUD of an attract demo drive (0x40e4b0 / 0x40e6e0): plates of the first station and
    /// of the first stop after it (0x40f780), clocks and the distance panel on (`0x856c80`,
    /// `0x856d0c`), the departure lamp `0x829d16` lit by the route demo at once.
    pub fn demo_setup(&mut self, s: &Snap, lamp: bool) {
        self.demo = true;
        // The demos set `0x855ff8 |= 0x204`, not the stage-run bit 2: no speedometer.
        self.hud.stage_run = false;
        self.clocks_visible = true;
        self.panel_active = true;
        self.lamp_lit = lamp;
        let first_stop = (1..s.stops.len()).find(|&i| s.stops[i]).unwrap_or(1);
        self.departure.plates = (self.name_id(0), self.name_id(first_stop));
    }

    /// `0x434fb0(1)`: the departure lamp `0x829d16` goes on.
    pub fn light_lamp(&mut self) {
        self.lamp_lit = true;
    }

    /// The segment start: 0x4080d0 sets the clocks and remaining time visible, 0x40cf40 resets
    /// the approach and gradient state, 0x4124f0 sets up the departure HUD.
    fn segment_setup(&mut self, s: &Snap, layer: &mut SpriteLayer) {
        self.popups.clear();
        self.hints.clear(layer);
        // List 3 is rebuilt: the coupling-ready task's destructor hides its slot.
        if self.ready.take().is_some() && !self.navigator.busy() {
            crate::slots::image(layer, crate::navigator::SLOT_NAVI, -1);
        }
        self.captions.reset_approach();
        self.clocks_visible = true;
        self.lamp_lit = false;
        self.game_over_ticks = None;
        let marks = |st: usize| {
            st.checked_sub(1)
                .and_then(|i| s.marks.get(i))
                .copied()
                .unwrap_or(0)
        };
        self.departure.segment_setup(
            &Segment {
                stage: self.stage,
                station: s.station,
                stops: &s.stops,
                mark_at: &marks,
                name_ids: &self.name_ids,
            },
            layer,
        );
    }

    /// `0x856d84`: category of the last 0x40dd60 call, reset by 0x40dd10 (stage start,
    /// continue). Several categories in one tick: the highest wins (call order not modelled).
    fn track_stats(&mut self, b: &Snap, s: &Snap) {
        if s.continues != b.continues {
            self.last_stat = -1;
            return;
        }
        if let Some(cat) = (0..STAT_COUNT).rev().find(|&c| s.stats[c] > b.stats[c]) {
            self.last_stat = cat as i32;
        }
    }

    /// 0x4158b0.
    fn time_up(&mut self, s: &Snap, layer: &mut SpriteLayer) {
        let cause = if s.stats[2] > 1 { 2 } else { self.last_stat };
        self.panel_active = false;
        self.hints.hide();
        self.captions.stop_all();
        self.popups
            .time_up(layer, time_up_advice(cause, s.early_ticks >= 1));
    }
}

/// A navigator animation's slot change (0x431920 / 0x4318c0); its sounds are the sound
/// module's.
fn navi_draw(layer: &mut SpriteLayer, e: Effect) {
    match e {
        Effect::Draw { slot, tbl, palette } => {
            crate::slots::image_palette(layer, slot, tbl, palette);
        }
        Effect::Hide(slot) => crate::slots::image(layer, slot, -1),
        Effect::Sound(_) => {}
    }
}

//! One run of one stage, tick by tick: rules, train physics and the in-cab track walk together,
//! with every track-derived input computed from the route data as the original does.
//!
//! The game binary calls [`Drive::tick`] once per 33 ms with the latched notches and buttons and
//! drives the stop flow with [`Drive::finish_station`] and [`Drive::continue_run`] from its
//! menus. Rendering reads [`Drive::pos`] / [`Drive::prev_pos`] (the camera's track position after
//! the last two ticks) and the HUD reads the [`Rules`] and [`TrainState`] accessors.
//!
//! # One tick (`docs/re/physics.md` §1, `docs/re/main-loop.md` §5)
//! 1. Track lookup 0x407c10 (driving iterations only): gradient `0x857228` and the track sound
//!    class 4 points ahead (`0x857202`, 3 = tunnel), from the camera position of the last tick.
//! 2. The rule functions of the loop body ([`Rules::step`]); the distance to the stop is
//!    `0x8500c8 × 10`. Passing a non-stop station adds the next leg to `0x8500c8` (0x41c820) and
//!    clears the leg distance (0x40d620); the 50 m leg marker of 0x40c4e0 follows.
//! 3. `Task_Tick`: the train task 0x422c20 ([`TrainState::step`]), then the in-cab camera task
//!    0x41d160 ([`CabWalker::advance`] by `0x850208`, `0x8500c8 −= 0x850208`, heading
//!    `0x850288` of the new chord), then the rule parts of the train, departure and HUD tasks
//!    ([`Rules::end_tick`]).
//!
//! Outside the loops (after the route load, on result and continue screens) only the train
//! task and the HUD run ([`Rules::idle_tick`]). The number of such ticks before a segment's
//! setup tick is fixed by `Stage_Run` (0x4080d0) and reproduced here (see [`Drive::finish_station`]
//! and [`Drive::continue_run`]); oracle traces confirm every count (`shuppatsu-tools trace-replay
//! --drive`).
//!
//! Route data loading stays outside the engine: a [`RouteLoader`] passed to [`Drive::new`]
//! returns the bytes of a `.COM` file by its name; it is called on segment changes.
//!
//! No serde derives: the state holds a parsed [`Route`] and the rule tables, whose types live in
//! `shuppatsu-formats` without serde. A trace of [`Drive`] state can be written from the accessors.

use crate::bonus::{self, BonusEvent, BonusGame};
use crate::demo::{DemoInputs, DemoKind, DemoTables};
use crate::physics::{Controls, TickContext, TrainSpec, TrainState, TrainTables, gradient_at};
use crate::rules::{
    Buttons, FadeSplit, Phase, RuleEvent, RuleTables, Rules, RulesError, StationOutcome,
    StationResult, TickInput, TickOutput, TrainReadout,
};
use crate::traffic::{CarPose, PlacedTrain, SpawnRequest, Traffic, TrafficEnv, TrafficSound};
use shuppatsu_formats::FormatError;
use shuppatsu_formats::route::{Route, UNITS_PER_POINT};
use shuppatsu_formats::scene::{CabWalker, TrackPos, cab_camera};
use shuppatsu_formats::stages::{Stage, StageTables, TimetableRow};
use shuppatsu_formats::traffic::TrainModels;

/// Error returned by a [`RouteLoader`].
pub type LoadError = Box<dyn std::error::Error + Send + Sync>;

/// Supplies `.COM` route files by name (the lower-case names of the exe's COM table; match
/// them case-insensitively on disk).
pub trait RouteLoader {
    fn load(&mut self, com_file: &str) -> Result<Vec<u8>, LoadError>;
}

impl<F> RouteLoader for F
where
    F: FnMut(&str) -> Result<Vec<u8>, LoadError>,
{
    fn load(&mut self, com_file: &str) -> Result<Vec<u8>, LoadError> {
        self(com_file)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DriveError {
    #[error(transparent)]
    Rules(#[from] RulesError),
    #[error("parsing a route: {0}")]
    Format(#[from] FormatError),
    #[error("loading {file}: {source}")]
    Load { file: String, source: LoadError },
    #[error("stage {stage} out of range")]
    BadStage { stage: usize },
    #[error("stage {stage} has no route segment for station {station}")]
    NoSegment { stage: usize, station: usize },
    #[error("COM index {0} has no file name")]
    NoComFile(u32),
    #[error("{file}: stop {stop} and the next one are not both in the route")]
    BadStop { file: String, stop: usize },
    #[error("{call} is not valid in phase {phase:?}")]
    WrongPhase {
        call: &'static str,
        phase: DrivePhase,
    },
}

/// Where the run is, for menus, HUD and rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrivePhase {
    /// Ticks between a (re)start and the segment's setup tick: route load, screens closing.
    /// The train is held; nothing moves.
    Loading,
    /// Doors / departure countdown (`0x855ff8 & 0x800`).
    PreDeparture,
    /// The driving loop (`0x855ff8 & 0x200`).
    Driving,
    /// A blocking sequence: stop evaluation, red-signal / ATS recovery, obstacle.
    Holding,
    /// The coupling minigame after a perfect stop at a coupling station; see
    /// [`Rules::coupling`](crate::rules::Rules::coupling).
    Coupling,
    /// The bonus game (0x41b540) between a station result and the next segment; see
    /// [`Drive::bonus`].
    Bonus,
    /// Stopped at a station and evaluated: show the result, then call [`Drive::finish_station`].
    Arrived,
    /// Out of time. The train coasts to a stop if it was moving; offer
    /// [`Drive::continue_run`] unless [`Drive::can_continue`] is false.
    GameOver,
    /// The final station was reached.
    Cleared,
}

/// The player's controls for one tick, already latched by the game's input code (the original
/// polls at the end of the previous tick, `docs/re/input.md`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DriverInput {
    /// 0 = off, 1..=5 = P1..P5.
    pub power: u8,
    /// 0 = released, 1..=8 = B1..B8, 9 = EB.
    pub brake: u8,
    pub horn_held: bool,
    pub horn_pressed: bool,
    /// A / confirm pressed this tick (ATS acknowledgement).
    pub confirm_pressed: bool,
}

/// What one tick produced.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DriveTick {
    /// Rule events of this tick (time messages, signals, limits, stations, …).
    pub events: Vec<RuleEvent>,
    /// The ATC tripped (speed above the course's derailment limit).
    pub atc_tripped: bool,
    /// This tick was a segment's setup tick: the route or the track position jumped, so the
    /// renderer must not interpolate from [`Drive::prev_pos`] (it equals [`Drive::pos`]).
    pub segment_started: bool,
    /// The rules forced the notches to N / EB for this tick (`input_force_notches(0, 9)`,
    /// 0x4332c0, in the red-signal / ATS recovery of 0x40a0a0; not with analog levers): the
    /// caller resets its digital levers as at a segment start.
    pub notches_forced: bool,
}

/// Ticks from the route load (0x4229c0) to the first segment's setup tick: the tick in
/// 0x40eed0 and the loop's `Task_Tick` (oracle: the physics task's first row is two rows before
/// the first `0x800` row in all three traces).
const LOAD_TICKS_AT_START: u32 = 2;
/// Ticks from the next-segment step 0x40d340 to the setup tick: `Task_Tick` after 0x40d340,
/// 0x4088ac's, and two at the top of the segment loop (oracle: hilly/stops traces).
const LOAD_TICKS_AFTER_STATION: u32 = 4;
/// Ticks from the continue rewind 0x40d930 to the setup tick (one fewer: no `Task_Tick`
/// between 0x40d340 and the 0x400 wait on this path).
const LOAD_TICKS_AFTER_CONTINUE: u32 = 3;
/// Ticks from the close of the continue screen to the rewind: `Task_Tick`s at 0x408839 and
/// 0x40883e (oracle: hilly-drive.csv rows 7775 → 7778).
const CONTINUE_REWIND_DELAY: u32 = 2;
/// The continue path that first moves on to the target station runs 10 extra ticks (0x4080d0).
const CONTINUE_IN_APPROACH_EXTRA_TICKS: u32 = 10;
/// After a game over the loop runs 20 `Task_Tick`s, then discards the pending time (0x4085d0).
const GAME_OVER_DRAIN_TICKS: u32 = 0x14;
/// The stop error is added to the odometer (0x40869d: `0x857004 += 0x85708c`) this many ticks
/// after the rules reached [`Phase::Arrived`] (`Task_Tick` at 0x408588 and 0x408685).
/// Oracle: stops-drive.csv rows 4171 / 8028.
const ARRIVAL_ODOMETER_TICK: u32 = 2;
/// Track sound class of a tunnel (0x857202 == 3, 0x407c10 / 0x40c4e0).
const TUNNEL_CLASS: i16 = 3;
/// The track sound lookup probes the point this far ahead of the camera point (0x407c10).
const CLASS_PROBE_AHEAD: usize = 4;
/// The three probes of the ambient track sounds, in points from the camera point
/// (`local_c = {-0x10, 4, 0x18}` in 0x407c10).
const SOUND_PROBES: [isize; 3] = [-16, 4, 24];
/// The turnout test reads the flags of the point this far ahead (0x407c10: index
/// `0x8502a4·3 + 0x12` dwords = point + 6, field +10).
const TURNOUT_PROBE_AHEAD: usize = 6;
/// Track units → metres (double 10.0 at 0x458b88, 0x40dad0).
const METRES_PER_UNIT: f64 = 10.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// `n` idle ticks left before the setup tick.
    Load(u32),
    /// The rules drive the flow.
    Run,
}

/// One stage run. See the module documentation.
pub struct Drive {
    rules: Rules,
    train: TrainState,
    spec: TrainSpec,
    /// Every train type (`Vehicle_Set` in the bonus sections switches types mid-run).
    specs: Vec<TrainSpec>,
    stage_data: Stage,
    com_files: Vec<Option<String>>,
    loader: Box<dyn RouteLoader + Send>,

    step: Step,
    route: Route,
    com: u32,
    /// Route of the next segment, loaded by `finish_station` / `continue_run` and installed at
    /// the setup tick.
    next_route: Option<(u32, Route)>,
    walker: CabWalker,
    prev_pos: TrackPos,
    /// Stop index in the `.COM` stop list of the station the train left last (0x850214 − 1).
    stop_index: usize,
    /// Track units to the stop point (0x8500c8).
    units_to_stop: f32,
    /// Gradient (0x857228), heading of the camera chord (0x850288), track sound class four
    /// points ahead (0x857202).
    gradient: f32,
    heading: f32,
    track_class_ahead: i16,
    /// Presentation only (sound): the classes at the three ambient probes (`0x857200[3]`),
    /// the turnout latch `0x857096` and its rising edge `0x856d96`, and the tick of the last
    /// lookup. None of it feeds the simulation.
    track_classes: [i16; 3],
    on_turnout: bool,
    turnout_entered: bool,
    lookup_tick: Option<u64>,
    /// The game ended while driving: the train coasts (flags 0x210) until the continue screen.
    coasting: bool,
    /// Ticks since the game over / the arrival.
    game_over_ticks: u32,
    arrived_ticks: u32,
    odometer_settled: bool,
    tick_count: u64,
    /// Idle ticks until the deferred continue rewind (0x40d930).
    pending_continue: Option<u32>,
    /// The global tick counter `0x84c494` after the last tick (see [`Drive::set_global_tick`]).
    global_tick: u32,
    /// AI trains (task list 5) and their model table.
    traffic: Traffic,
    train_models: TrainModels,
    /// An attract demo drive in progress ([`Drive::start_demo`]).
    demo: Option<Demo>,
    /// The bonus game (0x41b540): due after the next idle tick, running, and the vehicle code
    /// to restore, the horn of the last tick.
    bonus_due: bool,
    bonus: Option<BonusGame>,
    bonus_saved_vehicle: Option<u32>,
    bonus_horn: bool,
    /// The sky fade 0x40f320 that ran in the last tick (that tick was its `Task_Tick`).
    sky_fade: Option<FadeSplit>,
    /// The levers are analog (joystick modes 5/6): `input_force_notches` leaves them alone.
    analog_levers: bool,
}

impl std::fmt::Debug for Drive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Drive")
            .field("phase", &self.phase())
            .field("com", &self.com)
            .field("station", &self.rules.station)
            .field("pos", &self.walker.pos())
            .field("units_to_stop", &self.units_to_stop)
            .field("speed", &self.train.speed)
            .finish_non_exhaustive()
    }
}

impl Drive {
    /// Starts a run of `stage` at `difficulty` from station `start_station` (0x40d040 stage
    /// start, then the route load 0x4229c0). The first segment's setup tick is the third call
    /// of [`Drive::tick`].
    pub fn new(
        train_tables: &TrainTables,
        rule_tables: &RuleTables,
        stages: &StageTables,
        stage: usize,
        difficulty: usize,
        start_station: usize,
        loader: impl RouteLoader + Send + 'static,
    ) -> Result<Self, DriveError> {
        let stage_data = stages
            .stages
            .get(stage)
            .ok_or(DriveError::BadStage { stage })?
            .clone();
        let rules = Rules::new(rule_tables, &stage_data, stage, difficulty, start_station)?;
        let params = train_tables
            .stage_params(stage, difficulty)
            .ok_or(DriveError::BadStage { stage })?;
        let spec = *train_tables
            .specs
            .get(params.train_type)
            .ok_or(DriveError::BadStage { stage })?;
        let mut loader: Box<dyn RouteLoader + Send> = Box::new(loader);
        let (seg, stop) = segment_for(&stage_data, stage, start_station)?;
        let route = load_route(&mut *loader, &stages.com_files, seg)?;
        let stop = stop as usize;
        check_stop(&route, &stages.com_files, seg, stop)?;
        let walker = CabWalker::at_segment_start(&route, stop);
        // The heading 0x850288 at the route load belongs to the stage intro's camera; the
        // first idle tick overwrites the physics copy with |heading| anyway.
        let heading = 0.0;
        let mut train = TrainState::new(params, heading);
        train.odometer = rules.odometer_m;
        let traffic = Traffic::new(&stage_data.traffic_events, start_station);
        Ok(Self {
            rules,
            train,
            spec,
            specs: train_tables.specs.clone(),
            stage_data,
            com_files: stages.com_files.clone(),
            loader,
            step: Step::Load(LOAD_TICKS_AT_START),
            route,
            com: seg,
            next_route: None,
            prev_pos: walker.pos(),
            walker,
            stop_index: stop,
            units_to_stop: 0.0,
            gradient: 0.0,
            heading,
            track_class_ahead: 0,
            track_classes: [0; 3],
            on_turnout: false,
            turnout_entered: false,
            lookup_tick: None,
            coasting: false,
            game_over_ticks: 0,
            arrived_ticks: 0,
            odometer_settled: false,
            tick_count: 0,
            pending_continue: None,
            global_tick: 0,
            traffic,
            train_models: stages.train_models.clone(),
            demo: None,
            bonus_due: false,
            bonus: None,
            bonus_saved_vehicle: None,
            bonus_horn: false,
            sky_fade: None,
            analog_levers: false,
        })
    }

    // ----------------------------------------------------------------------------------------
    // Accessors
    // ----------------------------------------------------------------------------------------

    pub fn phase(&self) -> DrivePhase {
        if self.bonus.is_some() {
            return DrivePhase::Bonus;
        }
        match self.step {
            Step::Load(_) => DrivePhase::Loading,
            Step::Run => match self.rules.phase {
                Phase::PreDeparture => DrivePhase::PreDeparture,
                Phase::Driving => DrivePhase::Driving,
                Phase::Holding { .. } | Phase::Draining | Phase::Evaluating(_) => {
                    DrivePhase::Holding
                }
                Phase::Coupling => DrivePhase::Coupling,
                Phase::Arrived => DrivePhase::Arrived,
                Phase::GameOver => DrivePhase::GameOver,
                Phase::Cleared => DrivePhase::Cleared,
            },
        }
    }

    /// Whether the departure countdown is still running (`0x855ff8 & 0x800`).
    pub fn waiting_for_departure(&self) -> bool {
        self.step == Step::Run
            && self.rules.phase == Phase::PreDeparture
            && !self.rules.departure_done
    }

    /// Whether the train task runs its driving branch this tick (`0x855ff8 & 0x200`).
    pub fn train_driving(&self) -> bool {
        self.step == Step::Run
            && (self.rules.train_driving()
                || (self.rules.phase == Phase::GameOver && self.coasting))
    }

    /// A continue is offered after a game over unless the target was the final station
    /// (0x4080d0 sets `0x10` only when `0x8570ae == 0`).
    pub fn can_continue(&self) -> bool {
        self.phase() == DrivePhase::GameOver && !self.rules.final_station
    }

    /// The rule state: remaining / pending time, clock, signals, limits, ATS, statistics.
    pub fn rules(&self) -> &Rules {
        &self.rules
    }

    /// The train state: speed, odometer, brake cylinder, forces.
    pub fn train(&self) -> &TrainState {
        &self.train
    }

    pub fn train_spec(&self) -> &TrainSpec {
        &self.spec
    }

    pub fn stage_data(&self) -> &Stage {
        &self.stage_data
    }

    /// The `.COM` index of the loaded segment.
    pub fn com(&self) -> u32 {
        self.com
    }

    /// The `.COM` file name of the loaded segment.
    pub fn com_name(&self) -> &str {
        self.com_files
            .get(self.com as usize)
            .and_then(|n| n.as_deref())
            .unwrap_or("")
    }

    pub fn route(&self) -> &Route {
        &self.route
    }

    /// The in-cab camera walk (position and live scenery list).
    pub fn walker(&self) -> &CabWalker {
        &self.walker
    }

    /// Camera track position after the last tick.
    pub fn pos(&self) -> TrackPos {
        self.walker.pos()
    }

    /// Camera track position after the tick before; interpolate between the two along the
    /// track (`cab_camera` at an intermediate `TrackPos`). Equal to [`Drive::pos`] after a
    /// segment start.
    pub fn prev_pos(&self) -> TrackPos {
        self.prev_pos
    }

    /// Stop index in the loaded `.COM`'s stop list of the station left last.
    pub fn stop_index(&self) -> usize {
        self.stop_index
    }

    /// Track units to the stop point (0x8500c8).
    pub fn units_to_stop(&self) -> f32 {
        self.units_to_stop
    }

    /// Distance to the stop point in metres as the HUD shows it (0x85708c; negative = overrun).
    pub fn dist_to_stop_m(&self) -> f32 {
        self.rules.dist_m
    }

    pub fn speed_mps(&self) -> f32 {
        self.train.speed
    }

    pub fn speed_kmh(&self) -> f32 {
        self.train.speed_kmh()
    }

    /// Gradient (0x857228) as last looked up.
    pub fn gradient(&self) -> f32 {
        self.gradient
    }

    /// Heading of the camera's track chord, radians (0x850288).
    pub fn heading(&self) -> f32 {
        self.heading
    }

    /// Track sound class four points ahead (0x857202): 1–3 ambient sound classes, 3 = tunnel.
    pub fn track_class_ahead(&self) -> i16 {
        self.track_class_ahead
    }

    /// The ambient-sound track classes 16 points behind, 4 ahead and 24 ahead of the camera
    /// point (`0x857200[3]`), if this tick ran the track lookup 0x407c10 (driving-loop ticks
    /// only). A probe before the route start reads 0 (the original reads whatever precedes its
    /// point array); one past the end reads 0 like [`Drive::track_class_ahead`].
    pub fn track_classes(&self) -> Option<[i16; 3]> {
        (self.lookup_tick == Some(self.tick_count)).then_some(self.track_classes)
    }

    /// Whether this tick's track lookup found the point 6 ahead to be the first point of a
    /// turnout (`0x856d96`, rising edge of `0x857096`); the train task then starts the
    /// turnout clatter (0x4205c0). Presentation only.
    pub fn turnout_entered(&self) -> bool {
        self.lookup_tick == Some(self.tick_count) && self.turnout_entered
    }

    /// Index of the next station (the target), and its timetable row.
    pub fn next_station(&self) -> (usize, Option<&TimetableRow>) {
        let s = self.rules.station + 1;
        (s, self.rules.timetable().get(s))
    }

    /// Romanised name of a station, if the exe has one.
    pub fn station_name(&self, station: usize) -> Option<&str> {
        self.stage_data
            .station_names
            .get(station)
            .map(|s| s.trim_end())
    }

    /// Ticks run so far.
    pub fn tick_count(&self) -> u64 {
        self.tick_count
    }

    /// The AI trains: cars, event cursor, traffic state (`docs/re/traffic.md`).
    pub fn traffic(&self) -> &Traffic {
        &self.traffic
    }

    /// Where to draw the AI-train cars, `alpha` of the way from the tick before to the last
    /// tick (interpolated along the track).
    pub fn traffic_poses(&self, alpha: f32) -> Vec<CarPose> {
        self.traffic.poses(&self.route, alpha)
    }

    /// The AI-train sound commands of the last tick (passing cars, the recovery's passing
    /// train). Presentation only.
    pub fn traffic_sounds(&self) -> &[TrafficSound] {
        self.traffic.sounds()
    }

    /// Sets the HUD task's tick counter (task+0x34 of 0x40f7e0's task). The remaining-time
    /// transfer runs on every third tick of that counter, which starts at the stage load, a
    /// number of ticks before the route load that depends on how long the stage intro ran.
    /// Only needed to line up with a recording of the original.
    pub fn set_hud_counter(&mut self, counter: u32) {
        self.rules.hud_counter = counter;
    }

    /// Analog levers (joystick modes 5/6, input.md §4.1): `input_force_notches` (0x4332c0)
    /// does not touch them, so the red-signal / ATS recovery keeps the driver's notches.
    pub fn set_analog_levers(&mut self, analog: bool) {
        self.analog_levers = analog;
    }

    // ----------------------------------------------------------------------------------------
    // Flow
    // ----------------------------------------------------------------------------------------

    /// Turns this run, freshly made by [`Drive::new`] at station 0, into an attract demo drive
    /// (0x40e4b0 / 0x40e6e0): the segment is set up at once, the train runs on the exe's input
    /// table and [`Drive::tick`] runs the demo task instead of the stage loop. The demo ends
    /// when [`Drive::demo_finished`].
    pub fn start_demo(&mut self, kind: DemoKind, tables: &DemoTables) {
        let stage = self.rules.stage().0;
        let stop =
            segment_for(&self.stage_data, stage, self.rules.station).map_or(0, |(_, s)| s as usize);
        self.stop_index = stop;
        self.walker = CabWalker::at_segment_start(&self.route, stop);
        self.prev_pos = self.walker.pos();
        if let (Some(&a), Some(&b)) = (self.route.stops.get(stop), self.route.stops.get(stop + 1)) {
            self.units_to_stop = ((b as f64 - a as f64) * f64::from(UNITS_PER_POINT)) as f32;
        }
        self.step = Step::Run;
        let rin = self.rule_input(&DriverInput::default());
        let course = (kind == DemoKind::Lesson).then(|| tables.lesson_course.clone());
        self.rules.start_demo(&rin, course);
        self.demo = Some(Demo {
            kind,
            counter: 0,
            inputs: DemoInputs::default(),
            tables: tables.clone(),
            last: DriverInput::default(),
        });
    }

    /// The demo drive's task has ended (its counter reached [`DemoKind::ticks`]).
    pub fn demo_finished(&self) -> bool {
        self.demo
            .as_ref()
            .is_some_and(|d| d.counter >= d.kind.ticks())
    }

    /// The ending's camera placement (0x408080 → 0x41cb00): the camera task of the segment
    /// holding `station`, at that station's stop, with the route loaded and the AI trains gone
    /// (a new list 5).
    pub fn ending_place(&mut self, station: usize) -> Result<(), DriveError> {
        let stage = self.rules.stage().0;
        let (seg, stop) = segment_for(&self.stage_data, stage, station)?;
        if seg != self.com {
            self.route = load_route(&mut *self.loader, &self.com_files, seg)?;
            self.com = seg;
        }
        let stop = stop as usize;
        check_stop(&self.route, &self.com_files, seg, stop)?;
        self.walker = CabWalker::at_segment_start(&self.route, stop);
        self.prev_pos = self.walker.pos();
        self.traffic.clear();
        Ok(())
    }

    /// One tick of the ending's list 5: the camera task advances by `units` (`0x850208`, track
    /// units; negative goes back) and the AI-train tasks run. The player train is gone.
    pub fn ending_tick(&mut self, units: f32) {
        self.tick_count += 1;
        self.traffic.begin_tick();
        self.prev_pos = self.walker.pos();
        self.walker.advance(&self.route, units);
        self.heading = crate::physics::heading_at(&self.route, self.walker.pos());
        self.traffic_task();
        self.global_tick = self.global_tick.wrapping_add(1);
    }

    /// A tick without list 5 (the ending killed it): only the global counter runs.
    pub fn idle_global_tick(&mut self) {
        self.tick_count += 1;
        self.traffic.begin_tick();
        self.global_tick = self.global_tick.wrapping_add(1);
    }

    /// The spawner 0x404f10 on the current route (the ending's final shot).
    pub fn ending_spawn(&mut self, req: &SpawnRequest) {
        let env = TrafficEnv {
            route: &self.route,
            models: &self.train_models,
            obstacle_armed: false,
            dist_round_m: self.rules.dist_round_m,
            tick: u64::from(self.global_tick),
        };
        self.traffic.spawn(req, &env);
    }

    /// 0x406960 on the current route: a train placed relative to the camera's engine index
    /// `0x8502a4` (the stage intro's chase, 0x408f40).
    pub fn ending_place_train(&mut self, train: &PlacedTrain) {
        let env = TrafficEnv {
            route: &self.route,
            models: &self.train_models,
            obstacle_armed: false,
            dist_round_m: self.rules.dist_round_m,
            tick: u64::from(self.global_tick),
        };
        self.traffic
            .place_train(train, self.walker.pos().engine_index, &env);
    }

    /// Kills list 5: the camera task and the AI trains.
    pub fn ending_kill(&mut self) {
        self.traffic.clear();
    }

    /// The demo's notches and horn of the last tick (for the cab and the sounds).
    pub fn demo_controls(&self) -> Option<DriverInput> {
        self.demo.as_ref().map(|d| d.last)
    }

    /// The demo drive's counter (task+0x30), `None` outside a demo.
    pub fn demo_counter(&self) -> Option<u32> {
        self.demo.as_ref().map(|d| d.counter)
    }

    /// Runs one 1/30 s tick.
    pub fn tick(&mut self, input: DriverInput) -> DriveTick {
        self.sky_fade = None;
        if self.demo.is_some() {
            return self.demo_tick();
        }
        self.tick_count += 1;
        self.traffic.begin_tick();
        self.rules.global_tick = Some(self.global_tick);
        self.prev_pos = self.walker.pos();
        let out = match self.step {
            Step::Load(_) if self.bonus.is_some() => self.bonus_tick(input),
            Step::Load(0) => self.setup_tick(input),
            Step::Load(n) => {
                self.step = Step::Load(n - 1);
                if std::mem::take(&mut self.bonus_due) {
                    // After the `Task_Tick` at 0x4088ac: 0x40ee50(0x14), BonusGame_Create.
                    self.bonus = Some(BonusGame::new(&self.rules.tables().bonus));
                    self.bonus_saved_vehicle = self.rules.vehicle_code;
                    self.rules.vehicle_code = Some(bonus::VEHICLE);
                    self.bonus_horn = false;
                }
                match self.pending_continue {
                    Some(0) => {
                        self.pending_continue = None;
                        self.rules.continue_run();
                        self.traffic
                            .rewind(&self.stage_data.traffic_events, self.rules.station);
                        self.train.odometer = self.rules.odometer_m;
                    }
                    Some(k) => self.pending_continue = Some(k - 1),
                    None => {}
                }
                self.idle_tick(input)
            }
            Step::Run => self.run_tick(input),
        };
        // `Task_Tick` counts the tick after the task lists ran (main-loop.md §2).
        self.global_tick = self.global_tick.wrapping_add(1);
        out
    }

    /// The sky fade 0x40f320 started in the last tick: the weather start (to the darker
    /// gradient set) or end (back), and the last tick was the fade's own `Task_Tick` in the
    /// middle of the driving loop's body (the fade task 0x436a60 had its first update). The
    /// next tick finishes that body; both ticks are ordinary 1/30 s ticks.
    pub fn sky_fade(&self) -> Option<FadeSplit> {
        self.sky_fade
    }

    /// The global tick counter `0x84c494` after the last tick: every unpaused `Task_Tick`
    /// since boot, menus included. It decides the station-based weather starts and reseeds the
    /// AI-train spawner's random speeds.
    pub fn global_tick(&self) -> u32 {
        self.global_tick
    }

    /// Sets the counter (at the stage start: the ticks the game has run so far). Each
    /// [`Drive::tick`] then adds one; paused ticks, which do not call it, don't count.
    pub fn set_global_tick(&mut self, tick: u32) {
        self.global_tick = tick;
        self.rules.global_tick = Some(tick);
    }

    /// The station result (0x413d90's deductions) and the move on (0x40d340). Call once in
    /// [`DrivePhase::Arrived`], when the result screen has closed: the original then runs one
    /// more tick before 0x40d340, so call it after that tick. The next segment's setup tick
    /// follows four ticks later. Loads the next segment's route if it changes.
    pub fn finish_station(&mut self) -> Result<StationResult, DriveError> {
        if self.phase() != DrivePhase::Arrived {
            return Err(DriveError::WrongPhase {
                call: "finish_station",
                phase: self.phase(),
            });
        }
        self.settle_odometer();
        self.rules.global_tick = Some(self.global_tick);
        let result = self.rules.finish_station();
        if self.rules.bonus_jump {
            // 0x40d340 moved to the bonus section: the odometer and the AI-train cursor follow
            // (`0x856d90` skipped to station 24 like the course events).
            self.train.odometer = self.rules.odometer_m;
            self.traffic
                .skip_to(&self.stage_data.traffic_events, self.rules.station);
        }
        self.sync_train_type();
        // 0x4080d0 after the station result: `0x857216 = (0x8570b8 < 1)`.
        self.traffic.after_arrival(self.rules.continues);
        if result.outcome == StationOutcome::Continue {
            self.prepare_segment(self.rules.station, LOAD_TICKS_AFTER_STATION)?;
        }
        // Stage_Run: the bonus game after the result of its station, without a continue.
        let stage = self.rules.stage().0;
        self.bonus_due = result.outcome == StationOutcome::Continue
            && self.rules.continues == 0
            && self.rules.tables().bonus.at(stage, self.rules.station);
        self.coasting = false;
        self.game_over_ticks = 0;
        Ok(result)
    }

    /// Continue after a game over. Call right after the tick in which the continue screen
    /// closed with "continue". The coast ends at once; two idle ticks later the original
    /// rewinds (0x40d930; when the time ran out inside the station approach, 0x40d340 first
    /// moves on to the target station), and the setup tick follows three idle ticks after
    /// that (thirteen on the approach path). Loads the restart segment's route now.
    pub fn continue_run(&mut self) -> Result<(), DriveError> {
        if !self.can_continue() {
            return Err(DriveError::WrongPhase {
                call: "continue_run",
                phase: self.phase(),
            });
        }
        let extra = if self.rules.approach > 0 {
            CONTINUE_IN_APPROACH_EXTRA_TICKS
        } else {
            0
        };
        self.prepare_segment(
            self.rules.continue_station(),
            CONTINUE_REWIND_DELAY + LOAD_TICKS_AFTER_CONTINUE + extra,
        )?;
        self.pending_continue = Some(CONTINUE_REWIND_DELAY);
        // Task list 5 is cleared when the continue screen closes (0x4080d0).
        self.traffic.clear();
        self.rules.clear_overlay_tasks();
        self.coasting = false;
        self.game_over_ticks = 0;
        Ok(())
    }

    // ----------------------------------------------------------------------------------------
    // Ticks
    // ----------------------------------------------------------------------------------------

    /// Loads the route of the segment holding `station` if it differs and schedules its setup
    /// tick after `load_ticks` idle ticks.
    fn prepare_segment(&mut self, station: usize, load_ticks: u32) -> Result<(), DriveError> {
        let (seg, stop) = segment_for(&self.stage_data, self.rules.stage().0, station)?;
        if seg != self.com {
            let route = load_route(&mut *self.loader, &self.com_files, seg)?;
            self.next_route = Some((seg, route));
        } else {
            self.next_route = None;
        }
        let route = self.next_route.as_ref().map_or(&self.route, |(_, r)| r);
        check_stop(route, &self.com_files, seg, stop as usize)?;
        self.step = Step::Load(load_ticks);
        Ok(())
    }

    /// Applies a `Vehicle_Set` train type (`0x84c3fc`, read by the train task every tick).
    fn sync_train_type(&mut self) {
        if let Some(t) = self.rules.train_type
            && t != self.train.train_type
            && let Some(&spec) = self.specs.get(t)
        {
            self.train.train_type = t;
            self.spec = spec;
        }
    }

    /// The rules' view of this tick's inputs (0x40dad0 reads `0x8500c8` of the last tick).
    fn rule_input(&self, input: &DriverInput) -> TickInput {
        let next_leg = match (
            self.route.stops.get(self.stop_index + 1),
            self.route.stops.get(self.stop_index + 2),
        ) {
            (Some(&a), Some(&b)) => b as f64 - a as f64,
            _ => 0.0,
        };
        // 0x41c820: fild leg; fmul 0.125f; fadd units; fstp. Then × 10.0 (0x40dad0).
        let following_units =
            (next_leg * f64::from(UNITS_PER_POINT) + f64::from(self.units_to_stop)) as f32;
        TickInput {
            speed_mps: self.train.speed,
            dist_to_stop_m: (f64::from(self.units_to_stop) * METRES_PER_UNIT) as f32,
            dist_to_following_stop_m: (f64::from(following_units) * METRES_PER_UNIT) as f32,
            odometer_m: self.train.odometer,
            power: i32::from(input.power),
            brake: i32::from(input.brake),
            buttons: Buttons {
                horn_held: input.horn_held,
                horn_pressed: input.horn_pressed,
                confirm_pressed: input.confirm_pressed,
            },
            in_tunnel: self.track_class_ahead == TUNNEL_CLASS,
        }
    }

    /// 0x407c10: gradient from the heights 6 and 4 points ahead, sound class 4 points ahead.
    fn track_lookup(&mut self) {
        let pos = self.walker.pos();
        self.gradient = gradient_at(&self.route, pos);
        let probe = pos.slot(&self.route) + CLASS_PROBE_AHEAD;
        self.track_class_ahead = self.route.slots.get(probe).map_or(0, |p| {
            i16::from(
                self.stage_data
                    .track_sounds
                    .get(usize::from(p.track_type()))
                    .copied()
                    .unwrap_or(0),
            )
        });
        // Presentation-only part of the same lookup (sound).
        let slot = pos.slot(&self.route);
        let class_at = |at: Option<usize>| {
            at.and_then(|i| self.route.slots.get(i)).map_or(0, |p| {
                i16::from(
                    self.stage_data
                        .track_sounds
                        .get(usize::from(p.track_type()))
                        .copied()
                        .unwrap_or(0),
                )
            })
        };
        self.track_classes = SOUND_PROBES.map(|d| class_at(slot.checked_add_signed(d)));
        let on = self
            .route
            .slots
            .get(slot + TURNOUT_PROBE_AHEAD)
            .is_some_and(|p| p.is_turnout());
        self.turnout_entered = on && !self.on_turnout;
        self.on_turnout = on;
        self.lookup_tick = Some(self.tick_count);
    }

    /// 0x41c820 + the leg-distance reset of 0x40d620, when the rules passed a station.
    fn pass_station(&mut self) {
        if let (Some(&a), Some(&b)) = (
            self.route.stops.get(self.stop_index + 1),
            self.route.stops.get(self.stop_index + 2),
        ) {
            let leg = b as f64 - a as f64;
            self.units_to_stop =
                (leg * f64::from(UNITS_PER_POINT) + f64::from(self.units_to_stop)) as f32;
        }
        self.stop_index += 1;
        self.train.pass_station();
    }

    /// The train task (0x422c20). Returns whether the ATC tripped.
    fn physics(&mut self, input: &DriverInput, driving: bool, coast_down: bool) -> bool {
        let r = &self.rules;
        let tt = r.timetable();
        let row = tt.get(r.station);
        let next = tt.get(r.station + 1);
        let coast_hold = coast_down
            && next.is_some_and(|n| {
                // 0x422f53: trunc(0x8500c8 × 10) ≤ next row field +0x20.
                ((f64::from(self.units_to_stop) * METRES_PER_UNIT) as i32) <= n.params[4]
            });
        let ctx = TickContext {
            driving,
            coast_down,
            coast_hold,
            forced_stop: r.ats >= 4,
            power_cut: r.coasting != 0,
            slippery: r.weather_level > 1,
            atc_limit_kmh: r.derail_kmh,
            gradient: self.gradient,
            heading: self.heading,
            track_effects: true,
            brake_load: row.map_or(0, |row| row.params[0]),
        };
        let controls = Controls {
            power: input.power,
            brake: input.brake,
        };
        let atc = self
            .train
            .step(&self.spec, controls, &ctx, self.rules.clock)
            .atc_tripped;
        if !driving {
            // The not-driving branch also clears 0x857228 (physics.md §4.7).
            self.gradient = 0.0;
        }
        atc
    }

    fn readout(&self) -> TrainReadout {
        TrainReadout {
            speed_after: self.train.speed,
            decel_prev_tick: self.train.decel_prev_tick,
            brake_decel: self.train.brake_decel,
            brake_delta: self.train.brake_history_delta,
            brake_applied: self.train.prev_brake,
            jolt_limit: self.spec.jolt_limit,
            top_speed_kmh: self.spec.max_speed_kmh(),
        }
    }

    /// 0x40b710 (driving-loop iterations and the setup's priming call), and the passing train
    /// a red-signal recovery of this tick asked for (0x40a0a0).
    fn traffic_events(&mut self, loop_body: bool) {
        let env = TrafficEnv {
            route: &self.route,
            models: &self.train_models,
            obstacle_armed: self.rules.obstacle_armed,
            dist_round_m: self.rules.dist_round_m,
            tick: u64::from(self.global_tick),
        };
        if loop_body {
            self.traffic
                .event_step(&self.stage_data.traffic_events, self.rules.station, &env);
        }
        if let Some(p) = self.rules.passing_spawn.take() {
            self.traffic.schedule_passing(p);
        }
        // Course event 12 ran in this tick's rule step, before the camera task moved:
        // 0x406c50 counts from that tick's engine index `0x8502a4`.
        if let Some(set) = self.rules.waiting_set.take() {
            self.traffic
                .place_waiting_set(&set, self.walker.pos().engine_index, &env);
        }
        // `0x857244` is written by the coupling minigame only (success outro, time-up).
        if self.rules.phase == Phase::Coupling
            && let Some(c) = &self.rules.coupling
        {
            self.traffic.set_coupler_frame(c.coupler_frame != 0);
        }
    }

    /// The car tasks of this tick's `Task_Tick` and their draw-step sounds for the camera
    /// after it (eye height 0: it does not change the view-space depth the sounds test).
    fn traffic_task(&mut self) {
        let env = TrafficEnv {
            route: &self.route,
            models: &self.train_models,
            obstacle_armed: self.rules.obstacle_armed,
            dist_round_m: self.rules.dist_round_m,
            tick: u64::from(self.global_tick),
        };
        self.traffic.task_step(&env);
        let camera = cab_camera(&self.route, self.walker.pos(), 0.0, 0.0);
        self.traffic.draw_step(&self.route, &camera);
    }

    /// The in-cab camera task (0x41d160).
    fn camera(&mut self) {
        let step = self.train.units_per_tick;
        self.walker.advance(&self.route, step);
        // 0x41d181: fld 0x8500c8; fsub 0x850208; fstp (one f32 rounding).
        self.units_to_stop -= step;
        self.heading = crate::physics::heading_at(&self.route, self.walker.pos());
    }

    /// One `Task_Tick` of a demo drive: the train task on the table's notches (driving flag
    /// set), the camera, then the demo task (0x40e630 / 0x40e860) with its rule calls, the
    /// AI-train events of the route demo, and the car tasks. 0x407c10 does not run, so the
    /// gradient stays 0.
    fn demo_tick(&mut self) -> DriveTick {
        self.tick_count += 1;
        self.traffic.begin_tick();
        self.rules.global_tick = Some(self.global_tick);
        self.prev_pos = self.walker.pos();
        let Some(mut d) = self.demo.take() else {
            return DriveTick::default();
        };
        let mut out = TickOutput::default();
        let mut atc = false;
        if d.counter < d.kind.ticks() {
            let input = d.inputs.next(&d.tables, d.kind);
            d.last = input;
            atc = self.physics(&input, true, false);
            self.camera();
            let rin = self.rule_input(&input);
            let lesson = d.kind == DemoKind::Lesson;
            out = self.rules.demo_tick(&rin, lesson, d.counter);
            if out
                .events
                .iter()
                .any(|e| matches!(e, RuleEvent::StationPassed { .. }))
            {
                self.pass_station();
            }
            if !lesson {
                self.traffic_events(true);
            }
            d.counter += 1;
            self.traffic_task();
            self.rules.end_tick(&mut out, Some(&self.readout()));
        }
        self.demo = Some(d);
        self.global_tick = self.global_tick.wrapping_add(1);
        DriveTick {
            events: out.events,
            atc_tripped: atc,
            segment_started: false,
            notches_forced: false,
        }
    }

    /// One `Task_Tick` of the bonus game: its physics task and the bonus task; the player
    /// train is held, the HUD transfers the pending time.
    fn bonus_tick(&mut self, input: DriverInput) -> DriveTick {
        let Some(mut b) = self.bonus.take() else {
            return DriveTick::default();
        };
        let spec = self
            .specs
            .get(bonus::TRAIN_TYPE)
            .copied()
            .unwrap_or(self.spec);
        let released = self.bonus_horn && !input.horn_held;
        self.bonus_horn = input.horn_held;
        let events = b.tick(
            &spec,
            input.power,
            input.brake,
            (input.horn_pressed, released),
        );
        let rin = self.rule_input(&input);
        let mut out = self.rules.idle_tick(&rin, None);
        for e in events {
            if let BonusEvent::Points(p) = e {
                self.rules.award_bonus(&mut out, p);
            }
            out.events.push(RuleEvent::Bonus(e));
        }
        if b.done() {
            // Destroy 0x41c2e0: the stage's vehicle again. Stage 6 with four marks or more
            // goes on with the other train set and traffic mask state 2 (0x4080d0).
            self.rules.vehicle_code = self.bonus_saved_vehicle.take();
            let (stage, station) = (self.rules.stage().0, self.rules.station);
            let marks: i32 = self
                .rules
                .marks
                .iter()
                .take(station)
                .map(|&m| i32::from(m))
                .sum();
            if stage == 6 && marks >= 4 {
                self.traffic.set_state(2);
                self.rules.extra_section = true;
            }
        } else {
            self.bonus = Some(b);
        }
        DriveTick {
            events: out.events,
            atc_tripped: false,
            segment_started: false,
            notches_forced: false,
        }
    }

    /// The bonus game while it runs.
    pub fn bonus(&self) -> Option<&BonusGame> {
        self.bonus.as_ref()
    }

    fn idle_tick(&mut self, input: DriverInput) -> DriveTick {
        let rin = self.rule_input(&input);
        let atc = self.physics(&input, false, false);
        self.traffic_task();
        let out = self.rules.idle_tick(&rin, Some(&self.readout()));
        DriveTick {
            events: out.events,
            atc_tripped: atc,
            segment_started: false,
            notches_forced: false,
        }
    }

    /// The segment setup of 0x4080d0 (0x408010 → 0x41cb00 and 0x41c7b0, priming 0x40dad0)
    /// and its `Task_Tick`, in which the new camera task walks up to the stop.
    fn setup_tick(&mut self, input: DriverInput) -> DriveTick {
        if let Some((com, route)) = self.next_route.take() {
            self.com = com;
            self.route = route;
            self.traffic.clear();
        }
        let stage = self.rules.stage().0;
        // Checked when the segment was prepared.
        let stop =
            segment_for(&self.stage_data, stage, self.rules.station).map_or(0, |(_, s)| s as usize);
        self.stop_index = stop;
        self.walker = CabWalker::at_segment_start(&self.route, stop);
        let points = self.route.stops[stop + 1] as f64 - self.route.stops[stop] as f64;
        self.units_to_stop = (points * f64::from(UNITS_PER_POINT)) as f32;
        self.arrived_ticks = 0;
        self.odometer_settled = false;
        self.step = Step::Run;

        let rin = self.rule_input(&input);
        let mut out = self.rules.segment_setup(&rin);
        // The priming call of 0x40b710 after 0x40dad0's.
        self.traffic_events(true);
        let atc = self.physics(&input, false, false);
        self.camera();
        self.prev_pos = self.walker.pos();
        self.traffic_task();
        self.rules.end_tick(&mut out, Some(&self.readout()));
        DriveTick {
            events: out.events,
            atc_tripped: atc,
            segment_started: true,
            notches_forced: false,
        }
    }

    fn settle_odometer(&mut self) {
        if !self.odometer_settled {
            self.odometer_settled = true;
            // 0x40869d: fld 0x85708c; fadd 0x857004; fst 0x857004.
            self.train.odometer =
                (f64::from(self.rules.dist_m) + f64::from(self.train.odometer)) as f32;
        }
    }

    fn run_tick(&mut self, input: DriverInput) -> DriveTick {
        let before = self.rules.phase;
        let loop_body = before == Phase::Driving
            || (before == Phase::PreDeparture && self.rules.departure_done);
        match before {
            Phase::GameOver => self.game_over_ticks += 1,
            Phase::Arrived => {
                self.arrived_ticks += 1;
                if self.arrived_ticks == 1 {
                    self.rules.leave_driving_loop();
                }
                if self.arrived_ticks == ARRIVAL_ODOMETER_TICK {
                    self.settle_odometer();
                    // Task list 5 is cleared right after (0x4080d0).
                    self.traffic.clear();
                    self.rules.clear_overlay_tasks();
                }
            }
            _ => {}
        }
        // A sky fade (0x40f320) runs a `Task_Tick` inside the loop body: the tick that
        // finishes the body does not repeat the parts before the split.
        let resumed = self.rules.fade_pending();
        if loop_body && resumed.is_none() {
            self.track_lookup();
        }
        let rin = self.rule_input(&input);
        let mut out: TickOutput = self.rules.step(&rin);
        self.sync_train_type();
        if out
            .events
            .iter()
            .any(|e| matches!(e, RuleEvent::StationPassed { .. }))
        {
            self.pass_station();
        }
        // 0x40b710 (step 6) comes after a split in 0x40dad0 / 0x40acd0 and before one in
        // 0x40c4e0; the leg marker and the hints at the end of 0x40c4e0 after all of them.
        let split = out.fade_split;
        self.sky_fade = split;
        let ai_events = loop_body
            && !matches!(
                split,
                Some(FadeSplit::StationPassed | FadeSplit::CourseEvent)
            )
            && resumed != Some(FadeSplit::WeatherEnd);
        self.traffic_events(ai_events);
        if loop_body && split.is_none() {
            let rank = self.rules.stage_rank();
            let fired = self.train.leg_marker(rank);
            self.rules.advice_hints(&mut out, fired);
        }
        if out.force_stop {
            self.train.speed = 0.0;
        }
        if loop_body && self.rules.phase == Phase::GameOver {
            self.coasting = true;
        }
        let game_over = self.rules.phase == Phase::GameOver;
        let driving = self.rules.train_driving() || (game_over && self.coasting);
        // `input_force_notches(0, 9)` in the rules: N / EB for this tick's train task.
        let forced = out.force_notches && !self.analog_levers;
        let input = if forced {
            DriverInput {
                power: 0,
                brake: 9,
                ..input
            }
        } else {
            input
        };
        let atc = self.physics(&input, driving, game_over);
        self.camera();
        self.traffic_task();
        self.rules.end_tick(&mut out, Some(&self.readout()));
        if self.coasting && self.game_over_ticks + 1 == GAME_OVER_DRAIN_TICKS {
            // 0x4085d0 after the 20th `Task_Tick` (the game-over tick is the first), then
            // 0x4085e0.
            self.rules.pending_s = 0;
            self.rules.leave_driving_loop();
        }
        DriveTick {
            events: out.events,
            atc_tripped: atc,
            segment_started: false,
            notches_forced: forced,
        }
    }
}

/// An attract demo drive: the demo task's counter (task+0x30) and the train task's replay.
struct Demo {
    kind: DemoKind,
    counter: u32,
    inputs: DemoInputs,
    tables: DemoTables,
    last: DriverInput,
}

/// `stage_select_segment` (0x408010): the segment holding `station` and the stop index in it.
fn segment_for(stage_data: &Stage, stage: usize, station: usize) -> Result<(u32, u32), DriveError> {
    let no = DriveError::NoSegment { stage, station };
    let st = u32::try_from(station).map_err(|_| DriveError::NoSegment { stage, station })?;
    stage_data
        .segment_for_station(st)
        .map(|(seg, stop)| (seg.com, stop))
        .ok_or(no)
}

fn com_name(com_files: &[Option<String>], com: u32) -> Result<&str, DriveError> {
    com_files
        .get(com as usize)
        .and_then(|n| n.as_deref())
        .ok_or(DriveError::NoComFile(com))
}

fn load_route(
    loader: &mut dyn RouteLoader,
    com_files: &[Option<String>],
    com: u32,
) -> Result<Route, DriveError> {
    let file = com_name(com_files, com)?;
    let bytes = loader.load(file).map_err(|source| DriveError::Load {
        file: file.to_owned(),
        source,
    })?;
    Ok(Route::parse(&bytes)?)
}

fn check_stop(
    route: &Route,
    com_files: &[Option<String>],
    com: u32,
    stop: usize,
) -> Result<(), DriveError> {
    if stop + 1 < route.stops.len() && route.stops[stop] >= 1 {
        Ok(())
    } else {
        Err(DriveError::BadStop {
            file: com_name(com_files, com).unwrap_or("?").to_owned(),
            stop,
        })
    }
}

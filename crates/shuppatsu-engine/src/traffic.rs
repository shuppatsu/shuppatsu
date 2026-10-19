//! AI trains ("traffic"): the per-stage traffic event list, the spawner, the per-car tasks and
//! the sounds their draw step makes. `docs/re/traffic.md` has the behaviour and evidence.
//!
//! The original runs each car as its own task in task list 5 (0x84c1bc): the event list
//! (0x40b710) or the red-signal recovery (0x40a0a0) calls the spawner (0x404f10, 0x405e80 for
//! row 0x43), which creates one task per car. Each car keeps two track points 10 points apart
//! (its bogies) and a fraction `t` of a point; the update (0x405420 / 0x406220) moves it by its
//! speed and its speed by its mode; the draw (0x405640 / 0x405b10 / 0x406420) places the car's
//! LOD part between the bogies and plays the passing sounds. Nothing here feeds back into the
//! player's simulation: the rule interactions are the event cursor and the passing train of
//! the scripted red-signal recovery (`Rules::passing_spawn`).
//!
//! Car poses are presentation; [`Traffic::poses`] interpolates them along the track between the
//! last two ticks like the cab camera.

use shuppatsu_formats::route::{Route, UNITS_PER_POINT};
use shuppatsu_formats::scene::{Affine, CabCamera, TrackPos};
use shuppatsu_formats::stages::TrafficEvent;
use shuppatsu_formats::traffic::{CarModel, TrainModels};

/// Spawn mode flags (`param_2` of 0x404f10): low nibble = speed mode.
const FLAG_NO_EMERGE: i32 = 0x10;
const FLAG_REVERSE: i32 = 0x20;
const FLAG_JITTER: i32 = 0x40;
/// Mode 6: mode 0 with a random speed (0x404f10).
const MODE_JITTER: i32 = 6;

/// km/h → world units per tick (double 0.000926 at 0x458b78).
const KMH_TO_UNITS: f64 = 0.000926;
/// km/h → the ×1000 speed field `+0x58` (double 0.926 at 0x458b70).
const KMH_TO_SPEED_K: f64 = 0.926;
/// `+0x58` → units per tick (double 0.001 at 0x458ba0); the accel formulas use its inverse,
/// 1000.0 (0x458b60).
const SPEED_K_SCALE: f64 = 0.001;
const SPEED_K_INV: f64 = 1000.0;
/// Mode 1 accel per unit of the parameter: `p4 · 0.000926 · 0.033333` (0x458b78, 0x458b50).
const ACCEL_PER_PARAM: f64 = 0.033333;
/// Metres → units for the stop distance `+0x48` (double 0.1 at 0x44e8e0).
const METRES_TO_UNITS: f64 = 0.1;
/// Braking `v² / (p4 · 0.2)` stops within `p4` metres (double 0.2 at 0x458b30).
const BRAKE_DIVISOR: f64 = 0.2;
/// Mode 3 starts braking 30 units (300 m) before its stop (double 30 at 0x458b90); a stop
/// at ≥ 300 m brakes with `v² · 1/60` (0x458b58), i.e. over exactly those 300 m.
const MODE3_BRAKE_UNITS: f64 = 30.0;
const MODE3_LONG_PARAM: i32 = 300;
const MODE3_LONG_FACTOR: f64 = 0.016_666_666_666_666_666;
/// Mode 4: 10 units (100 m), `v² · 0.05` past 100 m (0x458b88, 0x458b68).
const MODE4_BRAKE_UNITS: f64 = 10.0;
const MODE4_LONG_PARAM: i32 = 100;
const MODE4_LONG_FACTOR: f64 = 0.05;
/// A mode-4 train waiting for the obstacle stops accelerating at 0.08334 units/tick
/// (90 km/h, double at 0x458b98) and cruises.
const RESTART_TOP_SPEED: f64 = 0.08334;

/// The rear bogie is this many points behind the front one (0x404f10; 12 for row 0x43).
const BOGIE_POINTS: usize = 10;
const BOGIE_POINTS_SPECIAL: usize = 12;
/// Rows above this start one point further on (0x404f10: `0x45 < row`).
const STEP_ONCE_ABOVE_ROW: usize = 0x45;
/// The row with its own spawner, update and draw (0x405e80, 0x406220, 0x406420).
const SPECIAL_ROW: usize = 0x43;
/// Rows drawn by 0x405b10 (an extra part 0x24 at local (0, 0.05, −0.02), no sounds).
const ROWS_WITH_EXTRA: [usize; 2] = [0x47, 0x4b];
const EXTRA_PART: u32 = 0x24;
const EXTRA_OFFSET: [f32; 3] = [0.0, 0.05, -0.02];
/// First car of these rows, running backwards, is turned round (flag 0x1000, 0x404f10).
const TURNED_ROWS: [usize; 6] = [0x34, 0x35, 0x1e, 0x1f, 0x20, 0x21];
/// Row 0x43 turns every car round except these parts (0x406420).
const SPECIAL_UNTURNED_PARTS: [u32; 2] = [0x2eb8, 0x2ebc];

/// View-space depth bands of the five LODs (doubles 0x458c30..0x458c00): z < −15 or z ≥ 2.5
/// is not drawn; −15.. → LOD 4, −12.. → 3, −9.. → 2, −6.. → 1, −3..2.5 → 0.
pub const LOD_DEPTHS: [f64; 5] = [-15.0, -12.0, -9.0, -6.0, -3.0];
const LOD_BEHIND: f64 = 2.5;
/// Lateral bands (0x458be0..0x458bf8): |x| > 16 / 24 / 32 adds 1 / 2 / 3, > 64 hides.
const LOD_LATERAL: [f64; 4] = [16.0, 24.0, 32.0, 64.0];

/// Sound codes of the draw step (0x405640 / 0x406420) and the recovery (0x40a0a0).
pub const SOUND_PASSING_CAR: u16 = 0x33;
pub const SOUND_SPECIAL_SEQUENCE: u16 = 0x1028;
pub const SOUND_MODE5: u16 = 0x196;
/// Passing sounds play while the car is within 1 unit sideways and runs at ≥ 0.02 units/tick
/// (21.6 km/h; doubles 1.0 at 0x457718 and 0.02 at 0x458bd0).
const SOUND_LATERAL: f64 = 1.0;
const SOUND_MIN_SPEED: f64 = 0.02;
/// Oncoming cars: windows −0.2..0 and −1.9..−1.6 units of depth (0x458bc8, 0x458bb8/0x458bc0).
const ONCOMING_WINDOWS: [(f64, f64); 2] = [(-0.2, 0.0), (-1.9, -1.6)];
/// Same-direction cars: 0..0.1 with gate > 999, then 1.6..1.7 with gate > 99 (0x44e8e0,
/// 0x458bb0, 0x458ba8); the gate starts at 10000.
const SAME_WINDOWS: [(f64, f64, i32, i32); 2] = [(0.0, 0.1, 999, 100), (1.6, 1.7, 99, 10)];
const SOUND_GATE_START: i32 = 10_000;
/// Row 0x43's sequence plays once when its first car is −0.4..0 deep (0x458c50).
const SPECIAL_SEQUENCE_WINDOW: (f64, f64) = (-0.4, 0.0);

/// The coupling minigame's waiting set (course event 12, 0x40acd0 → 0x406c50): its head stands
/// this many metres past the stop marker, for coupling partner 1 / 2 (f64 294.25 at 0x4594e0,
/// 239.5 at 0x4594f0). With their car lists (10 × 25.5 m, 8 × 25 m) both leave the last car's
/// front bogie about 64.6 m past the marker.
const WAITING_HEAD_M: [f64; 2] = [294.25, 239.5];
/// Metres → track points of the placement, negated (f64 −0.8 at 0x4594e8).
const WAITING_POINTS_PER_M: f64 = -0.8;
/// Rows of the waiting set (0x45735c[row · 3]), partner 1 / 2: the set as it waits and, from
/// the coupler frame `0x857244 = 1` on, with the coupler closed (only the last car differs).
const WAITING_ROWS: [(usize, usize); 2] = [(20, 21), (17, 18)];
/// 0x406c50 walks the rear bogie 12 points back (10 in the spawner) and starts the passing-
/// sound gate at 9000 (it stands still, so no sound).
const WAITING_BOGIE_POINTS: usize = 12;
const WAITING_GATE: i32 = 9000;

/// 0x406960 (the stage intro's chase): 12-point bogies like 0x406c50, and the same passing-
/// sound gate.
const PLACED_BOGIE_POINTS: usize = 12;
const PLACED_GATE: i32 = 9000;

/// Bit of the traffic event mask for each state of `0x857216`.
fn mask_bit(state: u8) -> i16 {
    1 << state
}

/// Arguments of one spawner call (0x404f10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnRequest {
    /// Index into the `.COM` traffic spawn table.
    pub spawn: i16,
    /// Low nibble: speed mode 0–6; `0x10` place the cars spaced out instead of letting them
    /// emerge from the spawn point; `0x20` reverse the spawn's direction; `0x40` random speed.
    pub flags: i32,
    pub speed_kmh: i32,
    /// Mode parameter: acceleration (mode 1, in 0.001 km/h/s), braking or stop distance in m
    /// (modes 2–4), points to skip ahead (mode 5).
    pub param: i32,
    /// Model row of 0x457358; −1 = the spawn's own type − 1.
    pub model: i32,
}

impl SpawnRequest {
    pub fn from_event(e: &TrafficEvent) -> Self {
        Self {
            spawn: e.spawn,
            flags: i32::from(e.args[0]),
            speed_kmh: i32::from(e.args[1]),
            param: i32::from(e.args[2]),
            model: i32::from(e.args[3]),
        }
    }
}

/// Sound cues of the scripted recovery's passing train (0x40a0a0, hold loop index `i`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassingSound {
    None,
    /// Code 8 at i = 40, released at i = 100.
    Horn,
    /// Codes 1 and 4 at i = 45, 4 released at i = 100.
    HornAndRumble,
}

/// The passing train of a scripted red-signal recovery: spawned `after_ticks` ticks after the
/// tick the stop was detected in (after the 30 standstill ticks, 0x40a0a0).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PassingTrain {
    pub request: SpawnRequest,
    pub after_ticks: u32,
    pub sound: PassingSound,
}

/// The waiting set of the coupling minigame, as course event 12 (0x40acd0) asks 0x406c50 to
/// place it: two car lists and how far ahead of the cab's track point its head stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitingSet {
    /// The set's rows: as it waits, and with the coupler closed (`+0x6c` / `+0x70`).
    pub rows: (usize, usize),
    /// Track points from the cab's engine index (`0x8502a4`) to the head car's front bogie.
    pub ahead_points: i64,
}

impl WaitingSet {
    /// The set of coupling partner `partner` (1 or 2, `Rules::coupling_partner`) placed when
    /// the distance to the stop is `dist_m` (`0x85708c`): the head at
    /// `0x8502a4 − ftol((dist + head_m) × −0.8)`.
    pub fn new(partner: u8, dist_m: f32) -> Self {
        let i = usize::from(partner >= 2);
        let back = ((f64::from(dist_m) + WAITING_HEAD_M[i]) * WAITING_POINTS_PER_M).trunc();
        Self {
            rows: WAITING_ROWS[i],
            ahead_points: -(back as i64),
        }
    }
}

/// A train that 0x406960 places at an explicit track point instead of a `.COM` spawn (the
/// stage intro's chase, 0x408f40: the player's own train just ahead of the camera).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacedTrain {
    /// Model row of 0x457358 (`param_5 & 0xfff`).
    pub row: usize,
    /// `param_5 & 0x1000`: runs towards the end of its segment (`+0x30`) with the row's
    /// second car list.
    pub forward: bool,
    /// Track points from the cab's engine index (`0x8502a4`) to the head car's front bogie.
    pub ahead_points: i64,
    /// Speed mode (`param_2`).
    pub mode: i32,
    pub speed_kmh: i32,
    /// Mode parameter (`param_4`): braking distance for mode 2, acceleration otherwise.
    pub param: i32,
    /// `param_6 == 1`: every car placed at once, walked back by its spacing; otherwise the
    /// cars stand on the head's point and leave it one after another (`+0x60` = offset).
    pub spaced: bool,
}

/// A sound command of this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrafficSound {
    /// `snd_request(code)` (`| 0x1000` = a sequence).
    Play(u16),
    /// Release a sound code.
    Release(u16),
}

/// MSVC `rand`/`srand` (0x437d10 / 0x437d03): the spawner's random speed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CrtRand(u32);

impl CrtRand {
    fn next(&mut self) -> i32 {
        self.0 = self.0.wrapping_mul(214_013).wrapping_add(2_531_011);
        ((self.0 >> 16) & 0x7fff) as i32
    }
}

/// The two bogie points and the fraction of a point (`+0x64`, `+0x68`, `+0x50`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Bogies {
    front: usize,
    rear: usize,
    /// Track units past `front` in the running direction: ≥ 0 forwards, ≤ 0 backwards.
    t: f32,
}

/// One car (one task of list 5).
#[derive(Debug, Clone, PartialEq)]
pub struct Car {
    row: usize,
    model: CarModel,
    /// The list of the coupler frame `0x857244 = 1` (`+0x70`); the same as `model` except
    /// for the coupling minigame's waiting set.
    coupled: CarModel,
    /// Position in its train (`+0x44`).
    index: u32,
    /// `+0x30`: runs towards the end of its segment.
    forward: bool,
    /// Flag 0x1000: drawn turned round.
    turned: bool,
    mode: i32,
    /// `+0x40`: a mode-4 train waiting for the obstacle event to end.
    waits_for_obstacle: bool,
    at: Bogies,
    /// `+0x4c`: units run since the spawn.
    run: f32,
    /// `+0x48`: units to the stop (modes 3, 4).
    stop_at: f32,
    /// `+0x54`: units per tick; `+0x58`: the same × 1000; `+0x5c`: change per tick of `+0x58`.
    speed: f32,
    speed_k: f32,
    accel: f32,
    /// `+0x60`: units still to run before this car leaves the spawn point.
    delay: f32,
    /// `+0x3c`: passing-sound gate.
    gate: i32,
    /// Row 0x43: the sequence played (`+0x44` set to 1).
    sequence_done: bool,
    /// The bogies before this tick's update and the signed fraction change it applied, for
    /// interpolation.
    prev: Bogies,
    moved: f32,
}

impl Car {
    pub fn row(&self) -> usize {
        self.row
    }

    pub fn index(&self) -> u32 {
        self.index
    }

    pub fn forward(&self) -> bool {
        self.forward
    }

    /// Slot of the front bogie and the fraction.
    pub fn track(&self) -> (usize, f32) {
        (self.at.front, self.at.t)
    }

    pub fn speed_kmh(&self) -> f32 {
        (f64::from(self.speed) / KMH_TO_UNITS) as f32
    }

    fn special(&self) -> bool {
        self.row == SPECIAL_ROW
    }
}

/// Where a car is drawn, at some point between two ticks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CarPose {
    /// LOD parts 0..4.
    pub lods: [u32; 5],
    /// The model transform (0x4186a0 + pitch), turned round where the original turns it.
    pub transform: Affine,
    /// The same, not turned: row 0x43 picks per LOD part (see [`CarPose::instance`]).
    unturned: Affine,
    special: bool,
    /// Rows 0x47/0x4b also draw part 0x24.
    pub extra: Option<(u32, Affine)>,
}

impl CarPose {
    /// Model origin (the midpoint between the bogies).
    pub fn origin(&self) -> [f32; 3] {
        self.transform.translation
    }

    /// The part and transform for LOD `lod` (0..=4).
    pub fn instance(&self, lod: usize) -> (u32, Affine) {
        let part = self.lods[lod.min(4)];
        if self.special && SPECIAL_UNTURNED_PARTS.contains(&part) {
            (part, self.unturned)
        } else {
            (part, self.transform)
        }
    }
}

/// View-space position of `p` for the in-cab camera (0x856bd0 / 0x856bd8: the translation
/// row of model × view): `(x, z)` with forward = −z. Only |x| matters.
pub fn view_xz(camera: &CabCamera, p: [f32; 3]) -> (f64, f64) {
    let f = camera.forward().map(f64::from);
    let d = [0, 1, 2].map(|i| f64::from(p[i]) - f64::from(camera.eye[i]));
    let z = -(d[0] * f[0] + d[1] * f[1] + d[2] * f[2]);
    // The camera's +x is the world vector (−dz, dx) of the view direction (scene.md §1).
    let h = (f[0] * f[0] + f[2] * f[2]).sqrt().max(1e-9);
    let x = (-f[2] * d[0] + f[0] * d[2]) / h;
    (x, z)
}

/// The LOD of 0x405640 for a view-space position, or `None` when the car is not drawn.
pub fn lod_for_view(x: f64, z: f64) -> Option<usize> {
    if !(LOD_DEPTHS[0]..LOD_BEHIND).contains(&z) {
        return None;
    }
    let mut lod = 4 - LOD_DEPTHS.iter().rposition(|&d| z >= d).unwrap_or(0);
    let ax = x.abs();
    if ax > LOD_LATERAL[3] {
        return None;
    }
    lod += LOD_LATERAL[..3].iter().filter(|&&b| ax > b).count();
    (lod <= 4).then_some(lod)
}

/// The track walk of 0x407880 (`next`) and 0x4078c0 (`prev`): the neighbouring slot, through
/// a segment marker to the segment's link, or the same slot where the link is null.
pub fn next_slot(route: &Route, slot: usize) -> usize {
    match route.slots.get(slot + 1) {
        Some(p) if p.flags & 0xf000 == 0xe000 => route
            .segments
            .get(p.height_raw as u16 as usize)
            .and_then(|s| s.link_next)
            .unwrap_or(slot),
        Some(_) => slot + 1,
        None => slot,
    }
}

pub fn prev_slot(route: &Route, slot: usize) -> usize {
    let Some(i) = slot.checked_sub(1) else {
        return slot;
    };
    match route.slots.get(i) {
        Some(p) if p.flags & 0xf000 == 0xf000 => route
            .segments
            .get(p.height_raw as u16 as usize)
            .and_then(|s| s.link_prev)
            .unwrap_or(slot),
        Some(_) => i,
        None => slot,
    }
}

fn step(route: &Route, slot: usize, forward: bool) -> usize {
    if forward {
        next_slot(route, slot)
    } else {
        prev_slot(route, slot)
    }
}

fn step_n(route: &Route, mut slot: usize, forward: bool, n: usize) -> usize {
    for _ in 0..n {
        slot = step(route, slot, forward);
    }
    slot
}

/// Moves `b` along the track until `|t| ≤ 0.125` (0x405420's two loops). Returns false when the
/// front bogie could not move on the first step of a loop (the task ends).
fn normalise(route: &Route, b: &mut Bogies) -> bool {
    let start = b.front;
    while b.t > UNITS_PER_POINT {
        let f = next_slot(route, b.front);
        b.t -= UNITS_PER_POINT;
        b.front = f;
        if f == start {
            return false;
        }
        b.rear = next_slot(route, b.rear);
    }
    let start = b.front;
    while b.t < -UNITS_PER_POINT {
        let f = prev_slot(route, b.front);
        b.t += UNITS_PER_POINT;
        b.front = f;
        if f == start {
            return false;
        }
        b.rear = prev_slot(route, b.rear);
    }
    true
}

/// What the spawner and the car tasks read from the rest of the game.
#[derive(Debug, Clone, Copy)]
pub struct TrafficEnv<'a> {
    pub route: &'a Route,
    pub models: &'a TrainModels,
    /// `0x856d76`: the obstacle event is armed.
    pub obstacle_armed: bool,
    /// `0x85722c`: rounded distance to the stop, m.
    pub dist_round_m: i32,
    /// The global tick counter `0x84c494` (reseeds `rand` after a random speed).
    pub tick: u64,
}

/// The cars, the event cursor and this tick's sounds.
#[derive(Debug, Clone, PartialEq)]
pub struct Traffic {
    cars: Vec<Car>,
    /// `0x856d90`: index of the next traffic event.
    cursor: usize,
    /// `0x857216`: 1 at the stage start and after arrivals without a continue, 0 after a
    /// continue; selects the event mask bit.
    pub state: u8,
    rand: CrtRand,
    /// The recovery's passing train: ticks until its spawn, the request and its sounds.
    pending: Option<(u32, PassingTrain)>,
    /// Hold-loop index of the recovery sounds (counts from the spawn).
    passing_sound: Option<(PassingSound, u32)>,
    sounds: Vec<TrafficSound>,
    /// `0x857244`: the cars draw their `+0x70` list. 0 when a waiting set is placed, 1 once
    /// the coupling minigame couples or runs out of time; never cleared otherwise.
    coupler_frame: bool,
}

impl Traffic {
    /// The state after 0x40d040 (stage start at `station`).
    pub fn new(events: &[TrafficEvent], station: usize) -> Self {
        let mut t = Self {
            cars: Vec::new(),
            cursor: 0,
            state: 1,
            // The CRT's initial seed; the original's state depends on every earlier `rand`
            // call of the session, so any value is as good (traffic.md §6).
            rand: CrtRand(1),
            pending: None,
            passing_sound: None,
            sounds: Vec::new(),
            coupler_frame: false,
        };
        t.seek(events, station);
        t
    }

    /// 0x40d930 (continue): the cursor restarts from the list head at `station`; state 0.
    pub fn rewind(&mut self, events: &[TrafficEvent], station: usize) {
        self.cursor = 0;
        self.seek(events, station);
        self.state = 0;
    }

    /// 0x40d340's jump to the stage-1 bonus section: the cursor moves past the records of
    /// earlier stations, from where it is.
    pub fn skip_to(&mut self, events: &[TrafficEvent], station: usize) {
        self.seek(events, station);
    }

    /// Sets the mask state `0x857216` (2 after stage 6's bonus game with four marks).
    pub fn set_state(&mut self, state: u8) {
        self.state = state;
    }

    /// The station result path of 0x4080d0: `0x857216 = (continues < 1)`.
    pub fn after_arrival(&mut self, continues: i32) {
        self.state = u8::from(continues < 1);
    }

    fn seek(&mut self, events: &[TrafficEvent], station: usize) {
        while events
            .get(self.cursor)
            .is_some_and(|e| (i32::from(e.station)) < station as i32)
        {
            self.cursor += 1;
        }
    }

    /// Task list 5 is cleared (after an arrival, after the continue screen): every car goes.
    pub fn clear(&mut self) {
        self.cars.clear();
        self.pending = None;
        self.passing_sound = None;
    }

    pub fn cars(&self) -> &[Car] {
        &self.cars
    }

    /// Index of the next event in the stage's list.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Sound commands of the last tick.
    pub fn sounds(&self) -> &[TrafficSound] {
        &self.sounds
    }

    /// Start of a tick: forget last tick's sounds.
    pub fn begin_tick(&mut self) {
        self.sounds.clear();
    }

    /// 0x40b710: at most one event per call. It fires once the rounded distance to the stop
    /// is down to its distance at its station; it spawns only if its mask has the state's bit.
    pub fn event_step(&mut self, events: &[TrafficEvent], station: usize, env: &TrafficEnv<'_>) {
        let Some(e) = events.get(self.cursor) else {
            return;
        };
        if env.dist_round_m > i32::from(e.distance_m) || i32::from(e.station) != station as i32 {
            return;
        }
        self.cursor += 1;
        if e.mask & mask_bit(self.state) != 0 {
            self.spawn(&SpawnRequest::from_event(e), env);
        }
    }

    /// Schedules the passing train of a scripted red-signal recovery.
    pub fn schedule_passing(&mut self, p: PassingTrain) {
        self.pending = Some((p.after_ticks, p));
    }

    /// The spawner (0x404f10, 0x405e80 for row 0x43).
    pub fn spawn(&mut self, req: &SpawnRequest, env: &TrafficEnv<'_>) {
        let route = env.route;
        let Some(sp) = usize::try_from(req.spawn)
            .ok()
            .and_then(|i| route.traffic.get(i))
            .copied()
        else {
            return;
        };
        let Some(slot) = sp.slot else {
            return;
        };
        let row = if req.model == -1 {
            let ty = sp.train_type as i16;
            if ty < 1 {
                return;
            }
            (ty - 1) as usize
        } else {
            match usize::try_from(req.model) {
                Ok(r) => r,
                Err(_) => return,
            }
        };
        let Some(model) = env.models.row(row) else {
            return;
        };
        let special = row == SPECIAL_ROW;
        let mut dir = u32::from(sp.flags & 1);
        if !special && req.flags & FLAG_REVERSE != 0 {
            dir ^= 1;
        }
        // Row 0x43 runs against its spawn's direction flag (0x405e80: `+0x30 = flag ^ 1`).
        let forward = if special { dir == 0 } else { dir == 1 };
        let cars = if !special && matches!(model.kind, 3 | 7 | 8) {
            reversed(&model.cars[(dir ^ 1) as usize])
        } else {
            model.cars[dir as usize].clone()
        };

        let mut pos = slot;
        if !special && row > STEP_ONCE_ABOVE_ROW {
            pos = step(route, pos, forward);
        }
        let mut speed_kmh = req.speed_kmh;
        let mut mode = req.flags;
        if !special && (mode == MODE_JITTER || mode & FLAG_JITTER != 0) {
            speed_kmh += 15 - self.rand.next() % 31;
            if speed_kmh < 0 {
                speed_kmh = 10;
            }
            mode = if mode == MODE_JITTER { 0 } else { mode & 0xf };
            let r = self.rand.next();
            self.rand = CrtRand(r.wrapping_mul(env.tick as i32) as u32);
        }
        mode &= 0xf;
        let spaced = req.flags & FLAG_NO_EMERGE != 0;
        let mut param = req.param;
        let mut carry = 0.0f32;
        let bogie_points = if special {
            BOGIE_POINTS_SPECIAL
        } else {
            BOGIE_POINTS
        };

        for (index, m) in cars.iter().enumerate() {
            if mode == 5 {
                pos = step_n(route, pos, forward, param.max(0) as usize);
                mode = 0;
                param = 0;
            }
            let delay = if !spaced && mode != 0 {
                // Cars leave the spawn point one after another.
                if special { -m.offset } else { m.offset.abs() }
            } else {
                let mut d = -m.spacing - carry.abs();
                while d > 0.0 {
                    d -= UNITS_PER_POINT;
                    if forward {
                        pos = prev_slot(route, pos);
                        carry = -d;
                    } else {
                        pos = next_slot(route, pos);
                        carry = d;
                    }
                }
                0.0
            };
            let rear = step_n(route, pos, !forward, bogie_points);
            let v = f64::from(speed_kmh) * KMH_TO_UNITS;
            let p = f64::from(param);
            let mut waits = false;
            let mut stop_at = 0.0f32;
            let accel;
            match mode {
                4 if env.obstacle_armed => {
                    stop_at = (p * METRES_TO_UNITS) as f32;
                    waits = true;
                    accel = if param < MODE4_LONG_PARAM {
                        v * v / (p * BRAKE_DIVISOR) * SPEED_K_INV
                    } else {
                        v * v * MODE4_LONG_FACTOR * SPEED_K_INV
                    };
                }
                4 => {
                    accel = 0.0;
                    mode = 0;
                }
                3 => {
                    stop_at = (p * METRES_TO_UNITS) as f32;
                    accel = if param < MODE3_LONG_PARAM {
                        v * v / (p * BRAKE_DIVISOR) * SPEED_K_INV
                    } else {
                        v * v * MODE3_LONG_FACTOR * SPEED_K_INV
                    };
                }
                2 => accel = v * v / (p * BRAKE_DIVISOR) * SPEED_K_INV,
                _ => accel = p * KMH_TO_UNITS * ACCEL_PER_PARAM,
            }
            let at = Bogies {
                front: pos,
                rear,
                t: carry,
            };
            self.cars.push(Car {
                row,
                model: *m,
                coupled: *m,
                index: index as u32,
                forward,
                turned: !special && index == 0 && !forward && TURNED_ROWS.contains(&row),
                mode,
                waits_for_obstacle: waits,
                at,
                run: 0.0,
                stop_at,
                speed: v as f32,
                speed_k: (f64::from(speed_kmh) * KMH_TO_SPEED_K) as f32,
                accel: accel as f32,
                delay,
                gate: SOUND_GATE_START,
                sequence_done: false,
                prev: at,
                moved: 0.0,
            });
        }
    }

    /// 0x406c50: the coupling minigame's waiting set, parked (speed 0, mode 0) facing forward
    /// with its head `ahead_points` past the cab at `engine_index`, each later car walked back
    /// by its spacing; the coupler frame goes back to 0.
    pub fn place_waiting_set(&mut self, set: &WaitingSet, engine_index: usize, env: &TrafficEnv) {
        let route = env.route;
        let (Some(a), Some(b)) = (env.models.row(set.rows.0), env.models.row(set.rows.1)) else {
            return;
        };
        let Some(head) = (engine_index as i64)
            .checked_add(set.ahead_points)
            .and_then(|i| usize::try_from(i).ok())
            .map(|i| {
                TrackPos {
                    engine_index: i,
                    t: 0.0,
                }
                .slot(route)
            })
            .filter(|&s| s < route.slots.len())
        else {
            return;
        };
        let mut pos = head;
        let mut carry = 0.0f32;
        for (index, (m, m_coupled)) in a.cars[1].iter().zip(&b.cars[1]).enumerate() {
            let mut d = -m.spacing - carry;
            while d > 0.0 {
                d -= UNITS_PER_POINT;
                carry = -d;
                pos = prev_slot(route, pos);
            }
            let at = Bogies {
                front: pos,
                rear: step_n(route, pos, false, WAITING_BOGIE_POINTS),
                t: carry,
            };
            self.cars.push(Car {
                row: set.rows.0,
                model: *m,
                coupled: *m_coupled,
                index: index as u32,
                forward: true,
                turned: false,
                mode: 0,
                waits_for_obstacle: false,
                at,
                run: 0.0,
                stop_at: 0.0,
                speed: 0.0,
                speed_k: 0.0,
                accel: 0.0,
                delay: 0.0,
                gate: WAITING_GATE,
                sequence_done: false,
                prev: at,
                moved: 0.0,
            });
        }
        self.coupler_frame = false;
    }

    /// 0x406960: a train of `t.row` placed with its head `t.ahead_points` past the cab at
    /// `engine_index`. Unlike the spawner the bogies are 12 points apart and the gate starts
    /// at 9000; the car list is picked as by the spawner (reversed for kinds 3, 7 and 8).
    pub fn place_train(&mut self, t: &PlacedTrain, engine_index: usize, env: &TrafficEnv) {
        let route = env.route;
        let Some(model) = env.models.row(t.row) else {
            return;
        };
        let Some(head) = (engine_index as i64)
            .checked_add(t.ahead_points)
            .and_then(|i| usize::try_from(i).ok())
            .map(|i| {
                TrackPos {
                    engine_index: i,
                    t: 0.0,
                }
                .slot(route)
            })
            .filter(|&s| s < route.slots.len())
        else {
            return;
        };
        let dir = usize::from(t.forward);
        let cars = if matches!(model.kind, 3 | 7 | 8) {
            reversed(&model.cars[dir ^ 1])
        } else {
            model.cars[dir].clone()
        };
        let speed_k = t.speed_kmh as f32 * KMH_TO_SPEED_K as f32;
        let accel = if t.mode == 2 {
            speed_k * speed_k / (t.param as f32 + t.param as f32)
        } else {
            t.param as f32 * KMH_TO_UNITS as f32 * ACCEL_PER_PARAM as f32
        };
        let mut pos = head;
        let mut carry = 0.0f32;
        for (index, m) in cars.iter().enumerate() {
            let delay = if t.spaced {
                let mut d = -m.spacing - carry.abs();
                while d > 0.0 {
                    d -= UNITS_PER_POINT;
                    pos = step(route, pos, !t.forward);
                    // The fraction is only taken over while it is not zero: a spacing of
                    // whole points keeps the one of the walk's last but one step.
                    if d.abs() > 0.0 {
                        carry = if t.forward { -d } else { d };
                    }
                }
                0.0
            } else {
                -m.offset
            };
            let at = Bogies {
                front: pos,
                rear: step_n(route, pos, !t.forward, PLACED_BOGIE_POINTS),
                t: carry,
            };
            self.cars.push(Car {
                row: t.row,
                model: *m,
                coupled: *m,
                index: index as u32,
                forward: t.forward,
                turned: false,
                mode: t.mode,
                waits_for_obstacle: false,
                at,
                run: 0.0,
                stop_at: 0.0,
                speed: t.speed_kmh as f32 * KMH_TO_UNITS as f32,
                speed_k,
                accel,
                delay,
                gate: PLACED_GATE,
                sequence_done: false,
                prev: at,
                moved: 0.0,
            });
        }
    }

    /// Sets the coupler frame `0x857244` (the coupling minigame's success outro and time-up).
    pub fn set_coupler_frame(&mut self, on: bool) {
        self.coupler_frame = on;
    }

    pub fn coupler_frame(&self) -> bool {
        self.coupler_frame
    }

    /// The car tasks of one `Task_Tick` (0x405420 / 0x406220). Runs every tick the task lists
    /// run (driving, holds, result and loading ticks alike).
    pub fn task_step(&mut self, env: &TrafficEnv<'_>) {
        if let Some((ticks, p)) = self.pending {
            if ticks == 0 {
                self.pending = None;
                self.spawn(&p.request, env);
                self.passing_sound = Some((p.sound, 0));
            } else {
                self.pending = Some((ticks - 1, p));
            }
        }
        if let Some((kind, i)) = self.passing_sound {
            match (kind, i) {
                (PassingSound::Horn, 40) => self.sounds.push(TrafficSound::Play(8)),
                (PassingSound::Horn, 100) => self.sounds.push(TrafficSound::Release(8)),
                (PassingSound::HornAndRumble, 45) => {
                    self.sounds.push(TrafficSound::Play(1));
                    self.sounds.push(TrafficSound::Play(4));
                }
                (PassingSound::HornAndRumble, 100) => self.sounds.push(TrafficSound::Release(4)),
                _ => {}
            }
            self.passing_sound = (i < 100).then_some((kind, i + 1));
        }
        let route = env.route;
        let sounds = &mut self.sounds;
        self.cars.retain_mut(|c| {
            c.prev = c.at;
            update(c, env, sounds) && normalise(route, &mut c.at)
        });
    }

    /// The draw step's sounds (0x405640 / 0x406420) for the camera after this tick.
    pub fn draw_step(&mut self, route: &Route, camera: &CabCamera) {
        for c in &mut self.cars {
            if ROWS_WITH_EXTRA.contains(&c.row) {
                continue;
            }
            let Some(origin) = origin(route, &c.at, c.forward) else {
                continue;
            };
            let (x, z) = view_xz(camera, origin);
            if lod_for_view(x, z).is_none() {
                continue;
            }
            let inside = |(lo, hi): (f64, f64)| (lo..=hi).contains(&z);
            if c.special() && !c.sequence_done && c.index == 0 && inside(SPECIAL_SEQUENCE_WINDOW) {
                self.sounds.push(TrafficSound::Play(SOUND_SPECIAL_SEQUENCE));
                c.sequence_done = true;
            }
            if x.abs() > SOUND_LATERAL || f64::from(c.speed) < SOUND_MIN_SPEED {
                continue;
            }
            if c.forward {
                for (lo, hi, above, set) in SAME_WINDOWS {
                    if inside((lo, hi)) && c.gate > above {
                        self.sounds.push(TrafficSound::Play(SOUND_PASSING_CAR));
                        c.gate = set;
                    }
                }
            } else {
                for w in ONCOMING_WINDOWS {
                    if inside(w) {
                        self.sounds.push(TrafficSound::Play(SOUND_PASSING_CAR));
                    }
                }
            }
        }
    }

    /// Car poses at `alpha` (0 = the tick before, 1 = the last tick) between the last two
    /// ticks, interpolated along the track. Cars at the end of their track are not drawn.
    pub fn poses(&self, route: &Route, alpha: f32) -> Vec<CarPose> {
        self.cars
            .iter()
            .filter_map(|c| {
                let mut b = c.prev;
                b.t += c.moved * alpha.clamp(0.0, 1.0);
                normalise(route, &mut b);
                pose(route, c, &b, self.coupler_frame)
            })
            .collect()
    }
}

/// 0x404ea0: the list in reverse order with offsets and spacings recomputed for that order.
fn reversed(cars: &[CarModel]) -> Vec<CarModel> {
    let mut out = Vec::with_capacity(cars.len());
    // The list terminator's offset and spacing (0 in every list) start the walk.
    let (mut offset, mut spacing) = (0.0f32, 0.0f32);
    for c in cars.iter().rev() {
        out.push(CarModel {
            lods: c.lods,
            offset,
            spacing,
        });
        offset += c.spacing;
        spacing = c.spacing;
    }
    out
}

/// One car task update (0x405420; 0x406220 for row 0x43). Returns false when the car ends.
fn update(c: &mut Car, env: &TrafficEnv<'_>, sounds: &mut Vec<TrafficSound>) -> bool {
    if !env.obstacle_armed && c.waits_for_obstacle {
        c.mode = 1;
    }
    match c.mode {
        1 => {
            c.speed_k = (f64::from(c.accel) + f64::from(c.speed_k)) as f32;
            c.speed = (f64::from(c.speed_k) * SPEED_K_SCALE) as f32;
            if c.waits_for_obstacle && f64::from(c.speed) >= RESTART_TOP_SPEED {
                c.waits_for_obstacle = false;
                c.mode = 0;
            }
        }
        2 => {
            c.speed_k = (f64::from(c.speed_k) - f64::from(c.accel)) as f32;
            if c.speed_k < 0.0 {
                c.speed_k = 0.0;
            }
            c.speed = (f64::from(c.speed_k) * SPEED_K_SCALE) as f32;
        }
        3 | 4 => {
            let left = f64::from(c.stop_at) - f64::from(c.run);
            let at = if c.mode == 3 {
                MODE3_BRAKE_UNITS
            } else {
                MODE4_BRAKE_UNITS
            };
            if left < at {
                c.mode = 2;
            }
        }
        // Mode 5 never reaches a task (the spawner turns it into 0); kept as in 0x405420.
        5 if !c.special() && env.dist_round_m < 1 => {
            sounds.push(TrafficSound::Play(SOUND_MODE5));
            c.mode = 1;
        }
        _ => {}
    }
    if c.special() && env.dist_round_m < 1 && c.gate == SOUND_GATE_START {
        c.gate = SOUND_GATE_START - 1;
    }
    let before = c.at.t;
    if c.delay > 0.0 {
        let d = c.delay - c.speed;
        c.delay = d;
        if d <= 0.0 {
            if c.forward {
                c.at.t -= d;
            } else {
                c.at.t += d;
            }
        }
    } else if c.forward {
        c.at.t += c.speed;
    } else {
        c.at.t -= c.speed;
    }
    c.moved = c.at.t - before;
    c.run += c.speed.abs();
    true
}

/// Interpolated bogie point: `slot` towards its neighbour in the running direction by
/// `|t| · 8` (0x405640). `None` at the end of the track (the draw returns early).
fn bogie_point(route: &Route, slot: usize, forward: bool, t: f32) -> Option<[f32; 3]> {
    let next = step(route, slot, forward);
    if next == slot {
        return None;
    }
    let (a, b) = (route.slots.get(slot)?, route.slots.get(next)?);
    let f = f64::from(t.abs()) * 8.0;
    let lerp = |p: f32, q: f32| (f64::from(q) - f64::from(p)) * f + f64::from(p);
    Some([
        lerp(a.x, b.x) as f32,
        lerp(a.y(), b.y()) as f32,
        lerp(a.z, b.z) as f32,
    ])
}

fn origin(route: &Route, b: &Bogies, forward: bool) -> Option<[f32; 3]> {
    let f = bogie_point(route, b.front, forward, b.t)?;
    let r = bogie_point(route, b.rear, forward, b.t)?;
    Some([0, 1, 2].map(|i| ((f64::from(f[i]) + f64::from(r[i])) * 0.5) as f32))
}

/// The model transform of 0x405640: origin between the bogies, local z along the track
/// towards higher slots, a half turn where the original turns the car, and a pitch when the
/// bogie points differ in height.
fn pose(route: &Route, c: &Car, b: &Bogies, coupler_frame: bool) -> Option<CarPose> {
    let f = bogie_point(route, b.front, c.forward, b.t)?;
    let r = bogie_point(route, b.rear, c.forward, b.t)?;
    let origin = [0, 1, 2].map(|i| ((f64::from(f[i]) + f64::from(r[i])) * 0.5) as f32);
    let sign = if c.forward { 1.0 } else { -1.0 };
    let (dx, dz) = (
        sign * (f64::from(f[0]) - f64::from(r[0])),
        sign * (f64::from(f[2]) - f64::from(r[2])),
    );
    let len = (dx * dx + dz * dz).sqrt();
    if len == 0.0 {
        return None;
    }
    let (s, cs) = ((dx / len) as f32, (dz / len) as f32);
    let flat = Affine::yaw(origin, s, cs);
    // Rise along local +z per unit of horizontal length.
    let heights_differ = route.slots[b.front].height_raw != route.slots[b.rear].height_raw;
    let rise = sign * (f64::from(f[1]) - f64::from(r[1])) / len;
    let pitched = |m: Affine, rise: f64| {
        if !heights_differ {
            return m;
        }
        // Follow the track: tilt local z up by atan(rise) (sign: traffic.md §4, guess).
        let a = rise.atan();
        let (sn, cn) = (a.sin() as f32, a.cos() as f32);
        let mut out = m;
        for i in 0..3 {
            out.z_axis[i] = cn * m.z_axis[i] + sn * m.y_axis[i];
            out.y_axis[i] = cn * m.y_axis[i] - sn * m.z_axis[i];
        }
        out
    };
    let unturned = pitched(flat, rise);
    let half_turn = |m: Affine| Affine {
        x_axis: m.x_axis.map(|v| -v),
        z_axis: m.z_axis.map(|v| -v),
        ..m
    };
    let turned = pitched(half_turn(flat), -rise);
    let transform = if c.special() || c.turned {
        turned
    } else {
        unturned
    };
    let extra = ROWS_WITH_EXTRA.contains(&c.row).then(|| {
        let mut m = transform;
        m.translation = m.transform_point(EXTRA_OFFSET);
        (EXTRA_PART, m)
    });
    Some(CarPose {
        lods: if coupler_frame {
            c.coupled.lods
        } else {
            c.model.lods
        },
        transform,
        unturned,
        special: c.special(),
        extra,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lod_bands() {
        assert_eq!(lod_for_view(0.0, 0.0), Some(0));
        assert_eq!(lod_for_view(0.0, 2.5), None);
        assert_eq!(lod_for_view(0.0, -2.9), Some(0));
        assert_eq!(lod_for_view(0.0, -3.0), Some(0));
        assert_eq!(lod_for_view(0.0, -3.1), Some(1));
        assert_eq!(lod_for_view(0.0, -14.0), Some(4));
        assert_eq!(lod_for_view(0.0, -15.1), None);
        assert_eq!(lod_for_view(20.0, -1.0), Some(1));
        assert_eq!(lod_for_view(33.0, -1.0), Some(3));
        assert_eq!(lod_for_view(33.0, -4.0), Some(4));
        assert_eq!(lod_for_view(33.0, -7.0), None);
        assert_eq!(lod_for_view(65.0, 0.0), None);
    }

    #[test]
    fn crt_rand_matches_msvc() {
        // MSVC: srand(1); rand() → 41, 18467, 6334.
        let mut r = CrtRand(1);
        assert_eq!([r.next(), r.next(), r.next()], [41, 18467, 6334]);
    }

    #[test]
    fn reversed_list_recomputes_offsets() {
        let car = |p: u32, offset: f32, spacing: f32| CarModel {
            lods: [p; 5],
            offset,
            spacing,
        };
        let list = [car(1, 0.0, 0.0), car(2, -2.0, -2.0), car(3, -4.5, -2.5)];
        let r = reversed(&list);
        assert_eq!(r.iter().map(|c| c.lods[0]).collect::<Vec<_>>(), [3, 2, 1]);
        assert_eq!(
            r.iter().map(|c| (c.offset, c.spacing)).collect::<Vec<_>>(),
            [(0.0, 0.0), (-2.5, -2.5), (-4.5, -2.0)]
        );
    }

    #[test]
    fn mask_bits() {
        assert_eq!(mask_bit(0), 1);
        assert_eq!(mask_bit(1), 2);
        assert_eq!(3 & mask_bit(1), 2);
        assert_eq!(1 & mask_bit(1), 0);
    }
}

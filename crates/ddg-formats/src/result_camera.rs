//! The station-result replay (0x419730, task update 0x41a010, draw 0x41a470): a small
//! platform set at the world origin, the player's train sliding in along +z as the last
//! 120 ticks of the approach are replayed, an orbit camera that follows the stop error, and a
//! fixed "platform monitor" camera. Layout, timing and evidence: docs/re/scene.md §8.
//!
//! Tables are read from the user's exe; the scalar constants are small and carry their
//! addresses.

use crate::error::Result;
use crate::exe::Exe;
use crate::stages::STAGE_COUNT;

const WHAT: &str = "result camera tables";

/// Orbit keys: 7 f32 each — centre x, centre z, eye radius, target radius, eye y, target y,
/// angle. Three per set (16 m short / at the stop / 13 m past): the normal set at 0x469454 and
/// the set for platform part 0x23 at 0x4694c4.
const KEYS_NORMAL: u32 = 0x469454;
const KEYS_PLATFORM_23: u32 = 0x4694c4;
const KEY_LEN: u32 = 28;
/// `0x4696cc[train type]` (f32): how far the train's head stops behind the platform origin.
const TRAIN_OFFSETS: u32 = 0x4696cc;
const TRAIN_OFFSET_COUNT: usize = 20;
/// `0x4596e0[stage]` (i32): row of the train-model table whose direction-1 car list is drawn
/// (0x419730 reads `0x45735c[row·3]`).
const STAGE_ROWS: u32 = 0x4596e0;

/// Stop error to world units along the platform: `z = dist × −0.1` (f64 at 0x469758).
const METRES_TO_Z: f64 = -0.1;
/// Past-the-stop range of the orbit, in units: 0.1 (f64 0x44e8e0) .. 1.3 (f64 0x469738, f32
/// 0x469730), scaled by 1/1.2 (f64 0x469728).
const PAST_FROM: f32 = 0.1;
const PAST_TO: f32 = 1.3;
const PAST_SCALE: f64 = 1.0 / 1.2;
/// Short-of-the-stop range: down to −1.6 units (f64 0x458bc0, f32 0x469748), scaled by 0.625
/// (f64 0x469740).
const SHORT_TO: f32 = -1.6;
const SHORT_SCALE: f64 = 0.625;
/// Train offset at the coupling stations (0x3f88f5c3 at 0x4198b6 / 0x41998d).
const COUPLING_OFFSET: f32 = f32::from_bits(0x3f88_f5c3);
/// The coupling stations shift the whole record by 40 m (f64 at 0x4594b0).
pub const COUPLING_SHIFT_M: f32 = 40.0;
/// The draw switches to the "after" car lists once the record shows the train stopped
/// (sample < 0.01, f64 at 0x469750) for more than 29 (lead) / 39 (waiting train) draws.
pub const STOPPED_BELOW_M: f32 = 0.01;
pub const LEAD_SWITCH_AFTER: i32 = 0x1d;
pub const WAITING_SWITCH_AFTER: i32 = 0x27;
/// `task+0x1cc` starts at −9999 (0xffffd8f1, 0x419839) unless `0x855ff8 & 0x40000`.
pub const STOPPED_COUNTER_START: i32 = -9999;

/// The 3D views of the result screen (`0x84c128 = 1`, render task 0x425680 mode 1) in the
/// 496×400 virtual screen, with the projection each one sets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResultViewport {
    /// Centre and half size, virtual units (0x436eb0 arguments).
    pub centre: [f32; 2],
    pub half: [f32; 2],
    /// Vertical FOV, tenths of a degree, and the projection aspect (0x4183b0 arguments).
    pub fov_tenths: u16,
    pub aspect: f32,
}

/// The orbit view: centre (336, 240), half (160, 156) (pushes at 0x425897..0x4258a6), FOV
/// 450.0 (0x43e10000) and aspect 1.02 (0x3f828f5c) at 0x4258cf / 0x4258d4.
pub const ORBIT_VIEWPORT: ResultViewport = ResultViewport {
    centre: [336.0, 240.0],
    half: [160.0, 156.0],
    fov_tenths: 450,
    aspect: f32::from_bits(0x3f82_8f5c),
};
/// The monitor view: centre (90, 240), half (76, 80) (pushes at 0x425934..0x42593d), FOV
/// 755.0 (0x443cc000) and aspect 1.0 at 0x425963 / 0x425968.
pub const MONITOR_VIEWPORT: ResultViewport = ResultViewport {
    centre: [90.0, 240.0],
    half: [76.0, 80.0],
    fov_tenths: 755,
    aspect: 1.0,
};

/// One orbit key.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct OrbitKey {
    pub centre: [f32; 2],
    pub eye_radius: f32,
    pub target_radius: f32,
    pub eye_y: f32,
    pub target_y: f32,
    pub angle: f32,
}

impl OrbitKey {
    fn lerp(&self, to: &OrbitKey, t: f32) -> OrbitKey {
        let l = |a: f32, b: f32| (b - a) * t + a;
        OrbitKey {
            centre: [
                l(self.centre[0], to.centre[0]),
                l(self.centre[1], to.centre[1]),
            ],
            eye_radius: l(self.eye_radius, to.eye_radius),
            target_radius: l(self.target_radius, to.target_radius),
            eye_y: l(self.eye_y, to.eye_y),
            target_y: l(self.target_y, to.target_y),
            angle: l(self.angle, to.angle),
        }
    }

    /// Eye and target: on opposite sides of the centre, the eye at `angle + π`.
    fn camera(&self) -> ResultCamera {
        let a = f64::from(self.angle);
        let at = |r: f32, a: f64, y: f32| {
            [
                (a.sin() * f64::from(r) + f64::from(self.centre[0])) as f32,
                y,
                (a.cos() * f64::from(r) + f64::from(self.centre[1])) as f32,
            ]
        };
        ResultCamera {
            // π: f64 at 0x458bd8.
            eye: at(self.eye_radius, a + std::f64::consts::PI, self.eye_y),
            target: at(self.target_radius, a, self.target_y),
        }
    }
}

/// A look-at pose in the platform set's space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResultCamera {
    pub eye: [f32; 3],
    pub target: [f32; 3],
}

/// The four platform sets, picked by timetable field 7 of the arrival station
/// (`row + 0x40`, 0x419a30).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformSet {
    /// Field 7 = 0 or anything not listed: part 0x1e.
    Default,
    /// 1: part 0x23, its own orbit keys and monitor pose.
    Set1,
    /// 2: part 0x21, the default monitor pose.
    Set2,
    /// 3: part 0x22, its own monitor pose.
    Set3,
}

impl PlatformSet {
    pub fn from_field(field7: i32) -> Self {
        match field7 {
            1 => Self::Set1,
            2 => Self::Set2,
            3 => Self::Set3,
            _ => Self::Default,
        }
    }

    /// The platform part drawn at the origin (`task+0x4c`, 0x419a34..0x419adb).
    pub fn part(self) -> u32 {
        match self {
            Self::Default => 0x1e,
            Self::Set1 => 0x23,
            Self::Set2 => 0x21,
            Self::Set3 => 0x22,
        }
    }

    /// The fixed monitor camera (`0x84c100..108` eye, `0x84c13c..144` target).
    pub fn monitor(self) -> ResultCamera {
        let f = f32::from_bits;
        match self {
            // 0x419a4c..0x419a7e.
            Self::Set1 => ResultCamera {
                eye: [f(0xbee0_7a8d), f(0x3e99_999a), f(0x3f16_ba6b)],
                target: [f(0x3f20_3ce2), f(0xbf05_1eb8), f(0xc00b_c5ca)],
            },
            // 0x419a9d..0x419acf.
            Self::Set3 => ResultCamera {
                eye: [f(0x3ef0_d845), f(0x3e94_7ae1), f(0x3f1c_2268)],
                target: [f(0xbf55_73eb), f(0xbf05_1eb8), f(0xc006_9446)],
            },
            // 0x419ae2..; the same pose RenderTask_Create (0x4254b0) starts with.
            Self::Default | Self::Set2 => ResultCamera {
                eye: [f(0x3f19_999a), f(0x3e99_999a), f(0x3f3b_645a)],
                target: [f(0xbf24_9ba6), f(0xbea3_d70a), f(0xbfcd_b22d)],
            },
        }
    }
}

/// Which train-model rows the replay draws (all direction-1 car lists of the table at
/// 0x457358; `TrainModels::rows[row].cars[1]`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrainSet {
    /// The player's train (`task+0x1d0`).
    pub lead: usize,
    /// Its list once stopped long enough (`+0x1d4`), coupling stations only.
    pub lead_after: Option<usize>,
    /// The waiting train's last two cars ahead of the stop (`+0x1d8`) and its "after" list
    /// (`+0x1dc`).
    pub waiting: Option<(usize, usize)>,
    /// `task+0x7c`: [`ResultTables::train_offset`] or 1.07 at the coupling stations.
    pub offset: f32,
    /// Added to the whole record before the replay (40 m with a waiting train).
    pub shift_m: f32,
}

/// The coupling-stage flags 0x419730 reads (course event 12, 0x40acd0, sets one of the first
/// two; the port's engine does not run that event yet).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CouplingFlags {
    /// `0x855ff8 & 0x8000000`.
    pub waiting_far: bool,
    /// `0x855ff8 & 0x10000000`.
    pub waiting_near: bool,
}

/// The exe tables of the replay.
#[derive(Debug, Clone, PartialEq)]
pub struct ResultTables {
    /// `[normal, platform 0x23]` × `[16 m short, at the stop, 13 m past]`.
    pub keys: [[OrbitKey; 3]; 2],
    pub train_offsets: Vec<f32>,
    pub stage_rows: Vec<i32>,
}

impl ResultTables {
    pub fn read(exe: &Exe<'_>) -> Result<Self> {
        let key = |va: u32| -> Result<OrbitKey> {
            let r = exe.reader_at(va, KEY_LEN as usize, WHAT)?;
            Ok(OrbitKey {
                centre: [r.f32(0)?, r.f32(4)?],
                eye_radius: r.f32(8)?,
                target_radius: r.f32(12)?,
                eye_y: r.f32(16)?,
                target_y: r.f32(20)?,
                angle: r.f32(24)?,
            })
        };
        let set = |va: u32| -> Result<[OrbitKey; 3]> {
            Ok([key(va)?, key(va + KEY_LEN)?, key(va + 2 * KEY_LEN)?])
        };
        let offsets = exe.reader_at(TRAIN_OFFSETS, 4 * TRAIN_OFFSET_COUNT, WHAT)?;
        let rows = exe.reader_at(STAGE_ROWS, 4 * STAGE_COUNT, WHAT)?;
        Ok(Self {
            keys: [set(KEYS_NORMAL)?, set(KEYS_PLATFORM_23)?],
            train_offsets: (0..TRAIN_OFFSET_COUNT)
                .map(|i| offsets.f32(4 * i))
                .collect::<Result<_>>()?,
            stage_rows: (0..STAGE_COUNT)
                .map(|i| rows.i32(4 * i))
                .collect::<Result<_>>()?,
        })
    }

    /// `0x4696cc[train type]`.
    pub fn train_offset(&self, train_type: usize) -> f32 {
        self.train_offsets.get(train_type).copied().unwrap_or(0.0)
    }

    /// The car lists of 0x419730 for `stage` (`0x8570a4`) at segment-start station `station`
    /// (`0x856d78`; the train arrived at `station + 1`) with train type `train_type`
    /// (`0x84c3fc`).
    pub fn train_set(
        &self,
        stage: usize,
        station: usize,
        train_type: usize,
        flags: CouplingFlags,
    ) -> TrainSet {
        let table_row = self
            .stage_rows
            .get(stage)
            .and_then(|&r| usize::try_from(r).ok())
            .unwrap_or(0);
        let mut set = TrainSet {
            lead: table_row,
            lead_after: None,
            waiting: None,
            offset: self.train_offset(train_type),
            shift_m: 0.0,
        };
        // Stages 0–2 couple at station 23, stages 17–19 at station 15; the others always use
        // the stage's row.
        let coupling_station = match stage {
            0..=2 => 23,
            17.. => 15,
            _ => return set,
        };
        if station < coupling_station {
            return set;
        }
        if station > coupling_station {
            // Rows 8 / 7 (0x4573bc / 0x4573b0) after the coupled train type 14 / 12. Any other
            // type leaves `task+0x1d0` unset in the original (unreachable after a coupling);
            // the stage's row stands in.
            set.lead = match train_type {
                14 => 8,
                12 => 7,
                _ => table_row,
            };
            return set;
        }
        // Rows 12 / 13 (0x4573ec / 0x4573f8).
        set.lead = 12;
        set.lead_after = Some(13);
        set.offset = COUPLING_OFFSET;
        if flags.waiting_far {
            // Rows 17 / 18 (0x457428 / 0x457434).
            set.waiting = Some((17, 18));
            set.shift_m = COUPLING_SHIFT_M;
        } else if flags.waiting_near {
            // Rows 20 / 21 (0x45744c / 0x457458).
            set.waiting = Some((20, 21));
            set.shift_m = COUPLING_SHIFT_M;
        }
        set
    }

    /// The orbit camera for one replay sample (`dist_m`, positive = short), as 0x41a010.
    pub fn orbit(&self, platform: PlatformSet, dist_m: f32) -> ResultCamera {
        let [short, stop, past] = &self.keys[usize::from(platform == PlatformSet::Set1)];
        let v = (f64::from(dist_m) * METRES_TO_Z) as f32;
        let key = if v >= PAST_FROM {
            let t = ((f64::from(v.min(PAST_TO)) - f64::from(PAST_FROM)) * PAST_SCALE) as f32;
            stop.lerp(past, t)
        } else {
            let t = (-f64::from(v.clamp(SHORT_TO, 0.0)) * SHORT_SCALE) as f32;
            stop.lerp(short, t)
        };
        key.camera()
    }
}

/// The lead car's z for a replay sample: `dist × −0.1 − offset` (`task+0x20`).
pub fn train_z(dist_m: f32, offset: f32) -> f32 {
    (f64::from(dist_m) * METRES_TO_Z) as f32 - offset
}

/// The detail level 0x41a470 draws a car at, from its origin in the set's space; `None` =
/// not drawn. Bands of z (doubles at 0x469780, 0x469778, 0x458c28, 0x458c18, 0x458c10,
/// 0x458c08, 0x458c00) and of |x| (0x458be0, 0x469770, 0x458c48, 0x459530).
pub fn car_lod(x: f32, z: f32) -> Option<usize> {
    let z = f64::from(z);
    let mut lod = if z < -24.0 || z >= 2.5 {
        5
    } else if z < -18.0 {
        4
    } else if z < -12.0 {
        3
    } else if z < -6.0 {
        2
    } else if z < -3.0 {
        1
    } else {
        0
    };
    let ax = f64::from(x.abs());
    if ax > 16.0 {
        return None;
    }
    if ax > 12.0 {
        lod += 3;
    } else if ax > 8.0 {
        lod += 2;
    } else if ax > 4.0 {
        lod += 1;
    }
    (lod < 5).then_some(lod)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(angle: f32, eye_radius: f32) -> OrbitKey {
        OrbitKey {
            centre: [0.0, 0.0],
            eye_radius,
            target_radius: 2.0,
            eye_y: 0.5,
            target_y: -0.5,
            angle,
        }
    }

    fn tables() -> ResultTables {
        let set = [key(1.0, 1.0), key(2.0, 2.0), key(3.0, 3.0)];
        ResultTables {
            keys: [set, set],
            train_offsets: vec![0.0, 0.98],
            stage_rows: (0..20).collect(),
        }
    }

    #[test]
    fn orbit_blends_by_stop_error() {
        let t = tables();
        let at = |d: f32| t.orbit(PlatformSet::Default, d);
        // At the stop and up to 1 m past: the middle key.
        assert_eq!(at(0.0), key(2.0, 2.0).camera());
        assert_eq!(at(-0.9), key(2.0, 2.0).camera());
        // 16 m short and beyond: the first key; 13 m past and beyond: the last.
        assert_eq!(at(16.0), at(40.0));
        assert!((at(40.0).eye[1] - 0.5).abs() < 1e-6);
        let far = key(3.0, 3.0).camera();
        let c = at(-20.0);
        assert!((c.eye[0] - far.eye[0]).abs() < 1e-5 && (c.eye[2] - far.eye[2]).abs() < 1e-5);
    }

    #[test]
    fn eye_and_target_face_each_other_through_the_centre() {
        let c = key(0.0, 1.0).camera();
        assert!((c.eye[2] + 1.0).abs() < 1e-6 && (c.target[2] - 2.0).abs() < 1e-6);
    }

    #[test]
    fn lod_bands() {
        assert_eq!(car_lod(0.0, 0.0), Some(0));
        assert_eq!(car_lod(0.0, -2.0), Some(0));
        assert_eq!(car_lod(0.0, -4.0), Some(1));
        assert_eq!(car_lod(0.0, -20.0), Some(4));
        assert_eq!(car_lod(0.0, -30.0), None);
        assert_eq!(car_lod(0.0, 3.0), None);
        assert_eq!(car_lod(5.0, -20.0), None);
    }

    #[test]
    fn coupling_stations() {
        let t = tables();
        let plain = t.train_set(13, 5, 1, CouplingFlags::default());
        assert_eq!((plain.lead, plain.offset, plain.waiting), (13, 0.98, None));
        let c = t.train_set(2, 23, 1, CouplingFlags::default());
        assert_eq!((c.lead, c.lead_after), (12, Some(13)));
        let w = t.train_set(
            18,
            15,
            1,
            CouplingFlags {
                waiting_near: true,
                ..Default::default()
            },
        );
        assert_eq!((w.waiting, w.shift_m), (Some((20, 21)), 40.0));
        assert_eq!(t.train_set(1, 24, 14, CouplingFlags::default()).lead, 8);
    }
}

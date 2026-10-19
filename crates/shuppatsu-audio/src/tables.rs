//! Sound tables read from the user's `dengo3k.exe` at runtime (audio-text.md §4–§6).
//!
//! Only table *addresses* and sizes are hardcoded here; the contents come from the exe.

use crate::code::LAST_BGM_CODE;
use crate::{Error, Result};
use shuppatsu_formats::exe::Exe;

/// Number of stages (routes) the per-stage tables cover.
pub const STAGE_COUNT: usize = 20;

/// Per-code volume bytes: `i8` at `0x486b4e + 2·code`, high byte unused (0x430da0).
const CODE_VOLUME_TABLE: u32 = 0x486b4e;
/// Sequence pointer table, 47 entries; the "SOUND VOLUME" string follows at 0x48624c.
const SEQUENCE_TABLE: u32 = 0x486190;
const SEQUENCE_COUNT: usize = 47;
/// (code, delay) pairs used by the two-phrase calls (0x41ffe0).
const CALL_PAIR_TABLE: u32 = 0x46c368;
/// Upper bound when scanning the pair table (its end is not delimited in the exe).
const CALL_PAIR_MAX: usize = 512;
/// Per-stage motor / rail-joint / horn codes, i16 × 20 each (read by 0x40d040).
const STAGE_MOTOR_CODE: u32 = 0x459e38;
const STAGE_JOINT_CODE: u32 = 0x459e60;
const STAGE_HORN_CODE: u32 = 0x459e88;
/// Per-stage base pitch parameter of the motor sound, i16 × 20 (0x4229c0, 0x424070).
const STAGE_MOTOR_BASE_PITCH: u32 = 0x45adb0;
/// Rail-joint patterns, 21 rows of 8 × i32 (0x422c20 at 0x423c79..0x423e2b).
const JOINT_TABLE: u32 = 0x471f88;
const JOINT_ROWS: usize = 21;

/// Number of entries in [`CodeVolumes`] (codes 0..=0x1a8).
pub const CODE_COUNT: usize = LAST_BGM_CODE as usize + 1;

/// The per-code "CODE VOLUME" table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeVolumes(pub [i8; CODE_COUNT]);

impl CodeVolumes {
    /// Every code at the same volume byte (for tests and tools without the exe).
    pub fn uniform(v: i8) -> Self {
        Self([v; CODE_COUNT])
    }

    /// The volume byte of `code` (0 for codes outside the table).
    pub fn get(&self, code: u16) -> i8 {
        self.0.get(code as usize).copied().unwrap_or(0)
    }
}

impl Default for CodeVolumes {
    fn default() -> Self {
        Self::uniform(0x7f)
    }
}

/// One sequence step: play `code`, then wait `delay` sequencer ticks before the next step.
/// `code == −1` ends a list; a step whose `delay` is −1 ends the sequence when reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub code: i16,
    pub delay: i16,
}

impl Step {
    pub const END: Self = Self {
        code: -1,
        delay: -1,
    };

    pub const fn new(code: i16, delay: i16) -> Self {
        Self { code, delay }
    }
}

/// Per-stage sound codes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StageSounds {
    /// Motor sound (0x8570b6); 0x31 for every stage on the 3000-bandai disc.
    pub motor: u16,
    /// Rail-joint click (0x856d98).
    pub joint: u16,
    /// Horn (0x8570a2). 0x1a3 is a BGM-slot melody horn.
    pub horn: u16,
    /// Motor pitch parameter while stopped (0x45adb0).
    pub motor_base_pitch: i16,
}

/// One rail-joint pattern row (stride 0x20 at 0x471f88).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JointPattern {
    /// Pattern length in 0.1 m (`trunc(odometer × 10) % period`).
    pub period: i32,
    /// Phases (0.1 m) of the four clicks.
    pub clicks: [i32; 4],
    /// +0x14: bit 0 brake-air sounds, bit 1 flange squeal / start sound, bit 2 rail joints.
    pub flags: u32,
}

impl JointPattern {
    pub const BRAKE_AIR: u32 = 1;
    pub const FLANGE: u32 = 2;
    pub const JOINTS: u32 = 4;
}

/// Everything the sound code reads from the exe.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SoundTables {
    pub code_volume: CodeVolumes,
    /// Sequences started by `snd_request(0x1000 | id)`, index = id.
    pub sequences: Vec<Vec<Step>>,
    /// Pair table for [`crate::SoundSystem::call_pair`].
    pub call_pairs: Vec<Step>,
    pub stages: [StageSounds; STAGE_COUNT],
    /// Rail-joint rows, indexed by train type; row 20 is the override row (0x8570c2).
    pub joints: Vec<JointPattern>,
}

impl SoundTables {
    pub fn from_exe(exe: &Exe<'_>) -> Result<Self> {
        let i16_at = |va: u32| -> Result<i16> {
            let b = exe.bytes_at(va, 2)?;
            Ok(i16::from_le_bytes([b[0], b[1]]))
        };
        let i32_at = |va: u32| -> Result<i32> { Ok(exe.u32_at(va)? as i32) };

        let mut code_volume = [0i8; CODE_COUNT];
        let raw = exe.bytes_at(CODE_VOLUME_TABLE, CODE_COUNT * 2)?;
        for (v, pair) in code_volume.iter_mut().zip(raw.as_chunks::<2>().0) {
            *v = pair[0] as i8;
        }

        let mut sequences = Vec::with_capacity(SEQUENCE_COUNT);
        for id in 0..SEQUENCE_COUNT {
            let list = exe.u32_at(SEQUENCE_TABLE + 4 * id as u32)?;
            sequences.push(read_steps(&i16_at, list, crate::system::SEQUENCE_STEPS)?);
        }

        let mut call_pairs = Vec::new();
        for i in 0..CALL_PAIR_MAX {
            let va = CALL_PAIR_TABLE + 4 * i as u32;
            let (Ok(code), Ok(delay)) = (i16_at(va), i16_at(va + 2)) else {
                break;
            };
            if !(1..=LAST_BGM_CODE as i16).contains(&code) || delay < 0 {
                break;
            }
            call_pairs.push(Step::new(code, delay));
        }
        if call_pairs.len() < 5 {
            return Err(Error::Table(format!(
                "call-pair table at {CALL_PAIR_TABLE:#x} has only {} entries",
                call_pairs.len()
            )));
        }

        let mut stages = [StageSounds::default(); STAGE_COUNT];
        for (s, st) in stages.iter_mut().enumerate() {
            let o = 2 * s as u32;
            *st = StageSounds {
                motor: i16_at(STAGE_MOTOR_CODE + o)? as u16,
                joint: i16_at(STAGE_JOINT_CODE + o)? as u16,
                horn: i16_at(STAGE_HORN_CODE + o)? as u16,
                motor_base_pitch: i16_at(STAGE_MOTOR_BASE_PITCH + o)?,
            };
        }

        let mut joints = Vec::with_capacity(JOINT_ROWS);
        for r in 0..JOINT_ROWS {
            let base = JOINT_TABLE + 0x20 * r as u32;
            let period = i32_at(base)?;
            if period <= 0 {
                return Err(Error::Table(format!(
                    "rail-joint row {r} has period {period}"
                )));
            }
            joints.push(JointPattern {
                period,
                clicks: [
                    i32_at(base + 4)?,
                    i32_at(base + 8)?,
                    i32_at(base + 12)?,
                    i32_at(base + 16)?,
                ],
                flags: i32_at(base + 20)? as u32,
            });
        }

        Ok(Self {
            code_volume: CodeVolumes(code_volume),
            sequences,
            call_pairs,
            stages,
            joints,
        })
    }
}

/// Reads up to `max` (code, delay) pairs at `va`, stopping at a code of −1 (0x41fdd0).
fn read_steps(i16_at: &impl Fn(u32) -> Result<i16>, va: u32, max: usize) -> Result<Vec<Step>> {
    let mut steps = Vec::new();
    for i in 0..max {
        let at = va + 4 * i as u32;
        let code = i16_at(at)?;
        if code == -1 {
            break;
        }
        steps.push(Step::new(code, i16_at(at + 2)?));
    }
    Ok(steps)
}

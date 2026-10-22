//! Controller vibration: the options page's 振動 (eeprom +0x333, docs/re/input.md §11).
//!
//! The original has one force-feedback effect, a 100 ms DirectInput ramp force created at
//! start-up (0x433d60), and `ff_play_jolt` (0x434940) restarts it with the gain of the
//! option: 切 off, 弱 3333, 並 6666, 強 10000 of 10000. The jolt comes with every rail-joint
//! click of the player's train (0x422c20) and every wheel set crossing a turnout (0x4204d0),
//! and only while a stage runs and the train is driving.
//!
//! The port plays the jolt on a modern pad's two rumble motors (a port design; DirectInput's
//! ramp along X has no direct equivalent there): the strong motor at the level's gain and the
//! weak one at half of it, for the jolt's duration counted in game ticks. [`Rumbler`] is the
//! pure part (jolts in, motor levels out, once per tick); `gamepad::PadSource::set_motors`
//! drives the device.

use anyhow::{Result, bail};
use shuppatsu_formats::exe::Exe;

/// The game's tick (main-loop.md §3: `timeSetEvent(33, …)`).
const TICK_MS: u32 = 33;

/// The weak (high-frequency) motor's share of the jolt; the strong motor takes the whole
/// gain (port design, input.md §11.3).
pub const WEAK_SHARE: f32 = 0.5;

/// Motor levels, 0..=1 each.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Motors {
    /// The low-frequency (left, heavy) motor.
    pub strong: f32,
    /// The high-frequency (right, light) motor.
    pub weak: f32,
}

impl Motors {
    pub const OFF: Self = Self {
        strong: 0.0,
        weak: 0.0,
    };

    pub fn is_off(self) -> bool {
        self.strong <= 0.0 && self.weak <= 0.0
    }
}

/// The jolt as the exe defines it, read from the code's immediates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JoltSpec {
    /// `DIEFFECT.dwDuration` in µs (0x434064).
    pub duration_us: u32,
    /// `DIEFFECT.dwGain` for 弱, 並 and 強 (0x434993, 0x43499d, 0x4349a7), of 10000.
    pub gains: [u32; 3],
}

impl JoltSpec {
    /// The values of the 3000-bandai exe (input.md §11.1), for tests and as documentation.
    pub const ORIGINAL: Self = Self {
        duration_us: 100_000,
        gains: [3333, 6666, 10000],
    };

    /// Reads the duration and the three gains from their `mov dword [esp+d8], imm32`
    /// instructions, checking the opcode bytes first (a different exe fails loudly).
    pub fn read(exe: &Exe<'_>) -> Result<Self> {
        let imm = |va: u32, disp: u8| -> Result<u32> {
            let b = exe.bytes_at(va, 8)?;
            if b[..4] != [0xc7, 0x44, 0x24, disp] {
                bail!(
                    "unexpected instruction at {va:#x}: {:02x?} (not the 3000-bandai exe?)",
                    &b[..4]
                );
            }
            Ok(u32::from_le_bytes([b[4], b[5], b[6], b[7]]))
        };
        Ok(Self {
            // 0x433d60: `mov [esp+0x68], 100000` (dwDuration of the ramp effect).
            duration_us: imm(0x434064, 0x68)?,
            // 0x434940: `mov [esp+0x14], gain` (dwGain) per option value 1..=3.
            gains: [
                imm(0x434993, 0x14)?,
                imm(0x43499d, 0x14)?,
                imm(0x4349a7, 0x14)?,
            ],
        })
    }

    /// [`JoltSpec::read`], or no vibration at all (all gains 0) when the exe has other code
    /// there: vibration is not worth failing a run for.
    pub fn read_or_off(exe: &Exe<'_>) -> Self {
        Self::read(exe).unwrap_or_else(|e| {
            eprintln!("vibration off: {e}");
            Self {
                gains: [0; 3],
                ..Self::ORIGINAL
            }
        })
    }

    /// The jolt's length in game ticks, rounded to the nearest tick (100 ms: 3 ticks).
    pub fn ticks(&self) -> u32 {
        let ms = self.duration_us / 1000;
        ((ms + TICK_MS / 2) / TICK_MS).max(1)
    }

    /// The motors for option value `level` (0 off, 1 弱, 2 並, 3 強).
    pub fn motors(&self, level: u8) -> Motors {
        let Some(&gain) = usize::from(level)
            .checked_sub(1)
            .and_then(|i| self.gains.get(i))
        else {
            return Motors::OFF;
        };
        let g = (gain as f32 / 10_000.0).clamp(0.0, 1.0);
        Motors {
            strong: g,
            weak: g * WEAK_SHARE,
        }
    }
}

/// Jolts to motor levels, one call per game tick.
#[derive(Debug, Clone)]
pub struct Rumbler {
    spec: JoltSpec,
    /// Ticks of the current jolt left (this tick included).
    left: u32,
    /// The motors of the current jolt (the gain is set when the effect starts, 0x434940).
    motors: Motors,
}

impl Rumbler {
    pub fn new(spec: JoltSpec) -> Self {
        Self {
            spec,
            left: 0,
            motors: Motors::OFF,
        }
    }

    /// One tick: `jolts` is how many times the game started the effect this tick (any number
    /// restarts it once, as `Start` does), `level` the option value, `active` whether the
    /// vibration may run (not paused, focused, in a run). Returns the motors for this tick.
    /// Inactive or off stops the jolt at once, and it does not resume.
    pub fn tick(&mut self, jolts: u32, level: u8, active: bool) -> Motors {
        let motors = self.spec.motors(level);
        if !active || motors.is_off() {
            self.stop();
            return Motors::OFF;
        }
        if jolts > 0 {
            self.left = self.spec.ticks();
            self.motors = motors;
        }
        if self.left == 0 {
            return Motors::OFF;
        }
        self.left -= 1;
        self.motors
    }

    /// Ends the current jolt.
    pub fn stop(&mut self) {
        self.left = 0;
        self.motors = Motors::OFF;
    }

    /// Whether a jolt is running.
    #[cfg(test)]
    pub fn busy(&self) -> bool {
        self.left > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rumbler() -> Rumbler {
        Rumbler::new(JoltSpec::ORIGINAL)
    }

    #[test]
    fn levels_follow_the_original_gains() {
        let s = JoltSpec::ORIGINAL;
        assert_eq!(s.motors(0), Motors::OFF);
        let low = s.motors(1);
        assert!((low.strong - 0.3333).abs() < 1e-4 && (low.weak - 0.16665).abs() < 1e-4);
        let mid = s.motors(2);
        assert!((mid.strong - 0.6666).abs() < 1e-4 && (mid.weak - 0.3333).abs() < 1e-4);
        assert_eq!(
            s.motors(3),
            Motors {
                strong: 1.0,
                weak: 0.5
            }
        );
        assert_eq!(s.motors(4), Motors::OFF, "out of range reads as off");
    }

    #[test]
    fn a_jolt_lasts_100_ms_of_game_ticks() {
        assert_eq!(JoltSpec::ORIGINAL.ticks(), 3);
        let mut r = rumbler();
        let on = JoltSpec::ORIGINAL.motors(3);
        let seq: Vec<Motors> = [1, 0, 0, 0, 0]
            .iter()
            .map(|&j| r.tick(j, 3, true))
            .collect();
        assert_eq!(seq, [on, on, on, Motors::OFF, Motors::OFF]);
    }

    #[test]
    fn a_new_jolt_restarts_the_effect() {
        let mut r = rumbler();
        let on = JoltSpec::ORIGINAL.motors(2);
        // Two clicks in one tick start it once; a click mid-jolt restarts the 3 ticks.
        let seq: Vec<Motors> = [2, 0, 1, 0, 0, 0]
            .iter()
            .map(|&j| r.tick(j, 2, true))
            .collect();
        assert_eq!(seq, [on, on, on, on, on, Motors::OFF]);
    }

    #[test]
    fn the_gain_is_latched_at_the_start() {
        let mut r = rumbler();
        let low = JoltSpec::ORIGINAL.motors(1);
        assert_eq!(r.tick(1, 1, true), low);
        assert_eq!(
            r.tick(0, 3, true),
            low,
            "a level change waits for the next jolt"
        );
        assert_eq!(r.tick(1, 3, true), JoltSpec::ORIGINAL.motors(3));
    }

    #[test]
    fn off_pause_and_focus_loss_stop_at_once() {
        let mut r = rumbler();
        assert_eq!(r.tick(1, 0, true), Motors::OFF, "切 never rumbles");
        assert!(!r.busy());
        assert!(!r.tick(1, 2, true).is_off());
        assert_eq!(r.tick(0, 2, false), Motors::OFF, "paused");
        assert!(!r.busy());
        assert_eq!(
            r.tick(0, 2, true),
            Motors::OFF,
            "does not resume after the pause"
        );
        assert_eq!(
            r.tick(1, 2, false),
            Motors::OFF,
            "jolts while inactive are dropped"
        );
        assert!(!r.tick(1, 2, true).is_off());
        assert_eq!(r.tick(0, 0, true), Motors::OFF, "switched off mid-jolt");
        assert!(!r.busy());
    }

    #[test]
    fn spec_from_exe() {
        let Some(dir) = std::env::var_os("SHUPPATSU_DATA_DIR") else {
            return;
        };
        let path =
            std::path::Path::new(&dir).join("install/Program_Executable_Files/BIN/dengo3k.exe");
        let Ok(bytes) = std::fs::read(path) else {
            return;
        };
        let exe = Exe::parse(&bytes).unwrap();
        assert_eq!(JoltSpec::read(&exe).unwrap(), JoltSpec::ORIGINAL);
    }
}

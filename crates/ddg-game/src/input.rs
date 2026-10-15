//! Keyboard controls the way the original decodes them (docs/re/input.md §4): keys are polled
//! once per tick, each held key has a frame counter, and the notch handles move on the press
//! and then on every third tick from the ninth while held.

use anyhow::Result;
use ddg_formats::exe::Exe;
use winit::keyboard::KeyCode;

/// Binding table: 7 modes × 14 bytes (input.md §4, 0x47c980).
const BINDINGS_VA: u32 = 0x47c980;
const BINDING_STRIDE: usize = 14;
pub const MODE_COUNT: usize = 7;
const UNBOUND: u8 = 0xff;

pub const MAX_POWER: u8 = 5;
/// Emergency brake: the highest notch the two-handle keyboard modes reach (input.md §4.1).
pub const EB: u8 = 9;
/// Mode 2's numpad 9 brake code, shown as EB.
const BRAKE_CODE_10: u8 = 10;
/// Repeat stops one short of EB, so holding the brake key never slides into it.
const REPEAT_BRAKE_MAX: u8 = 8;

// DirectInput scan codes used outside the binding table (input.md §6.1).
const DIK_SPACE: u8 = 0x39;
const DIK_RETURN: u8 = 0x1c;
const DIK_ESCAPE: u8 = 0x01;
/// Mode 2: digits 1..5 (0x02..=0x06) hold P1..P5.
const DIK_DIGITS: [u8; 5] = [0x02, 0x03, 0x04, 0x05, 0x06];
/// Mode 2: numpad 0..9.
const DIK_NUMPAD: [u8; 10] = [0x52, 0x4f, 0x50, 0x51, 0x4b, 0x4c, 0x4d, 0x47, 0x48, 0x49];

/// The DirectInput scan code (DIK_*) of a key, for the keys any keyboard mode uses.
pub fn dik(code: KeyCode) -> Option<u8> {
    use KeyCode::*;
    Some(match code {
        Escape => 0x01,
        Digit1 => 0x02,
        Digit2 => 0x03,
        Digit3 => 0x04,
        Digit4 => 0x05,
        Digit5 => 0x06,
        KeyR => 0x13,
        Enter => 0x1c,
        KeyA => 0x1e,
        KeyZ => 0x2c,
        Space => 0x39,
        Numpad7 => 0x47,
        Numpad8 => 0x48,
        Numpad9 => 0x49,
        Numpad4 => 0x4b,
        Numpad5 => 0x4c,
        Numpad6 => 0x4d,
        Numpad1 => 0x4f,
        Numpad2 => 0x50,
        Numpad3 => 0x51,
        Numpad0 => 0x52,
        NumpadEnter => 0x9c,
        ArrowUp => 0xc8,
        ArrowLeft => 0xcb,
        ArrowRight => 0xcd,
        ArrowDown => 0xd0,
        _ => return None,
    })
}

/// The lever bindings of one keyboard mode: power+, power−, brake+, brake− scan codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeverKeys {
    pub power_up: u8,
    pub power_down: u8,
    pub brake_up: u8,
    pub brake_down: u8,
}

/// Lever bindings per mode, read from the exe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bindings {
    pub modes: [LeverKeys; MODE_COUNT],
}

impl Bindings {
    pub fn read(exe: &Exe<'_>) -> Result<Self> {
        let bytes = exe.bytes_at(BINDINGS_VA, BINDING_STRIDE * MODE_COUNT)?;
        Ok(Self {
            modes: std::array::from_fn(|m| {
                let b = &bytes[m * BINDING_STRIDE..];
                LeverKeys {
                    power_up: b[0],
                    power_down: b[1],
                    brake_up: b[2],
                    brake_down: b[3],
                }
            }),
        })
    }
}

/// How the keyboard drives the levers (input.md §4 modes 0–3).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyboardMode {
    /// Modes 0 and 1: separate power and brake handles.
    TwoHandle,
    /// Mode 2: digits hold a power notch, the numpad sets the brake.
    Direct,
    /// Mode 3: one handle, N and EB are detents.
    OneHandle,
}

impl KeyboardMode {
    pub fn from_index(mode: usize) -> Option<Self> {
        Some(match mode {
            0 | 1 => Self::TwoHandle,
            2 => Self::Direct,
            3 => Self::OneHandle,
            _ => return None,
        })
    }
}

/// Per-tick keyboard state: hold counters per scan code and the two notch handles.
#[derive(Clone, Debug)]
pub struct Keyboard {
    mode: KeyboardMode,
    keys: LeverKeys,
    /// Frames each key has been held (0x829b00), saturating.
    held: [u16; 256],
    pub power: u8,
    pub brake: u8,
}

/// Buttons of one tick: `held` for the whole tick, `pressed` on the first tick only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ButtonState {
    pub horn_held: bool,
    pub horn_pressed: bool,
    pub confirm_pressed: bool,
    pub cancel_pressed: bool,
}

impl Keyboard {
    pub fn new(bindings: &Bindings, mode: usize) -> Option<Self> {
        Some(Self {
            mode: KeyboardMode::from_index(mode)?,
            keys: bindings.modes[mode],
            held: [0; 256],
            power: 0,
            brake: EB,
        })
    }

    /// Run or station start in a digital mode (0x4332c0): P0, EB.
    pub fn reset_levers(&mut self) {
        self.power = 0;
        self.brake = EB;
    }

    /// One poll: `down(dik)` says whether a key is down now. Updates the hold counters, the
    /// handles and returns the button edges.
    pub fn poll(&mut self, down: impl Fn(u8) -> bool) -> ButtonState {
        for (code, count) in self.held.iter_mut().enumerate() {
            *count = if down(code as u8) {
                count.saturating_add(1)
            } else {
                0
            };
        }
        match self.mode {
            KeyboardMode::TwoHandle => self.two_handle(),
            KeyboardMode::Direct => self.direct(),
            KeyboardMode::OneHandle => self.one_handle(),
        }
        ButtonState {
            horn_held: self.count(DIK_SPACE) > 0,
            horn_pressed: self.count(DIK_SPACE) == 1,
            confirm_pressed: self.count(DIK_RETURN) == 1,
            cancel_pressed: self.count(DIK_ESCAPE) == 1,
        }
    }

    fn count(&self, code: u8) -> u16 {
        if code == UNBOUND {
            0
        } else {
            self.held[usize::from(code)]
        }
    }

    /// A step on the press frame, then on frames 9, 12, 15, … (input.md §4.2).
    fn step(&self, code: u8) -> Step {
        match self.count(code) {
            1 => Step::Press,
            n if n > 8 && n % 3 == 0 => Step::Repeat,
            _ => Step::None,
        }
    }

    fn two_handle(&mut self) {
        let k = self.keys;
        if self.step(k.power_up) != Step::None {
            self.power = (self.power + 1).min(MAX_POWER);
        }
        if self.step(k.power_down) != Step::None {
            self.power = self.power.saturating_sub(1);
        }
        match self.step(k.brake_up) {
            Step::Press => self.brake = (self.brake + 1).min(EB),
            Step::Repeat if self.brake < REPEAT_BRAKE_MAX => self.brake += 1,
            _ => {}
        }
        if self.step(k.brake_down) != Step::None {
            self.brake = self.brake.saturating_sub(1);
        }
    }

    fn direct(&mut self) {
        // The lowest held digit wins; none held is N.
        self.power = DIK_DIGITS
            .iter()
            .position(|&d| self.count(d) > 0)
            .map_or(0, |i| i as u8 + 1);
        for (notch, &code) in DIK_NUMPAD.iter().enumerate() {
            if self.count(code) == 1 {
                self.brake = if notch == 9 {
                    BRAKE_CODE_10
                } else {
                    notch as u8
                };
            }
        }
    }

    fn one_handle(&mut self) {
        let k = self.keys;
        match self.step(k.power_up) {
            Step::None => {}
            step => {
                if self.brake > 0 {
                    self.brake -= 1;
                    self.power = 0;
                } else if self.power > 0 || step == Step::Press {
                    // Repeat doesn't leave neutral.
                    self.power = (self.power + 1).min(MAX_POWER);
                }
            }
        }
        match self.step(k.power_down) {
            Step::None => {}
            step => {
                if self.power > 0 {
                    self.power -= 1;
                    self.brake = 0;
                } else if step == Step::Press {
                    self.brake = (self.brake + 1).min(EB);
                } else if self.brake > 0 && self.brake < REPEAT_BRAKE_MAX {
                    self.brake += 1;
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    None,
    Press,
    Repeat,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bindings() -> Bindings {
        // Modes 0 and 3 rows of the exe table (input.md §4).
        let m0 = LeverKeys {
            power_up: 0xd0,
            power_down: 0xc8,
            brake_up: 0xcd,
            brake_down: 0xcb,
        };
        let m3 = LeverKeys {
            brake_up: UNBOUND,
            brake_down: UNBOUND,
            ..m0
        };
        Bindings {
            modes: [m0, m0, m0, m3, m0, m0, m0],
        }
    }

    fn hold(kb: &mut Keyboard, code: u8, ticks: usize) {
        for _ in 0..ticks {
            kb.poll(|c| c == code);
        }
        kb.poll(|_| false);
    }

    #[test]
    fn repeat_on_press_then_every_third_tick_from_nine() {
        let mut kb = Keyboard::new(&bindings(), 0).unwrap();
        kb.brake = 0;
        // Held for 12 ticks: steps on ticks 1, 9 and 12.
        hold(&mut kb, 0xd0, 12);
        assert_eq!(kb.power, 3);
        hold(&mut kb, 0xd0, 30);
        assert_eq!(kb.power, MAX_POWER);
    }

    #[test]
    fn brake_repeat_stops_short_of_eb() {
        let mut kb = Keyboard::new(&bindings(), 0).unwrap();
        kb.brake = 0;
        hold(&mut kb, 0xcd, 100);
        assert_eq!(kb.brake, REPEAT_BRAKE_MAX);
        hold(&mut kb, 0xcd, 1);
        assert_eq!(kb.brake, EB);
        hold(&mut kb, 0xcb, 1);
        assert_eq!(kb.brake, 8);
    }

    #[test]
    fn one_handle_detents() {
        let mut kb = Keyboard::new(&bindings(), 3).unwrap();
        // From EB, holding toward power releases the brake but stops at N.
        hold(&mut kb, 0xd0, 100);
        assert_eq!((kb.power, kb.brake), (0, 0));
        hold(&mut kb, 0xd0, 1);
        assert_eq!((kb.power, kb.brake), (1, 0));
        // Toward brake: back to N, then a fresh press for B1; repeat stops at B8.
        hold(&mut kb, 0xc8, 100);
        assert_eq!((kb.power, kb.brake), (0, 0));
        hold(&mut kb, 0xc8, 1);
        hold(&mut kb, 0xc8, 100);
        assert_eq!(kb.brake, REPEAT_BRAKE_MAX);
    }

    #[test]
    fn buttons_have_press_edges() {
        let mut kb = Keyboard::new(&bindings(), 0).unwrap();
        let first = kb.poll(|c| c == DIK_SPACE);
        let second = kb.poll(|c| c == DIK_SPACE);
        assert!(first.horn_pressed && first.horn_held);
        assert!(!second.horn_pressed && second.horn_held);
    }

    #[test]
    fn direct_mode() {
        let mut kb = Keyboard::new(&bindings(), 2).unwrap();
        kb.poll(|c| c == 0x04 || c == 0x06);
        assert_eq!(kb.power, 3);
        kb.poll(|c| c == 0x49);
        assert_eq!(kb.brake, BRAKE_CODE_10);
    }

    #[test]
    fn bindings_from_exe() {
        let Some(dir) = std::env::var_os("DDG_DATA_DIR") else {
            return;
        };
        let path =
            std::path::Path::new(&dir).join("install/Program_Executable_Files/BIN/dengo3k.exe");
        let bytes = std::fs::read(path).unwrap();
        let b = Bindings::read(&Exe::parse(&bytes).unwrap()).unwrap();
        assert_eq!(b.modes[0], bindings().modes[0]);
        assert_eq!(b.modes[1].power_up, 0x2c);
        assert_eq!(b.modes[3], bindings().modes[3]);
    }
}

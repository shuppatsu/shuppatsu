//! Keyboard controls the way the original decodes them (docs/re/input.md §4): keys are polled
//! once per tick, each held key has a frame counter, and the notch handles move on the press
//! and then on every third tick from the ninth while held.

use anyhow::Result;
use shuppatsu_formats::exe::Exe;
use winit::keyboard::KeyCode;

/// Binding table: 7 modes × 14 bytes (input.md §4, 0x47c980).
pub(crate) const BINDINGS_VA: u32 = 0x47c980;
pub(crate) const BINDING_STRIDE: usize = 14;
pub const MODE_COUNT: usize = 7;
pub(crate) const UNBOUND: u8 = 0xff;
/// Bytes [4..14] of a binding-table row: menu up/down/left/right, then the button sources.
pub(crate) const BUTTON_SLOTS: usize = 10;

pub const MAX_POWER: u8 = 5;
/// Emergency brake: the highest notch the two-handle keyboard modes reach (input.md §4.1).
pub const EB: u8 = 9;
/// Mode 2's numpad 9 brake code, shown as EB.
const BRAKE_CODE_10: u8 = 10;
/// Repeat stops one short of EB, so holding the brake key never slides into it.
const REPEAT_BRAKE_MAX: u8 = 8;

// Scan codes of mode 2, outside the binding table (input.md §6.1).
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
        ShiftLeft => 0x2a,
        KeyZ => 0x2c,
        ShiftRight => 0x36,
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
    /// Button slots [4..14] the keyboard uses in every mode: the keyboard routine reads them at
    /// absolute addresses, so always from the mode-0 row (input.md §4, 0x433b25/0x433bf4).
    pub keyboard_buttons: [u8; BUTTON_SLOTS],
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
            keyboard_buttons: std::array::from_fn(|i| bytes[4 + i]),
        })
    }
}

/// Bits of the button word (input.md §6). The original stores them active-low; here a set bit
/// means true.
pub mod bits {
    pub const UP: u32 = 0x01;
    pub const DOWN: u32 = 0x02;
    pub const LEFT: u32 = 0x04;
    pub const RIGHT: u32 = 0x08;
    /// Menu cursor pulses: on the press, then on the repeat frames (§4.2).
    pub const UP_PULSE: u32 = 0x10;
    pub const DOWN_PULSE: u32 = 0x20;
    pub const RIGHT_PULSE: u32 = 0x40;
    pub const LEFT_PULSE: u32 = 0x80;
    pub const HORN: u32 = 0x100;
    pub const MENU_CONFIRM: u32 = 0x1000;
    pub const CANCEL: u32 = 0x2000;
    /// Keyboard R, press frame only; nothing reads it.
    pub const KEY_R: u32 = 0x4000;
    /// The driving "A" button: HUD panel toggle and stop/door prompts.
    pub const A: u32 = 0x100000;
    pub const PAUSE: u32 = 0x800000;
}

/// One device's button word for a tick: held now, and the edges against the previous tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ButtonWord {
    pub held: u32,
    pub pressed: u32,
    pub released: u32,
}

impl ButtonWord {
    pub fn new(prev_held: u32, held: u32) -> Self {
        Self {
            held,
            pressed: held & !prev_held,
            released: prev_held & !held,
        }
    }
}

/// True on the press frame and then on frames 9, 12, 15, … of a hold (input.md §4.2).
pub(crate) fn repeat_pulse(count: u16) -> bool {
    count == 1 || (count > 8 && count.is_multiple_of(3))
}

/// The held word from binding-table slots [4..14] and a per-code hold counter (0x4335d0 and
/// 0x4341c0 tails). `count` must return 0 for unbound codes. Slot 9 (R, press only) only
/// exists on the keyboard.
pub(crate) fn held_word(
    slots: &[u8; BUTTON_SLOTS],
    count: impl Fn(u8) -> u16,
    keyboard: bool,
) -> u32 {
    let n = |slot: usize| {
        let code = slots[slot];
        if code == UNBOUND { 0 } else { count(code) }
    };
    let mut w = 0;
    for (slot, held, pulse) in [
        (0, bits::UP, bits::UP_PULSE),
        (1, bits::DOWN, bits::DOWN_PULSE),
        (2, bits::LEFT, bits::LEFT_PULSE),
        (3, bits::RIGHT, bits::RIGHT_PULSE),
    ] {
        if n(slot) > 0 {
            w |= held;
        }
        if repeat_pulse(n(slot)) {
            w |= pulse;
        }
    }
    for (slot, bit) in [
        (4, bits::A),
        (5, bits::HORN),
        (6, bits::MENU_CONFIRM),
        (7, bits::CANCEL),
        (8, bits::PAUSE),
    ] {
        if n(slot) > 0 {
            w |= bit;
        }
    }
    if keyboard && n(9) == 1 {
        w |= bits::KEY_R;
    }
    w
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

/// Lever directions another stepped device holds this tick (setting `hybrid_input`: the
/// modern pad's d-pad, stick and shoulder buttons). Each is merged with the keyboard's own key
/// for that direction into one virtual key, held while either holds it, so the repeat rules
/// run once per direction however many devices press it (input.md §10.6).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LeverHolds {
    pub power_up: bool,
    pub power_down: bool,
    pub brake_up: bool,
    pub brake_down: bool,
}

impl LeverHolds {
    pub const NONE: Self = Self {
        power_up: false,
        power_down: false,
        brake_up: false,
        brake_down: false,
    };

    fn array(self) -> [bool; 4] {
        [
            self.power_up,
            self.power_down,
            self.brake_up,
            self.brake_down,
        ]
    }
}

/// Per-tick keyboard state: hold counters per scan code and the two notch handles.
#[derive(Clone, Debug)]
pub struct Keyboard {
    mode: KeyboardMode,
    keys: LeverKeys,
    /// Frames each key has been held (0x829b00), saturating.
    held: [u16; 256],
    /// Frames each lever direction (power+, power−, brake+, brake−) has been held: the lever
    /// key's own counter, or with [`LeverHolds`] the merged virtual key's.
    lever_held: [u16; 4],
    /// Mode 2 with [`LeverHolds`]: another device stepped the power notch since the digits
    /// last changed, so the digits don't overwrite it (they are an absolute device).
    power_stepped: bool,
    /// Mode 2: the digits held on the previous poll (bit n = digit n + 1).
    prev_digits: u8,
    buttons: [u8; BUTTON_SLOTS],
    /// Held word of the previous poll, for the edges.
    prev_word: u32,
    pub power: u8,
    pub brake: u8,
}

/// Buttons of one tick: `held` for the whole tick, `pressed` on the first tick only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ButtonState {
    pub horn_held: bool,
    pub horn_pressed: bool,
    /// The driving A button (bit 0x100000): HUD panel, ATS confirm, continue.
    pub confirm_pressed: bool,
    pub cancel_pressed: bool,
    /// Menu confirm (bit 0x1000). Same key or button as `confirm_pressed` in every mode, except
    /// that keyboard Enter only raises this one while a joystick mode is active.
    pub menu_confirm_pressed: bool,
    pub pause_pressed: bool,
    /// The whole word, for menus (cursor pulses) and replays.
    pub word: ButtonWord,
}

impl ButtonState {
    pub fn from_word(word: ButtonWord) -> Self {
        Self {
            horn_held: word.held & bits::HORN != 0,
            horn_pressed: word.pressed & bits::HORN != 0,
            confirm_pressed: word.pressed & bits::A != 0,
            cancel_pressed: word.pressed & bits::CANCEL != 0,
            menu_confirm_pressed: word.pressed & bits::MENU_CONFIRM != 0,
            pause_pressed: word.pressed & bits::PAUSE != 0,
            word,
        }
    }
}

impl Keyboard {
    pub fn new(bindings: &Bindings, mode: usize) -> Option<Self> {
        Some(Self {
            mode: KeyboardMode::from_index(mode)?,
            keys: bindings.modes[mode],
            held: [0; 256],
            lever_held: [0; 4],
            power_stepped: false,
            prev_digits: 0,
            buttons: bindings.keyboard_buttons,
            prev_word: 0,
            power: 0,
            brake: EB,
        })
    }

    /// Whether either Shift key is held (LShift 0x2a, RShift 0x36; 0x4335d0 → 0x829d08).
    pub fn shift_held(&self) -> bool {
        self.held[0x2a] > 0 || self.held[0x36] > 0
    }

    /// Whether any key went down on the last poll (the hint device follows it).
    pub fn any_pressed(&self) -> bool {
        self.held.contains(&1)
    }

    /// Run or station start in a digital mode (0x4332c0): P0, EB.
    pub fn reset_levers(&mut self) {
        self.power = 0;
        self.brake = EB;
    }

    /// One poll: `down(dik)` says whether a key is down now. Updates the hold counters, the
    /// handles and returns the button edges.
    pub fn poll(&mut self, down: impl Fn(u8) -> bool) -> ButtonState {
        self.poll_with(down, LeverHolds::NONE)
    }

    /// [`Keyboard::poll`] with lever directions another stepped device holds (setting
    /// `hybrid_input`). With [`LeverHolds::NONE`] this is exactly the original's poll.
    ///
    /// Modes 0/1 merge each direction with its key. Mode 3 (one handle) folds the brake
    /// directions onto the handle: brake+ toward the brake side (power−), brake− toward power.
    /// Mode 2's digits and numpad are absolute: the other device steps the handles with the
    /// two-handle rules, and the digits take the power notch back when they change.
    pub fn poll_with(&mut self, down: impl Fn(u8) -> bool, extra: LeverHolds) -> ButtonState {
        for (code, count) in self.held.iter_mut().enumerate() {
            *count = if down(code as u8) {
                count.saturating_add(1)
            } else {
                0
            };
        }
        let extra = match self.mode {
            KeyboardMode::OneHandle => LeverHolds {
                power_up: extra.power_up || extra.brake_down,
                power_down: extra.power_down || extra.brake_up,
                brake_up: false,
                brake_down: false,
            },
            _ => extra,
        };
        let k = self.keys;
        let keys = [k.power_up, k.power_down, k.brake_up, k.brake_down];
        for ((count, key), more) in self.lever_held.iter_mut().zip(keys).zip(extra.array()) {
            // Mode 2's row keys are not levers: only the other device steps there.
            let own = self.mode != KeyboardMode::Direct && key != UNBOUND && down(key);
            *count = if own || more {
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
        let held = held_word(&self.buttons, |c| self.count(c), true);
        let word = ButtonWord::new(self.prev_word, held);
        self.prev_word = held;
        ButtonState::from_word(word)
    }

    fn count(&self, code: u8) -> u16 {
        if code == UNBOUND {
            0
        } else {
            self.held[usize::from(code)]
        }
    }

    /// A step of lever direction `dir` (power+, power−, brake+, brake−) on the press frame,
    /// then on frames 9, 12, 15, … (input.md §4.2).
    fn step(&self, dir: usize) -> Step {
        match self.lever_held[dir] {
            1 => Step::Press,
            n if n > 8 && n % 3 == 0 => Step::Repeat,
            _ => Step::None,
        }
    }

    fn two_handle(&mut self) {
        if self.step(POWER_UP) != Step::None {
            self.power = (self.power + 1).min(MAX_POWER);
        }
        if self.step(POWER_DOWN) != Step::None {
            self.power = self.power.saturating_sub(1);
        }
        match self.step(BRAKE_UP) {
            Step::Press => self.brake = (self.brake + 1).min(EB),
            Step::Repeat if self.brake < REPEAT_BRAKE_MAX => self.brake += 1,
            _ => {}
        }
        if self.step(BRAKE_DOWN) != Step::None {
            self.brake = self.brake.saturating_sub(1);
        }
    }

    fn direct(&mut self) {
        // Another device's steps (hybrid only; without them the counters stay 0).
        let before = self.power;
        self.two_handle();
        if self.power != before {
            self.power_stepped = true;
        }
        let digits = DIK_DIGITS
            .iter()
            .enumerate()
            .filter(|&(_, &d)| self.count(d) > 0)
            .fold(0u8, |m, (i, _)| m | 1 << i);
        if digits != self.prev_digits {
            self.power_stepped = false;
        }
        self.prev_digits = digits;
        if !self.power_stepped {
            // The lowest held digit wins; none held is N.
            self.power = if digits == 0 {
                0
            } else {
                digits.trailing_zeros() as u8 + 1
            };
        }
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
        match self.step(POWER_UP) {
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
        match self.step(POWER_DOWN) {
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

/// Lever directions, indices of [`Keyboard::lever_held`].
const POWER_UP: usize = 0;
const POWER_DOWN: usize = 1;
const BRAKE_UP: usize = 2;
const BRAKE_DOWN: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    None,
    Press,
    Repeat,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const DIK_SPACE: u8 = 0x39;
    pub(crate) const DIK_RETURN: u8 = 0x1c;
    const DIK_ESCAPE: u8 = 0x01;
    /// Mode-0 row bytes [4..14]: arrows, Enter, Space, Enter, Esc, Esc, R.
    pub(crate) const ROW0_BUTTONS: [u8; BUTTON_SLOTS] =
        [0xc8, 0xd0, 0xcb, 0xcd, 0x1c, 0x39, 0x1c, 0x01, 0x01, 0x13];

    pub(crate) fn bindings() -> Bindings {
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
            keyboard_buttons: ROW0_BUTTONS,
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
    fn keyboard_word() {
        let mut kb = Keyboard::new(&bindings(), 0).unwrap();
        // Esc is both cancel and pause; Enter is both A and menu confirm.
        let b = kb.poll(|c| c == DIK_ESCAPE || c == DIK_RETURN);
        assert!(b.pause_pressed && b.cancel_pressed);
        assert!(b.confirm_pressed && b.menu_confirm_pressed);
        let b = kb.poll(|_| false);
        assert_eq!(
            b.word.released,
            bits::PAUSE | bits::CANCEL | bits::A | bits::MENU_CONFIRM
        );
        // R is a press-only bit.
        assert_eq!(kb.poll(|c| c == 0x13).word.held, bits::KEY_R);
        assert_eq!(kb.poll(|c| c == 0x13).word.held, 0);
        // Held ↑ pulses the menu cursor on ticks 1, 9, 12.
        let pulses: Vec<usize> = (1..=12)
            .filter(|_| kb.poll(|c| c == 0xc8).word.pressed & bits::UP_PULSE != 0)
            .collect();
        assert_eq!(pulses, [1, 9, 12]);
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
        let Some(dir) = std::env::var_os("SHUPPATSU_DATA_DIR") else {
            return;
        };
        let path =
            std::path::Path::new(&dir).join("install/Program_Executable_Files/BIN/dengo3k.exe");
        let bytes = std::fs::read(path).unwrap();
        let b = Bindings::read(&Exe::parse(&bytes).unwrap()).unwrap();
        assert_eq!(b.modes[0], bindings().modes[0]);
        assert_eq!(b.modes[1].power_up, 0x2c);
        assert_eq!(b.modes[3], bindings().modes[3]);
        assert_eq!(b.keyboard_buttons, ROW0_BUTTONS);
    }
}

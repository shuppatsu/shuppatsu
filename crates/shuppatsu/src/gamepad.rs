//! Joystick control modes 4–6 the way the original decodes them (docs/re/input.md §4.3–§6),
//! plus the mapping of modern controllers onto them.
//!
//! The original reads one DirectInput joystick as a `DIJOYSTATE` with X/Y scaled to ±1000.
//! [`JoyState`] is that state; [`Joystick`] turns it into notches and the button word once
//! per tick; [`PadSource`] fills a [`JoyState`] from a gilrs gamepad. [`Controls`] combines
//! the keyboard and the joystick into the one poll the play loop makes per tick.

use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use shuppatsu_formats::exe::Exe;

use crate::hybrid::{
    HANDLE_DEADBAND, HandleTick, HintDevice, HintTracker, LeverArbiter, MenuRepeat, ModernPad,
    pad_axes, pad_buttons,
};
use crate::input::{
    BINDING_STRIDE, BINDINGS_VA, BUTTON_SLOTS, Bindings, ButtonState, ButtonWord, EB, Keyboard,
    LeverHolds, MAX_POWER, UNBOUND, bits, held_word, repeat_pulse,
};

/// Axis range set with `DIPROP_RANGE` on X and Y (0x433d60): −1000..=1000.
pub const AXIS_MAX: i32 = 1000;
/// Mode 4 axis directions become buttons 16..=19 (input.md §4.3).
const BTN_Y_NEG: u8 = 16;
const BTN_Y_POS: u8 = 17;
const BTN_X_NEG: u8 = 18;
const BTN_X_POS: u8 = 19;
/// Repeat stops one short of EB (input.md §4.2).
const REPEAT_BRAKE_MAX: u8 = 8;
/// Number of calibrated positions per handle (input.md §4.5).
pub const MASCON_POSITIONS: usize = 6;
pub const BRAKE_POSITIONS: usize = 11;
/// Default calibration in .data: 6 × i32 mascon Y, then 11 × i32 brake X (0x48c434).
const CALIBRATION_VA: u32 = 0x48c434;
/// `joystick.dat`: the same 68 bytes, no header (0x434a00).
pub const JOYSTICK_DAT_LEN: usize = 4 * (MASCON_POSITIONS + BRAKE_POSITIONS);

/// One joystick sample in DirectInput terms: axes in ±[`AXIS_MAX`], +X right, +Y toward the
/// player; bit n of `buttons` is `rgbButtons[n]`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JoyState {
    pub x: i32,
    pub y: i32,
    pub buttons: u32,
    /// Notches decoded by the device itself (button-combination controllers). When set, mode 6
    /// takes them instead of the calibrated axes; `None` in a handle holds it (port addition).
    pub notches: Option<DirectNotches>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DirectNotches {
    pub power: Option<u8>,
    pub brake: Option<u8>,
}

/// The joystick control modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoyMode {
    /// Mode 4: the stick is a d-pad; the levers step like the keyboard.
    Digital,
    /// Mode 5: one Y axis, fixed thresholds.
    OneHandle,
    /// Mode 6: Y mascon and X brake, nearest calibrated position.
    TwoHandle,
}

impl JoyMode {
    pub fn from_index(mode: usize) -> Option<Self> {
        Some(match mode {
            4 => Self::Digital,
            5 => Self::OneHandle,
            6 => Self::TwoHandle,
            _ => return None,
        })
    }

    pub fn index(self) -> usize {
        match self {
            Self::Digital => 4,
            Self::OneHandle => 5,
            Self::TwoHandle => 6,
        }
    }

    /// Analog-lever modes keep the physical handle at station starts (0x433270).
    pub fn is_analog(self) -> bool {
        self != Self::Digital
    }
}

/// The code constants of `input_poll_joystick` (0x4341c0), read from the instructions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thresholds {
    /// Mode 4: an axis at or below `.0` presses the negative button, at or above `.1` the
    /// positive one. Index 0 is X, 1 is Y.
    pub digital: [(i32, i32); 2],
    /// Mode 5, Y < 0: the first `(limit, brake)` with `y < limit` wins; none is B0.
    pub one_handle_brake: [(i32, u8); 10],
    /// Mode 5, Y ≥ 0: the first `(limit, power)` with `y > limit` wins; none is N.
    pub one_handle_power: [(i32, u8); 5],
    /// Mode 6: the mascon is decoded only while `y <= mascon_gate`.
    pub mascon_gate: i32,
    /// Mode 6: the brake is decoded only while `x <= cal.brake[10] + brake_margin`.
    pub brake_margin: i32,
    /// Mode 6: the nearest position must be EB for more than this many polls.
    pub eb_polls: u32,
}

/// An instruction whose immediate operand is one of the thresholds.
struct Imm {
    va: u32,
    opcode: &'static [u8],
    /// imm32 (true) or sign-extended imm8.
    wide: bool,
}

const fn imm32(va: u32, opcode: &'static [u8]) -> Imm {
    Imm {
        va,
        opcode,
        wide: true,
    }
}

const fn imm8(va: u32, opcode: &'static [u8]) -> Imm {
    Imm {
        va,
        opcode,
        wide: false,
    }
}

const CMP_EAX: &[u8] = &[0x3d];
const CMP_EAX_8: &[u8] = &[0x83, 0xf8];

impl Imm {
    fn read(&self, exe: &Exe<'_>) -> Result<i32> {
        let len = self.opcode.len() + if self.wide { 4 } else { 1 };
        let b = exe.bytes_at(self.va, len)?;
        let (op, imm) = b.split_at(self.opcode.len());
        if op != self.opcode {
            bail!(
                "unexpected instruction at {:#x}: {op:02x?} (not the 3000-bandai exe?)",
                self.va
            );
        }
        Ok(if self.wide {
            i32::from_le_bytes([imm[0], imm[1], imm[2], imm[3]])
        } else {
            i32::from(imm[0] as i8)
        })
    }
}

impl Thresholds {
    pub fn read(exe: &Exe<'_>) -> Result<Self> {
        let r = |i: Imm| i.read(exe);
        // Mode 4: `cmp eax, -500; jg` then `cmp eax, 500; jl` for X, then the same for Y.
        let digital = [
            (r(imm32(0x43425f, CMP_EAX))?, r(imm32(0x43426f, CMP_EAX))?),
            (r(imm32(0x434282, CMP_EAX))?, r(imm32(0x434292, CMP_EAX))?),
        ];
        // Mode 5 brake side: a chain of `cmp eax, limit; jl/jge` from B10 down to B1.
        let brake_sites = [
            imm32(0x434442, CMP_EAX),
            imm32(0x43445e, CMP_EAX),
            imm32(0x434469, CMP_EAX),
            imm32(0x434485, CMP_EAX),
            imm32(0x4344a1, CMP_EAX),
            imm32(0x4344bd, CMP_EAX),
            imm32(0x4344d9, CMP_EAX),
            imm32(0x4344f1, CMP_EAX),
            imm32(0x43450d, CMP_EAX),
            imm8(0x434527, CMP_EAX_8),
        ];
        let mut one_handle_brake = [(0, 0); 10];
        for (i, site) in brake_sites.into_iter().enumerate() {
            one_handle_brake[i] = (r(site)?, 10 - i as u8);
        }
        // Mode 5 power side: `cmp eax, limit; jle` from P5 down to P1.
        let power_sites = [
            imm32(0x43453e, CMP_EAX),
            imm32(0x43455a, CMP_EAX),
            imm32(0x434572, CMP_EAX),
            imm32(0x43458e, CMP_EAX),
            imm8(0x4345a8, CMP_EAX_8),
        ];
        let mut one_handle_power = [(0, 0); 5];
        for (i, site) in power_sites.into_iter().enumerate() {
            one_handle_power[i] = (r(site)?, MAX_POWER - i as u8);
        }
        let eb_polls = r(imm8(0x4343d6, CMP_EAX_8))?;
        Ok(Self {
            digital,
            one_handle_brake,
            one_handle_power,
            // `cmp ebx, 950; jle`
            mascon_gate: r(imm32(0x43431c, &[0x81, 0xfb]))?,
            // `add eax, 50` on cal.brake[10]
            brake_margin: r(imm8(0x43436e, &[0x83, 0xc0]))?,
            eb_polls: u32::try_from(eb_polls).context("negative EB debounce")?,
        })
    }
}

/// Calibrated lever positions in raw axis units (input.md §4.5, §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Calibration {
    /// Y at P0..P5.
    pub mascon: [i32; MASCON_POSITIONS],
    /// X at B0..B8, EB, and code 10 (written as a copy of EB).
    pub brake: [i32; BRAKE_POSITIONS],
}

impl Calibration {
    /// The exe's built-in calibration, used when there is no `joystick.dat`.
    pub fn read_default(exe: &Exe<'_>) -> Result<Self> {
        Self::from_bytes(exe.bytes_at(CALIBRATION_VA, JOYSTICK_DAT_LEN)?)
            .context("default calibration")
    }

    fn from_bytes(b: &[u8]) -> Option<Self> {
        let mut cal = Self {
            mascon: [0; MASCON_POSITIONS],
            brake: [0; BRAKE_POSITIONS],
        };
        cal.overlay(b);
        (b.len() >= JOYSTICK_DAT_LEN).then_some(cal)
    }

    /// Reads `joystick.dat` over `self`. Like the original's two `fread`s into the live table,
    /// a short file only replaces the values it fully contains.
    pub fn overlay(&mut self, file: &[u8]) {
        let words = file
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| i32::from_le_bytes(*c));
        for (slot, v) in self
            .mascon
            .iter_mut()
            .chain(self.brake.iter_mut())
            .zip(words)
        {
            *slot = v;
        }
    }

    /// The 68-byte `joystick.dat` image.
    pub fn to_bytes(self) -> Vec<u8> {
        self.mascon
            .iter()
            .chain(&self.brake)
            .flat_map(|v| v.to_le_bytes())
            .collect()
    }
}

/// Index of the value nearest `v`; a tie keeps the lower index (strict `<`, 0x434346).
fn nearest(values: &[i32], v: i32) -> usize {
    let mut best = 0;
    let mut best_d = (values[0] - v).abs();
    for (i, &c) in values.iter().enumerate().skip(1) {
        let d = (c - v).abs();
        if d < best_d {
            best = i;
            best_d = d;
        }
    }
    best
}

/// Everything the joystick modes read from the exe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JoyTables {
    /// Binding-table rows of modes 4, 5 and 6 (0x47c9b8..).
    pub rows: [[u8; BINDING_STRIDE]; 3],
    pub thresholds: Thresholds,
    pub default_calibration: Calibration,
}

impl JoyTables {
    pub fn read(exe: &Exe<'_>) -> Result<Self> {
        let first = BINDINGS_VA + 4 * BINDING_STRIDE as u32;
        let bytes = exe.bytes_at(first, 3 * BINDING_STRIDE)?;
        Ok(Self {
            rows: std::array::from_fn(|m| std::array::from_fn(|i| bytes[m * BINDING_STRIDE + i])),
            thresholds: Thresholds::read(exe)?,
            default_calibration: Calibration::read_default(exe)?,
        })
    }

    pub fn row(&self, mode: JoyMode) -> &[u8; BINDING_STRIDE] {
        &self.rows[mode.index() - 4]
    }
}

/// Per-tick joystick decoder for modes 4–6.
#[derive(Clone, Debug)]
pub struct Joystick {
    mode: JoyMode,
    row: [u8; BINDING_STRIDE],
    th: Thresholds,
    cal: Calibration,
    /// Polls each of the 32 buttons has been held (0x829900), saturating.
    held: [u16; 32],
    prev_word: u32,
    /// Mode 6 EB debounce counter (0x829d0c).
    eb_count: u32,
    pub power: u8,
    pub brake: u8,
    /// Setting `keyboard_with_controller`: the keyboard's Space (horn) and Enter (A button) pass
    /// too; off, only Esc, Enter as menu confirm and R, as the original.
    pub keyboard_buttons: bool,
}

impl Joystick {
    pub fn new(tables: &JoyTables, mode: JoyMode, cal: Calibration) -> Self {
        Self {
            mode,
            row: *tables.row(mode),
            th: tables.thresholds.clone(),
            cal,
            held: [0; 32],
            prev_word: 0,
            eb_count: 0,
            power: 0,
            brake: EB,
            keyboard_buttons: true,
        }
    }

    pub fn mode(&self) -> JoyMode {
        self.mode
    }

    /// The binding-table row in use; [`PadSource`] places its buttons by it.
    pub fn row(&self) -> &[u8; BINDING_STRIDE] {
        &self.row
    }

    /// Run or station start (0x4332c0): P0/EB, except in the analog-lever modes, where the
    /// physical handle stays authoritative.
    pub fn reset_levers(&mut self) {
        if !self.mode.is_analog() {
            self.power = 0;
            self.brake = EB;
        }
    }

    /// One poll (0x4341c0). `keyboard_held` is this tick's keyboard word. The original lets
    /// only Esc (pause + cancel), Enter (menu confirm) and R through while the joystick is
    /// active (input.md §6); the port also lets the keyboard's driving buttons through, Space
    /// (horn) and Enter (the A button: ATS confirm, HUD panel), OR-ed with the pad's (decided
    /// port behaviour, input.md §10.3). The levers stay the joystick's.
    pub fn poll(&mut self, state: &JoyState, keyboard_held: u32) -> ButtonState {
        let mut buttons = state.buttons;
        if self.mode == JoyMode::Digital {
            let [(xn, xp), (yn, yp)] = self.th.digital;
            for (v, neg, pos, bn, bp) in [
                (state.x, xn, xp, BTN_X_NEG, BTN_X_POS),
                (state.y, yn, yp, BTN_Y_NEG, BTN_Y_POS),
            ] {
                if v <= neg {
                    buttons |= 1 << bn;
                } else if v >= pos {
                    buttons |= 1 << bp;
                }
            }
        }
        for (i, count) in self.held.iter_mut().enumerate() {
            *count = if buttons & (1 << i) != 0 {
                count.saturating_add(1)
            } else {
                0
            };
        }
        match self.mode {
            JoyMode::Digital => self.digital(),
            JoyMode::OneHandle => self.one_handle(state.y),
            JoyMode::TwoHandle => match state.notches {
                Some(n) => self.two_handle_direct(n),
                None => self.two_handle(state.x, state.y),
            },
        }

        let slots: &[u8; BUTTON_SLOTS] = self.row[4..].try_into().expect("row tail");
        let mut held = held_word(slots, |c| self.count(c), false);
        if keyboard_held & bits::PAUSE != 0 {
            held |= bits::PAUSE | bits::CANCEL;
        }
        let passed = if self.keyboard_buttons {
            bits::KEY_R | bits::MENU_CONFIRM | bits::HORN | bits::A
        } else {
            bits::KEY_R | bits::MENU_CONFIRM
        };
        held |= keyboard_held & passed;
        let word = ButtonWord::new(self.prev_word, held);
        self.prev_word = held;
        ButtonState::from_word(word)
    }

    fn count(&self, code: u8) -> u16 {
        self.held.get(usize::from(code)).copied().unwrap_or(0)
    }

    fn step(&self, code: u8) -> Option<bool> {
        let n = if code == UNBOUND { 0 } else { self.count(code) };
        repeat_pulse(n).then_some(n == 1)
    }

    /// Mode 4: the keyboard two-handle rules on buttons (input.md §4.2).
    fn digital(&mut self) {
        let [pu, pd, bu, bd] = [self.row[0], self.row[1], self.row[2], self.row[3]];
        if self.step(pu).is_some() && self.power < MAX_POWER {
            self.power += 1;
        }
        if self.step(pd).is_some() && self.power > 0 {
            self.power -= 1;
        }
        match self.step(bu) {
            Some(true) if self.brake < EB => self.brake += 1,
            Some(false) if self.brake < REPEAT_BRAKE_MAX => self.brake += 1,
            _ => {}
        }
        if self.step(bd).is_some() && self.brake > 0 {
            self.brake -= 1;
        }
    }

    /// Mode 5: Y < 0 is the brake side, otherwise the power side; each forces the other to 0.
    fn one_handle(&mut self, y: i32) {
        if y < 0 {
            self.power = 0;
            self.brake = self
                .th
                .one_handle_brake
                .iter()
                .find(|&&(limit, _)| y < limit)
                .map_or(0, |&(_, b)| b);
        } else {
            self.brake = 0;
            self.power = self
                .th
                .one_handle_power
                .iter()
                .find(|&&(limit, _)| y > limit)
                .map_or(0, |&(_, p)| p);
        }
    }

    /// Mode 6: nearest calibrated position on each axis, each behind its gate.
    fn two_handle(&mut self, x: i32, y: i32) {
        if y <= self.th.mascon_gate {
            self.power = nearest(&self.cal.mascon, y) as u8;
        }
        if x <= self.cal.brake[BRAKE_POSITIONS - 1] + self.th.brake_margin {
            self.set_brake_debounced(nearest(&self.cal.brake, x) as u8);
        }
    }

    fn two_handle_direct(&mut self, n: DirectNotches) {
        if let Some(p) = n.power {
            self.power = p.min(MAX_POWER);
        }
        if let Some(b) = n.brake {
            self.set_brake_debounced(b.min(EB));
        }
    }

    /// EB needs more than `eb_polls` consecutive polls; anything else applies at once.
    fn set_brake_debounced(&mut self, b: u8) {
        if b == EB {
            self.eb_count += 1;
            if self.eb_count > self.th.eb_polls {
                self.eb_count = self.th.eb_polls;
                self.brake = EB;
            }
        } else {
            self.eb_count = 0;
            self.brake = b;
        }
    }
}

/// The ADJUSTMENT screen (0x434a00): six mascon samples of Y, then ten brake samples of X,
/// each taken on a menu-confirm edge; a cancel edge aborts without saving.
#[derive(Clone, Debug, Default)]
pub struct Calibrator {
    mascon: Vec<i32>,
    brake: Vec<i32>,
}

/// What the calibration screen shows or did this tick.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalStep {
    /// Waiting for mascon position n (prompt sprite 0x4b1 + n).
    Mascon(usize),
    /// Waiting for brake position n (prompt sprite 0x4a7 + n).
    Brake(usize),
    /// Calibrated: save it to `joystick.dat`. Sound 0x17 was due on the last sample.
    Done(Calibration),
    /// Cancelled: sound 0x13, keep the old calibration.
    Aborted,
}

impl Calibrator {
    pub fn new() -> Self {
        Self::default()
    }

    /// The prompt currently shown.
    pub fn prompt(&self) -> CalStep {
        if self.mascon.len() < MASCON_POSITIONS {
            CalStep::Mascon(self.mascon.len())
        } else {
            CalStep::Brake(self.brake.len())
        }
    }

    /// One tick: confirm wins over cancel in the same tick, as in the original's loop. Returns
    /// the new prompt, or the outcome. `sampled` is true when a sample was taken (sound 0x17).
    pub fn tick(&mut self, state: &JoyState, buttons: &ButtonState) -> (CalStep, bool) {
        if buttons.menu_confirm_pressed {
            if self.mascon.len() < MASCON_POSITIONS {
                self.mascon.push(state.y);
            } else {
                self.brake.push(state.x);
            }
            if self.brake.len() == BRAKE_POSITIONS - 1 {
                let mut brake = [0; BRAKE_POSITIONS];
                brake[..BRAKE_POSITIONS - 1].copy_from_slice(&self.brake);
                // Entry 10 duplicates EB, so brake code 10 is unreachable in mode 6.
                brake[BRAKE_POSITIONS - 1] = brake[BRAKE_POSITIONS - 2];
                let mascon = self.mascon[..].try_into().expect("six samples");
                return (CalStep::Done(Calibration { mascon, brake }), true);
            }
            return (self.prompt(), true);
        }
        if buttons.cancel_pressed {
            return (CalStep::Aborted, false);
        }
        (self.prompt(), false)
    }
}

// ---------------------------------------------------------------------------------------------
// Modern controllers (port additions, input.md §10).

/// The physical inputs of a generic gamepad that matter here, as gilrs reports them
/// (stick Y positive up, values −1..=1).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GenericInputs {
    pub left_x: f32,
    pub left_y: f32,
    pub dpad_up: bool,
    pub dpad_down: bool,
    pub dpad_left: bool,
    pub dpad_right: bool,
    /// Bottom face button (A / Cross): the A and menu-confirm role.
    pub south: bool,
    /// Right face button (B / Circle): horn and cancel (cancel only with `hybrid_input`).
    pub east: bool,
    /// Start / Menu: pause.
    pub start: bool,
    /// The rest are read by the Xbox-layout mapping of `hybrid_input` only
    /// ([`crate::hybrid::ModernPad`]): left face button (X / Square), the horn.
    pub west: bool,
    /// Top face button (Y / Triangle): the R key.
    pub north: bool,
    pub lb: bool,
    pub rb: bool,
    /// Trigger travel, 0 at rest to 1 fully pulled (a digital trigger reads 0 or 1).
    pub lt: f32,
    pub rt: f32,
    /// Right stick, same convention as the left one (+Y up).
    pub right_x: f32,
    pub right_y: f32,
    /// Back / View.
    pub select: bool,
}

/// What a gamepad gives one poll: the DirectInput state for the joystick decoder and the
/// inputs as they are (the Xbox-layout mapping and the hint device read those). Recorded and
/// replayed by input scripts ([`crate::script`]).
pub type PadSample = (JoyState, GenericInputs);

fn axis(v: f32) -> i32 {
    ((v.clamp(-1.0, 1.0) * AXIS_MAX as f32).round()) as i32
}

/// A generic gamepad as a `DIJOYSTATE`: left stick (or the d-pad, which wins) on X/Y with +Y
/// toward the player, so stick back = power in modes 4 and 5 like the arcade lever; face
/// buttons placed on the DirectInput indices the mode's binding row names, so their roles
/// stay the same in every mode.
pub fn generic_state(i: &GenericInputs, row: &[u8; BINDING_STRIDE]) -> JoyState {
    let mut x = axis(i.left_x);
    let mut y = -axis(i.left_y);
    if i.dpad_left != i.dpad_right {
        x = if i.dpad_left { -AXIS_MAX } else { AXIS_MAX };
    }
    if i.dpad_up != i.dpad_down {
        y = if i.dpad_up { -AXIS_MAX } else { AXIS_MAX };
    }
    let mut buttons = 0u32;
    // Row bytes: [8] A, [9] horn, [10] menu confirm, [11] cancel, [12] pause.
    for (on, slot) in [(i.south, 8), (i.east, 9), (i.start, 12)] {
        let idx = row[slot];
        if on && idx < 32 {
            buttons |= 1 << idx;
        }
    }
    JoyState {
        x,
        y,
        buttons,
        notches: None,
    }
}

/// A raw device input for combination decoding: a button code, or an axis code past half
/// travel in one direction.
pub type RawInput = u64;
const RAW_AXIS_NEG: u64 = 1 << 32;
const RAW_AXIS_POS: u64 = 2 << 32;

pub fn raw_button(code: u32) -> RawInput {
    u64::from(code)
}

pub fn raw_axis(code: u32, value: f32) -> Option<RawInput> {
    if value <= -0.5 {
        Some(RAW_AXIS_NEG | u64::from(code))
    } else if value >= 0.5 {
        Some(RAW_AXIS_POS | u64::from(code))
    } else {
        None
    }
}

/// A learned map from input combinations to notches, for controllers that report the handle
/// positions as button patterns (the PS1/PS2 two-handle controllers on a USB adapter).
/// Learned rather than built in because no layout could be verified without the hardware.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ComboTable {
    /// Active inputs at P0..P5, recorded with the brake handle still.
    pub power: Vec<BTreeSet<RawInput>>,
    /// Active inputs at B0..B8 and EB, recorded with the power handle still.
    pub brake: Vec<BTreeSet<RawInput>>,
}

const COMBO_HEADER: &str = "shuppatsu-combo 1";
/// The header written before the project was renamed; still read, so learned tables keep working.
const OLD_COMBO_HEADER: &str = "ddg-combo 1";

impl ComboTable {
    /// Inputs that change as one handle moves; inputs the same in every position belong to
    /// the other handle or to nothing.
    fn varying(sets: &[BTreeSet<RawInput>]) -> BTreeSet<RawInput> {
        let all: BTreeSet<RawInput> = sets.iter().flatten().copied().collect();
        all.into_iter()
            .filter(|i| !sets.iter().all(|s| s.contains(i)))
            .collect()
    }

    fn match_handle(sets: &[BTreeSet<RawInput>], active: &BTreeSet<RawInput>) -> Option<u8> {
        let used = Self::varying(sets);
        let now: BTreeSet<RawInput> = active.intersection(&used).copied().collect();
        let pos = sets.iter().position(|s| {
            s.intersection(&used)
                .copied()
                .collect::<BTreeSet<_>>()
                .eq(&now)
        })?;
        u8::try_from(pos).ok()
    }

    /// Notches for the active inputs; an unknown pattern (mid-transition) holds that handle.
    pub fn decode(&self, active: &BTreeSet<RawInput>) -> DirectNotches {
        DirectNotches {
            power: Self::match_handle(&self.power, active),
            brake: Self::match_handle(&self.brake, active),
        }
    }

    /// All inputs either handle uses; the button mapping ignores them.
    pub fn lever_inputs(&self) -> BTreeSet<RawInput> {
        let mut s = Self::varying(&self.power);
        s.extend(Self::varying(&self.brake));
        s
    }

    pub fn to_text(&self) -> String {
        let mut out = format!("{COMBO_HEADER}\n");
        for (name, sets) in [("power", &self.power), ("brake", &self.brake)] {
            for (n, s) in sets.iter().enumerate() {
                out.push_str(&format!("{name} {n}"));
                for i in s {
                    out.push_str(&format!(" {i:#x}"));
                }
                out.push('\n');
            }
        }
        out
    }

    pub fn parse(text: &str) -> Result<Self> {
        let mut lines = text.lines().filter(|l| !l.trim().is_empty());
        if !matches!(
            lines.next().map(str::trim),
            Some(COMBO_HEADER | OLD_COMBO_HEADER)
        ) {
            bail!("not a {COMBO_HEADER} file");
        }
        let mut t = Self::default();
        for line in lines {
            let mut f = line.split_whitespace();
            let sets = match f.next() {
                Some("power") => &mut t.power,
                Some("brake") => &mut t.brake,
                other => bail!("unknown handle {other:?}"),
            };
            let n: usize = f.next().context("missing notch")?.parse()?;
            if n != sets.len() {
                bail!("notch {n} out of order");
            }
            let set = f
                .map(|v| {
                    u64::from_str_radix(v.trim_start_matches("0x"), 16)
                        .with_context(|| format!("bad input {v}"))
                })
                .collect::<Result<_>>()?;
            sets.push(set);
        }
        if t.power.len() != MASCON_POSITIONS || t.brake.len() != usize::from(EB) + 1 {
            bail!(
                "need {MASCON_POSITIONS} power and {} brake positions",
                EB + 1
            );
        }
        Ok(t)
    }
}

/// Records a [`ComboTable`]: P0..P5 with the brake handle left alone, then B0..EB with the
/// power handle left alone, one confirm per position (the ADJUSTMENT order, port addition).
#[derive(Clone, Debug, Default)]
pub struct ComboLearner {
    table: ComboTable,
}

impl ComboLearner {
    pub fn new() -> Self {
        Self::default()
    }

    /// The position to record next: `(is_power, notch)`, or `None` when complete.
    pub fn prompt(&self) -> Option<(bool, u8)> {
        if self.table.power.len() < MASCON_POSITIONS {
            Some((true, self.table.power.len() as u8))
        } else if self.table.brake.len() <= usize::from(EB) {
            Some((false, self.table.brake.len() as u8))
        } else {
            None
        }
    }

    pub fn record(&mut self, active: BTreeSet<RawInput>) -> Option<ComboTable> {
        match self.prompt()? {
            (true, _) => self.table.power.push(active),
            (false, _) => self.table.brake.push(active),
        }
        self.prompt()
            .is_none()
            .then(|| std::mem::take(&mut self.table))
    }
}

/// How a gilrs gamepad is read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PadProfile {
    /// Left stick / d-pad as X/Y, South/East/Start as buttons. Works with any joystick mode;
    /// mode 6 also fits USB lever devices that report the handles as two axes.
    Generic,
    /// Handles as learned button combinations (forces mode 6 semantics); buttons as Generic.
    Combo(ComboTable),
}

/// Linux evdev codes as gilrs' `Code::into_u32` gives them there (`type << 16 | code`), for
/// pads gilrs has no SDL mapping for (input.md §10.6).
mod evdev {
    const KEY: u32 = 1 << 16;
    const ABS: u32 = 3 << 16;
    pub const BTN_TL: u32 = KEY | 0x136;
    pub const BTN_TR: u32 = KEY | 0x137;
    /// The analog triggers of the Xbox pads on Bluetooth (hid-generic / hid-microsoft):
    /// LT = ABS_BRAKE, RT = ABS_GAS, 0..1023 (measured: 045e:0b13 v0521).
    pub const ABS_GAS: u32 = ABS | 0x09;
    pub const ABS_BRAKE: u32 = ABS | 0x0a;
}

/// Microsoft's USB vendor id: its pads use the kernel's legacy X/Y codes (input.md §10.6).
const VENDOR_MICROSOFT: u16 = 0x045e;

/// What gilrs reports for one pad this tick, as plain data so the mapping onto
/// [`GenericInputs`] ([`generic_inputs`]) is pure and testable. Axis values are gilrs' (−1..=1,
/// sticks +Y up, already scaled from the device's own range, so a 0..65535 stick resting at
/// 34354 reads about +0.05); button values are 0..=1 (analog triggers mapped as buttons too).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PadReading {
    /// gilrs found an SDL mapping for the pad (otherwise it names the kernel codes itself).
    pub sdl_mapping: bool,
    pub vendor: Option<u16>,
    /// The Linux evdev backend (the raw codes below are evdev codes).
    pub evdev: bool,
    /// Buttons gilrs names for the pad, with their value (0 until first seen).
    pub buttons: Vec<(gilrs::Button, f32)>,
    /// Axes gilrs names for the pad, with their value (`None` until first seen).
    pub axes: Vec<(gilrs::Axis, Option<f32>)>,
    /// Every input seen so far by its raw code (Linux only; empty elsewhere): what a pad
    /// without a mapping reports under no name.
    pub raw_buttons: Vec<(u32, f32)>,
    pub raw_axes: Vec<(u32, f32)>,
}

impl PadReading {
    fn button(&self, b: gilrs::Button) -> f32 {
        self.buttons
            .iter()
            .find(|(n, _)| *n == b)
            .map_or(0.0, |(_, v)| *v)
    }

    fn named(&self, a: gilrs::Axis) -> bool {
        self.axes.iter().any(|(n, _)| *n == a)
    }

    fn axis(&self, a: gilrs::Axis) -> Option<f32> {
        self.axes
            .iter()
            .find(|(n, _)| *n == a)
            .and_then(|(_, v)| *v)
    }

    fn raw_button(&self, code: u32) -> f32 {
        self.raw_buttons
            .iter()
            .find(|(c, _)| *c == code)
            .map_or(0.0, |(_, v)| *v)
    }

    fn raw_axis(&self, code: u32) -> Option<f32> {
        self.raw_axes
            .iter()
            .find(|(c, _)| *c == code)
            .map(|(_, v)| *v)
    }
}

/// The trigger travel 0..=1 of an axis that rests at −1 and reads +1 fully pulled: a trigger
/// that gilrs reports as `Axis::LeftZ`/`RightZ`, or as a raw axis without a mapping (gilrs
/// scales any axis to −1..=1 from the device's range, so a 0..1023 trigger at rest is −1).
pub fn trigger_travel_from_axis(v: f32) -> f32 {
    ((v + 1.0) / 2.0).clamp(0.0, 1.0)
}

/// The Xbox-layout inputs from what gilrs reports, robust to the ways a backend can name them
/// (input.md §10.6):
///
/// - Triggers: the most travel of the named analog `LeftTrigger2`/`RightTrigger2` (an SDL
///   mapping, XInput / Windows.Gaming.Input), `LeftZ`/`RightZ` when the pad also has a named
///   right stick (Linux xpad without a mapping: ABS_Z/ABS_RZ are the triggers, rest −1), and
///   the raw ABS_BRAKE/ABS_GAS (Bluetooth Xbox pads without a mapping). A pad can list a
///   digital BTN_TL2 that never fires next to its analog axis, hence the most of all.
/// - Right stick: the named `RightStickX/Y`, else `LeftZ`/`RightZ` (Bluetooth Xbox pads
///   without a mapping: ABS_Z/ABS_RZ are the right stick; gilrs does not flip an unnamed Y,
///   so −RZ is up).
/// - LB/RB: the named buttons or the raw BTN_TL/BTN_TR.
/// - X/Y: on Linux, a Microsoft pad without an SDL mapping reports X as BTN_NORTH and Y as BTN_WEST
///   (the kernel's legacy BTN_X/BTN_Y), which gilrs names North/West: swapped back.
pub fn generic_inputs(r: &PadReading) -> GenericInputs {
    use gilrs::{Axis, Button};
    let pressed = |b: Button| r.button(b) >= 0.5;
    let value = |a: Axis| r.axis(a).unwrap_or(0.0);
    let dpad_x = value(Axis::DPadX);
    let dpad_y = value(Axis::DPadY);
    let named_right = r.named(Axis::RightStickX) || r.named(Axis::RightStickY);
    let trigger = |b: Button, z: Axis, raw: u32| {
        let mut t = r.button(b);
        if named_right && let Some(v) = r.axis(z) {
            t = t.max(trigger_travel_from_axis(v));
        }
        if let Some(v) = r.raw_axis(raw) {
            t = t.max(trigger_travel_from_axis(v));
        }
        t
    };
    let (right_x, right_y) = if named_right {
        (value(Axis::RightStickX), value(Axis::RightStickY))
    } else {
        (value(Axis::LeftZ), -value(Axis::RightZ))
    };
    let legacy_xy = r.evdev && !r.sdl_mapping && r.vendor == Some(VENDOR_MICROSOFT);
    let (west, north) = if legacy_xy {
        (pressed(Button::North), pressed(Button::West))
    } else {
        (pressed(Button::West), pressed(Button::North))
    };
    GenericInputs {
        left_x: value(Axis::LeftStickX),
        left_y: value(Axis::LeftStickY),
        dpad_up: pressed(Button::DPadUp) || dpad_y >= 0.5,
        dpad_down: pressed(Button::DPadDown) || dpad_y <= -0.5,
        dpad_left: pressed(Button::DPadLeft) || dpad_x <= -0.5,
        dpad_right: pressed(Button::DPadRight) || dpad_x >= 0.5,
        south: pressed(Button::South),
        east: pressed(Button::East),
        start: pressed(Button::Start),
        west,
        north,
        lb: pressed(Button::LeftTrigger) || r.raw_button(evdev::BTN_TL) >= 0.5,
        rb: pressed(Button::RightTrigger) || r.raw_button(evdev::BTN_TR) >= 0.5,
        lt: trigger(Button::LeftTrigger2, Axis::LeftZ, evdev::ABS_BRAKE),
        rt: trigger(Button::RightTrigger2, Axis::RightZ, evdev::ABS_GAS),
        right_x,
        right_y,
        select: pressed(Button::Select),
    }
}

/// Every named gilrs button and axis, for [`PadReading`] and `shuppatsu pad monitor`.
const NAMED_BUTTONS: [gilrs::Button; 19] = {
    use gilrs::Button::*;
    [
        South,
        East,
        North,
        West,
        C,
        Z,
        LeftTrigger,
        LeftTrigger2,
        RightTrigger,
        RightTrigger2,
        Select,
        Start,
        Mode,
        LeftThumb,
        RightThumb,
        DPadUp,
        DPadDown,
        DPadLeft,
        DPadRight,
    ]
};
const NAMED_AXES: [gilrs::Axis; 8] = {
    use gilrs::Axis::*;
    [
        LeftStickX,
        LeftStickY,
        LeftZ,
        RightStickX,
        RightStickY,
        RightZ,
        DPadX,
        DPadY,
    ]
};

/// The first connected gilrs gamepad (a force-feedback one if any, like 0x433d60).
pub struct PadSource {
    gilrs: gilrs::Gilrs,
    id: Option<gilrs::GamepadId>,
    profile: PadProfile,
    /// The vibration effects on the picked pad, made at its first use (input.md §11).
    rumble: Option<PadRumble>,
    /// A force-feedback error was reported (once per session).
    rumble_failed: bool,
}

/// One gilrs effect per motor, each played at full magnitude scaled by its gain, so the
/// game sets the two levels independently. gilrs' force-feedback thread mixes the playing
/// effects every 50 ms and hands them to the device (Linux: an `FF_RUMBLE` effect of 100 ms,
/// renewed while playing, so a stopped or dropped effect, or an exited process, goes quiet
/// within 100 ms); the messages never block the caller.
struct PadRumble {
    id: gilrs::GamepadId,
    strong: gilrs::ff::Effect,
    weak: gilrs::ff::Effect,
    now: crate::rumble::Motors,
}

impl PadRumble {
    fn new(gilrs: &mut gilrs::Gilrs, id: gilrs::GamepadId) -> Result<Self, gilrs::ff::Error> {
        use gilrs::ff::{BaseEffect, BaseEffectType, EffectBuilder};
        let mut motor = |kind| {
            EffectBuilder::new()
                .add_effect(BaseEffect {
                    kind,
                    ..BaseEffect::default()
                })
                .gamepads(&[id])
                .finish(gilrs)
        };
        Ok(Self {
            id,
            strong: motor(BaseEffectType::Strong {
                magnitude: u16::MAX,
            })?,
            weak: motor(BaseEffectType::Weak {
                magnitude: u16::MAX,
            })?,
            now: crate::rumble::Motors::OFF,
        })
    }

    fn set(&mut self, m: crate::rumble::Motors) -> Result<(), gilrs::ff::Error> {
        if m == self.now {
            return Ok(());
        }
        for (effect, level, was) in [
            (&self.strong, m.strong, self.now.strong),
            (&self.weak, m.weak, self.now.weak),
        ] {
            if level > 0.0 {
                effect.set_gain(level.min(1.0))?;
                if was <= 0.0 {
                    effect.play()?;
                }
            } else if was > 0.0 {
                effect.stop()?;
            }
        }
        self.now = m;
        Ok(())
    }
}

impl Drop for PadRumble {
    fn drop(&mut self) {
        let _ = self.strong.stop();
        let _ = self.weak.stop();
    }
}

impl PadSource {
    /// `None` when gilrs can't start (no backend or no access to input devices).
    pub fn open(profile: PadProfile) -> Option<Self> {
        // Raw values: the original sets no dead zone and the thresholds sit far from centre.
        let gilrs = gilrs::GilrsBuilder::new()
            .with_default_filters(false)
            .build()
            .map_err(|e| eprintln!("gamepad support unavailable: {e}"))
            .ok()?;
        let mut s = Self {
            gilrs,
            id: None,
            profile,
            rumble: None,
            rumble_failed: false,
        };
        s.pick();
        Some(s)
    }

    fn pick(&mut self) {
        let pads: Vec<_> = self.gilrs.gamepads().collect();
        let id = pads
            .iter()
            .find(|(_, g)| g.is_ff_supported())
            .or(pads.first())
            .map(|(id, _)| *id);
        // A different (or no) pad: the old pad's effects go (unplugged: gilrs closed it).
        if self.rumble.as_ref().is_some_and(|r| Some(r.id) != id) {
            self.rumble = None;
        }
        self.id = id;
        if let Some(id) = self.id {
            let g = self.gilrs.gamepad(id);
            eprintln!(
                "gamepad: {} ({:04x}:{:04x})",
                g.name(),
                g.vendor_id().unwrap_or(0),
                g.product_id().unwrap_or(0)
            );
        }
    }

    pub fn connected(&self) -> bool {
        self.id.is_some()
    }

    /// Whether the picked pad can vibrate (gilrs: Linux evdev rumble, Windows XInput/WGI).
    pub fn ff_supported(&self) -> bool {
        self.id
            .and_then(|id| self.gilrs.connected_gamepad(id))
            .is_some_and(|g| g.is_ff_supported())
    }

    /// Sets the pad's motors (0..=1 each) until the next call; unchanged levels send
    /// nothing. A pad without force feedback, or none, is left alone. Never blocks: the
    /// levels go to gilrs' force-feedback thread as messages. An error is reported once.
    pub fn set_motors(&mut self, m: crate::rumble::Motors) {
        if let Err(e) = self.try_set_motors(m)
            && !self.rumble_failed
        {
            self.rumble_failed = true;
            eprintln!("vibration: {e}");
        }
    }

    /// [`PadSource::set_motors`] with its error, for `shuppatsu pad rumble`.
    pub fn try_set_motors(&mut self, m: crate::rumble::Motors) -> Result<()> {
        if self.rumble.is_none() {
            if m.is_off() || !self.ff_supported() {
                return Ok(());
            }
            let id = self.id.context("no gamepad")?;
            self.rumble = Some(PadRumble::new(&mut self.gilrs, id)?);
        }
        if let Some(r) = self.rumble.as_mut() {
            r.set(m)?;
        }
        Ok(())
    }

    /// Drains pending gilrs events and follows hot-plugging.
    fn pump(&mut self) {
        let mut changed = false;
        while let Some(ev) = self.gilrs.next_event() {
            if matches!(
                ev.event,
                gilrs::EventType::Connected | gilrs::EventType::Disconnected
            ) {
                changed = true;
            }
        }
        if changed
            || self
                .id
                .is_some_and(|id| !self.gilrs.gamepad(id).is_connected())
        {
            self.pick();
        }
    }

    /// Everything active on the pad now, as [`RawInput`]s (for learning combinations).
    pub fn raw_inputs(&mut self) -> Option<BTreeSet<RawInput>> {
        self.pump();
        let g = self.gilrs.connected_gamepad(self.id?)?;
        let st = g.state();
        let mut set: BTreeSet<RawInput> = st
            .buttons()
            .filter(|(_, d)| d.is_pressed())
            .map(|(c, _)| raw_button(c.into_u32()))
            .collect();
        set.extend(
            st.axes()
                .filter_map(|(c, d)| raw_axis(c.into_u32(), d.value())),
        );
        Some(set)
    }

    /// What gilrs reports for the pad now (call after [`PadSource::pump`]); `None` with no pad.
    pub fn reading(&self) -> Option<PadReading> {
        let g = self.gilrs.connected_gamepad(self.id?)?;
        let st = g.state();
        let buttons = NAMED_BUTTONS
            .iter()
            .filter(|&&b| g.button_code(b).is_some())
            .map(|&b| {
                let v = g.button_data(b).map_or(0.0, |d| d.value());
                (b, if g.is_pressed(b) { v.max(1.0) } else { v })
            })
            .collect();
        let axes = NAMED_AXES
            .iter()
            .filter(|&&a| g.axis_code(a).is_some())
            .map(|&a| (a, g.axis_data(a).map(|d| d.value())))
            .collect();
        let linux = cfg!(target_os = "linux");
        Some(PadReading {
            sdl_mapping: g.mapping_source() == gilrs::MappingSource::SdlMappings,
            vendor: g.vendor_id(),
            evdev: linux,
            buttons,
            axes,
            raw_buttons: if linux {
                st.buttons()
                    .map(|(c, d)| (c.into_u32(), if d.is_pressed() { 1.0 } else { d.value() }))
                    .collect()
            } else {
                Vec::new()
            },
            raw_axes: if linux {
                st.axes().map(|(c, d)| (c.into_u32(), d.value())).collect()
            } else {
                Vec::new()
            },
        })
    }

    /// The pad's identity for `shuppatsu pad monitor`.
    pub fn describe(&self) -> Option<String> {
        let g = self.gilrs.connected_gamepad(self.id?)?;
        let uuid: String = g.uuid().iter().map(|b| format!("{b:02x}")).collect();
        Some(format!(
            "{} (os name {:?}, {:04x}:{:04x}, SDL GUID {uuid}), mapping: {:?}{}",
            g.name(),
            g.os_name(),
            g.vendor_id().unwrap_or(0),
            g.product_id().unwrap_or(0),
            g.mapping_source(),
            g.map_name().map(|n| format!(" ({n})")).unwrap_or_default(),
        ))
    }

    /// This tick's state for a joystick whose binding row is `row`; `None` with no pad.
    pub fn sample(&mut self, row: &[u8; BINDING_STRIDE]) -> Option<JoyState> {
        self.read(row).map(|(s, _)| s)
    }

    /// This tick's state for `row` and the pad's inputs as they are (for the Xbox-layout
    /// mapping and the hint device); `None` with no pad.
    pub fn read(&mut self, row: &[u8; BINDING_STRIDE]) -> Option<(JoyState, GenericInputs)> {
        let raw = match &self.profile {
            PadProfile::Combo(_) => Some(self.raw_inputs()?),
            PadProfile::Generic => {
                self.pump();
                None
            }
        };
        let reading = self.reading()?;
        let mut inputs = generic_inputs(&reading);
        let g = self.gilrs.connected_gamepad(self.id?)?;
        use gilrs::Button;
        let state = match (&self.profile, raw) {
            (PadProfile::Combo(table), Some(raw)) => {
                // A button that is part of a handle's pattern is not also a button.
                let levers = table.lever_inputs();
                let free = |b: Button| {
                    g.button_code(b)
                        .is_none_or(|c| !levers.contains(&raw_button(c.into_u32())))
                };
                inputs.south &= free(Button::South);
                inputs.east &= free(Button::East);
                inputs.start &= free(Button::Start);
                let mut s = generic_state(
                    &GenericInputs {
                        left_x: 0.0,
                        left_y: 0.0,
                        dpad_up: false,
                        dpad_down: false,
                        dpad_left: false,
                        dpad_right: false,
                        ..inputs
                    },
                    row,
                );
                s.notches = Some(table.decode(&raw));
                s
            }
            _ => generic_state(&inputs, row),
        };
        Some((state, inputs))
    }
}

/// Options for [`Controls::new`].
#[derive(Clone, Debug)]
pub struct PadOptions {
    /// `joystick.dat` to load (mode 6 calibration); the exe default when absent or missing.
    pub joystick_dat: Option<PathBuf>,
    /// A learned combination table (`shuppatsu-combo 1` text); selects the combination profile.
    pub combo_table: Option<PathBuf>,
    /// Setting `keyboard_with_controller` (default on): the keyboard's horn and A button work
    /// in the controller modes too ([`Joystick::keyboard_buttons`]). With `hybrid_input` on
    /// the whole keyboard works anyway, so this one only matters with it off.
    pub keyboard_with_controller: bool,
    /// Setting `hybrid_input` (default on): keyboard and controller both live in every
    /// control mode (input.md §10.6); off = the original's one selected device.
    pub hybrid_input: bool,
    /// Open no gamepad at all (headless runs stay deterministic).
    pub no_pad: bool,
    /// Fixed hint device (`--hint-device`, `SHUPPATSU_HINT_DEVICE`; headless shots have no pad).
    pub hint_device: Option<HintDevice>,
    /// The pad comes from an input script ([`Controls::feed_pad`]) instead of a device, and
    /// is connected at the start (`Some(true)`) or not. `hybrid_input` only.
    pub scripted_pad: Option<bool>,
}

impl Default for PadOptions {
    fn default() -> Self {
        Self {
            joystick_dat: None,
            combo_table: None,
            keyboard_with_controller: true,
            hybrid_input: true,
            no_pad: false,
            hint_device: None,
            scripted_pad: None,
        }
    }
}

/// Keyboard plus optional joystick, polled once per tick (0x4333d0).
pub struct Controls {
    keyboard: Keyboard,
    /// `hybrid_input` off: the selected joystick mode with its pad (the original's one
    /// device).
    joystick: Option<(Joystick, PadSource)>,
    /// `hybrid_input` on.
    hybrid: Option<Hybrid>,
    hint_override: Option<HintDevice>,
    /// [`PadOptions::scripted_pad`]: the sample the next poll reads instead of the device.
    scripted: Option<Option<PadSample>>,
    /// The pad sample the last poll read (`hybrid_input`), for the input log.
    last_pad: Option<PadSample>,
}

/// The state of `hybrid_input` (input.md §10.6). The keyboard's notches are the shared ones:
/// the pad's lever directions step them through [`Keyboard::poll_with`], and a physical
/// handle sets them through the [`LeverArbiter`].
struct Hybrid {
    /// Any gamepad, followed through hot-plugging; `None` without gilrs or headless.
    pad: Option<PadSource>,
    /// Modes 5/6 (or a learned combination controller): the selected decoder for the pad's
    /// handles; its buttons follow its binding row. Otherwise the pad is read in the Xbox
    /// layout ([`crate::hybrid::ModernPad`]).
    analog: Option<(Joystick, LeverArbiter)>,
    /// Mode 4's row: where [`PadSource::read`] places the buttons of a state that the Xbox
    /// mapping does not use.
    row: [u8; BINDING_STRIDE],
    menu: MenuRepeat,
    prev_held: u32,
    /// Y on the previous poll (its R bit is a press-frame bit).
    prev_north: bool,
    /// The Xbox layout's stick and trigger latches.
    modern: ModernPad,
    hint: HintTracker,
}

impl Controls {
    /// `mode` is the control mode 0..=6. Joystick modes fall back to keyboard mode 0 when no
    /// gamepad is connected, as `input_init` does.
    pub fn new(exe: &Exe<'_>, mode: usize, opts: &PadOptions) -> Result<Self> {
        if opts.hybrid_input {
            return Self::new_hybrid(exe, mode, opts);
        }
        let bindings = Bindings::read(exe)?;
        if mode <= 3 {
            let keyboard = Keyboard::new(&bindings, mode).context("keyboard mode")?;
            return Ok(Self {
                keyboard,
                joystick: None,
                hybrid: None,
                hint_override: None,
                scripted: None,
                last_pad: None,
            });
        }
        let (joy_mode, profile) = Self::profile(mode, opts)?;
        // The keyboard still supplies its word (and Shift override) in joystick modes; its
        // own levers are unused there.
        let keyboard = Keyboard::new(&bindings, 0).context("keyboard mode 0")?;
        // A scripted pad drives the hybrid input only.
        let pad = if opts.no_pad || opts.scripted_pad.is_some() {
            None
        } else {
            PadSource::open(profile).filter(PadSource::connected)
        };
        let Some(pad) = pad else {
            eprintln!("no gamepad: control mode {mode} falls back to keyboard mode 0");
            return Ok(Self {
                keyboard,
                joystick: None,
                hybrid: None,
                hint_override: None,
                scripted: None,
                last_pad: None,
            });
        };
        let tables = JoyTables::read(exe)?;
        Ok(Self {
            keyboard,
            joystick: Some((
                Joystick {
                    keyboard_buttons: opts.keyboard_with_controller,
                    ..Joystick::new(&tables, joy_mode, Self::calibration(&tables, opts))
                },
                pad,
            )),
            hybrid: None,
            hint_override: None,
            scripted: None,
            last_pad: None,
        })
    }

    /// The joystick mode and pad profile for a joystick `mode` (a combination table forces
    /// mode 6).
    fn profile(mode: usize, opts: &PadOptions) -> Result<(JoyMode, PadProfile)> {
        let joy_mode = JoyMode::from_index(mode).context("control modes are 0..=6")?;
        Ok(match &opts.combo_table {
            Some(path) => {
                let text = std::fs::read_to_string(path)
                    .with_context(|| format!("reading {}", path.display()))?;
                (
                    JoyMode::TwoHandle,
                    PadProfile::Combo(ComboTable::parse(&text)?),
                )
            }
            None => (joy_mode, PadProfile::Generic),
        })
    }

    fn calibration(tables: &JoyTables, opts: &PadOptions) -> Calibration {
        let mut cal = tables.default_calibration;
        if let Some(path) = &opts.joystick_dat
            && let Ok(bytes) = std::fs::read(path)
        {
            cal.overlay(&bytes);
        }
        cal
    }

    fn new_hybrid(exe: &Exe<'_>, mode: usize, opts: &PadOptions) -> Result<Self> {
        let bindings = Bindings::read(exe)?;
        let tables = JoyTables::read(exe)?;
        let (joy_mode, profile) = if mode <= 3 {
            (None, PadProfile::Generic)
        } else {
            let (m, p) = Self::profile(mode, opts)?;
            (Some(m), p)
        };
        let pad = if opts.no_pad || opts.scripted_pad.is_some() {
            None
        } else {
            PadSource::open(profile)
        };
        let connected = opts
            .scripted_pad
            .unwrap_or_else(|| pad.as_ref().is_some_and(PadSource::connected));
        // The handle decoder of the analog modes needs its device now, as `input_init` does;
        // any pad connected later is read in the Xbox layout.
        let joy_mode = joy_mode.filter(|_| connected);
        if mode > 3 && joy_mode.is_none() {
            eprintln!("no gamepad: control mode {mode} falls back to keyboard mode 0");
        }
        let keyboard =
            Keyboard::new(&bindings, if mode <= 3 { mode } else { 0 }).context("keyboard mode")?;
        let analog = joy_mode.filter(|m| m.is_analog()).map(|m| {
            let joy = Joystick::new(&tables, m, Self::calibration(&tables, opts));
            (joy, LeverArbiter::new(m == JoyMode::OneHandle))
        });
        let device = if joy_mode.is_some() {
            HintDevice::Pad
        } else {
            HintDevice::Keyboard
        };
        let mut controls = Self {
            keyboard,
            joystick: None,
            hybrid: Some(Hybrid {
                pad,
                analog,
                row: *tables.row(JoyMode::Digital),
                menu: MenuRepeat::default(),
                prev_held: 0,
                prev_north: false,
                modern: ModernPad::default(),
                hint: HintTracker::new(device),
            }),
            hint_override: opts.hint_device,
            // Before the first scripted poll: connected and at rest, or not.
            scripted: opts.scripted_pad.map(|on| on.then(PadSample::default)),
            last_pad: None,
        };
        controls.reset_levers();
        Ok(controls)
    }

    /// One poll. `down(dik)` is the keyboard state; `shift_allowed` is the original's gate for
    /// the Shift override: not driving, or paused (`(0x855ff8 & 2) == 0 || (0x855ffc & 4)`).
    pub fn poll(&mut self, down: impl Fn(u8) -> bool, shift_allowed: bool) -> ButtonState {
        if self.hybrid.is_some() {
            return self.poll_hybrid(down);
        }
        let kb = self.keyboard.poll(down);
        let shift = shift_allowed && self.keyboard.shift_held();
        let Some((joy, pad)) = self.joystick.as_mut() else {
            return kb;
        };
        if shift {
            return kb;
        }
        match pad.sample(joy.row()) {
            Some(state) => joy.poll(&state, kb.word.held),
            // Unplugged: hold the levers and release the buttons (port choice; the original
            // keeps its last word, edges included).
            None => {
                let levers = (joy.power, joy.brake);
                let b = joy.poll(&JoyState::default(), kb.word.held);
                (joy.power, joy.brake) = levers;
                b
            }
        }
    }

    /// `hybrid_input`: both devices every tick. Buttons are OR-ed before the edges are taken;
    /// stepped levers share one repeat counter per direction; a physical handle and the steps
    /// hand the notches to each other, last used wins. The Shift override has nothing left to
    /// do (the keyboard always counts).
    fn poll_hybrid(&mut self, down: impl Fn(u8) -> bool) -> ButtonState {
        let h = self.hybrid.as_mut().expect("hybrid");
        let row = match &h.analog {
            Some((joy, _)) => *joy.row(),
            None => h.row,
        };
        let sample = match self.scripted {
            Some(s) => s,
            None => h.pad.as_mut().and_then(|p| p.read(&row)),
        };
        self.last_pad = sample;
        self.tick_hybrid(down, sample)
    }

    /// The pad sample the next poll reads ([`PadOptions::scripted_pad`]; `None`: no pad).
    pub fn feed_pad(&mut self, sample: Option<PadSample>) {
        if self.scripted.is_some() {
            self.scripted = Some(sample);
        }
    }

    /// The pad sample the last poll read (`hybrid_input`; `None`: no pad, or the original's
    /// one-device input, whose pad is not recorded).
    pub fn last_pad(&self) -> Option<PadSample> {
        self.last_pad
    }

    /// The device the hints follow from now on, until the next key or pad use (a replayed
    /// run starts where the session was).
    pub fn set_hint_device(&mut self, device: HintDevice) {
        if let Some(h) = self.hybrid.as_mut() {
            h.hint.device = device;
        }
    }

    /// [`Controls::poll_hybrid`] with the pad's sample of this tick (`None`: no pad).
    fn tick_hybrid(
        &mut self,
        down: impl Fn(u8) -> bool,
        sample: Option<(JoyState, GenericInputs)>,
    ) -> ButtonState {
        let h = self.hybrid.as_mut().expect("hybrid");
        let mut pad_word = 0;
        let mut levers = LeverHolds::NONE;
        let mut readings = [None, None];
        let mut deadband = HANDLE_DEADBAND;
        match (&mut h.analog, &sample) {
            (None, Some((_, inputs))) => {
                (pad_word, levers) = h.modern.map(inputs);
                if inputs.north && !h.prev_north {
                    pad_word |= bits::KEY_R;
                }
            }
            (Some((joy, _)), Some((state, _))) => {
                pad_word = joy.poll(state, 0).word.held;
                readings = match (state.notches, joy.mode()) {
                    (Some(n), _) => {
                        deadband = 0;
                        [n.power.map(i32::from), n.brake.map(i32::from)]
                    }
                    (None, JoyMode::OneHandle) => [Some(state.y); 2],
                    (None, _) => [Some(state.y), Some(state.x)],
                };
            }
            // Unplugged: the pad adds nothing (its buttons release once, no stale edges) and
            // a handle holds its notch.
            (_, None) => {}
        }
        h.prev_north = sample.as_ref().is_some_and(|(_, i)| i.north);
        if sample.is_none() {
            h.modern.reset();
        }

        let before = [self.keyboard.power, self.keyboard.brake];
        let kb = self.keyboard.poll_with(down, levers);
        if let Some((joy, arbiter)) = &mut h.analog {
            let stepped = [self.keyboard.power, self.keyboard.brake];
            let handle = [joy.power, joy.brake];
            let ticks = [0, 1].map(|i| HandleTick {
                before: before[i],
                stepped: stepped[i],
                handle: handle[i],
                reading: readings[i],
            });
            [self.keyboard.power, self.keyboard.brake] = arbiter.tick(ticks, deadband);
        }

        let held = h.menu.merge(&[kb.word.held, pad_word]);
        let word = ButtonWord::new(h.prev_held, held);
        h.prev_held = held;
        let pad_view = sample.as_ref().map(|(_, i)| (pad_buttons(i), pad_axes(i)));
        h.hint.update(
            self.keyboard.any_pressed(),
            pad_view.as_ref().map(|(b, a)| (*b, &a[..])),
        );
        ButtonState::from_word(word)
    }

    pub fn power(&self) -> u8 {
        match &self.joystick {
            Some((j, _)) => j.power,
            None => self.keyboard.power,
        }
    }

    pub fn brake(&self) -> u8 {
        match &self.joystick {
            Some((j, _)) => j.brake,
            None => self.keyboard.brake,
        }
    }

    /// Run or station start: P0/EB in the digital modes (0x4332c0). With `hybrid_input` and a
    /// handle, the handle is authoritative again, as in the analog modes.
    pub fn reset_levers(&mut self) {
        if let Some(h) = self.hybrid.as_mut() {
            match h.analog.as_mut() {
                Some((joy, arbiter)) => {
                    arbiter.hand_back();
                    (self.keyboard.power, self.keyboard.brake) = (joy.power, joy.brake);
                }
                None => self.keyboard.reset_levers(),
            }
            return;
        }
        match self.joystick.as_mut() {
            Some((j, _)) => j.reset_levers(),
            None => self.keyboard.reset_levers(),
        }
    }

    /// Puts the notches back, as the pause menu does on exit (0x42b7f0 restores 0x829d00 /
    /// 0x829d04). Analog levers follow the next poll anyway.
    pub fn set_levers(&mut self, power: u8, brake: u8) {
        match self.joystick.as_mut() {
            Some((j, _)) => (j.power, j.brake) = (power, brake),
            None => (self.keyboard.power, self.keyboard.brake) = (power, brake),
        }
    }

    /// The active joystick mode, if a joystick drives the levers (with `hybrid_input`: the
    /// analog handle decoder of modes 5/6).
    pub fn joy_mode(&self) -> Option<JoyMode> {
        match &self.hybrid {
            Some(h) => h.analog.as_ref().map(|(j, _)| j.mode()),
            None => self.joystick.as_ref().map(|(j, _)| j.mode()),
        }
    }

    /// Mode 5/6: the levers are physical (pause-menu cursor follows the mascon, 0x42b930).
    pub fn is_analog(&self) -> bool {
        self.joy_mode().is_some_and(JoyMode::is_analog)
    }

    /// Raw joystick state and raw inputs for the calibration and learning screens.
    pub fn pad_mut(&mut self) -> Option<(&mut Joystick, &mut PadSource)> {
        match &mut self.hybrid {
            Some(h) => match (&mut h.analog, &mut h.pad) {
                (Some((j, _)), Some(p)) => Some((j, p)),
                _ => None,
            },
            None => self.joystick.as_mut().map(|(j, p)| (j, p)),
        }
    }

    /// The gamepad, whichever way it is read (for `shuppatsu pad monitor`).
    pub fn pad_source(&self) -> Option<&PadSource> {
        match &self.hybrid {
            Some(h) => h.pad.as_ref(),
            None => self.joystick.as_ref().map(|(_, p)| p),
        }
    }

    fn pad_source_mut(&mut self) -> Option<&mut PadSource> {
        match &mut self.hybrid {
            Some(h) => h.pad.as_mut(),
            None => self.joystick.as_mut().map(|(_, p)| p),
        }
    }

    /// A pad that can vibrate is connected (the original's `g_has_ff`, 0x816838, which
    /// shows the 振動 option): with `hybrid_input` any pad, in every control mode; off, the
    /// selected joystick mode's pad, as the original vibrates only the joystick it reads.
    /// Never with a scripted pad (a replayed or headless run).
    pub fn ff_supported(&self) -> bool {
        self.scripted.is_none() && self.pad_source().is_some_and(PadSource::ff_supported)
    }

    /// Sets the vibration motors of that pad until the next call (input.md §11).
    pub fn rumble(&mut self, m: crate::rumble::Motors) {
        if self.scripted.is_some() {
            return;
        }
        if let Some(p) = self.pad_source_mut() {
            p.set_motors(m);
        }
    }

    /// At exit: stops the motors and, if they were running, waits two of gilrs' 50 ms
    /// force-feedback ticks so the stop reaches the device before the process ends (Windows
    /// keeps a vibration level set until it is changed; Linux ends it with the effect's
    /// 100 ms length anyway).
    pub fn stop_rumble_for_exit(&mut self) {
        let Some(p) = self.pad_source_mut() else {
            return;
        };
        let on = p.rumble.as_ref().is_some_and(|r| !r.now.is_off());
        p.set_motors(crate::rumble::Motors::OFF);
        if on {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    /// The device the on-screen hints show (`hybrid_input` only; `None` = the original's
    /// legends for the selected control mode).
    pub fn hint_device(&self) -> Option<HintDevice> {
        let h = self.hybrid.as_ref()?;
        Some(self.hint_override.unwrap_or(h.hint.device))
    }

    /// A gamepad is connected now (`hybrid_input` follows hot-plugging; otherwise the
    /// selected joystick mode's pad).
    pub fn pad_connected(&self) -> bool {
        if let Some(s) = self.scripted {
            return s.is_some();
        }
        match &self.hybrid {
            Some(h) => h.pad.as_ref().is_some_and(PadSource::connected),
            None => self.joystick.is_some(),
        }
    }
}

/// The evdev name of a raw Linux code, for the monitor.
fn evdev_name(code: u32) -> String {
    let (kind, c) = (code >> 16, code & 0xffff);
    let name = match (kind, c) {
        (1, 0x130) => "BTN_SOUTH",
        (1, 0x131) => "BTN_EAST",
        (1, 0x132) => "BTN_C",
        (1, 0x133) => "BTN_NORTH",
        (1, 0x134) => "BTN_WEST",
        (1, 0x135) => "BTN_Z",
        (1, 0x136) => "BTN_TL",
        (1, 0x137) => "BTN_TR",
        (1, 0x138) => "BTN_TL2",
        (1, 0x139) => "BTN_TR2",
        (1, 0x13a) => "BTN_SELECT",
        (1, 0x13b) => "BTN_START",
        (1, 0x13c) => "BTN_MODE",
        (1, 0x13d) => "BTN_THUMBL",
        (1, 0x13e) => "BTN_THUMBR",
        (3, 0x00) => "ABS_X",
        (3, 0x01) => "ABS_Y",
        (3, 0x02) => "ABS_Z",
        (3, 0x03) => "ABS_RX",
        (3, 0x04) => "ABS_RY",
        (3, 0x05) => "ABS_RZ",
        (3, 0x09) => "ABS_GAS",
        (3, 0x0a) => "ABS_BRAKE",
        (3, 0x10) => "ABS_HAT0X",
        (3, 0x11) => "ABS_HAT0Y",
        _ => return format!("{code:#x}"),
    };
    name.to_owned()
}

/// The monitor's view of one reading: what gilrs names and the raw codes, each value to one
/// decimal (so stick noise does not flood the terminal), then the Xbox-layout result.
fn monitor_lines(r: &PadReading) -> [String; 3] {
    let f = |v: f32| format!("{:+.1}", (v * 10.0).round() / 10.0 + 0.0);
    let named = r
        .buttons
        .iter()
        .map(|(b, v)| format!("{b:?} {}", f(*v)))
        .chain(
            r.axes
                .iter()
                .map(|(a, v)| format!("{a:?} {}", v.map_or("--".into(), f))),
        )
        .collect::<Vec<_>>()
        .join(", ");
    let raw = r
        .raw_buttons
        .iter()
        .filter(|(_, v)| *v != 0.0)
        .chain(&r.raw_axes)
        .map(|(c, v)| format!("{} {}", evdev_name(*c), f(*v)))
        .collect::<Vec<_>>()
        .join(", ");
    let i = generic_inputs(r);
    let held = [
        (i.south, "A"),
        (i.east, "B"),
        (i.west, "X"),
        (i.north, "Y"),
        (i.lb, "LB"),
        (i.rb, "RB"),
        (i.select, "View"),
        (i.start, "Menu"),
        (i.dpad_up, "D-up"),
        (i.dpad_down, "D-down"),
        (i.dpad_left, "D-left"),
        (i.dpad_right, "D-right"),
    ]
    .iter()
    .filter(|(on, _)| *on)
    .map(|(_, n)| *n)
    .collect::<Vec<_>>()
    .join(" ");
    let xbox = format!(
        "left stick {}/{}, right stick {}/{}, LT {:.1}, RT {:.1}, held [{held}]",
        f(i.left_x),
        f(i.left_y),
        f(i.right_x),
        f(i.right_y),
        i.lt,
        i.rt,
    );
    [
        format!("  gilrs: {named}"),
        format!("  raw:   {raw}"),
        format!("  xbox:  {xbox}"),
    ]
}

/// `shuppatsu pad monitor`: the notches and the button word the game reads, plus what gilrs
/// reports (named buttons and axes, raw codes) and the Xbox-layout reading. Without `--mode`
/// it watches the pad as the game does by default: `hybrid_input` in keyboard mode 0.
fn monitor(exe: &Exe<'_>, mode: Option<usize>, opts: PadOptions) -> Result<()> {
    let (mode, opts) = match mode {
        None => (
            0,
            PadOptions {
                hybrid_input: true,
                ..opts
            },
        ),
        Some(m @ 4..=6) => (m, opts),
        Some(m) => {
            bail!("monitor: --mode {m} is not a joystick mode (4..=6); omit it for the Xbox layout")
        }
    };
    let mut controls = Controls::new(exe, mode, &opts)?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines() {
            if line.is_err() || tx.send(()).is_err() {
                break;
            }
        }
    });
    println!(
        "mode {mode} ({}; analog levers: {}); Enter = station start, Ctrl+C = quit",
        if opts.hybrid_input {
            "hybrid input, pad in the Xbox layout"
        } else {
            "the selected joystick mode alone"
        },
        controls.is_analog()
    );
    let mut last = None;
    let mut last_pad = None;
    let mut last_lines: [String; 3] = Default::default();
    loop {
        if rx.try_recv().is_ok() {
            controls.reset_levers();
        }
        let b = controls.poll(|_| false, false);
        let pad = controls.pad_source();
        let who = pad.and_then(|p| p.describe());
        if who != last_pad {
            match &who {
                Some(w) => println!("pad: {w}"),
                None => println!("pad: none connected"),
            }
            last_pad = who;
        }
        if let Some(r) = controls.pad_source().and_then(|p| p.reading()) {
            let lines = monitor_lines(&r);
            for (new, old) in lines.iter().zip(&mut last_lines) {
                if new != old {
                    println!("{new}");
                    *old = new.clone();
                }
            }
        }
        let now = (controls.power(), controls.brake(), b.word.held);
        if last != Some(now) {
            println!("P{} B{} buttons {:#08x}", now.0, now.1, now.2);
            last = Some(now);
        }
        std::thread::sleep(std::time::Duration::from_millis(33));
    }
}

/// Whether bit `bit` is set in a sysfs capability mask (`capabilities/ff`: hex words of a
/// `long`, the most significant first, as the kernel's `input_print_bitmap` writes them).
#[cfg(any(target_os = "linux", test))]
fn sysfs_bit(mask: &str, bit: u32) -> bool {
    let bits = usize::BITS;
    let words: Vec<&str> = mask.split_whitespace().collect();
    let Some(i) = usize::try_from(bit / bits)
        .ok()
        .and_then(|i| words.len().checked_sub(i + 1))
    else {
        return false;
    };
    usize::from_str_radix(words[i], 16).is_ok_and(|w| w >> (bit % bits) & 1 == 1)
}

/// What the kernel says about the pad's force feedback (Linux): for each evdev node of
/// `vendor:product`, its driver, its `EV_FF` capabilities and whether this user may open
/// it for writing, which uploading an effect needs.
#[cfg(target_os = "linux")]
fn linux_ff_report(vendor: u16, product: u16) -> Vec<String> {
    use std::path::Path;
    let read = |p: &Path| std::fs::read_to_string(p).map(|s| s.trim().to_owned());
    let Ok(dir) = std::fs::read_dir("/sys/class/input") else {
        return vec!["  /sys/class/input unreadable".into()];
    };
    let mut nodes: Vec<String> = dir
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.starts_with("event"))
        .collect();
    nodes.sort();
    let mut out = Vec::new();
    for node in nodes {
        let dev = Path::new("/sys/class/input").join(&node).join("device");
        let id = |f: &str| {
            read(&dev.join("id").join(f))
                .ok()
                .and_then(|s| u16::from_str_radix(&s, 16).ok())
        };
        if id("vendor") != Some(vendor) || id("product") != Some(product) {
            continue;
        }
        let name = read(&dev.join("name")).unwrap_or_default();
        let driver = std::fs::read_link(dev.join("device").join("driver"))
            .ok()
            .and_then(|p| Some(p.file_name()?.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "?".into());
        let mask = read(&dev.join("capabilities").join("ff")).unwrap_or_default();
        let named = [
            (0x50, "RUMBLE"),
            (0x51, "PERIODIC"),
            (0x58, "SQUARE"),
            (0x59, "TRIANGLE"),
            (0x5a, "SINE"),
            (0x60, "GAIN"),
        ];
        let caps: Vec<&str> = named
            .iter()
            .filter(|(b, _)| sysfs_bit(&mask, *b))
            .map(|(_, n)| *n)
            .collect();
        let path = Path::new("/dev/input").join(&node);
        let write = match std::fs::OpenOptions::new().write(true).open(&path) {
            Ok(_) => "writable".to_owned(),
            Err(e) => format!("not writable ({e})"),
        };
        out.push(format!(
            "  {} {name:?}: driver {driver}, FF [{}] (mask {mask:?}), {write}",
            path.display(),
            caps.join(" ")
        ));
    }
    if out.is_empty() {
        out.push("  no evdev node with this vendor:product".into());
    }
    out.push(
        "  gilrs needs FF_SQUARE, FF_TRIANGLE, FF_SINE and FF_GAIN (rumble pads get them from \
         ff-memless) and a writable node"
            .into(),
    );
    out
}

/// `shuppatsu pad rumble [strong weak [ms]]`: vibrates the pad for checking by hand. With no
/// values it plays the game's jolt at 弱, 並 and 強, then a long pulse at 強.
fn rumble_tool(exe: &Exe<'_>, args: &[String]) -> Result<()> {
    use crate::rumble::{JoltSpec, Motors};
    let nums = args
        .iter()
        .map(|a| {
            a.parse::<f32>()
                .with_context(|| format!("{a:?} is not a number"))
        })
        .collect::<Result<Vec<_>>>()?;
    let spec = JoltSpec::read(exe)?;
    let jolt_ms = spec.duration_us / 1000;
    let pulses: Vec<(String, Motors, u32)> = match *nums.as_slice() {
        [] => {
            let mut v: Vec<_> = (1..=3u8)
                .zip(["弱 (Low)", "並 (Mid)", "強 (High)"])
                .map(|(l, n)| (format!("jolt {n}"), spec.motors(l), jolt_ms))
                .collect();
            v.push(("long pulse 強 (High)".into(), spec.motors(3), 1000));
            v
        }
        [strong, weak] => vec![("pulse".into(), Motors { strong, weak }, 500)],
        [strong, weak, ms] if ms >= 0.0 => {
            vec![("pulse".into(), Motors { strong, weak }, ms as u32)]
        }
        _ => bail!("usage: shuppatsu pad rumble [strong weak [ms]] (levels 0..1)"),
    };
    let mut pad = PadSource::open(PadProfile::Generic).context("gamepad support unavailable")?;
    let who = pad.describe().context("no gamepad connected")?;
    println!("pad: {who}");
    let ff = pad.ff_supported();
    println!(
        "force feedback (gilrs): {}",
        if ff { "supported" } else { "not supported" }
    );
    #[cfg(target_os = "linux")]
    if let Some(g) = pad.id.and_then(|id| pad.gilrs.connected_gamepad(id)) {
        let (v, p) = (g.vendor_id().unwrap_or(0), g.product_id().unwrap_or(0));
        for line in linux_ff_report(v, p) {
            println!("{line}");
        }
    }
    if !ff {
        bail!("this pad cannot vibrate through gilrs");
    }
    let gap = std::time::Duration::from_millis(700);
    for (what, m, ms) in pulses {
        let m = Motors {
            strong: m.strong.clamp(0.0, 1.0),
            weak: m.weak.clamp(0.0, 1.0),
        };
        println!(
            "{what}: strong {:.3}, weak {:.3}, {ms} ms",
            m.strong, m.weak
        );
        pad.try_set_motors(m)
            .context("starting the effects (uploaded to gilrs' force-feedback thread)")?;
        std::thread::sleep(std::time::Duration::from_millis(u64::from(ms)));
        pad.try_set_motors(Motors::OFF)?;
        std::thread::sleep(gap);
    }
    println!("done (gilrs reports device write errors only through its log, not here)");
    Ok(())
}

/// `shuppatsu pad`: watch, calibrate or learn a controller from the terminal, or test its
/// vibration.
pub fn run_tool(exe: &Exe<'_>, args: &[String]) -> Result<()> {
    const USAGE: &str = "usage: shuppatsu pad <monitor|calibrate|learn> [--mode <0..6>] \
                         [--joystick-dat <file>] [--combo <file>]\n       \
                         shuppatsu pad rumble [strong weak [ms]]";
    if args.first().is_some_and(|a| a == "rumble") {
        return rumble_tool(exe, &args[1..]);
    }
    let mut it = args.iter();
    let action = it.next().context(USAGE)?.clone();
    let mut mode = None;
    // The tool watches the selected device alone (the original's one-device model).
    let mut opts = PadOptions {
        hybrid_input: false,
        ..PadOptions::default()
    };
    while let Some(a) = it.next() {
        let mut value = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--mode" => mode = Some(value()?.parse()?),
            "--joystick-dat" => opts.joystick_dat = Some(value()?.into()),
            "--combo" => opts.combo_table = Some(value()?.into()),
            other => bail!("unknown option {other}\n{USAGE}"),
        }
    }
    if action == "monitor" {
        return monitor(exe, mode, opts);
    }
    let mode = mode.unwrap_or(6);
    if !(4..=6).contains(&mode) {
        bail!("joystick modes are 4..=6");
    }
    let learn = action == "learn";
    // Learning reads the pad raw; the table it writes doesn't exist yet.
    let ctl_opts = PadOptions {
        combo_table: if learn {
            None
        } else {
            opts.combo_table.clone()
        },
        ..opts.clone()
    };
    let mut controls = Controls::new(exe, mode, &ctl_opts)?;
    let Some((joy, pad)) = controls.pad_mut() else {
        bail!("no gamepad found");
    };
    let row = *joy.row();
    // Enter in the terminal confirms a position; "q" cancels.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines() {
            let Ok(line) = line else { break };
            if tx.send(line.trim() == "q").is_err() {
                break;
            }
        }
    });
    let tick = std::time::Duration::from_millis(33);
    match action.as_str() {
        "calibrate" => {
            let path = opts
                .joystick_dat
                .context("calibrate needs --joystick-dat")?;
            let mut cal = Calibrator::new();
            let mut shown = None;
            loop {
                let prompt = cal.prompt();
                if shown != Some(prompt) {
                    match prompt {
                        CalStep::Mascon(n) => println!("mascon to P{n}, then Enter"),
                        CalStep::Brake(n) => println!("brake to B{n}, then Enter"),
                        _ => {}
                    }
                    shown = Some(prompt);
                }
                let state = pad.sample(&row).unwrap_or_default();
                let line = rx.try_recv().ok();
                let buttons = ButtonState {
                    menu_confirm_pressed: line == Some(false),
                    cancel_pressed: line == Some(true),
                    ..Default::default()
                };
                match cal.tick(&state, &buttons).0 {
                    CalStep::Done(c) => {
                        std::fs::write(&path, c.to_bytes())?;
                        println!("wrote {}: {c:?}", path.display());
                        return Ok(());
                    }
                    CalStep::Aborted => bail!("calibration cancelled"),
                    _ => std::thread::sleep(tick),
                }
            }
        }
        _ if learn => {
            let path = opts.combo_table.context("learn needs --combo")?;
            let mut learner = ComboLearner::new();
            while let Some((is_power, n)) = learner.prompt() {
                if is_power {
                    println!("power handle to P{n} (leave the brake alone), then Enter");
                } else {
                    println!("brake handle to B{n} (leave the power alone), then Enter");
                }
                if rx.recv()? {
                    bail!("learning cancelled");
                }
                let active = pad.raw_inputs().context("gamepad disconnected")?;
                println!("  {active:x?}");
                if let Some(table) = learner.record(active) {
                    std::fs::write(&path, table.to_text())?;
                    println!("wrote {}", path.display());
                }
            }
            Ok(())
        }
        _ => bail!("{USAGE}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::tests::{DIK_RETURN, DIK_SPACE};

    fn thresholds() -> Thresholds {
        // The values input.md §4.3–4.5 documents for 0x4341c0.
        Thresholds {
            digital: [(-500, 500), (-500, 500)],
            one_handle_brake: [
                (-950, 10),
                (-850, 9),
                (-750, 8),
                (-650, 7),
                (-550, 6),
                (-450, 5),
                (-350, 4),
                (-250, 3),
                (-150, 2),
                (-50, 1),
            ],
            one_handle_power: [(900, 5), (700, 4), (500, 3), (300, 2), (100, 1)],
            mascon_gate: 950,
            brake_margin: 50,
            eb_polls: 5,
        }
    }

    fn tables() -> JoyTables {
        JoyTables {
            rows: [
                [
                    0x11, 0x10, 0x13, 0x12, 0x10, 0x11, 0x12, 0x13, 0, 1, 0, 1, 2, 0x13,
                ],
                [
                    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0, 1, 0, 1, 2, 0x13,
                ],
                [
                    0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 1, 0, 1, 0, 2, 0x13,
                ],
            ],
            thresholds: thresholds(),
            default_calibration: Calibration {
                mascon: [0, -102, -310, -480, -730, -1000],
                brake: [0, 39, 116, 165, 235, 285, 340, 366, 392, 426, 426],
            },
        }
    }

    fn joy(mode: JoyMode) -> Joystick {
        let t = tables();
        Joystick::new(&t, mode, t.default_calibration)
    }

    fn axes(x: i32, y: i32) -> JoyState {
        JoyState {
            x,
            y,
            ..Default::default()
        }
    }

    fn hold(j: &mut Joystick, s: JoyState, polls: usize) {
        for _ in 0..polls {
            j.poll(&s, 0);
        }
        j.poll(&JoyState::default(), 0);
    }

    #[test]
    fn digital_axes_are_buttons_with_keyboard_repeat() {
        let mut j = joy(JoyMode::Digital);
        j.brake = 0;
        // Y ≥ 500 is power+; 499 does nothing.
        hold(&mut j, axes(0, 499), 20);
        assert_eq!(j.power, 0);
        // Steps on polls 1, 9 and 12.
        hold(&mut j, axes(0, 500), 12);
        assert_eq!(j.power, 3);
        hold(&mut j, axes(0, -1000), 1);
        assert_eq!(j.power, 2);
        // Brake+ (X ≥ 500) repeats to B8 only; one more press reaches EB.
        hold(&mut j, axes(1000, 0), 100);
        assert_eq!(j.brake, 8);
        hold(&mut j, axes(1000, 0), 1);
        assert_eq!(j.brake, EB);
        hold(&mut j, axes(-500, 0), 1);
        assert_eq!(j.brake, 8);
        // Menu pulses come from the same virtual buttons (Y− = up).
        let b = j.poll(&axes(0, -800), 0);
        assert!(b.word.pressed & bits::UP_PULSE != 0);
        assert!(b.word.held & bits::UP != 0);
    }

    #[test]
    fn digital_reset_forces_eb_analog_does_not() {
        let mut j = joy(JoyMode::Digital);
        j.power = 3;
        j.brake = 0;
        j.reset_levers();
        assert_eq!((j.power, j.brake), (0, EB));
        let mut j = joy(JoyMode::TwoHandle);
        j.poll(&axes(0, -1000), 0);
        j.reset_levers();
        assert_eq!(j.power, 5);
    }

    #[test]
    fn one_handle_thresholds() {
        let mut j = joy(JoyMode::OneHandle);
        let cases = [
            (1000, (5, 0)),
            (901, (5, 0)),
            (900, (4, 0)),
            (701, (4, 0)),
            (501, (3, 0)),
            (500, (2, 0)),
            (101, (1, 0)),
            (100, (0, 0)),
            (0, (0, 0)),
            (-50, (0, 0)),
            (-51, (0, 1)),
            (-150, (0, 1)),
            (-151, (0, 2)),
            (-750, (0, 7)),
            (-751, (0, 8)),
            (-850, (0, 8)),
            (-851, (0, 9)),
            (-950, (0, 9)),
            (-951, (0, 10)),
        ];
        for (y, want) in cases {
            j.poll(&axes(0, y), 0);
            assert_eq!((j.power, j.brake), want, "y = {y}");
        }
    }

    #[test]
    fn two_handle_nearest_with_gates() {
        let mut j = joy(JoyMode::TwoHandle);
        j.poll(&axes(0, -300), 0);
        assert_eq!((j.power, j.brake), (2, 0));
        // Tie between −102 and −310 (−206) keeps the lower index.
        j.poll(&axes(0, -206), 0);
        assert_eq!(j.power, 1);
        // Y above the gate holds the mascon; X past cal[10] + 50 holds the brake.
        j.poll(&axes(165, -1000), 0);
        assert_eq!((j.power, j.brake), (5, 3));
        j.poll(&axes(477, 951), 0);
        assert_eq!((j.power, j.brake), (5, 3));
        j.poll(&axes(476, 950), 0);
        assert_eq!(j.power, 0);
    }

    #[test]
    fn two_handle_eb_needs_six_polls() {
        let mut j = joy(JoyMode::TwoHandle);
        j.poll(&axes(392, 0), 0);
        assert_eq!(j.brake, 8);
        for _ in 0..5 {
            j.poll(&axes(426, 0), 0);
            assert_eq!(j.brake, 8);
        }
        j.poll(&axes(426, 0), 0);
        assert_eq!(j.brake, EB);
        j.poll(&axes(426, 0), 0);
        assert_eq!(j.brake, EB);
        // Leaving EB applies at once and resets the count.
        j.poll(&axes(0, 0), 0);
        assert_eq!(j.brake, 0);
        // Code 10 is unreachable with a game-written calibration (cal[10] == cal[9]).
        for _ in 0..10 {
            j.poll(&axes(476, 0), 0);
        }
        assert_eq!(j.brake, EB);
    }

    #[test]
    fn button_roles_follow_the_row() {
        // Modes 4/5: btn0 = A + menu confirm, btn1 = horn + cancel, btn2 = pause.
        let mut j = joy(JoyMode::OneHandle);
        let b = j.poll(
            &JoyState {
                buttons: 0b001,
                ..Default::default()
            },
            0,
        );
        assert!(b.confirm_pressed && b.menu_confirm_pressed && !b.horn_held);
        // Mode 6 swaps buttons 0 and 1.
        let mut j = joy(JoyMode::TwoHandle);
        let b = j.poll(
            &JoyState {
                buttons: 0b001,
                ..Default::default()
            },
            0,
        );
        assert!(b.horn_pressed && b.cancel_pressed && !b.confirm_pressed);
        let b = j.poll(
            &JoyState {
                buttons: 0b101,
                ..Default::default()
            },
            0,
        );
        assert!(b.pause_pressed && b.horn_held && !b.horn_pressed);
    }

    #[test]
    fn keyboard_passes_through_in_joystick_modes() {
        let mut j = joy(JoyMode::Digital);
        // Keyboard Esc is pause + cancel, Enter menu confirm and (port) the A button, Space
        // (port) the horn. The original drops A and the horn (input.md §6).
        let b = j.poll(
            &JoyState::default(),
            bits::PAUSE | bits::CANCEL | bits::A | bits::MENU_CONFIRM | bits::HORN,
        );
        assert!(b.pause_pressed && b.cancel_pressed && b.menu_confirm_pressed);
        assert!(b.confirm_pressed && b.horn_pressed && b.horn_held);
    }

    /// Setting `keyboard_with_controller` off: the original's mask, Esc, Enter as menu confirm
    /// and R only; Space does not horn and Enter is not the A button (input.md §6).
    #[test]
    fn keyboard_buttons_off_is_the_original_mask() {
        let mut j = Joystick {
            keyboard_buttons: false,
            ..joy(JoyMode::Digital)
        };
        let b = j.poll(
            &JoyState::default(),
            bits::PAUSE | bits::A | bits::MENU_CONFIRM | bits::HORN | bits::KEY_R,
        );
        assert!(b.pause_pressed && b.cancel_pressed && b.menu_confirm_pressed);
        assert!(!b.confirm_pressed && !b.horn_pressed && !b.horn_held);
        assert_ne!(b.word.held & bits::KEY_R, 0);
        assert!(PadOptions::default().keyboard_with_controller);
    }

    /// Decided port behaviour (input.md §10.3): in mode 4 with a pad, the keyboard's horn and
    /// ATS confirm work while driving, OR-ed with the pad's buttons; the levers stay the pad's.
    #[test]
    fn keyboard_drives_buttons_alongside_a_mode_4_pad() {
        use crate::input::tests::{DIK_RETURN, DIK_SPACE, bindings};
        let mut kb = Keyboard::new(&bindings(), 0).unwrap();
        let mut j = joy(JoyMode::Digital);
        j.brake = 0;
        // A fake pad: nothing held, then btn1 (horn in mode 4).
        let idle = JoyState::default();
        let pad_horn = JoyState {
            buttons: 0b010,
            ..Default::default()
        };
        let mut tick = |pad: &JoyState, key: Option<u8>| {
            let k = kb.poll(|c| Some(c) == key);
            j.poll(pad, k.word.held)
        };

        let b = tick(&idle, Some(DIK_SPACE));
        assert!(b.horn_pressed && b.horn_held && !b.confirm_pressed);
        let b = tick(&idle, Some(DIK_SPACE));
        assert!(b.horn_held && !b.horn_pressed, "one edge per hold");
        tick(&idle, None);

        let b = tick(&idle, Some(DIK_RETURN));
        assert!(b.confirm_pressed && b.menu_confirm_pressed && !b.horn_held);
        tick(&idle, None);

        // Pad horn held, then the keyboard joins: no second edge; either keeps it held.
        let b = tick(&pad_horn, None);
        assert!(b.horn_pressed);
        let b = tick(&pad_horn, Some(DIK_SPACE));
        assert!(b.horn_held && !b.horn_pressed);
        let b = tick(&idle, Some(DIK_SPACE));
        assert!(b.horn_held && !b.horn_pressed);

        // The keyboard's lever keys do not move the pad's levers (↓ is power+ in mode 0).
        for _ in 0..20 {
            tick(&idle, Some(0xd0));
        }
        assert_eq!((j.power, j.brake), (0, 0));
    }

    #[test]
    fn calibration_file_round_trip_and_short_files() {
        let t = tables();
        let cal = t.default_calibration;
        let bytes = cal.to_bytes();
        assert_eq!(bytes.len(), JOYSTICK_DAT_LEN);
        assert_eq!(Calibration::from_bytes(&bytes), Some(cal));
        let mut c = cal;
        c.overlay(&[0x10, 0, 0, 0, 0x20, 0, 0]);
        assert_eq!(c.mascon[0], 0x10);
        assert_eq!(c.mascon[1], cal.mascon[1]);
    }

    #[test]
    fn calibrator_flow() {
        let confirm = ButtonState {
            menu_confirm_pressed: true,
            ..Default::default()
        };
        let idle = ButtonState::default();
        let mut c = Calibrator::new();
        assert_eq!(c.tick(&axes(0, 0), &idle), (CalStep::Mascon(0), false));
        for (i, y) in [10, -100, -300, -500, -700, -990].into_iter().enumerate() {
            assert_eq!(c.prompt(), CalStep::Mascon(i));
            c.tick(&axes(999, y), &confirm);
        }
        let mut last = None;
        for (i, x) in (0..10).map(|i| i * 40).enumerate() {
            assert_eq!(c.prompt(), CalStep::Brake(i));
            last = Some(c.tick(&axes(x, 999), &confirm));
        }
        let Some((CalStep::Done(cal), true)) = last else {
            panic!("{last:?}");
        };
        assert_eq!(cal.mascon, [10, -100, -300, -500, -700, -990]);
        assert_eq!(cal.brake[9], 360);
        assert_eq!(cal.brake[10], 360);

        let mut c = Calibrator::new();
        let both = ButtonState {
            cancel_pressed: true,
            ..confirm
        };
        assert_eq!(c.tick(&axes(0, 5), &both), (CalStep::Mascon(1), true));
        let cancel = ButtonState {
            cancel_pressed: true,
            ..Default::default()
        };
        assert_eq!(c.tick(&axes(0, 0), &cancel).0, CalStep::Aborted);
    }

    #[test]
    fn generic_pad_mapping() {
        let t = tables();
        let row4 = t.row(JoyMode::Digital);
        let s = generic_state(
            &GenericInputs {
                left_x: 0.25,
                left_y: -1.0,
                south: true,
                ..Default::default()
            },
            row4,
        );
        // Stick back is +Y (power+ in mode 4, power side in mode 5).
        assert_eq!((s.x, s.y, s.buttons), (250, 1000, 0b001));
        let s = generic_state(
            &GenericInputs {
                left_y: -1.0,
                dpad_up: true,
                dpad_right: true,
                east: true,
                start: true,
                ..Default::default()
            },
            t.row(JoyMode::TwoHandle),
        );
        // The d-pad wins; East is horn, which mode 6 puts on button 0.
        assert_eq!((s.x, s.y, s.buttons), (1000, -1000, 0b101));
        let mut j = joy(JoyMode::Digital);
        j.brake = 0;
        j.poll(
            &generic_state(
                &GenericInputs {
                    dpad_down: true,
                    ..Default::default()
                },
                row4,
            ),
            0,
        );
        assert_eq!(j.power, 1);
    }

    fn set(v: &[u64]) -> BTreeSet<RawInput> {
        v.iter().copied().collect()
    }

    /// A made-up two-handle encoding: power on inputs 1..3, brake on inputs 10..13, and
    /// input 99 held throughout (not part of either handle).
    fn learned() -> ComboTable {
        let mut l = ComboLearner::new();
        let power = [&[2][..], &[1, 2], &[1], &[1, 3], &[3], &[2, 3]];
        for p in power {
            let mut s = set(p);
            s.extend([10, 11, 12, 13, 99]); // brake handle at "released" throughout
            assert!(l.record(s).is_none());
        }
        let brake: [&[u64]; 10] = [
            &[10, 11, 12, 13],
            &[11, 12, 13],
            &[10, 12, 13],
            &[12, 13],
            &[10, 11, 13],
            &[11, 13],
            &[10, 13],
            &[13],
            &[10, 11, 12],
            &[11, 12],
        ];
        let mut done = None;
        for b in brake {
            let mut s = set(b);
            s.extend([2, 99]); // power at N
            done = l.record(s);
        }
        done.expect("ten brake positions complete the table")
    }

    #[test]
    fn combo_decode_and_transitions() {
        let t = learned();
        let d = t.decode(&set(&[1, 3, 10, 13, 99]));
        assert_eq!(
            d,
            DirectNotches {
                power: Some(3),
                brake: Some(6)
            }
        );
        // The input 99 doesn't matter; an unknown pattern holds.
        let d = t.decode(&set(&[1, 2, 3, 11, 12]));
        assert_eq!(
            d,
            DirectNotches {
                power: None,
                brake: Some(9)
            }
        );
        // Through mode 6: EB still needs six polls.
        let mut j = joy(JoyMode::TwoHandle);
        let s = |active: &[u64]| JoyState {
            notches: Some(t.decode(&set(active))),
            ..Default::default()
        };
        j.poll(&s(&[2, 10, 11, 12, 13]), 0);
        assert_eq!((j.power, j.brake), (0, 0));
        for _ in 0..5 {
            j.poll(&s(&[1, 11, 12]), 0);
        }
        assert_eq!((j.power, j.brake), (2, 0));
        j.poll(&s(&[1, 11, 12]), 0);
        assert_eq!(j.brake, EB);
        assert_eq!(t.lever_inputs(), set(&[1, 2, 3, 10, 11, 12, 13]));
    }

    #[test]
    fn combo_text_round_trip() {
        let t = learned();
        let text = t.to_text();
        assert!(text.starts_with("shuppatsu-combo 1\n"));
        assert_eq!(ComboTable::parse(&text).unwrap(), t);
        let old = text.replacen("shuppatsu-combo 1", "ddg-combo 1", 1);
        assert_eq!(
            ComboTable::parse(&old).unwrap(),
            t,
            "pre-rename tables load"
        );
        assert!(ComboTable::parse("shuppatsu-combo 1\npower 1\n").is_err());
        assert!(ComboTable::parse("junk").is_err());
        assert_eq!(raw_axis(5, 0.7), Some(RAW_AXIS_POS | 5));
        assert_eq!(raw_axis(5, -0.2), None);
    }

    // --- hybrid_input (input.md §10.6) --------------------------------------------------

    const DIK_DOWN: u8 = 0xd0;
    const DIK_UP: u8 = 0xc8;

    /// Controls with `hybrid_input` on, keyboard mode `kb_mode`, an optional handle decoder
    /// and no real pad (the tests feed the samples).
    fn hybrid(kb_mode: usize, analog: Option<JoyMode>) -> Controls {
        use crate::input::tests::bindings;
        let t = tables();
        let mut c = Controls {
            keyboard: Keyboard::new(&bindings(), kb_mode).unwrap(),
            joystick: None,
            hybrid: Some(Hybrid {
                pad: None,
                analog: analog.map(|m| {
                    let j = Joystick::new(&t, m, t.default_calibration);
                    (j, LeverArbiter::new(m == JoyMode::OneHandle))
                }),
                row: *t.row(JoyMode::Digital),
                menu: MenuRepeat::default(),
                prev_held: 0,
                prev_north: false,
                modern: ModernPad::default(),
                hint: HintTracker::new(HintDevice::Keyboard),
            }),
            hint_override: None,
            scripted: None,
            last_pad: None,
        };
        c.reset_levers();
        c
    }

    fn xpad(f: impl FnOnce(&mut GenericInputs)) -> Option<(JoyState, GenericInputs)> {
        let mut i = GenericInputs::default();
        f(&mut i);
        Some((generic_state(&i, tables().row(JoyMode::Digital)), i))
    }

    fn handle(x: i32, y: i32) -> Option<(JoyState, GenericInputs)> {
        Some((axes(x, y), GenericInputs::default()))
    }

    fn keys(codes: &[u8]) -> impl Fn(u8) -> bool + '_ {
        move |c| codes.contains(&c)
    }

    #[test]
    fn hybrid_buttons_are_ored_before_the_edges() {
        let mut c = hybrid(0, None);
        let idle = xpad(|_| {});
        // Keyboard Space, then the pad's X joins: one horn edge, held while either holds.
        let b = c.tick_hybrid(keys(&[DIK_SPACE]), idle);
        assert!(b.horn_pressed && b.horn_held);
        let b = c.tick_hybrid(keys(&[DIK_SPACE]), xpad(|i| i.west = true));
        assert!(b.horn_held && !b.horn_pressed);
        let b = c.tick_hybrid(keys(&[]), xpad(|i| i.west = true));
        assert!(b.horn_held && !b.horn_pressed && b.word.released == 0);
        let b = c.tick_hybrid(keys(&[]), idle);
        assert!(!b.horn_held && b.word.released & bits::HORN != 0);
        // Pad A and keyboard Enter in the same tick: one confirm edge, one menu-confirm edge.
        let b = c.tick_hybrid(keys(&[DIK_RETURN]), xpad(|i| i.south = true));
        assert!(b.confirm_pressed && b.menu_confirm_pressed);
        let b = c.tick_hybrid(keys(&[]), xpad(|i| i.south = true));
        assert!(!b.confirm_pressed && !b.menu_confirm_pressed);
        // Pad B cancels, Menu pauses, Y is R on the press frame only.
        c.tick_hybrid(keys(&[]), idle);
        let b = c.tick_hybrid(keys(&[]), xpad(|i| i.east = true));
        assert!(b.cancel_pressed && !b.horn_held && !b.pause_pressed);
        let b = c.tick_hybrid(keys(&[]), xpad(|i| i.start = true));
        assert!(b.pause_pressed && !b.cancel_pressed);
        let r: Vec<bool> = (0..3)
            .map(|_| {
                c.tick_hybrid(keys(&[]), xpad(|i| i.north = true)).word.held & bits::KEY_R != 0
            })
            .collect();
        assert_eq!(r, [true, false, false]);
        // Menu cursor: the keyboard's ↑ and the pad's d-pad ↑ make one pulse train.
        c.tick_hybrid(keys(&[]), idle);
        let mut pulses = Vec::new();
        for t in 1..=12 {
            let k: &[u8] = if t <= 4 { &[DIK_UP] } else { &[] };
            let p = xpad(|i| i.dpad_up = t >= 2);
            if c.tick_hybrid(keys(k), p).word.pressed & bits::UP_PULSE != 0 {
                pulses.push(t);
            }
        }
        assert_eq!(pulses, [1, 9, 12]);
    }

    #[test]
    fn hybrid_two_devices_never_double_step() {
        let mut c = hybrid(0, None);
        c.keyboard.brake = 0;
        // Keyboard ↓ (power+ in mode 0) and the pad's d-pad ↓ in the same tick: one step.
        c.tick_hybrid(keys(&[DIK_DOWN]), xpad(|i| i.dpad_down = true));
        assert_eq!(c.power(), 1);
        // LT, d-pad ↓ and the key together, held: still one virtual key (steps on 1, 9, 12).
        for _ in 1..12 {
            c.tick_hybrid(
                keys(&[DIK_DOWN]),
                xpad(|i| {
                    i.dpad_down = true;
                    i.lt = 1.0;
                }),
            );
        }
        assert_eq!(c.power(), 3);
        // Staggered: the key from tick 1, the pad joining on tick 3 and holding on after the
        // key is let go is one hold, not a second press.
        c.tick_hybrid(keys(&[]), xpad(|_| {}));
        c.keyboard.power = 0;
        for t in 1..=12 {
            let k: &[u8] = if t <= 5 { &[DIK_DOWN] } else { &[] };
            c.tick_hybrid(keys(k), xpad(|i| i.lt = if t >= 3 { 1.0 } else { 0.0 }));
        }
        assert_eq!(c.power(), 3);
        // Brake: RT and → together reach EB on the press only; the repeat stops at B8.
        c.tick_hybrid(keys(&[]), xpad(|_| {}));
        for _ in 0..60 {
            c.tick_hybrid(
                keys(&[]),
                xpad(|i| {
                    i.rt = 1.0;
                    i.dpad_right = true;
                }),
            );
        }
        assert_eq!(c.brake(), 8);
        c.tick_hybrid(keys(&[]), xpad(|_| {}));
        c.tick_hybrid(keys(&[0xcd]), xpad(|i| i.rt = 1.0));
        assert_eq!(c.brake(), EB);
        // Either device steps alone too.
        c.tick_hybrid(keys(&[]), xpad(|_| {}));
        c.tick_hybrid(keys(&[]), xpad(|i| i.rb = true));
        c.tick_hybrid(keys(&[]), xpad(|_| {}));
        c.tick_hybrid(keys(&[0xcb]), xpad(|_| {}));
        assert_eq!(c.brake(), 7);
    }

    /// gilrs' scaling of a raw axis value in `min..=max` to −1..=1 (gilrs 0.11 `axis_value`,
    /// before its Y flip), to build synthetic readings from the evdev numbers a pad reports.
    fn gilrs_axis(raw: i32, min: i32, max: i32) -> f32 {
        let (mut range, mut v) = ((max - min) as f32, (raw - min) as f32);
        if (max - min) % 2 == 1 {
            range += 1.0;
            v += 1.0;
        }
        (v / range * 2.0 - 1.0).clamp(-1.0, 1.0)
    }

    /// The Xbox Wireless Controller on Bluetooth (045e:0b13 v0521, measured with evtest): no
    /// SDL mapping in gilrs' database, so gilrs names the kernel codes itself.
    fn xbox_bt(f: impl FnOnce(&mut PadReading)) -> GenericInputs {
        use gilrs::{Axis, Button};
        let stick = |raw| Some(gilrs_axis(raw, 0, 65535));
        let mut r = PadReading {
            sdl_mapping: false,
            vendor: Some(VENDOR_MICROSOFT),
            evdev: true,
            buttons: [
                Button::South,
                Button::East,
                Button::North,
                Button::West,
                Button::LeftTrigger,
                Button::RightTrigger,
                // BTN_TL2/TR2 are listed but never fire.
                Button::LeftTrigger2,
                Button::RightTrigger2,
                Button::Select,
                Button::Start,
            ]
            .map(|b| (b, 0.0))
            .to_vec(),
            // Resting values from the capture: sticks off centre, triggers at 0.
            axes: vec![
                (Axis::LeftStickX, stick(32136)),
                (Axis::LeftStickY, Some(-gilrs_axis(32767, 0, 65535))),
                (Axis::LeftZ, stick(34354)),
                (Axis::RightZ, stick(33319)),
                (Axis::DPadX, Some(0.0)),
                (Axis::DPadY, Some(0.0)),
            ],
            raw_buttons: Vec::new(),
            raw_axes: vec![
                (evdev::ABS_GAS, gilrs_axis(0, 0, 1023)),
                (evdev::ABS_BRAKE, gilrs_axis(0, 0, 1023)),
            ],
        };
        f(&mut r);
        generic_inputs(&r)
    }

    fn set_axis(r: &mut PadReading, a: gilrs::Axis, v: f32) {
        r.axes.iter_mut().find(|(n, _)| *n == a).unwrap().1 = Some(v);
    }

    fn set_raw(r: &mut PadReading, code: u32, v: f32) {
        r.raw_axes.iter_mut().find(|(c, _)| *c == code).unwrap().1 = v;
    }

    fn press(r: &mut PadReading, b: gilrs::Button) {
        r.buttons.iter_mut().find(|(n, _)| *n == b).unwrap().1 = 1.0;
    }

    #[test]
    fn bluetooth_xbox_pad_without_a_mapping() {
        use gilrs::{Axis, Button};
        // At rest nothing is held: sticks within a few percent of centre, triggers at 0.
        let rest = xbox_bt(|_| {});
        assert!(rest.left_x.abs() < 0.03 && rest.left_y.abs() < 0.01);
        assert!(rest.right_x.abs() < 0.06 && rest.right_y.abs() < 0.03);
        assert!(rest.lt < 0.01 && rest.rt < 0.01);
        assert_eq!(
            crate::hybrid::ModernPad::default().map(&rest),
            (0, LeverHolds::NONE)
        );
        // LT = ABS_BRAKE, RT = ABS_GAS, 0..1023: travel 0..1 with half at 511.
        let i = xbox_bt(|r| {
            set_raw(r, evdev::ABS_BRAKE, gilrs_axis(1023, 0, 1023));
            set_raw(r, evdev::ABS_GAS, gilrs_axis(511, 0, 1023));
        });
        assert_eq!(i.lt, 1.0);
        assert!((i.rt - 0.5).abs() < 0.001);
        // The right stick is ABS_Z / ABS_RZ (LeftZ / RightZ): RZ at 0 is the stick pushed up.
        let i = xbox_bt(|r| {
            set_axis(r, Axis::LeftZ, gilrs_axis(65535, 0, 65535));
            set_axis(r, Axis::RightZ, gilrs_axis(0, 0, 65535));
        });
        assert!(i.right_x == 1.0 && i.right_y > 0.999);
        // The Z axes are not triggers on this pad.
        assert!(i.lt < 0.01 && i.rt < 0.01);
        // X arrives as BTN_NORTH, Y as BTN_WEST: X is the horn (west), Y the R key (north).
        let i = xbox_bt(|r| press(r, Button::North));
        assert!(i.west && !i.north);
        let i = xbox_bt(|r| press(r, Button::West));
        assert!(i.north && !i.west);
        // LB / RB are the named shoulders, or the raw BTN_TL / BTN_TR.
        assert!(xbox_bt(|r| press(r, Button::LeftTrigger)).lb);
        let i = xbox_bt(|r| {
            r.buttons.retain(|(b, _)| *b != Button::RightTrigger);
            r.raw_buttons.push((evdev::BTN_TR, 1.0));
        });
        assert!(i.rb && !i.lb);
        // The same pad with an SDL mapping (or `SDL_GAMECONTROLLERCONFIG`): no swap.
        let i = xbox_bt(|r| {
            r.sdl_mapping = true;
            press(r, Button::North);
        });
        assert!(i.north && !i.west);
    }

    #[test]
    fn mapped_and_xpad_triggers() {
        use gilrs::{Axis, Button};
        // An SDL mapping (or XInput): analog triggers are buttons with a value, the right
        // stick is named.
        let mapped = PadReading {
            sdl_mapping: true,
            vendor: Some(VENDOR_MICROSOFT),
            evdev: true,
            buttons: vec![(Button::LeftTrigger2, 0.62), (Button::RightTrigger2, 0.1)],
            axes: vec![
                (Axis::RightStickX, Some(-0.7)),
                (Axis::RightStickY, Some(0.4)),
            ],
            ..PadReading::default()
        };
        let i = generic_inputs(&mapped);
        assert_eq!((i.lt, i.rt, i.right_x, i.right_y), (0.62, 0.1, -0.7, 0.4));
        // Linux xpad without a mapping: ABS_Z / ABS_RZ (0..255) are the triggers, resting
        // at −1, and the right stick is RX / RY. Unseen axes count as at rest.
        let xpad = PadReading {
            evdev: true,
            axes: vec![
                (Axis::LeftZ, Some(gilrs_axis(255, 0, 255))),
                (Axis::RightZ, None),
                (Axis::RightStickX, Some(0.0)),
                (Axis::RightStickY, Some(0.0)),
            ],
            ..PadReading::default()
        };
        let i = generic_inputs(&xpad);
        assert_eq!((i.lt, i.rt), (1.0, 0.0));
        assert!(trigger_travel_from_axis(gilrs_axis(0, 0, 255)) < 0.01);
        assert_eq!(trigger_travel_from_axis(-1.0), 0.0);
        assert_eq!(trigger_travel_from_axis(0.0), 0.5);
    }

    #[test]
    fn hybrid_sticks_and_dpad_never_double_step() {
        use crate::hybrid::tests::{BRAKE_PULL, PULL};
        let mut c = hybrid(0, None);
        c.keyboard.brake = 0;
        // D-pad ↓ and the left stick pulled in the same tick: one power step; held together
        // they repeat on one counter (steps on 1, 9, 12).
        for _ in 0..12 {
            c.tick_hybrid(
                keys(&[]),
                xpad(|i| {
                    i.dpad_down = true;
                    i.left_y = PULL;
                }),
            );
        }
        assert_eq!(c.power(), 3);
        // The stick taking over from the d-pad mid-hold continues the hold.
        c.tick_hybrid(keys(&[]), xpad(|_| {}));
        c.keyboard.power = 0;
        for t in 1..=12 {
            c.tick_hybrid(
                keys(&[]),
                xpad(|i| {
                    i.dpad_down = t <= 4;
                    i.left_y = if t >= 3 { PULL } else { 0.0 };
                }),
            );
        }
        assert_eq!(c.power(), 3);
        // Brake: d-pad → and the right stick pulled together, one step; the stick's
        // hysteresis keeps a wobble around half travel from stepping again.
        c.tick_hybrid(keys(&[]), xpad(|_| {}));
        for y in [1.0, 0.6, 0.45, 0.55, 0.4, 0.5] {
            c.tick_hybrid(
                keys(&[]),
                xpad(|i| {
                    i.dpad_right = y == 1.0;
                    i.right_y = y * BRAKE_PULL;
                }),
            );
        }
        assert_eq!(c.brake(), 1);
        // Released and pulled again: the next step. Pushed up: release.
        c.tick_hybrid(keys(&[]), xpad(|_| {}));
        c.tick_hybrid(keys(&[]), xpad(|i| i.right_y = 0.9 * BRAKE_PULL));
        assert_eq!(c.brake(), 2);
        c.tick_hybrid(keys(&[]), xpad(|_| {}));
        c.tick_hybrid(keys(&[]), xpad(|i| i.right_y = -0.9 * BRAKE_PULL));
        assert_eq!(c.brake(), 1);
        // The left stick's → is not the brake any more.
        c.tick_hybrid(keys(&[]), xpad(|_| {}));
        c.tick_hybrid(keys(&[]), xpad(|i| i.left_x = 1.0));
        assert_eq!((c.power(), c.brake()), (3, 1));
    }

    #[test]
    fn hybrid_pad_on_the_one_handle_and_direct_modes() {
        // Mode 3: the pad's brake+ moves the one handle toward the brake side.
        let mut c = hybrid(3, None);
        c.keyboard.brake = 0;
        c.tick_hybrid(keys(&[]), xpad(|i| i.lt = 1.0));
        c.tick_hybrid(keys(&[]), xpad(|_| {}));
        assert_eq!((c.power(), c.brake()), (1, 0));
        c.tick_hybrid(keys(&[]), xpad(|i| i.rt = 1.0));
        c.tick_hybrid(keys(&[]), xpad(|_| {}));
        c.tick_hybrid(keys(&[]), xpad(|i| i.rt = 1.0));
        assert_eq!((c.power(), c.brake()), (0, 1));
        // Mode 2: the pad steps the power notch; the digits take it back when they change.
        let mut c = hybrid(2, None);
        c.tick_hybrid(keys(&[]), xpad(|i| i.lt = 1.0));
        c.tick_hybrid(keys(&[]), xpad(|_| {}));
        c.tick_hybrid(keys(&[]), xpad(|i| i.lt = 1.0));
        assert_eq!(c.power(), 2);
        c.tick_hybrid(keys(&[]), xpad(|_| {}));
        assert_eq!(c.power(), 2, "no digit held is not a change");
        c.tick_hybrid(keys(&[0x05]), xpad(|_| {}));
        assert_eq!(c.power(), 4);
        c.tick_hybrid(keys(&[]), xpad(|_| {}));
        assert_eq!(c.power(), 0, "letting the digit go is N, as the original");
    }

    #[test]
    fn hybrid_handle_last_used_wins() {
        // Mode 6 handle at P2 (y = -310) and B0.
        let mut c = hybrid(0, Some(JoyMode::TwoHandle));
        c.tick_hybrid(keys(&[]), handle(0, -310));
        assert_eq!((c.power(), c.brake()), (2, 0));
        // Keyboard ↓ steps from the handle's notch and takes over.
        c.tick_hybrid(keys(&[DIK_DOWN]), handle(0, -310));
        assert_eq!(c.power(), 3);
        // Jitter inside the deadband: the stepped notch stays.
        for y in [-330, -290, -355, -265] {
            c.tick_hybrid(keys(&[]), handle(0, y));
            assert_eq!(c.power(), 3, "y = {y}");
        }
        // The handle moves past the deadband: the notch snaps to it.
        c.tick_hybrid(keys(&[]), handle(0, -102));
        assert_eq!(c.power(), 1);
        // Brake: a key step, then the handle pulled to B8 (a snap) and on to EB: EB only
        // after the debounce.
        c.tick_hybrid(keys(&[0xcd]), handle(0, -102));
        assert_eq!(c.brake(), 1);
        c.tick_hybrid(keys(&[]), handle(392, -102));
        assert_eq!(c.brake(), 8);
        let seen: Vec<u8> = (0..7)
            .map(|_| {
                c.tick_hybrid(keys(&[]), handle(426, -102));
                c.brake()
            })
            .collect();
        assert_eq!(seen, [8, 8, 8, 8, 8, 9, 9]);
        // Station start: the handle is authoritative again, at once.
        c.tick_hybrid(keys(&[0xcb]), handle(426, -102));
        assert_eq!(c.brake(), 8);
        c.reset_levers();
        assert_eq!(c.brake(), EB);
        // A pad's stepped levers are not a handle: no takeover by stick noise in mode 4.
        let mut c = hybrid(0, None);
        c.tick_hybrid(keys(&[DIK_UP]), xpad(|i| i.left_x = 0.3));
        assert_eq!((c.power(), c.brake()), (0, EB));
    }

    #[test]
    fn hybrid_unplug_and_hint_device() {
        let mut c = hybrid(0, None);
        assert_eq!(c.hint_device(), Some(HintDevice::Keyboard));
        let b = c.tick_hybrid(keys(&[]), xpad(|i| i.south = true));
        assert!(b.confirm_pressed);
        assert_eq!(c.hint_device(), Some(HintDevice::Pad));
        // Unplugged with A held: one release, no stale edges; hints back to the keyboard.
        let b = c.tick_hybrid(keys(&[]), None);
        assert_eq!(b.word.pressed, 0);
        assert_ne!(b.word.released & bits::A, 0);
        assert_eq!(c.tick_hybrid(keys(&[]), None).word, ButtonWord::default());
        assert_eq!(c.hint_device(), Some(HintDevice::Keyboard));
        // Plugged in again with the stick resting off centre: no switch until it is used.
        c.tick_hybrid(keys(&[]), xpad(|i| i.left_x = 0.2));
        c.tick_hybrid(keys(&[]), xpad(|i| i.left_x = 0.3));
        assert_eq!(c.hint_device(), Some(HintDevice::Keyboard));
        c.tick_hybrid(keys(&[]), xpad(|i| i.left_x = 0.8));
        assert_eq!(c.hint_device(), Some(HintDevice::Pad));
        c.tick_hybrid(keys(&[DIK_RETURN]), xpad(|i| i.left_x = 0.8));
        assert_eq!(c.hint_device(), Some(HintDevice::Keyboard));
        // The override (headless shots) wins.
        c.hint_override = Some(HintDevice::Pad);
        assert_eq!(c.hint_device(), Some(HintDevice::Pad));
    }

    /// `hybrid_input` off keeps the original's one device: no hint device, and the keyboard
    /// poll with no other device is the plain poll.
    #[test]
    fn hybrid_off_is_the_old_path() {
        use crate::input::tests::bindings;
        let c = Controls {
            keyboard: Keyboard::new(&bindings(), 0).unwrap(),
            joystick: None,
            hybrid: None,
            hint_override: Some(HintDevice::Pad),
            scripted: None,
            last_pad: None,
        };
        assert_eq!(c.hint_device(), None);
        let mut a = Keyboard::new(&bindings(), 0).unwrap();
        let mut b = a.clone();
        a.brake = 0;
        b.brake = 0;
        for t in 0..40u8 {
            let k = [DIK_DOWN, 0xcd, DIK_SPACE][usize::from(t / 14)];
            let down = |c: u8| c == k && t % 17 != 0;
            assert_eq!(a.poll(down), b.poll_with(down, LeverHolds::NONE));
            assert_eq!((a.power, a.brake), (b.power, b.brake));
        }
    }

    #[test]
    fn sysfs_ff_capabilities() {
        // The Bluetooth Xbox pad (hid-microsoft through ff-memless, out/pad/xbox-bt-evtest.txt):
        // FF_RUMBLE 0x50, PERIODIC 0x51, SQUARE 0x58, TRIANGLE 0x59, SINE 0x5a, GAIN 0x60.
        let mask = if usize::BITS == 64 {
            "107030000 0"
        } else {
            "1 7030000 0 0"
        };
        for bit in [0x50, 0x51, 0x58, 0x59, 0x5a, 0x60] {
            assert!(sysfs_bit(mask, bit), "{bit:#x}");
        }
        for bit in [0, 0x52, 0x5b, 0x61, 0x7f, 0x200] {
            assert!(!sysfs_bit(mask, bit), "{bit:#x}");
        }
        assert!(!sysfs_bit("", 0x50));
    }

    #[test]
    fn tables_from_exe() {
        let Some(dir) = std::env::var_os("SHUPPATSU_DATA_DIR") else {
            return;
        };
        let path =
            std::path::Path::new(&dir).join("install/Program_Executable_Files/BIN/dengo3k.exe");
        let Ok(bytes) = std::fs::read(path) else {
            return;
        };
        let exe = Exe::parse(&bytes).unwrap();
        assert_eq!(JoyTables::read(&exe).unwrap(), tables());
    }
}

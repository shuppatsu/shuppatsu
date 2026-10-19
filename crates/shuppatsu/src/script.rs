//! Input scripts (`--inputs <file>`): scripted keys, notches and gamepad samples for headless
//! runs, and the [`InputLog`] that records what a live session's polls consumed, as such a
//! script (issue marks, docs/marks.md).
//!
//! The format, one directive per line (`#` starts a comment):
//!
//! - `<tick> <notch> <brake>`: the oracle's notches, held from drive tick `tick` on.
//! - `key <tick>[-<last>] <name>`: the key is down for the poll at the end of tick `tick`
//!   (through `last`). Names: [`KEY_NAMES`].
//! - `pad <tick> off` / `pad <tick> [<field>…]`: the gamepad as the poll at the end of `tick`
//!   reads it, held until the next `pad` line; `off` = no pad. Fields left out are at rest:
//!   `lx= ly= rx= ry= lt= rt=` (axis values, sticks +Y up, triggers 0..1), the buttons `a b x
//!   y lb rb view menu up down left right` (Xbox names, d-pad as up/down/left/right), and
//!   `joy=<x>,<y>,<buttons hex>[,<power>,<brake>]` (the DirectInput state the pad gives the
//!   joystick decoder; `-` for a notch the device leaves alone). A script with `pad` lines
//!   replaces the real gamepad; tick 0 is its state at the start (connected or not).
//! - `hint <pad|keyboard>`: the device the button hints follow at the start.
//!
//! Ticks are play ticks in `shuppatsu play` and app ticks (a run's ticks included) in
//! `shuppatsu`.

use crate::gamepad::{GenericInputs, JoyState, PadSample};
use crate::hybrid::HintDevice;
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Scripted inputs (see the [module docs](self)).
#[derive(Debug, Default)]
pub struct Script {
    /// `(drive tick, notch, brake)`, sorted.
    pub notches: Vec<(u64, u8, u8)>,
    /// `(tick, DIK code)`: single-tick presses added by code (tests); the file's `key` lines
    /// are [`Script::holds`].
    pub keys: Vec<(u64, u8)>,
    /// Per DIK code, the tick ranges (inclusive, sorted) it is held over.
    holds: BTreeMap<u8, Vec<(u64, u64)>>,
    /// `(tick, sample)`, sorted: the scripted gamepad from each tick on.
    pad: Vec<(u64, Option<PadSample>)>,
    /// `hint`: the hint device at the start.
    pub hint: Option<HintDevice>,
}

/// The script's key names and their DirectInput codes: every key a keyboard mode reads
/// ([`crate::input::dik`]).
pub const KEY_NAMES: &[(&str, u8)] = &[
    ("esc", 0x01),
    ("1", 0x02),
    ("2", 0x03),
    ("3", 0x04),
    ("4", 0x05),
    ("5", 0x06),
    ("r", 0x13),
    ("enter", 0x1c),
    ("a", 0x1e),
    ("lshift", 0x2a),
    ("z", 0x2c),
    ("rshift", 0x36),
    ("space", 0x39),
    ("kp7", 0x47),
    ("kp8", 0x48),
    ("kp9", 0x49),
    ("kp4", 0x4b),
    ("kp5", 0x4c),
    ("kp6", 0x4d),
    ("kp1", 0x4f),
    ("kp2", 0x50),
    ("kp3", 0x51),
    ("kp0", 0x52),
    ("kpenter", 0x9c),
    ("up", 0xc8),
    ("left", 0xcb),
    ("right", 0xcd),
    ("down", 0xd0),
];

fn key_code(name: &str) -> Option<u8> {
    KEY_NAMES.iter().find(|(n, _)| *n == name).map(|&(_, c)| c)
}

fn key_name(code: u8) -> Option<&'static str> {
    KEY_NAMES.iter().find(|&&(_, c)| c == code).map(|&(n, _)| n)
}

impl Script {
    /// The keys down for the poll at the end of `tick`, by DIK code.
    pub fn down_at(&self, tick: u64) -> [bool; 256] {
        let mut down = [false; 256];
        for (&code, ranges) in &self.holds {
            let i = ranges.partition_point(|&(from, _)| from <= tick);
            if i > 0 && ranges[i - 1].1 >= tick {
                down[usize::from(code)] = true;
            }
        }
        for &(t, code) in &self.keys {
            if t == tick {
                down[usize::from(code)] = true;
            }
        }
        down
    }

    /// Whether the script drives the gamepad (it has `pad` lines).
    pub fn has_pad(&self) -> bool {
        !self.pad.is_empty()
    }

    /// The scripted pad for the poll at the end of `tick` (`None`: no pad then).
    pub fn pad_at(&self, tick: u64) -> Option<PadSample> {
        let i = self.pad.partition_point(|&(t, _)| t <= tick);
        i.checked_sub(1).and_then(|i| self.pad[i].1)
    }

    /// Holds `code` from `from` through `to` (inclusive).
    pub fn hold(&mut self, from: u64, to: u64, code: u8) {
        let ranges = self.holds.entry(code).or_default();
        let i = ranges.partition_point(|&(f, _)| f <= from);
        ranges.insert(i, (from, to));
    }
}

/// Parses the input script.
pub fn parse_script(text: &str) -> Result<Script> {
    let mut s = Script::default();
    for (n, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let v: Vec<&str> = line.split_whitespace().collect();
        let at = || format!("line {}", n + 1);
        match v[..] {
            ["key", t, k] => {
                let code = key_code(k).with_context(|| format!("{}: unknown key {k}", at()))?;
                // `key <from>-<to> name` holds the key over a range of ticks.
                let (from, to) = t.split_once('-').unwrap_or((t, t));
                let from = from.parse().with_context(at)?;
                let to = to.parse().with_context(at)?;
                s.hold(from, to, code);
            }
            ["pad", t, ref fields @ ..] => {
                let sample = parse_pad(fields).with_context(at)?;
                s.pad.push((t.parse().with_context(at)?, sample));
            }
            ["hint", d] => {
                s.hint = Some(HintDevice::parse(d).with_context(|| format!("{}: hint {d}", at()))?)
            }
            [t, p, b] => s.notches.push((
                t.parse().with_context(at)?,
                p.parse().with_context(at)?,
                b.parse().with_context(at)?,
            )),
            _ => bail!(
                "{}: expected `tick notch brake`, `key tick name`, `pad tick …` or `hint device`",
                at()
            ),
        }
    }
    s.notches.sort_by_key(|s| s.0);
    s.pad.sort_by_key(|s| s.0);
    Ok(s)
}

/// The buttons of a `pad` line and their [`GenericInputs`] fields.
const PAD_BUTTONS: [&str; 12] = [
    "a", "b", "x", "y", "lb", "rb", "view", "menu", "up", "down", "left", "right",
];

fn pad_button<'a>(i: &'a mut GenericInputs, name: &str) -> Option<&'a mut bool> {
    Some(match name {
        "a" => &mut i.south,
        "b" => &mut i.east,
        "x" => &mut i.west,
        "y" => &mut i.north,
        "lb" => &mut i.lb,
        "rb" => &mut i.rb,
        "view" => &mut i.select,
        "menu" => &mut i.start,
        "up" => &mut i.dpad_up,
        "down" => &mut i.dpad_down,
        "left" => &mut i.dpad_left,
        "right" => &mut i.dpad_right,
        _ => return None,
    })
}

fn pad_axis<'a>(i: &'a mut GenericInputs, name: &str) -> Option<&'a mut f32> {
    Some(match name {
        "lx" => &mut i.left_x,
        "ly" => &mut i.left_y,
        "rx" => &mut i.right_x,
        "ry" => &mut i.right_y,
        "lt" => &mut i.lt,
        "rt" => &mut i.rt,
        _ => return None,
    })
}

const PAD_AXES: [&str; 6] = ["lx", "ly", "rx", "ry", "lt", "rt"];

fn parse_pad(fields: &[&str]) -> Result<Option<PadSample>> {
    if fields == ["off"] {
        return Ok(None);
    }
    let mut inputs = GenericInputs::default();
    let mut joy = JoyState::default();
    for f in fields {
        match f.split_once('=') {
            Some(("joy", v)) => joy = parse_joy(v).with_context(|| format!("joy={v}"))?,
            Some((axis, v)) => {
                *pad_axis(&mut inputs, axis).with_context(|| format!("unknown axis {axis}"))? =
                    v.parse().with_context(|| format!("{axis}={v}"))?;
            }
            None => {
                *pad_button(&mut inputs, f).with_context(|| format!("unknown button {f}"))? = true
            }
        }
    }
    Ok(Some((joy, inputs)))
}

fn parse_joy(v: &str) -> Result<JoyState> {
    let parts: Vec<&str> = v.split(',').collect();
    let notch =
        |s: &str| -> Result<Option<u8>> { Ok(if s == "-" { None } else { Some(s.parse()?) }) };
    let (x, y, buttons) = match parts[..] {
        [x, y, b, ..] => (x, y, b),
        _ => bail!("expected x,y,buttons"),
    };
    let notches = match parts[3..] {
        [] => None,
        [p, b] => Some(crate::gamepad::DirectNotches {
            power: notch(p)?,
            brake: notch(b)?,
        }),
        _ => bail!("expected x,y,buttons[,power,brake]"),
    };
    Ok(JoyState {
        x: x.parse()?,
        y: y.parse()?,
        buttons: u32::from_str_radix(buttons.trim_start_matches("0x"), 16)?,
        notches,
    })
}

/// A pad sample as the fields of a `pad` line (`off` for none). Axis values are written in
/// full (`f32`'s shortest round-trip form), so a replay reads back the same bits.
pub fn pad_text(sample: Option<&PadSample>) -> String {
    let Some((joy, inputs)) = sample else {
        return "off".to_owned();
    };
    let mut i = *inputs;
    let mut out = Vec::new();
    for a in PAD_AXES {
        let v = *pad_axis(&mut i, a).expect("an axis");
        if v.to_bits() != 0 {
            out.push(format!("{a}={v}"));
        }
    }
    for b in PAD_BUTTONS {
        if *pad_button(&mut i, b).expect("a button") {
            out.push(b.to_owned());
        }
    }
    if *joy != JoyState::default() {
        let mut j = format!("joy={},{},{:#x}", joy.x, joy.y, joy.buttons);
        if let Some(n) = joy.notches {
            let s = |v: Option<u8>| v.map_or("-".to_owned(), |v| v.to_string());
            let _ = write!(j, ",{},{}", s(n.power), s(n.brake));
        }
        out.push(j);
    }
    if out.is_empty() {
        // Connected, at rest.
        out.push("lx=0".to_owned());
    }
    out.join(" ")
}

/// What a session's polls consumed, as edges: a key going down or up, the pad changing.
/// Small: a two-hour run of key presses is a few thousand entries.
#[derive(Debug, Default, Clone)]
pub struct InputLog {
    /// `(tick, DIK code, down)`.
    keys: Vec<(u64, u8, bool)>,
    /// `(tick, sample)` whenever the pad's sample changed.
    pads: Vec<(u64, Option<PadSample>)>,
    held: [u64; 4],
    pad: Option<PadSample>,
    /// Whether View was held at the last poll (a mark is its press edge).
    view: bool,
}

impl InputLog {
    /// The session starts: the pad is connected (`true`) or not before the first poll.
    pub fn start(&mut self, pad_connected: bool) {
        if pad_connected {
            let at_rest = Some(PadSample::default());
            self.pads.push((0, at_rest));
            self.pad = at_rest;
        }
    }

    /// The poll at the end of `tick` read the keys `down` and the pad sample `pad`. Returns
    /// true on the poll where the pad's View button (the two squares) goes down: that
    /// press saves an issue mark.
    pub fn record(&mut self, tick: u64, down: &[bool; 256], pad: Option<PadSample>) -> bool {
        for (code, &d) in down.iter().enumerate() {
            let (word, bit) = (code / 64, 1u64 << (code % 64));
            if (self.held[word] & bit != 0) != d {
                self.held[word] ^= bit;
                self.keys.push((tick, code as u8, d));
            }
        }
        if pad != self.pad {
            self.pads.push((tick, pad));
            self.pad = pad;
        }
        let view = pad.is_some_and(|(_, i)| i.select);
        let pressed = view && !self.view;
        self.view = view;
        pressed
    }

    /// The recording from after tick `from` through `to` as a script whose tick 1 is
    /// `from + 1` (`from` 0: the session's own ticks). Keys held at `from` start at tick 1;
    /// the pad's state at `from` is its tick 0. `hint`: the hint device at `from`.
    pub fn script(&self, from: u64, to: u64, hint: Option<HintDevice>) -> String {
        let mut out = String::new();
        if let Some(h) = hint {
            let _ = writeln!(
                out,
                "hint {}",
                match h {
                    HintDevice::Pad => "pad",
                    HintDevice::Keyboard => "keyboard",
                }
            );
        }
        // Key holds: (start, end, code), in start order.
        let mut open: BTreeMap<u8, u64> = BTreeMap::new();
        let mut holds: Vec<(u64, u64, u8)> = Vec::new();
        for &(t, code, down) in self.keys.iter().take_while(|e| e.0 <= to) {
            let t = t.max(from + 1);
            if down {
                open.insert(code, t);
            } else if let Some(start) = open.remove(&code)
                && t > start
            {
                holds.push((start, t - 1, code));
            }
        }
        holds.extend(open.into_iter().map(|(code, start)| (start, to, code)));
        holds.retain(|h| h.1 > from);
        holds.sort_unstable();
        let pad_at_from = self
            .pads
            .iter()
            .take_while(|p| p.0 <= from)
            .last()
            .map(|p| p.1);
        let mut pads: Vec<(u64, Option<PadSample>)> = pad_at_from
            .map(|p| (from, p))
            .into_iter()
            .chain(
                self.pads
                    .iter()
                    .copied()
                    .filter(|p| p.0 > from && p.0 <= to),
            )
            .collect();
        // A pad that is only ever absent needs no lines.
        if pads.iter().all(|p| p.1.is_none()) {
            pads.clear();
        }
        let rel = |t: u64| t - from;
        let (mut k, mut p) = (holds.iter().peekable(), pads.iter().peekable());
        loop {
            let next_key = k.peek().map(|h| h.0);
            let next_pad = p.peek().map(|e| e.0);
            let key_first = match (next_key, next_pad) {
                (None, None) => break,
                (Some(a), Some(b)) => a < b,
                (k, _) => k.is_some(),
            };
            match key_first {
                true => {
                    let (s, e, code) = *k.next().expect("peeked");
                    let name = key_name(code).unwrap_or("?");
                    if s == e {
                        let _ = writeln!(out, "key {} {name}", rel(s));
                    } else {
                        let _ = writeln!(out, "key {}-{} {name}", rel(s), rel(e));
                    }
                }
                false => {
                    let (t, sample) = p.next().expect("peeked");
                    let _ = writeln!(out, "pad {} {}", rel(*t), pad_text(sample.as_ref()));
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_ranges_and_lookup() {
        let s = parse_script("key 5-7 up # held\nkey 9 enter\n10 2 0\n").unwrap();
        let down = |t| s.down_at(t);
        assert!(!down(4)[0xc8] && down(5)[0xc8] && down(7)[0xc8] && !down(8)[0xc8]);
        assert!(down(9)[0x1c] && !down(10)[0x1c]);
        assert_eq!(s.notches, [(10, 2, 0)]);
        assert!(!s.has_pad());
    }

    #[test]
    fn every_key_name_parses() {
        for &(name, code) in KEY_NAMES {
            let s = parse_script(&format!("key 1 {name}")).unwrap();
            assert!(s.down_at(1)[usize::from(code)], "{name}");
            assert_eq!(key_name(code), Some(name), "one name per code");
        }
        // Every key a keyboard mode reads has a name.
        use winit::keyboard::KeyCode::*;
        for k in [
            Escape,
            Digit1,
            Digit2,
            Digit3,
            Digit4,
            Digit5,
            KeyR,
            Enter,
            KeyA,
            ShiftLeft,
            KeyZ,
            ShiftRight,
            Space,
            Numpad7,
            Numpad8,
            Numpad9,
            Numpad4,
            Numpad5,
            Numpad6,
            Numpad1,
            Numpad2,
            Numpad3,
            Numpad0,
            NumpadEnter,
            ArrowUp,
            ArrowLeft,
            ArrowRight,
            ArrowDown,
        ] {
            let code = crate::input::dik(k).unwrap();
            assert!(key_name(code).is_some(), "{k:?}");
        }
    }

    fn sample() -> PadSample {
        let inputs = GenericInputs {
            left_y: -0.512_345_7,
            lt: 1.0,
            north: true,
            select: true,
            dpad_up: true,
            ..Default::default()
        };
        let joy = JoyState {
            x: -1000,
            y: 512,
            buttons: 0x101,
            notches: Some(crate::gamepad::DirectNotches {
                power: Some(3),
                brake: None,
            }),
        };
        (joy, inputs)
    }

    #[test]
    fn pad_lines_round_trip_exactly() {
        let s = sample();
        let text = pad_text(Some(&s));
        let parsed = parse_script(&format!("pad 3 {text}\npad 8 off\npad 0 lx=0")).unwrap();
        assert_eq!(parsed.pad_at(3), Some(s), "{text}");
        assert_eq!(parsed.pad_at(7), Some(s));
        assert_eq!(parsed.pad_at(8), None);
        assert_eq!(parsed.pad_at(1), Some(PadSample::default()));
        assert_eq!(pad_text(Some(&PadSample::default())), "lx=0");
    }

    /// The log's script reads back as the polls it recorded, over the whole session and from
    /// a cut (a run's start).
    #[test]
    fn log_replays_as_recorded() {
        let mut log = InputLog::default();
        log.start(false);
        let mut polls = Vec::new();
        for t in 1..=200u64 {
            let mut down = [false; 256];
            down[0xc8] = (10..=40).contains(&t);
            down[0x1c] = t == 50 || t == 51;
            down[0x39] = t >= 150;
            let pad = match t {
                0..=99 => None,
                100..=120 => Some(PadSample::default()),
                _ => Some(sample()),
            };
            log.record(t, &down, pad);
            polls.push((t, down, pad));
        }
        for from in [0, 30, 120, 160] {
            let s = parse_script(&log.script(from, 200, None)).unwrap();
            for &(t, down, pad) in polls.iter().filter(|p| p.0 > from) {
                let r = t - from;
                assert_eq!(s.down_at(r), down, "from {from}, tick {t}");
                assert_eq!(s.pad_at(r), pad, "from {from}, tick {t}");
            }
        }
        // Up to the mark only.
        let s = parse_script(&log.script(0, 45, None)).unwrap();
        assert!(s.down_at(40)[0xc8] && !s.down_at(50)[0x1c]);
    }

    #[test]
    fn view_press_fires_once() {
        let mut log = InputLog::default();
        let (joy, mut rest) = sample();
        rest.select = false;
        let mut view = rest;
        view.select = true;
        // Held over ticks 10..=40, pressed again at 60.
        let fired: Vec<u64> = (1..=100)
            .filter(|&t| {
                let i = if (10..=40).contains(&t) || t == 60 {
                    view
                } else {
                    rest
                };
                log.record(t, &[false; 256], Some((joy, i)))
            })
            .collect();
        assert_eq!(fired, [10, 60]);
    }

    #[test]
    fn hint_line() {
        let log = InputLog::default();
        let s = parse_script(&log.script(0, 10, Some(HintDevice::Pad))).unwrap();
        assert_eq!(s.hint, Some(HintDevice::Pad));
    }
}

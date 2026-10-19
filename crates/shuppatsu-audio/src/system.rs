//! The game-thread side of the sound system (audio-text.md §1, §5).
//!
//! The original game thread never touches DirectSound: it posts messages to the sound thread
//! (0x430100–0x4301e0) and runs the phrase sequencer (0x41ff60) itself. [`SoundSystem`] does the
//! same: every call appends [`Command`]s that the caller forwards to a [`crate::Mixer`] (directly,
//! or through [`crate::AudioOutput`]). Everything here is deterministic and tick-driven.

use crate::code::SEQUENCE_FLAG;
use crate::mixer::Command;
use crate::tables::{SoundTables, Step};
use crate::voice::{Origin, Started, VoiceEvent};
use std::sync::Arc;

/// Sequence slots (0x84fb00, 4 × 0x38 bytes).
pub const SEQUENCE_SLOTS: usize = 4;
/// Steps copied into a slot; a 13th entry holds the terminator (0x41fdd0 copies at most 12).
pub const SEQUENCE_STEPS: usize = 12;
/// Sequencer ticks per game tick: 0x430210 runs the sequencer twice per call, once per 33 ms
/// tick (from PumpMessages 0x42a130), so delays count in ~1/60.6 s.
pub const SEQUENCE_TICKS_PER_FRAME: usize = 2;

/// Phrase table of the "prefix + speed" call (0x420180). The original builds it on the stack:
/// seven (code, delay) prefixes, and the speed phrases start at element 12, so speed index 0 is
/// the last prefix's code 0xc3 and index 1 its delay (−1 = no speed phrase). Element 12 + n is
/// the phrase for `kmh / 5 = n`, capped at 26 (no phrase).
const PREFIX_SPEED_TABLE: [i16; 39] = [
    0xc2, 0x22, 0xa8, 0x28, 0xa9, 0x24, 0xaa, 0x26, 0xab, 0x26, 0xc1, 0x22, 0xc3, -1, 0xae, 0xaf,
    0xb0, 0xb1, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xbb, 0xc4, 0xc5, 0xc6, 0xc7,
    0xc8, 0xc9, 0xca, 0xcb, 0xcc, 0xcd, -1,
];
const PREFIX_COUNT: usize = 7;
/// Voice events kept unread at most ([`SoundSystem::take_voice_events`]).
const VOICE_LOG_MAX: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SeqSlot {
    /// Current step, −1 = free.
    step: i16,
    counter: i16,
    steps: [Step; SEQUENCE_STEPS + 1],
}

impl SeqSlot {
    const FREE: Self = Self {
        step: -1,
        counter: 0,
        steps: [Step::END; SEQUENCE_STEPS + 1],
    };
}

/// Game-side sound state: request gating, pause, and the four phrase-sequence slots.
#[derive(Debug, Clone)]
pub struct SoundSystem {
    tables: Arc<SoundTables>,
    /// Audio device available (0x816ae8). When false every request is a no-op.
    enabled: bool,
    /// Pause flag (0x816998): blocks new sounds and the sequencer.
    paused: bool,
    /// Sequencer enable (0x84faf8): when false, starts are ignored and running sequences end.
    sequences_enabled: bool,
    slots: [SeqSlot; SEQUENCE_SLOTS],
    commands: Vec<Command>,
    /// Port addition for subtitles: record [`VoiceEvent`]s ([`SoundSystem::set_voice_log`]).
    log_voices: bool,
    voices: Vec<VoiceEvent>,
}

impl SoundSystem {
    /// A sound system with audio enabled, sequencer enabled (as after 0x41fd50).
    pub fn new(tables: Arc<SoundTables>) -> Self {
        Self {
            tables,
            enabled: true,
            paused: false,
            sequences_enabled: true,
            slots: [SeqSlot::FREE; SEQUENCE_SLOTS],
            commands: Vec::new(),
            log_voices: false,
            voices: Vec::new(),
        }
    }

    pub fn tables(&self) -> &SoundTables {
        &self.tables
    }

    /// Enables or disables all sound (the original disables it when DirectSound fails).
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Takes the commands produced since the last call, in order.
    pub fn take_commands(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.commands)
    }

    /// Like [`Self::take_commands`] but reuses the internal buffer.
    pub fn drain_commands(&mut self) -> std::vec::Drain<'_, Command> {
        self.commands.drain(..)
    }

    /// Port addition for subtitles: from now on, record a [`VoiceEvent`] for every single
    /// sound requested and every sequence started, and for every stop-all and sequencer
    /// shutdown ([`SoundSystem::take_voice_events`]). Off by default.
    pub fn set_voice_log(&mut self, on: bool) {
        self.log_voices = on;
        if !on {
            self.voices.clear();
        }
    }

    /// Takes the voice events recorded since the last call, in order.
    pub fn take_voice_events(&mut self) -> Vec<VoiceEvent> {
        std::mem::take(&mut self.voices)
    }

    fn log(&mut self, e: VoiceEvent) {
        if self.log_voices && self.enabled {
            // Bounded, for owners that stop reading for a while (menus, attract).
            if self.voices.len() >= VOICE_LOG_MAX {
                self.voices.remove(0);
            }
            self.voices.push(e);
        }
    }

    fn post(&mut self, cmd: Command) {
        if self.enabled {
            self.commands.push(cmd);
        }
    }

    /// `snd_request` (0x41fda0): `code | 0x1000` starts sequence `code & 0xfff`, anything else
    /// plays the sound.
    pub fn request(&mut self, code: u16) {
        if !self.enabled {
            return;
        }
        if code & SEQUENCE_FLAG != 0 {
            self.start_sequence((code & 0xfff) as usize);
        } else {
            if !self.paused {
                self.log(VoiceEvent::Started(Started {
                    origin: Origin::Code(code),
                    steps: vec![Step::new(code as i16, 0)],
                }));
            }
            self.play(code);
        }
    }

    /// `snd_post_play` (0x430100): plays `code` unless paused.
    pub fn play(&mut self, code: u16) {
        if !self.paused {
            self.post(Command::Play(code));
        }
    }

    /// A play message posted straight to the sound thread, bypassing the pause flag (the pause
    /// dialog's clicks, menus.md §1.4). The mixer plays it even while other voices are held.
    pub fn play_ungated(&mut self, code: u16) {
        self.post(Command::Play(code));
    }

    /// `snd_post_release` (0x430130).
    pub fn release(&mut self, code: u16) {
        self.post(Command::Release(code));
    }

    /// `snd_post_stop_all` (0x430160).
    pub fn stop_all(&mut self) {
        self.log(VoiceEvent::Silenced);
        self.post(Command::StopAll);
    }

    /// `snd_post_pitch` (0x430180).
    pub fn set_pitch(&mut self, code: u16, p: i16) {
        self.post(Command::SetPitch(code, p));
    }

    /// `snd_post_volume` (0x4301b0).
    pub fn set_volume(&mut self, code: u16, v: i16) {
        self.post(Command::SetVolume(code, v));
    }

    /// `snd_post_pause` (0x4301e0).
    pub fn set_paused(&mut self, on: bool) {
        self.paused = on;
        self.post(Command::Pause(on));
    }

    /// Once per 33 ms game tick (0x430210): advances the sequencer twice unless paused.
    pub fn frame(&mut self) {
        if !self.paused {
            for _ in 0..SEQUENCE_TICKS_PER_FRAME {
                self.sequence_tick();
            }
        }
    }

    /// The sequencer enable flag 0x84faf8 (cleared by the stage flow around results screens).
    pub fn set_sequences_enabled(&mut self, on: bool) {
        self.sequences_enabled = on;
    }

    /// Frees every sequence slot without stopping sounds (0x4206b0).
    pub fn clear_sequences(&mut self) {
        self.slots = [SeqSlot::FREE; SEQUENCE_SLOTS];
    }

    /// True if any sequence slot is running.
    pub fn sequence_active(&self) -> bool {
        self.slots.iter().any(|s| s.step >= 0)
    }

    /// Starts table sequence `id` (0x41fdd0). Ignored when no slot is free, the sequencer is
    /// disabled or `id` is outside the table.
    pub fn start_sequence(&mut self, id: usize) {
        let Some(list) = self.tables.sequences.get(id).cloned() else {
            return;
        };
        if list.is_empty() {
            return;
        }
        self.start_steps(&list, Origin::Sequence(id));
    }

    /// Starts a caller-built sequence (0x41fea0); ignored if it is empty or starts with −1.
    pub fn start_sequence_list(&mut self, list: &[Step], origin: Origin) {
        if list.first().is_some_and(|s| s.code != -1) {
            self.start_steps(list, origin);
        }
    }

    fn start_steps(&mut self, list: &[Step], origin: Origin) {
        if !self.enabled || !self.sequences_enabled {
            return;
        }
        let Some(slot) = self.slots.iter().position(|s| s.step < 0) else {
            return;
        };
        let mut steps = [Step::END; SEQUENCE_STEPS + 1];
        for (d, s) in steps.iter_mut().zip(
            list.iter()
                .take_while(|s| s.code != -1)
                .take(SEQUENCE_STEPS),
        ) {
            *d = *s;
        }
        self.slots[slot] = SeqSlot {
            step: 0,
            counter: 0,
            steps,
        };
        self.log(VoiceEvent::Started(Started {
            origin,
            steps: steps.iter().copied().take_while(|s| s.code != -1).collect(),
        }));
        // Step 0 goes through snd_request, so it may itself start a sequence (logged as its
        // own line, not as a single sound).
        let first = steps[0].code as u16;
        if first & SEQUENCE_FLAG != 0 {
            self.request(first);
        } else {
            self.play(first);
        }
    }

    /// One sequencer tick (0x41ff60).
    fn sequence_tick(&mut self) {
        let mut dropped = false;
        for i in 0..SEQUENCE_SLOTS {
            let slot = &mut self.slots[i];
            if slot.step < 0 {
                continue;
            }
            if !self.sequences_enabled {
                slot.step = -1;
                dropped = true;
            } else if slot.counter >= slot.steps[slot.step as usize].delay {
                slot.step += 1;
                slot.counter = 0;
                let next = slot
                    .steps
                    .get(slot.step as usize)
                    .copied()
                    .unwrap_or(Step::END);
                if next.delay == -1 {
                    slot.step = -1;
                } else {
                    // Later steps are played directly (0x42ff20), not through snd_request.
                    self.play(next.code as u16);
                }
            }
            let slot = &mut self.slots[i];
            slot.counter = slot.counter.wrapping_add(1);
        }
        if dropped {
            self.log(VoiceEvent::Silenced);
        }
    }

    /// Two-phrase call (0x41ffe0): phrase `second` of the pair table, then phrase `first`.
    /// Used for "station name + suffix" announcements (0x40c4e0 passes a per-station index as
    /// `second` and 2 or 3 as `first`).
    pub fn call_pair(&mut self, first: usize, second: usize) {
        let pairs = &self.tables.call_pairs;
        let (Some(&a), Some(&b)) = (pairs.get(second), pairs.get(first)) else {
            return;
        };
        self.start_sequence_list(&[a, b, Step::END], Origin::Pair { first, second });
    }

    /// "Prefix + speed" call (0x420180), e.g. a speed-limit call: prefix phrase `prefix`
    /// (0..=6; the game always passes 0), then the number phrase for `kmh` (in 5 km/h steps,
    /// 10..=125; 0 gives phrase 0xc3, other values no number phrase).
    pub fn call_prefix_speed(&mut self, prefix: usize, kmh: i32) {
        if prefix >= PREFIX_COUNT {
            return;
        }
        // Negative speeds would index before the table in the original; clamp them to 0.
        let n = (kmh / 5).clamp(0, 26) as usize;
        let speed = PREFIX_SPEED_TABLE[12 + n];
        let (code, delay) = (
            PREFIX_SPEED_TABLE[2 * prefix],
            PREFIX_SPEED_TABLE[2 * prefix + 1],
        );
        if speed == -1 {
            self.request(code as u16);
        } else {
            self.start_sequence_list(
                &[Step::new(code, delay), Step::new(speed, 0), Step::END],
                Origin::PrefixSpeed { prefix, kmh },
            );
        }
    }

    /// Signal call-out (0x420320): up to three phrases built from code immediates, each part
    /// skipped when its argument is out of range. `prefix` 1..=3 → 0xc1 / 0xc0 / 0xd4 (0x40c010
    /// passes 3 on passing-station routes, 0 = none); `mode` 0..=2 → 0xd1 / 0xc1 / 0xc0 (block,
    /// home, starting signal); `aspect` 0..=6 → 0xa7 0xa8 0xa9 0xaa 0xd2 0xbe 0xc2 (green … red,
    /// then two extra words used for repeaters).
    pub fn call_signal(&mut self, prefix: i32, mode: i32, aspect: i32) {
        let mut list = Vec::with_capacity(4);
        if let Some(&(code, delay)) = usize::try_from(prefix - 1)
            .ok()
            .and_then(|i| SIGNAL_PREFIXES.get(i))
        {
            list.push(Step::new(code, delay));
        }
        if let Some(&(code, delay)) = usize::try_from(mode).ok().and_then(|i| SIGNAL_MODES.get(i)) {
            list.push(Step::new(code, delay));
        }
        if let Some(&code) = usize::try_from(aspect)
            .ok()
            .and_then(|i| SIGNAL_ASPECTS.get(i))
        {
            list.push(Step::new(code, SIGNAL_ASPECT_DELAY));
        }
        list.push(Step::END);
        self.start_sequence_list(
            &list,
            Origin::Signal {
                prefix,
                mode,
                aspect,
            },
        );
    }

    /// Station announcement of course event 2 (0x420040): `name` indexes the second block of
    /// the pair table (entry [`STATION_CALL_BLOCK`] + `name`). Mode 1 says the name twice,
    /// mode 0 "next is <name>, <name>", mode 2 "next is <name> <suffix>"; the "next is" and
    /// suffix phrases depend on the name's code range, and names below 0x112 get the chime
    /// 0x1a4 first. Other modes pass an uninitialised list in the original; here they play
    /// nothing.
    pub fn call_station(&mut self, mode: i32, name: usize) {
        let Some(&station) = name
            .checked_add(STATION_CALL_BLOCK)
            .and_then(|i| self.tables.call_pairs.get(i))
        else {
            return;
        };
        let code = station.code;
        let mut list = Vec::with_capacity(5);
        match mode {
            1 => {
                if code < 0x112 {
                    list.push(Step::new(STATION_CHIME, 0x28));
                }
                list.extend([station, station]);
            }
            0 | 2 => {
                let (intro, suffix) = if code < 0x12d {
                    if code < 0x112 {
                        list.push(Step::new(STATION_CHIME, 0x46));
                    }
                    STATION_INTROS[0]
                } else if code < 0x147 {
                    STATION_INTROS[1]
                } else {
                    STATION_INTROS[2]
                };
                list.push(Step::new(intro, 0x28));
                list.push(station);
                list.push(if mode == 2 {
                    Step::new(suffix, 0)
                } else {
                    station
                });
            }
            _ => return,
        }
        list.push(Step::END);
        self.start_sequence_list(&list, Origin::Station { mode, name });
    }
}

/// 0x420320: optional prefix phrases (argument 1..=3), as (code, delay).
const SIGNAL_PREFIXES: [(i16, i16); 3] = [(0xc1, 0x1e), (0xc0, 0x24), (0xd4, 0x1e)];
/// 0x420320: signal kind phrases (argument 0..=2).
const SIGNAL_MODES: [(i16, i16); 3] = [(0xd1, 0x1e), (0xc1, 0x1e), (0xc0, 0x24)];
/// 0x420320: aspect phrases (argument 0..=6), each with delay 0x1c.
pub const SIGNAL_ASPECTS: [i16; 7] = [0xa7, 0xa8, 0xa9, 0xaa, 0xd2, 0xbe, 0xc2];
const SIGNAL_ASPECT_DELAY: i16 = 0x1c;
/// 0x420040 reads the pair table at 0x46c4d0 = 0x46c368 + 90 entries.
pub const STATION_CALL_BLOCK: usize = 90;
/// The "next" phrase and the closing phrase of 0x420040 modes 0 / 2, as (intro, suffix), for
/// station codes below 0x12d, below 0x147 and the rest (code immediates in 0x420040).
pub const STATION_INTROS: [(i16, i16); 3] = [(0x111, 0x112), (0x147, 0x148), (0x168, 0x169)];
/// BGM-slot chime before low-numbered station names (0x420040).
const STATION_CHIME: i16 = 0x1a4;

#[cfg(test)]
mod tests {
    use super::*;

    fn system() -> SoundSystem {
        let tables = SoundTables {
            sequences: vec![
                vec![Step::new(0x10, 2), Step::new(0x11, 0), Step::new(0x12, 0)],
                vec![Step::new(0x1001, 3)],
                vec![],
            ],
            call_pairs: (0..8).map(|i| Step::new(0x60 + i, 5 + i)).collect(),
            ..SoundTables::default()
        };
        SoundSystem::new(Arc::new(tables))
    }

    /// Runs `frames` game ticks, returning (tick index, code) of every Play.
    fn plays(s: &mut SoundSystem, frames: usize) -> Vec<(usize, u16)> {
        let mut out: Vec<(usize, u16)> = s
            .take_commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Play(code) => Some((0, code)),
                _ => None,
            })
            .collect();
        for f in 1..=frames {
            s.frame();
            out.extend(s.take_commands().into_iter().filter_map(|c| match c {
                Command::Play(code) => Some((f, code)),
                _ => None,
            }));
        }
        out
    }

    #[test]
    fn request_routes_sounds_and_sequences() {
        let mut s = system();
        s.request(0x31);
        assert_eq!(s.take_commands(), [Command::Play(0x31)]);
        s.request(0x1000);
        assert_eq!(s.take_commands(), [Command::Play(0x10)]);
        assert!(s.sequence_active());
        // Unknown and empty sequences do nothing.
        s.request(0x1005);
        s.request(0x1002);
        assert!(s.take_commands().is_empty());
    }

    #[test]
    fn sequence_timing() {
        let mut s = system();
        s.request(0x1000);
        // Step 0 (delay 2) advances on the third sequencer tick (frame 2, first half): the
        // first gap is delay + 1 ticks. The counter is already 1 on the next tick, so delay 0
        // advances at once (frame 2, second half); later gaps are max(delay, 1) ticks.
        assert_eq!(plays(&mut s, 5), [(0, 0x10), (2, 0x11), (2, 0x12)]);
        assert!(!s.sequence_active());
    }

    #[test]
    fn step_zero_may_chain_a_sequence() {
        let mut s = system();
        s.request(0x1001);
        // Sequence 1's first step is 0x1001 itself: it starts again in every free slot, then
        // the fourth start finds no slot and stops the recursion.
        let cmds = s.take_commands();
        assert!(cmds.is_empty(), "{cmds:?}");
        assert_eq!(
            s.slots.iter().filter(|x| x.step >= 0).count(),
            SEQUENCE_SLOTS
        );
    }

    #[test]
    fn slots_limit_and_disable() {
        let mut s = system();
        for _ in 0..6 {
            s.start_sequence(0);
        }
        assert_eq!(s.take_commands().len(), SEQUENCE_SLOTS);
        s.set_sequences_enabled(false);
        s.frame();
        assert!(!s.sequence_active());
        s.start_sequence(0);
        assert!(s.take_commands().is_empty());
        s.set_sequences_enabled(true);
        s.start_sequence(0);
        s.clear_sequences();
        assert!(!s.sequence_active());
    }

    #[test]
    fn pause_blocks_play_and_sequencer() {
        let mut s = system();
        s.request(0x1000);
        s.set_paused(true);
        s.request(0x22);
        s.release(0x22);
        assert_eq!(
            s.take_commands(),
            [
                Command::Play(0x10),
                Command::Pause(true),
                Command::Release(0x22)
            ]
        );
        assert!(plays(&mut s, 10).is_empty());
        s.set_paused(false);
        s.take_commands();
        assert_eq!(plays(&mut s, 2), [(2, 0x11), (2, 0x12)]);
    }

    #[test]
    fn disabled_audio_is_silent() {
        let mut s = system();
        s.set_enabled(false);
        s.request(0x1000);
        s.set_volume(1, 2);
        assert!(s.take_commands().is_empty());
        assert!(!s.sequence_active());
    }

    #[test]
    fn caller_built_lists() {
        let mut s = system();
        s.start_sequence_list(&[Step::END], Origin::List);
        s.start_sequence_list(&[], Origin::List);
        assert!(!s.sequence_active());
        // Long lists are cut at 12 steps.
        let long: Vec<Step> = (0..20).map(|i| Step::new(0x40 + i, 0)).collect();
        s.start_sequence_list(&long, Origin::List);
        let p = plays(&mut s, 20);
        assert_eq!(p.len(), 12);
        assert_eq!(p.last().unwrap().1, 0x40 + 11);
    }

    #[test]
    fn pair_call_plays_second_index_first() {
        let mut s = system();
        s.call_pair(2, 5);
        // pair[5] = (0x65, 10), then pair[2] = (0x62, 7) after 11 sequencer ticks (frame 6).
        let p = plays(&mut s, 10);
        assert_eq!(p, [(0, 0x65), (6, 0x62)]);
        s.call_pair(2, 99);
        assert!(s.take_commands().is_empty());
    }

    #[test]
    fn prefix_speed_call() {
        let mut s = system();
        s.call_prefix_speed(0, 95);
        // 0xc2 (delay 0x22 = 34, so 35 sequencer ticks), then 95/5 = 19 → element 31 = 0xc7.
        let p = plays(&mut s, 20);
        assert_eq!(p, [(0, 0xc2), (18, 0xc7)]);
        s.call_prefix_speed(0, 0);
        assert_eq!(s.take_commands(), [Command::Play(0xc2)]);
        let p = plays(&mut s, 20);
        assert_eq!(p, [(18, 0xc3)]);
        // 5 km/h and ≥ 130 km/h: prefix only, through snd_request.
        s.call_prefix_speed(0, 5);
        s.call_prefix_speed(0, 200);
        assert_eq!(
            s.take_commands(),
            [Command::Play(0xc2), Command::Play(0xc2)]
        );
        s.call_prefix_speed(0, 10);
        s.call_prefix_speed(0, 125);
        let codes: Vec<u16> = plays(&mut s, 20).into_iter().map(|p| p.1).collect();
        assert!(codes.contains(&0xae) && codes.contains(&0xcd));
        s.call_prefix_speed(7, 50);
        assert!(s.take_commands().is_empty());
    }

    /// Codes of every phrase a call plays, in order.
    fn codes(s: &mut SoundSystem) -> Vec<u16> {
        plays(s, 400).into_iter().map(|p| p.1).collect()
    }

    #[test]
    fn signal_call() {
        let mut s = system();
        s.call_signal(3, 1, 2);
        assert_eq!(codes(&mut s), [0xd4, 0xc1, 0xa9]);
        s.call_signal(0, 0, 4);
        assert_eq!(codes(&mut s), [0xd1, 0xd2]);
        s.call_signal(0, 2, 0);
        assert_eq!(codes(&mut s), [0xc0, 0xa7]);
        // Out-of-range parts are skipped; nothing left plays nothing.
        s.call_signal(4, 3, 7);
        assert!(codes(&mut s).is_empty());
        // Timing: prefix delay 0x1e → the second phrase 31 sequencer ticks later (frame 16).
        s.call_signal(1, -1, 5);
        assert_eq!(plays(&mut s, 40), [(0, 0xc1), (16, 0xbe)]);
    }

    #[test]
    fn voice_log_records_lines_with_their_call() {
        let mut s = system();
        s.request(0x31);
        assert!(s.take_voice_events().is_empty(), "off by default");
        s.set_voice_log(true);
        s.request(0x31);
        s.request(0x1000);
        s.call_pair(2, 5);
        s.stop_all();
        let started = |origin, steps: &[Step]| {
            VoiceEvent::Started(Started {
                origin,
                steps: steps.to_vec(),
            })
        };
        assert_eq!(
            s.take_voice_events(),
            [
                started(Origin::Code(0x31), &[Step::new(0x31, 0)]),
                started(
                    Origin::Sequence(0),
                    &[Step::new(0x10, 2), Step::new(0x11, 0), Step::new(0x12, 0)]
                ),
                started(
                    Origin::Pair {
                        first: 2,
                        second: 5
                    },
                    &[Step::new(0x65, 10), Step::new(0x62, 7)]
                ),
                VoiceEvent::Silenced,
            ]
        );
        // A sequence that finds no slot was never heard; dropping running ones silences.
        for _ in 0..3 {
            s.start_sequence(0);
        }
        assert_eq!(
            s.take_voice_events().len(),
            2,
            "two of four slots were free"
        );
        s.set_sequences_enabled(false);
        s.frame();
        assert_eq!(s.take_voice_events(), [VoiceEvent::Silenced]);
        // Paused: single sounds are not played, so not logged.
        s.set_paused(true);
        s.request(0x31);
        assert!(s.take_voice_events().is_empty());
    }

    #[test]
    fn station_call() {
        let mut pairs: Vec<Step> = (0..180).map(|_| Step::new(0x150, 30)).collect();
        pairs[STATION_CALL_BLOCK] = Step::new(0x100, 20);
        pairs[STATION_CALL_BLOCK + 1] = Step::new(0x130, 20);
        pairs[STATION_CALL_BLOCK + 2] = Step::new(0x160, 20);
        let tables = SoundTables {
            call_pairs: pairs,
            ..SoundTables::default()
        };
        let mut s = SoundSystem::new(Arc::new(tables));
        s.call_station(1, 0);
        assert_eq!(codes(&mut s), [0x1a4, 0x100, 0x100]);
        s.call_station(1, 1);
        assert_eq!(codes(&mut s), [0x130, 0x130]);
        s.call_station(0, 0);
        assert_eq!(codes(&mut s), [0x1a4, 0x111, 0x100, 0x100]);
        s.call_station(2, 0);
        assert_eq!(codes(&mut s), [0x1a4, 0x111, 0x100, 0x112]);
        s.call_station(2, 1);
        assert_eq!(codes(&mut s), [0x147, 0x130, 0x148]);
        s.call_station(0, 2);
        assert_eq!(codes(&mut s), [0x168, 0x160, 0x160]);
        s.call_station(3, 0);
        s.call_station(0, 90);
        assert!(codes(&mut s).is_empty());
    }
}

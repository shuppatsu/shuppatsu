//! Navigator reaction animations (docs/re/hud.md §3.17): `NaviAnim(n)` 0x410cb0 → task
//! 0x411080, the little conductor who reacts in slot 0x61 at (80, 145) when the driver pulls
//! the emergency brake, slams the notches or overruns the stop, and the flashing emergency-stop
//! signal (animation 0) in the signal's slot 0x60 during an obstacle. [`ReadyAnim`] is the E3
//! coupling-ready animation (0x411240), which shares the navigator's slot.
//!
//! The model is shared: [`crate::cab`] draws the frames and [`crate::sound`] plays the sound
//! each animation requests. [`calls`] decides from two rule snapshots and the tick's events
//! which `NaviAnim` calls the original makes in that tick, in its order.

use anyhow::{Context, Result};
use shuppatsu_engine::drive::{Drive, DrivePhase};
use shuppatsu_engine::rules::{AfterHold, Phase, RuleEvent, TimeReason};
use shuppatsu_formats::exe::Exe;

pub const ANIMS: usize = 20;
/// The frame lists `{i32 tilemap, i32 ticks}`, ended by `{−1, repeats}`: stack pointers set up
/// by 0x410cb0.
const FRAME_TABLES: [u32; ANIMS] = [
    0x465e04, 0x465e34, 0x465e54, 0x465eb4, 0x465ed4, 0x465f24, 0x465f44, 0x465f84, 0x465fac,
    0x465ffc, 0x466014, 0x46606c, 0x46609c, 0x4660cc, 0x466104, 0x466134, 0x466184, 0x46619c,
    0x4661d4, 0x466214,
];
const FRAMES_MAX: usize = 64;
/// Per animation `{update, sound, palette}` (stack immediates of 0x410cb0): the sound is
/// requested on the update whose counter `+0x34` equals `update` (0 = the first visible one);
/// sound 0 = none. The palette is passed to every 0x431920 draw.
const PARAMS: [(i32, u16, u8); ANIMS] = [
    (-2, 0, 4),
    (0x14, 0x1025, 8),
    (0x14, 0xdc, 8),
    (0x14, 0xe2, 8),
    (0x14, 0x1012, 8),
    (0x14, 0xe5, 8),
    (0x14, 0x1011, 8),
    (10, 0, 0x10),
    (10, 0xd8, 0x10),
    (0, 0xd7, 0x10),
    (0, 0xd6, 0x10),
    (0, 0xd6, 0x10),
    (0x10, 0xda, 8),
    (4, 0xe0, 8),
    (0x1e, 0xdd, 8),
    (0x24, 0xdc, 8),
    (3, 0xe2, 8),
    (0, 0, 8),
    (0x41, 0xd8, 8),
    (3, 0xd7, 8),
];
/// Animation 0 (the emergency-stop signal 特殊信号発光機) draws in the signal's slot.
pub const SLOT_SIGNAL: usize = 0x60;
pub const SLOT_NAVI: usize = 0x61;
/// Updates before the first frame (`+0x38 = 5`, 0x410cb0).
const DELAY: i32 = 5;
/// Per stage four i16 animation numbers (0x45addc + 8·stage): emergency brake, notch slam and
/// two overrun reactions picked by the clock's low bit.
const STAGE_TABLE: u32 = 0x45addc;
const STAGES: usize = 20;
/// 0x40c4e0: the reaction to an EB held 0x28 ticks, or applied for 0x14 ticks of the approach.
const EB_HELD_TICKS: i32 = 0x28;
const EB_APPROACH_TICKS: i32 = 0x14;
/// Approach state `0x856d9a` 4: the stop is overrun (0x40c4e0).
const APPROACH_OVERRUN: i16 = 4;
/// Statistic 11 counts notch slams (0x422c20, `0x40dd60(0xb, 2)`).
const STAT_SLAM: usize = 11;

/// One frame: the tilemap and how many updates it stays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    pub tbl: i32,
    pub ticks: i32,
}

/// The animations and the stage's choice of them, from the exe.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NaviTables {
    /// EB, notch slam, overrun (clock even), overrun (clock odd).
    pub reactions: [i16; 4],
    /// Each animation's frames up to and including the `{−1, repeats}` terminator.
    pub frames: Vec<Vec<Frame>>,
}

impl NaviTables {
    pub fn read(exe: &Exe<'_>, stage: usize) -> Result<Self> {
        anyhow::ensure!(stage < STAGES, "stage {stage} out of range");
        let mut reactions = [0i16; 4];
        for (i, n) in reactions.iter_mut().enumerate() {
            let b = exe.bytes_at(STAGE_TABLE + 8 * stage as u32 + 2 * i as u32, 2)?;
            *n = i16::from_le_bytes([b[0], b[1]]);
        }
        let frames = FRAME_TABLES
            .iter()
            .map(|&va| {
                let mut frames = Vec::new();
                for i in 0..FRAMES_MAX {
                    let at = va + 8 * i as u32;
                    let tbl = exe.u32_at(at)? as i32;
                    let ticks = exe.u32_at(at + 4)? as i32;
                    frames.push(Frame { tbl, ticks });
                    if tbl == -1 {
                        return Ok(frames);
                    }
                }
                None.with_context(|| format!("navigator animation at {va:#x} unterminated"))
            })
            .collect::<Result<_>>()?;
        Ok(Self { reactions, frames })
    }
}

/// What an animation update does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// 0x41fda0: a sound or voice.
    Sound(u16),
    /// 0x431920: the slot shows `tbl` with `palette`.
    Draw { slot: usize, tbl: i16, palette: u8 },
    /// The destructor 0x4111f0 hides the slot.
    Hide(usize),
}

/// One animation task (0x411080).
#[derive(Debug, Clone, PartialEq)]
struct Task {
    id: u32,
    anim: usize,
    /// `+0x38`: updates before the first frame; −1 once running.
    delay: i32,
    /// `+0x3c`: cleared by `NaviAnim(-1)` or a replacement; the task then ends at a frame
    /// boundary.
    enabled: bool,
    /// `+0x34`: updates since the first frame.
    count: i32,
    /// `+0x40` / `+0x44`: frame index and updates into the frame.
    frame: usize,
    in_frame: i32,
    /// `+0x48`: repeats left, −1 until the terminator is first reached.
    repeats: i32,
    /// `+0x68 / 2`: the frame drawn.
    shown: usize,
}

impl Task {
    fn slot(&self) -> usize {
        if self.anim == 0 {
            SLOT_SIGNAL
        } else {
            SLOT_NAVI
        }
    }

    /// 0x411080; false when the task ends (the destructor runs).
    fn update(&mut self, frames: &[Frame], out: &mut impl FnMut(Effect)) -> bool {
        let tbl = |i: usize| frames.get(i).map_or(-1, |f| f.tbl);
        let ticks = |i: usize| frames.get(i).map_or(0, |f| f.ticks);
        if self.delay > 0 {
            self.delay -= 1;
            return true;
        }
        if self.delay == 0 {
            self.delay = -1;
            self.shown = 0;
            if !self.enabled {
                return false;
            }
        } else {
            self.in_frame += 1;
            self.count += 1;
        }
        let (at, sound, palette) = PARAMS[self.anim];
        if self.count == at && sound != 0 {
            out(Effect::Sound(sound));
        }
        if ticks(self.frame) == self.in_frame {
            self.in_frame = 0;
            self.frame += 1;
            if tbl(self.frame) == -1 {
                if self.repeats == -1 {
                    self.repeats = ticks(self.frame);
                }
                if self.repeats == 0 {
                    return false;
                }
                self.frame = 0;
                self.repeats -= 1;
            }
            // The next frame's image is preloaded, and the cancel flag checked, unless this is
            // the final frame of the final pass.
            let preload =
                tbl(self.frame + 1) != -1 || (self.repeats != 0 && ticks(self.frame + 1) != 0);
            if preload && !self.enabled {
                return false;
            }
            self.shown = self.frame;
        }
        out(Effect::Draw {
            slot: self.slot(),
            tbl: tbl(self.shown) as i16,
            palette,
        });
        true
    }
}

/// The animation tasks and the current one (`0x856c68`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Navigator {
    tasks: Vec<Task>,
    current: Option<u32>,
    next_id: u32,
}

/// A `NaviAnim` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Call {
    Show(i32),
    /// `NaviAnim(-1)`.
    Cancel,
    /// The segment start rebuilds the task lists.
    Clear,
}

impl Navigator {
    pub fn apply(&mut self, call: Call, tables: &NaviTables, out: &mut impl FnMut(Effect)) {
        match call {
            Call::Show(n) => self.show(n, tables),
            Call::Cancel => self.show(-1, tables),
            Call::Clear => self.clear(out),
        }
    }

    /// `NaviAnim(n)` 0x410cb0: −1 cancels the current animation. While one is current (until
    /// its task ends, even when cancelled) only animation 0 starts, replacing it, and not
    /// over another animation 0.
    pub fn show(&mut self, n: i32, tables: &NaviTables) {
        let current = self
            .current
            .and_then(|id| self.tasks.iter_mut().find(|t| t.id == id));
        match current {
            Some(cur) if n < 0 => {
                cur.enabled = false;
                return;
            }
            Some(cur) if n != 0 || cur.anim == 0 => return,
            Some(cur) => cur.enabled = false,
            None if n < 0 => return,
            None => {}
        }
        let Some(anim) = usize::try_from(n).ok().filter(|&a| a < tables.frames.len()) else {
            return;
        };
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.tasks.push(Task {
            id,
            anim,
            delay: DELAY,
            enabled: true,
            count: 0,
            frame: 0,
            in_frame: 0,
            repeats: -1,
            shown: 0,
        });
        self.current = Some(id);
    }

    /// An animation is current (`0x856c68 ≠ 0`; it stays set while a cancelled one runs out).
    pub fn busy(&self) -> bool {
        self.current.is_some()
    }

    /// Every task gone at once, slots hidden.
    pub fn clear(&mut self, out: &mut impl FnMut(Effect)) {
        for t in self.tasks.drain(..) {
            out(Effect::Hide(t.slot()));
        }
        self.current = None;
    }

    /// One update of every task, in creation order.
    pub fn update(&mut self, tables: &NaviTables, out: &mut impl FnMut(Effect)) {
        let mut i = 0;
        while i < self.tasks.len() {
            let t = &mut self.tasks[i];
            let frames = tables.frames.get(t.anim).map_or(&[][..], Vec::as_slice);
            if t.update(frames, out) {
                i += 1;
            } else {
                let t = self.tasks.remove(i);
                out(Effect::Hide(t.slot()));
                if self.current == Some(t.id) {
                    self.current = None;
                }
            }
        }
    }
}

/// The E3 coupling-ready animation's frames (`{tbl, ticks}` at 0x46622c, ended by −1:
/// 0x163 E3 併合準備 for 60 updates, 0x164 and 0x165 for 3, 0x166 for 100).
const READY_FRAMES: u32 = 0x46622c;
/// Palette-4 entries 0xb0..0xbf are set from the 16 colours at 0x463700 (0x436770, as at boot).
const READY_PALETTE_ENTRY: usize = 0xb0;
const READY_COLOURS: u32 = 0x463700;
const READY_COLOUR_COUNT: usize = 16;
/// `+0x38 = 6`: the first five updates only count down (0x411240 / 0x411300).
const READY_DELAY: i32 = 6;
/// Sequence 0x29 on counter 0x3c, drawn with palette 4 in slot 0x61.
const READY_SOUND_AT: i32 = 0x3c;
const READY_SOUND: u16 = 0x1029;
const READY_PALETTE: u8 = 4;

/// What the coupling-ready animation reads from the exe.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReadyTables {
    /// The frames, without the terminator.
    pub frames: Vec<Frame>,
    pub colours: Vec<u32>,
}

impl ReadyTables {
    pub fn read(exe: &Exe<'_>) -> Result<Self> {
        let mut frames = Vec::new();
        for i in 0..FRAMES_MAX as u32 {
            let tbl = exe.u32_at(READY_FRAMES + 8 * i)? as i32;
            if tbl == -1 {
                let colours = (0..READY_COLOUR_COUNT as u32)
                    .map(|k| exe.u32_at(READY_COLOURS + 4 * k))
                    .collect::<Result<_, _>>()?;
                return Ok(Self { frames, colours });
            }
            let ticks = exe.u32_at(READY_FRAMES + 8 * i + 4)? as i32;
            frames.push(Frame { tbl, ticks });
        }
        anyhow::bail!("coupling-ready animation at {READY_FRAMES:#x} unterminated")
    }
}

/// The E3 coupling-ready animation (course event 23 → 0x411240, task 0x411300 in list 3,
/// docs/re/hud.md §3.16): "E3 併合準備" in the navigator's slot 0x61, before the coupling
/// minigame of stages 17–19. It yields the slot to a navigator reaction: it draws, and its
/// destructor 0x411390 hides the slot, only while no reaction is current. It cannot be
/// cancelled; it plays once and ends.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadyAnim {
    /// `+0x38`: counts down every update; the animation runs from 0.
    delay: i32,
    /// `+0x34`: updates since the first frame (1 on the first).
    count: i32,
    /// `+0x40` / `+0x44`: frame index and updates into the frame.
    frame: usize,
    in_frame: i32,
}

impl ReadyAnim {
    /// 0x411240; the caller sets the palette entries (`ReadyTables::colours`).
    pub fn new() -> Self {
        Self {
            delay: READY_DELAY,
            count: 0,
            frame: 0,
            in_frame: 0,
        }
    }

    /// 0x411300; false when the task ends (the destructor ran: `Hide` unless `busy`). `busy`:
    /// a navigator reaction is current.
    pub fn update(&mut self, t: &ReadyTables, busy: bool, out: &mut impl FnMut(Effect)) -> bool {
        self.delay -= 1;
        if self.delay >= 1 {
            return true;
        }
        self.in_frame += 1;
        self.count += 1;
        let Some(&f) = t.frames.get(self.frame) else {
            return false;
        };
        if !busy {
            out(Effect::Draw {
                slot: SLOT_NAVI,
                tbl: f.tbl as i16,
                palette: READY_PALETTE,
            });
        }
        if self.count == READY_SOUND_AT {
            out(Effect::Sound(READY_SOUND));
        }
        if f.ticks == self.in_frame {
            self.in_frame = 0;
            self.frame += 1;
            if self.frame >= t.frames.len() {
                if !busy {
                    out(Effect::Hide(SLOT_NAVI));
                }
                return false;
            }
        }
        true
    }

    /// The palette entries 0x411240 sets.
    pub fn palette(t: &ReadyTables) -> (usize, &[u32]) {
        (READY_PALETTE_ENTRY, &t.colours)
    }
}

/// The rule state the triggers read, at one instant.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NaviState {
    pub phase: Phase,
    /// Between a station result and the next segment (`DrivePhase::Loading`).
    pub loading: bool,
    pub departure_done: bool,
    pub clock: i32,
    pub ats: i16,
    pub approach: i16,
    pub eb_ticks: i32,
    pub eb_approach_ticks: i32,
    pub obstacle: i16,
    pub slams: i32,
    pub final_station: bool,
}

impl Default for NaviState {
    fn default() -> Self {
        Self {
            phase: Phase::Driving,
            loading: false,
            departure_done: false,
            clock: 0,
            ats: 0,
            approach: 0,
            eb_ticks: 0,
            eb_approach_ticks: 0,
            obstacle: 0,
            slams: 0,
            final_station: false,
        }
    }
}

impl NaviState {
    pub fn of(drive: &Drive) -> Self {
        let r = drive.rules();
        Self {
            phase: r.phase,
            loading: drive.phase() == DrivePhase::Loading,
            departure_done: r.departure_done,
            clock: r.clock,
            ats: r.ats,
            approach: r.approach,
            eb_ticks: r.eb_ticks,
            eb_approach_ticks: r.eb_approach_ticks,
            obstacle: r.obstacle,
            slams: r.stats[STAT_SLAM],
            final_station: r.final_station,
        }
    }

    /// The driving-loop body (0x4084f5) runs in the tick that starts from this state.
    fn runs_loop_body(&self) -> bool {
        self.phase == Phase::Driving || (self.phase == Phase::PreDeparture && self.departure_done)
    }
}

/// One tick as the triggers see it.
#[derive(Debug, Clone, Copy)]
pub struct NaviTick<'a> {
    pub before: NaviState,
    pub after: NaviState,
    pub events: &'a [RuleEvent],
    pub segment_started: bool,
    /// The game is over (time-up running, `0x855ff8 & 0x400`).
    pub game_over: bool,
    /// The stage loop is left after a game over (0x4085e0, 20 ticks after it).
    pub loop_exit: bool,
}

/// The `NaviAnim` calls of one tick, in the original's order: 0x40acd0 (obstacle) and the
/// EB penalty of 0x40c4e0, the rest of 0x40c4e0 (EB in the approach, overrun), the stopped
/// train handler 0x40a0a0 (obstacle cleared, arrival, the all-round clear's 0x4113e0) and the
/// train task 0x422c20 (notch slam).
pub fn calls(t: &NaviTick<'_>, tables: &NaviTables) -> Vec<Call> {
    let (b, a) = (&t.before, &t.after);
    let mut calls = Vec::new();
    if t.segment_started {
        calls.push(Call::Clear);
        return calls;
    }
    let show = |i: usize| Call::Show(i32::from(tables.reactions[i]));
    for e in t.events {
        match e {
            RuleEvent::ObstacleStop => calls.push(Call::Show(0)),
            // The 40-tick EB branch shows the reaction; the every-4th-application penalty
            // (first tick) does not.
            RuleEvent::Time {
                reason: TimeReason::EmergencyBrake,
                ..
            } if b.eb_ticks == EB_HELD_TICKS => calls.push(show(0)),
            _ => {}
        }
    }
    if b.runs_loop_body() {
        let ats_now = b.ats != 0
            || t.events
                .iter()
                .any(|e| matches!(e, RuleEvent::AtsBell | RuleEvent::AtsEmergency));
        let obstacle_now = b.obstacle != 0
            || t.events
                .iter()
                .any(|e| matches!(e, RuleEvent::ObstacleStop));
        if b.eb_approach_ticks == EB_APPROACH_TICKS
            && b.eb_ticks < EB_HELD_TICKS
            && !ats_now
            && !obstacle_now
        {
            calls.push(show(0));
        }
        if a.approach == APPROACH_OVERRUN && b.approach != APPROACH_OVERRUN {
            calls.push(show(2 + (b.clock & 1) as usize));
        }
        if matches!(
            a.phase,
            Phase::Holding {
                then: AfterHold::ObstacleCleared,
                ..
            }
        ) {
            calls.push(Call::Cancel);
        }
    }
    // 0x4113e0 (the 乗務完了 chibi at the final station), the end of 0x40a0a0's arrival
    // branch, and the stage-loop exit 0x4085e0 after an arrival or a game over.
    let all_clear = t.events.iter().any(|e| matches!(e, RuleEvent::Arrived(_))) && a.final_station;
    let hold_end = matches!(
        b.phase,
        Phase::Holding {
            ticks: ..=1,
            then: AfterHold::Arrived,
        }
    );
    let arrived = b.phase == Phase::Arrived && !b.loading;
    if all_clear || hold_end || arrived || t.loop_exit {
        calls.push(Call::Cancel);
    }
    if a.slams > b.slams && a.phase != Phase::GameOver && !t.game_over {
        calls.push(show(1));
    }
    calls
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(tbl: i32, ticks: i32) -> Frame {
        Frame { tbl, ticks }
    }

    /// Stage 0's choices and the real shapes of animations 0 (looping), 2 and 3 (one-shot).
    fn tables() -> NaviTables {
        let mut frames: Vec<Vec<Frame>> = (0..ANIMS).map(|_| vec![f(0x219, 1), f(-1, 0)]).collect();
        frames[0] = (0x44b..=0x44f)
            .map(|t| f(t, 4))
            .chain([f(-1, 999)])
            .collect();
        frames[2] = vec![
            f(0x219, 20),
            f(0x21b, 4),
            f(0x21c, 15),
            f(0x21f, 30),
            f(-1, 0),
        ];
        frames[3] = vec![f(0x219, 20), f(0x220, 20), f(0x21f, 20), f(-1, 0)];
        frames[9] = vec![f(0x2b6, 7), f(0x2b7, 8), f(-1, 7)];
        NaviTables {
            reactions: [2, 1, 3, 3],
            frames,
        }
    }

    /// Per update the drawn `(slot, tbl)`, and the sounds with their update.
    type Run = (Vec<Option<(usize, i16)>>, Vec<(usize, u16)>);

    /// Runs `n` updates; per update the drawn `(slot, tbl)` or `None`, and the sounds.
    fn run(nav: &mut Navigator, t: &NaviTables, n: usize) -> Run {
        let mut drawn = Vec::new();
        let mut sounds = Vec::new();
        for i in 0..n {
            let mut d = None;
            nav.update(t, &mut |e| match e {
                Effect::Draw { slot, tbl, .. } => d = Some((slot, tbl)),
                Effect::Sound(s) => sounds.push((i, s)),
                Effect::Hide(_) => {}
            });
            drawn.push(d);
        }
        (drawn, sounds)
    }

    #[test]
    fn one_shot_timing() {
        let t = tables();
        let mut nav = Navigator::default();
        nav.show(2, &t);
        let (drawn, sounds) = run(&mut nav, &t, 80);
        // Five hidden updates, then each frame for its ticks: 20 + 4 + 15 + 30 = 69 updates.
        assert!(drawn[..5].iter().all(Option::is_none));
        let tbl = |i: usize| drawn[i].map(|d| d.1);
        assert_eq!(drawn[5], Some((SLOT_NAVI, 0x219)));
        assert_eq!(tbl(24), Some(0x219));
        assert_eq!(tbl(25), Some(0x21b));
        assert_eq!(tbl(28), Some(0x21b));
        assert_eq!(tbl(29), Some(0x21c));
        assert_eq!(tbl(44), Some(0x21f));
        assert_eq!(tbl(73), Some(0x21f));
        assert_eq!(tbl(74), None);
        assert!(nav.tasks.is_empty());
        // The voice on counter 0x14 = update 5 + 20.
        assert_eq!(sounds, [(25, 0xdc)]);
    }

    #[test]
    fn repeats_and_cancel() {
        let t = tables();
        // Animation 9: 7 + 8 updates, played 1 + 7 times.
        let mut nav = Navigator::default();
        nav.show(9, &t);
        let (drawn, sounds) = run(&mut nav, &t, 200);
        assert_eq!(drawn.iter().flatten().count(), 15 * 8);
        assert_eq!(drawn[5 + 15 * 8 - 1], Some((SLOT_NAVI, 0x2b7)));
        assert_eq!(drawn[5 + 7], Some((SLOT_NAVI, 0x2b7)));
        assert_eq!(drawn[5 + 15], Some((SLOT_NAVI, 0x2b6)));
        // Sound on counter 0: the first visible update.
        assert_eq!(sounds, [(5, 0xd7)]);

        // A cancel ends a looping animation at its next frame boundary.
        let mut nav = Navigator::default();
        nav.show(0, &t);
        run(&mut nav, &t, 7);
        nav.show(-1, &t);
        let (drawn, _) = run(&mut nav, &t, 10);
        // Frame 0 drawn on updates 5..9; the boundary at update 9 ends the task.
        assert_eq!(
            drawn[..2],
            [Some((SLOT_SIGNAL, 0x44b)), Some((SLOT_SIGNAL, 0x44b))]
        );
        assert!(drawn[2..].iter().all(Option::is_none));
        assert!(nav.tasks.is_empty());

        // On a one-shot's last frame the cancel is not checked: it plays out.
        let mut nav = Navigator::default();
        nav.show(3, &t);
        run(&mut nav, &t, 5 + 41);
        nav.show(-1, &t);
        let (drawn, _) = run(&mut nav, &t, 30);
        assert_eq!(drawn.iter().flatten().count(), 19);
    }

    #[test]
    fn only_the_emergency_signal_replaces() {
        let t = tables();
        let mut nav = Navigator::default();
        nav.show(2, &t);
        nav.show(3, &t);
        assert_eq!(nav.tasks.len(), 1);
        // A cancelled animation stays current until it ends: still nothing new but 0.
        nav.show(-1, &t);
        nav.show(3, &t);
        assert_eq!(nav.tasks.len(), 1);
        nav.show(0, &t);
        assert_eq!(nav.tasks.len(), 2);
        nav.show(0, &t);
        assert_eq!(nav.tasks.len(), 2);
        // The replaced task ends on its first update (still in its delay → at delay 0).
        let (drawn, _) = run(&mut nav, &t, 6);
        assert_eq!(drawn[5], Some((SLOT_SIGNAL, 0x44b)));
        assert_eq!(nav.tasks.len(), 1);
    }

    #[test]
    fn triggers() {
        let t = tables();
        let base = NaviState::default();
        let tick = |b: NaviState, a: NaviState, events: &[RuleEvent]| {
            calls(
                &NaviTick {
                    before: b,
                    after: a,
                    events,
                    segment_started: false,
                    game_over: false,
                    loop_exit: false,
                },
                &t,
            )
        };
        let eb = RuleEvent::Time {
            reason: TimeReason::EmergencyBrake,
            seconds: -10,
        };
        let held = NaviState {
            eb_ticks: EB_HELD_TICKS,
            ..base
        };
        assert_eq!(tick(held, held, std::slice::from_ref(&eb)), [Call::Show(2)]);
        let first = NaviState {
            eb_ticks: 1,
            ..base
        };
        assert!(tick(first, first, std::slice::from_ref(&eb)).is_empty());
        let approach = NaviState {
            eb_approach_ticks: EB_APPROACH_TICKS,
            ..base
        };
        assert_eq!(tick(approach, approach, &[]), [Call::Show(2)]);
        let slam = NaviState { slams: 2, ..base };
        assert_eq!(tick(base, slam, &[]), [Call::Show(1)]);
        let b = NaviState {
            approach: 3,
            clock: 7,
            ..base
        };
        let a = NaviState { approach: 4, ..b };
        assert_eq!(tick(b, a, &[]), [Call::Show(3)]);
        assert_eq!(
            tick(base, base, &[RuleEvent::ObstacleStop]),
            [Call::Show(0)]
        );
        let cleared = NaviState {
            phase: Phase::Holding {
                ticks: 36,
                then: AfterHold::ObstacleCleared,
            },
            ..base
        };
        assert_eq!(tick(base, cleared, &[]), [Call::Cancel]);
    }

    fn ready_tables() -> ReadyTables {
        ReadyTables {
            frames: vec![f(0x163, 60), f(0x164, 3), f(0x165, 3), f(0x166, 100)],
            colours: vec![0; READY_COLOUR_COUNT],
        }
    }

    /// Per update (1 = the creation tick) the drawn tilemap, the sounds and the hides, until
    /// the task ends; `busy_at` makes a reaction current on those updates.
    fn run_ready(busy_at: impl Fn(usize) -> bool) -> (Vec<Option<i16>>, Vec<usize>, Vec<usize>) {
        let t = ready_tables();
        let mut r = ReadyAnim::new();
        let (mut drawn, mut sounds, mut hides) = (Vec::new(), Vec::new(), Vec::new());
        for u in 1.. {
            let mut d = None;
            let alive = r.update(&t, busy_at(u), &mut |e| match e {
                Effect::Draw { slot, tbl, palette } => {
                    assert_eq!((slot, palette), (SLOT_NAVI, READY_PALETTE));
                    d = Some(tbl);
                }
                Effect::Sound(s) => {
                    assert_eq!(s, READY_SOUND);
                    sounds.push(u);
                }
                Effect::Hide(_) => hides.push(u),
            });
            drawn.push(d);
            if !alive {
                break;
            }
        }
        (drawn, sounds, hides)
    }

    /// Five hidden updates, then 0x163 ×60 (sound 0x1029 on update 65, as the sound module
    /// has it), 0x164 ×3, 0x165 ×3, 0x166 ×100; the slot is hidden in the last update.
    #[test]
    fn coupling_ready_timeline() {
        let (drawn, sounds, hides) = run_ready(|_| false);
        assert_eq!(drawn.len(), 171);
        assert!(drawn[..5].iter().all(Option::is_none));
        let count = |tbl: i16| drawn.iter().filter(|&&d| d == Some(tbl)).count();
        assert_eq!(
            [count(0x163), count(0x164), count(0x165), count(0x166)],
            [60, 3, 3, 100]
        );
        assert_eq!(
            (drawn[5], drawn[65], drawn[68], drawn[71]),
            (Some(0x163), Some(0x164), Some(0x165), Some(0x166))
        );
        assert_eq!(sounds, [65]);
        assert_eq!(hides, [171]);
        // While a reaction is current it neither draws nor hides; it still counts.
        let (drawn, sounds, hides) = run_ready(|u| u > 100);
        assert_eq!(drawn.len(), 171);
        assert!(drawn[100..].iter().all(Option::is_none));
        assert_eq!((sounds, hides), (vec![65], vec![]));
    }

    #[test]
    fn coupling_ready_from_the_exe() {
        let Ok(data) = crate::scene::GameData::from_env() else {
            return;
        };
        let Ok(bytes) = data.exe() else { return };
        let exe = Exe::parse(&bytes).unwrap();
        let t = ReadyTables::read(&exe).unwrap();
        assert_eq!(t.frames, ready_tables().frames);
        assert_eq!((t.colours.len(), t.colours[2]), (16, 0xffffff));
    }
}

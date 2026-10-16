//! Game sound: feeds the simulation into `ddg_audio` once per tick and forwards the commands to
//! the device. Without an output device the game runs silently.
//!
//! Per tick, in the original's order (docs/re/audio-text.md §10): the driving-loop body (track
//! sounds 0x407c10, the rule functions' call-outs and announcements, the stopped-train handler,
//! [`stage::StageSound::loop_part`]), the train task 0x422c20 ([`TrainSound`]), the task lists
//! (departure task, HUD, navigator, time-up, [`stage::StageSound::task_part`]), then the phrase
//! sequencer.
//!
//! Use: `let before = Before::of(&drive)` before [`Drive::tick`], then [`Sound::events`] with
//! the tick's rule events and [`Sound::after_tick`] every tick.

mod stage;

use anyhow::Result;
use ddg_audio::{AudioOutput, HornEdge, MixerConfig, SoundBank, SoundSystem, SoundTables};
use ddg_audio::{TrainSound, TrainTick};
use ddg_engine::drive::Drive;
use ddg_engine::rules::RuleEvent;
use ddg_formats::exe::Exe;
use stage::{Snap, StageSound, StageTables, TickInfo};
use std::path::Path;
use std::sync::Arc;

/// What the simulation looked like before a tick, for the next [`Sound::after_tick`].
pub type Before = Snap;

pub struct Sound {
    out: AudioOutput,
    snd: SoundSystem,
    train: TrainSound,
    stage: StageSound,
    horn_was_held: bool,
    /// Rule events of the current tick, from [`Sound::events`].
    events: Vec<RuleEvent>,
}

impl Sound {
    /// Opens the default device; `Ok(None)` when there is none (the game runs silently).
    pub fn open(data_dir: &Path, exe: &Exe<'_>, drive: &Drive) -> Result<Option<Self>> {
        let bank = Arc::new(SoundBank::load(data_dir)?);
        let tables = Arc::new(SoundTables::from_exe(exe)?);
        let stage_tables = StageTables::read(exe, drive.rules().stage().0)?;
        let out = match AudioOutput::open(bank, tables.code_volume.clone(), MixerConfig::default())
        {
            Ok(o) => o,
            Err(e) => {
                eprintln!("no sound: {e}");
                return Ok(None);
            }
        };
        let mut snd = SoundSystem::new(tables.clone());
        let train = TrainSound::new(
            &tables,
            drive.rules().stage().0,
            drive.train().train_type,
            &mut snd,
        );
        Ok(Some(Self {
            out,
            snd,
            train,
            stage: StageSound::new(stage_tables),
            horn_was_held: false,
            events: Vec::new(),
        }))
    }

    /// The rule events of this tick ([`ddg_engine::drive::DriveTick::events`]). Call before
    /// [`Sound::after_tick`] of the same tick; events given later are heard one tick late.
    pub fn events(&mut self, events: &[RuleEvent]) {
        self.events.extend_from_slice(events);
    }

    /// After one simulation tick: loop-body sounds, train sounds, task sounds, sequencer, then
    /// flush. Call every tick, including result, game-over and loading ticks.
    pub fn after_tick(
        &mut self,
        drive: &Drive,
        before: Before,
        horn_held: bool,
        segment_started: bool,
    ) {
        let events = std::mem::take(&mut self.events);
        let info = TickInfo {
            before,
            after: Snap::of(drive),
            events: &events,
            segment_started,
            track_classes: drive.track_classes(),
            turnout_entered: drive.turnout_entered(),
        };
        self.stage.loop_part(&mut self.snd, &mut self.train, &info);

        let t = drive.train();
        let rules = drive.rules();
        let horn = match (self.horn_was_held, horn_held) {
            (false, true) => HornEdge::Pressed,
            (true, false) => HornEdge::Released,
            _ => HornEdge::None,
        };
        self.horn_was_held = horn_held;
        self.train.tick(
            &TrainTick {
                driving: drive.train_driving(),
                horn_enabled: true,
                horn,
                train_type: t.train_type,
                top_speed_kmh: drive.train_spec().max_speed_kmh(),
                prev_speed: before.speed,
                speed: t.speed,
                traction_fade: t.traction_ramp,
                prev_power: before.power,
                prev_brake: before.brake,
                brake: t.prev_brake,
                brake_moving_ticks: t.brake_moving_ticks,
                odometer: t.odometer,
                curve_sharpness: t.curve_sharpness,
                stage_tick: rules.clock,
                joint_override: rules.signal_variant & 1 != 0,
            },
            &mut self.snd,
        );

        self.stage.task_part(&mut self.snd, &self.train, &info);
        self.snd.frame();
        self.flush();
    }

    /// `snd_request` (0x41fda0): starts a sound code (`| 0x1000` = a sequence).
    pub fn request(&mut self, code: u16) {
        self.snd.request(code);
        self.flush();
    }

    /// Releases a looping sound code.
    pub fn release(&mut self, code: u16) {
        self.snd.release(code);
        self.flush();
    }

    /// Plays `code` even while paused: the pause dialog's clicks (0x26), which the original
    /// posts straight to the sound thread (menus.md §1.4).
    pub fn request_ungated(&mut self, code: u16) {
        self.snd.play_ungated(code);
        self.flush();
    }

    /// `0x420660`: stops every voice (the end of a run). Running sequences keep stepping, as in
    /// the original.
    pub fn stop_all(&mut self) {
        self.snd.stop_all();
        self.flush();
    }

    pub fn set_paused(&mut self, on: bool) {
        self.snd.set_paused(on);
        self.flush();
    }

    fn flush(&mut self) {
        self.out.send_all(self.snd.take_commands());
        for e in self.out.take_errors() {
            eprintln!("audio: {e}");
        }
    }
}

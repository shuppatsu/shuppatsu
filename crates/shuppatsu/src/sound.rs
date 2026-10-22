//! Game sound: feeds the simulation into `shuppatsu_audio` once per tick and forwards the commands to
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
use shuppatsu_audio::{
    AudioOutput, HornEdge, Interpolation, MixerConfig, SoundBank, SoundSystem, SoundTables,
};
use shuppatsu_audio::{TrainSound, TrainTick, VoiceEvent};
use shuppatsu_engine::bonus::BonusEvent;
use shuppatsu_engine::coupling::CouplingEvent;
use shuppatsu_engine::drive::Drive;
use shuppatsu_engine::rules::RuleEvent;
use shuppatsu_engine::traffic::TrafficSound;
use shuppatsu_formats::exe::Exe;
use stage::{Snap, StageSound, StageTables, TickInfo};
use std::path::Path;
use std::sync::Arc;

/// What the simulation looked like before a tick, for the next [`Sound::after_tick`].
pub type Before = Snap;

pub struct Sound {
    /// `None`: a silent sound system, run only for its voice log (subtitles without a device).
    out: Option<AudioOutput>,
    /// Record the voice lines for subtitles ([`Sound::take_voice_events`]).
    voice_log: bool,
    snd: SoundSystem,
    tables: Arc<SoundTables>,
    /// The sound option (`0x856356`): off makes the sound thread ignore play messages.
    enabled: bool,
    stage: Option<StageParts>,
    /// Force-feedback jolts (0x434940) since the last [`Sound::take_jolts`].
    jolts: u32,
}

/// The per-stage half: train sounds and the stage's sound mapping.
struct StageParts {
    train: TrainSound,
    stage: StageSound,
    horn_was_held: bool,
    /// Rule events of the current tick, from [`Sound::events`].
    events: Vec<RuleEvent>,
}

impl Sound {
    /// Opens the default device with no stage (menus), resampling with `resampler` (setting
    /// `audio_resampler`); `Ok(None)` when there is no device.
    pub fn open_device(
        data_dir: &Path,
        exe: &Exe<'_>,
        resampler: Interpolation,
    ) -> Result<Option<Self>> {
        let bank = Arc::new(SoundBank::load(data_dir)?);
        let tables = Arc::new(SoundTables::from_exe(exe)?);
        let config = MixerConfig {
            interpolation: resampler,
            ..MixerConfig::default()
        };
        let out = match AudioOutput::open(bank, tables.code_volume.clone(), config) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("no sound: {e}");
                return Ok(None);
            }
        };
        Ok(Some(Self {
            out: Some(out),
            voice_log: false,
            snd: SoundSystem::new(tables.clone()),
            tables,
            enabled: true,
            stage: None,
            jolts: 0,
        }))
    }

    /// The sound logic without a device: nothing is heard, but the voice lines are logged for
    /// subtitles ([`Sound::set_voice_log`]).
    pub fn silent(exe: &Exe<'_>) -> Result<Self> {
        let tables = Arc::new(SoundTables::from_exe(exe)?);
        Ok(Self {
            out: None,
            voice_log: false,
            snd: SoundSystem::new(tables.clone()),
            tables,
            enabled: true,
            stage: None,
            jolts: 0,
        })
    }

    /// Changes the resampler (setting `audio_resampler`) from the next device buffer on.
    pub fn set_resampler(&mut self, resampler: Interpolation) {
        if let Some(out) = self.out.as_mut() {
            let config = MixerConfig {
                interpolation: resampler,
                ..out.config()
            };
            out.set_config(config);
        }
    }

    /// Whether this plays on a device (not [`Sound::silent`]).
    pub fn has_device(&self) -> bool {
        self.out.is_some()
    }

    /// Records the voice lines started from now on, for subtitles.
    pub fn set_voice_log(&mut self, on: bool) {
        self.voice_log = on;
        self.snd.set_voice_log(on);
    }

    /// The voice lines started and cut off since the last call.
    pub fn take_voice_events(&mut self) -> Vec<VoiceEvent> {
        self.snd.take_voice_events()
    }

    /// Sets up the train and stage sounds for `drive`'s stage, on a fresh sound system (the
    /// previous stage's voices are stopped).
    pub fn begin_stage(&mut self, exe: &Exe<'_>, drive: &Drive) -> Result<()> {
        let stage_tables = StageTables::read(exe, drive.rules().stage().0)?;
        self.snd.stop_all();
        self.flush();
        self.snd = SoundSystem::new(self.tables.clone());
        self.snd.set_enabled(self.enabled);
        self.snd.set_voice_log(self.voice_log);
        let train = TrainSound::new(
            &self.tables,
            drive.rules().stage().0,
            drive.train().train_type,
            &mut self.snd,
        );
        self.stage = Some(StageParts {
            train,
            stage: StageSound::new(stage_tables),
            horn_was_held: false,
            events: Vec::new(),
        });
        Ok(())
    }

    /// An attract demo drive's creator (0x40e4b0 / 0x40e6e0) starts the running noise and the
    /// motor as a segment start does (0x4080d0); the demos have no segment start of their own.
    pub fn start_demo(&mut self) {
        if let Some(p) = self.stage.as_ref() {
            p.train.start_segment(&mut self.snd);
        }
        self.flush();
    }

    /// Back to the menus: every voice stopped, the stage half dropped.
    pub fn end_stage(&mut self) {
        self.stage = None;
        self.jolts = 0;
        self.snd.set_paused(false);
        self.snd.stop_all();
        self.flush();
    }

    /// The stage half dropped with the voices left playing: the stage intro (0x4091a0, 0x408f40)
    /// leaves its music to run out over the departure board.
    pub fn leave_stage(&mut self) {
        self.stage = None;
        self.jolts = 0;
        self.snd.set_paused(false);
        self.flush();
    }

    /// The sound option (`0x856356`): when off, the sound thread ignores play messages.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        self.snd.set_enabled(on);
    }

    /// A menu tick: the sequencer runs (jingles), then flush.
    pub fn menu_frame(&mut self) {
        self.snd.frame();
        self.flush();
    }

    /// The rule events of this tick ([`shuppatsu_engine::drive::DriveTick::events`]). Call before
    /// [`Sound::after_tick`] of the same tick; events given later are heard one tick late.
    pub fn events(&mut self, events: &[RuleEvent]) {
        if let Some(p) = self.stage.as_mut() {
            p.events.extend_from_slice(events);
        }
        // The coupling minigame (0x4093c0) plays its sounds itself.
        for e in events {
            match *e {
                RuleEvent::Coupling(CouplingEvent::Sound(code)) => self.snd.request(code),
                RuleEvent::Coupling(CouplingEvent::Music(code)) => self.snd.play_ungated(code),
                RuleEvent::Coupling(CouplingEvent::StopSounds) => self.snd.stop_all(),
                RuleEvent::Bonus(BonusEvent::Sound(code)) => self.snd.request(code),
                RuleEvent::Bonus(BonusEvent::Release(code)) => self.snd.release(code),
                RuleEvent::Bonus(BonusEvent::Music(code)) => self.snd.play_ungated(code),
                _ => {}
            }
        }
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
        let Some(p) = self.stage.as_mut() else {
            self.menu_frame();
            return;
        };
        let events = std::mem::take(&mut p.events);
        let info = TickInfo {
            before,
            after: Snap::of(drive),
            events: &events,
            segment_started,
            track_classes: drive.track_classes(),
            turnout_entered: drive.turnout_entered(),
            demo: drive.demo_counter().is_some(),
        };
        p.stage.loop_part(&mut self.snd, &mut p.train, &info);

        let t = drive.train();
        let rules = drive.rules();
        let horn = match (p.horn_was_held, horn_held) {
            (false, true) => HornEdge::Pressed,
            (true, false) => HornEdge::Released,
            _ => HornEdge::None,
        };
        p.horn_was_held = horn_held;
        let joints_before = p.train.clicks;
        p.train.tick(
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

        p.stage.task_part(&mut self.snd, &p.train, &info);
        // Each rail-joint click (0x422c20) and turnout click (0x4204d0) also starts the
        // force-feedback jolt, which 0x434940 lets through only while a stage runs and the
        // train is driving (`0x855ff8 & 2` and `& 0x200`): never in the attract demos
        // (input.md §11).
        let jolts = p.train.clicks.wrapping_sub(joints_before) + p.stage.take_turnout_clicks();
        if !info.demo && drive.train_driving() {
            self.jolts = self.jolts.saturating_add(jolts);
        }
        // The car tasks' draw step runs in the render list, after the task lists.
        self.traffic(drive.traffic_sounds());
        self.snd.frame();
        self.flush();
    }

    /// The AI-train sounds of a tick ([`Drive::traffic_sounds`], docs/re/traffic.md §5): passing
    /// cars (0x33), row 0x43's sequence (0x1028) and the scripted stop's passing train (8, 1,
    /// 4). [`Sound::after_tick`] plays the drive's own; for other sources call it before
    /// that tick's `after_tick`.
    pub fn traffic(&mut self, sounds: &[TrafficSound]) {
        for s in sounds {
            match *s {
                TrafficSound::Play(code) => self.snd.request(code),
                TrafficSound::Release(code) => self.snd.release(code),
            }
        }
        self.flush();
    }

    /// `snd_request` (0x41fda0): starts a sound code (`| 0x1000` = a sequence).
    pub fn request(&mut self, code: u16) {
        self.snd.request(code);
        self.flush();
    }

    /// Plays a PCM stream beside the voices (the Komachi movie's soundtrack), or stops it.
    pub fn play_stream(&self, stream: Option<std::sync::Arc<shuppatsu_audio::PcmStream>>) {
        if let Some(out) = &self.out {
            out.set_stream(stream);
        }
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

    /// The force-feedback jolts (0x434940 calls that pass its gates) since the last call.
    pub fn take_jolts(&mut self) -> u32 {
        std::mem::take(&mut self.jolts)
    }

    /// The rail-joint clicks of the stage's train so far (`0x84c3dc`).
    pub fn joint_clicks(&self) -> Option<u32> {
        self.stage.as_ref().map(|p| p.train.clicks)
    }

    pub fn set_paused(&mut self, on: bool) {
        self.snd.set_paused(on);
        self.flush();
    }

    fn flush(&mut self) {
        let commands = self.snd.take_commands();
        let Some(out) = self.out.as_mut() else {
            return;
        };
        out.send_all(commands);
        for e in out.take_errors() {
            eprintln!("audio: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shuppatsu_engine::demo::{DemoKind, DemoTables};
    use shuppatsu_engine::drive::{DriverInput, LoadError};
    use shuppatsu_engine::physics::TrainTables;
    use shuppatsu_engine::rules::{Phase, RuleTables};

    fn drive(exe: &Exe<'_>, stage: usize) -> Drive {
        let data = crate::scene::GameData::from_env().unwrap();
        let loader = move |name: &str| -> Result<Vec<u8>, LoadError> {
            data.read(name)
                .map_err(|e| LoadError::from(format!("{e:#}")))
        };
        let train = TrainTables::read(exe).unwrap();
        let rules = RuleTables::read(exe).unwrap();
        let stages = shuppatsu_formats::stages::StageTables::read(exe).unwrap();
        Drive::new(&train, &rules, &stages, stage, 1, 0, loader).unwrap()
    }

    /// The vibration's jolts on the user's data (input.md §11): one per rail-joint click while
    /// driving (plus the turnouts'), none in an attract demo although its train clicks. Runs
    /// on a silent sound (no device, as with `--mute`). Skips without `SHUPPATSU_DATA_DIR`.
    #[test]
    fn jolts_follow_the_clicks_while_driving() {
        let Ok(data) = crate::scene::GameData::from_env() else {
            eprintln!("SHUPPATSU_DATA_DIR not set; skipped");
            return;
        };
        let Ok(exe_bytes) = data.exe() else {
            eprintln!("no exe under SHUPPATSU_DATA_DIR; skipped");
            return;
        };
        let exe = Exe::parse(&exe_bytes).unwrap();

        let mut d = drive(&exe, 0);
        let mut s = Sound::silent(&exe).unwrap();
        s.begin_stage(&exe, &d).unwrap();
        let (mut jolts, mut clicks_driving) = (0, 0);
        for _ in 0..3000 {
            let before = Before::of(&d);
            let r = d.rules();
            let v = d.speed_mps();
            let input = if r.phase == Phase::PreDeparture && !r.departure_done {
                DriverInput {
                    brake: 8,
                    ..DriverInput::default()
                }
            } else if r.dist_m < f32::max(v * v, 30.0) {
                DriverInput {
                    brake: 6,
                    ..DriverInput::default()
                }
            } else if v < 15.0 {
                DriverInput {
                    power: 4,
                    ..DriverInput::default()
                }
            } else {
                DriverInput::default()
            };
            let clicks = s.joint_clicks().unwrap();
            let out = d.tick(input);
            s.events(&out.events);
            s.after_tick(&d, before, false, out.segment_started);
            let tick_jolts = s.take_jolts();
            let tick_clicks = s.joint_clicks().unwrap() - clicks;
            if d.train_driving() {
                clicks_driving += tick_clicks;
                assert!(tick_jolts >= tick_clicks);
            } else {
                assert_eq!(tick_jolts, 0, "no jolt while not driving");
            }
            jolts += tick_jolts;
        }
        eprintln!("stage 0: {jolts} jolts, {clicks_driving} joint clicks while driving");
        assert!(clicks_driving > 10, "the train never clicked");
        assert!(jolts >= clicks_driving);

        let mut d = drive(&exe, 1);
        d.start_demo(DemoKind::Route, &DemoTables::read(&exe).unwrap());
        let mut s = Sound::silent(&exe).unwrap();
        s.begin_stage(&exe, &d).unwrap();
        s.start_demo();
        while !d.demo_finished() {
            let before = Before::of(&d);
            let out = d.tick(DriverInput::default());
            s.events(&out.events);
            s.after_tick(&d, before, false, out.segment_started);
            assert_eq!(s.take_jolts(), 0, "the demos never vibrate");
        }
        assert!(s.joint_clicks().unwrap() > 0, "the demo train clicks");
    }
}

//! Game sound: feeds the simulation into `ddg_audio` once per tick (train sounds in the order
//! of the train task 0x422c20, HUD transfer clicks) and forwards the commands to the device.
//! Without an output device the game runs silently.

use anyhow::Result;
use ddg_audio::{AudioOutput, HornEdge, MixerConfig, SoundBank, SoundSystem, SoundTables};
use ddg_audio::{TrainSound, TrainTick};
use ddg_engine::drive::Drive;
use ddg_formats::exe::Exe;
use std::path::Path;
use std::sync::Arc;

/// HUD remaining-time transfer sounds (hud.md §3.2): +1 s, −1 s.
const SND_TIME_UP: u16 = 0x26;
const SND_TIME_DOWN: u16 = 0x27;

/// What the train looked like before a tick's physics, for the next [`Sound::after_tick`].
#[derive(Clone, Copy, Debug, Default)]
pub struct Before {
    speed: f32,
    power: i32,
    brake: i32,
    remaining_s: i32,
}

impl Before {
    pub fn of(drive: &Drive) -> Self {
        let t = drive.train();
        Self {
            speed: t.speed,
            power: t.prev_power,
            brake: t.prev_brake,
            remaining_s: drive.rules().remaining_s,
        }
    }
}

pub struct Sound {
    out: AudioOutput,
    snd: SoundSystem,
    train: TrainSound,
    horn_was_held: bool,
}

impl Sound {
    /// Opens the default device; `Ok(None)` when there is none (the game runs silently).
    pub fn open(data_dir: &Path, exe: &Exe<'_>, drive: &Drive) -> Result<Option<Self>> {
        let bank = Arc::new(SoundBank::load(data_dir)?);
        let tables = Arc::new(SoundTables::from_exe(exe)?);
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
            horn_was_held: false,
        }))
    }

    /// After one simulation tick: train sounds, transfer clicks, sequencer, then flush.
    pub fn after_tick(
        &mut self,
        drive: &Drive,
        before: Before,
        horn_held: bool,
        segment_started: bool,
    ) {
        if segment_started {
            self.train.start_segment(&mut self.snd);
        }
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
        match rules.remaining_s - before.remaining_s {
            1 => self.snd.request(SND_TIME_UP),
            -1 => self.snd.request(SND_TIME_DOWN),
            _ => {}
        }
        self.snd.frame();
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

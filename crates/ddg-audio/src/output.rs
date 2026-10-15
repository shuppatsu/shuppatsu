//! cpal output: owns the device stream and runs a [`Mixer`] on the audio callback thread.
//!
//! Commands travel over a channel and are applied at the start of each callback, so they take
//! effect within one device buffer (the original's sound thread also applied posted messages
//! asynchronously).

use crate::bank::SoundBank;
use crate::mixer::{Command, Mixer, MixerConfig};
use crate::tables::CodeVolumes;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum OutputError {
    #[error("no default audio output device")]
    NoDevice,
    #[error("audio device: {0}")]
    Device(String),
    #[error("unsupported output sample format {0:?}")]
    Format(SampleFormat),
}

/// An open output stream. Dropping it stops playback.
pub struct AudioOutput {
    stream: cpal::Stream,
    sender: Sender<Command>,
    sample_rate: u32,
    channels: u16,
    errors: Arc<Mutex<Vec<String>>>,
}

impl AudioOutput {
    /// Opens the default output device at its default rate and starts a mixer on it.
    pub fn open(
        bank: Arc<SoundBank>,
        volumes: CodeVolumes,
        config: MixerConfig,
    ) -> Result<Self, OutputError> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or(OutputError::NoDevice)?;
        let supported = device
            .default_output_config()
            .map_err(|e| OutputError::Device(e.to_string()))?;
        let format = supported.sample_format();
        let stream_config: StreamConfig = supported.config();
        let sample_rate = stream_config.sample_rate;
        let channels = stream_config.channels;
        let (sender, receiver) = channel();
        let errors = Arc::new(Mutex::new(Vec::new()));
        let mixer = Mixer::new(bank, volumes, sample_rate, config);

        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, stream_config, mixer, receiver, &errors),
            SampleFormat::I16 => build::<i16>(&device, stream_config, mixer, receiver, &errors),
            SampleFormat::U16 => build::<u16>(&device, stream_config, mixer, receiver, &errors),
            SampleFormat::I32 => build::<i32>(&device, stream_config, mixer, receiver, &errors),
            other => return Err(OutputError::Format(other)),
        }?;
        stream
            .play()
            .map_err(|e| OutputError::Device(e.to_string()))?;
        Ok(Self {
            stream,
            sender,
            sample_rate,
            channels,
            errors,
        })
    }

    /// The device rate the mixer renders at.
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// Sends one command to the mixer.
    pub fn send(&self, cmd: Command) {
        // The receiver only goes away with the stream, which `self` owns.
        let _ = self.sender.send(cmd);
    }

    /// Sends commands in order (typically `SoundSystem::drain_commands()` once per tick).
    pub fn send_all(&self, cmds: impl IntoIterator<Item = Command>) {
        for c in cmds {
            self.send(c);
        }
    }

    /// A sender usable from other threads.
    pub fn sender(&self) -> Sender<Command> {
        self.sender.clone()
    }

    /// Stream errors reported by the backend since the last call.
    pub fn take_errors(&self) -> Vec<String> {
        self.errors
            .lock()
            .map(|mut e| std::mem::take(&mut *e))
            .unwrap_or_default()
    }

    /// Pauses or resumes the device stream itself (not the game's sound pause).
    pub fn set_stream_running(&self, on: bool) -> Result<(), OutputError> {
        let r = if on {
            self.stream.play()
        } else {
            self.stream.pause()
        };
        r.map_err(|e| OutputError::Device(e.to_string()))
    }
}

fn build<T>(
    device: &cpal::Device,
    config: StreamConfig,
    mut mixer: Mixer,
    receiver: Receiver<Command>,
    errors: &Arc<Mutex<Vec<String>>>,
) -> Result<cpal::Stream, OutputError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels.max(1) as usize;
    let mut stereo: Vec<f32> = Vec::new();
    let errors = Arc::clone(errors);
    device
        .build_output_stream::<T, _, _>(
            config,
            move |out: &mut [T], _| {
                while let Ok(cmd) = receiver.try_recv() {
                    mixer.apply(cmd);
                }
                let frames = out.len() / channels;
                stereo.resize(frames * 2, 0.0);
                mixer.render(&mut stereo);
                for (frame, lr) in out
                    .chunks_exact_mut(channels)
                    .zip(stereo.as_chunks::<2>().0)
                {
                    if channels == 1 {
                        frame[0] = T::from_sample(0.5 * (lr[0] + lr[1]));
                    } else {
                        frame[0] = T::from_sample(lr[0]);
                        frame[1] = T::from_sample(lr[1]);
                        for s in &mut frame[2..] {
                            *s = T::from_sample(0.0);
                        }
                    }
                }
            },
            move |e| {
                if let Ok(mut v) = errors.lock() {
                    v.push(e.to_string());
                }
            },
            None,
        )
        .map_err(|e| OutputError::Device(e.to_string()))
}

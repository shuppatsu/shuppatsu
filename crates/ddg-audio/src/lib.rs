//! Audio for the reimplementation: the original's sound-code model, a deterministic software
//! mixer, and a thin cpal output backend.
//!
//! Behaviour follows `docs/re/audio-text.md` (§1–§6), which carries the evidence and confidence
//! levels. The crate has two layers:
//!
//! * **Core (pure, deterministic, no cpal types):**
//!   - [`SoundBank`]: `effect.rom` / `bgm.rom` sample data from the user's disc.
//!   - [`SoundTables`]: per-code volumes, sequences and train-sound tables read from the exe.
//!   - [`Mixer`]: the audio-thread side. Ten voices with the original's slot, streaming, loop,
//!     release, pitch and volume semantics, mixed to interleaved `f32` stereo at any rate.
//!   - [`SoundSystem`]: the game-thread side (`snd_request`, sequences, pause), producing
//!     [`Command`]s for the mixer.
//!   - [`TrainSound`] and [`AmbientSound`]: the per-tick motor, running-noise, rail-joint,
//!     horn and track-class sounds, driven by plain parameters from the simulation.
//! * **Output:** [`AudioOutput`] owns a cpal stream and a [`Mixer`] on the audio thread; the game
//!   sends it the commands a [`SoundSystem`] produced each tick.
//!
//! There is no CD audio: the disc has no CD-DA tracks (docs/recon.md), all music is in `bgm.rom`.

pub mod bank;
pub mod code;
pub mod mixer;
pub mod output;
pub mod sinc;
pub mod stream;
pub mod system;
pub mod tables;
pub mod train;
pub mod voice;

pub use bank::SoundBank;
pub use mixer::{Command, Interpolation, Mixer, MixerConfig};
pub use output::{AudioOutput, OutputError};
pub use stream::PcmStream;
pub use system::SoundSystem;
pub use tables::{SoundTables, Step};
pub use train::{AmbientSound, HornEdge, TrainSound, TrainTick, TurnoutClatter};
pub use voice::{Origin, Started, VoiceEvent};

use thiserror::Error;

/// Errors while loading sound data.
#[derive(Debug, Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Format(#[from] ddg_formats::FormatError),
    #[error("{0} not found in {1}")]
    Missing(&'static str, String),
    #[error("dengo3k.exe sound tables: {0}")]
    Table(String),
}

pub type Result<T> = std::result::Result<T, Error>;

//! Parsers for the original game's file formats. Pure: no I/O beyond `&[u8]`.
//!
//! Format specifications with evidence and confidence levels live in `docs/re/`.

pub mod audio;
pub mod cg;
pub mod error;
pub mod exe;
pub mod hud;
pub mod lz;
pub mod part;
pub mod reader;
pub mod rom2;
pub mod route;
pub mod scene;
pub mod sha256;
pub mod stages;
pub mod strings;
pub mod tex;

pub use error::{FormatError, Result};

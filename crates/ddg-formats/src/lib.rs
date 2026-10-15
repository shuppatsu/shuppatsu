//! Parsers for the original game's file formats. Pure: no I/O beyond `&[u8]`.
//!
//! Format specifications with evidence and confidence levels live in `docs/re/`.

pub mod error;
pub mod lz;
pub mod part;
pub mod reader;
pub mod rom2;
pub mod route;
pub mod sha256;
pub mod stages;
pub mod strings;

pub use error::{FormatError, Result};

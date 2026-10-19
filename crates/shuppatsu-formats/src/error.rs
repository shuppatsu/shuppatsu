//! Error type shared by all parsers.

use thiserror::Error;

/// Result alias for parser functions.
pub type Result<T> = std::result::Result<T, FormatError>;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FormatError {
    /// A read would run past the end of the input.
    #[error("{what}: need {need} bytes at offset {offset:#x}, input is {len} bytes")]
    Truncated {
        what: &'static str,
        offset: usize,
        need: usize,
        len: usize,
    },
    /// The data doesn't start with the expected signature.
    #[error("{what}: bad magic {found:02x?}")]
    BadMagic { what: &'static str, found: Vec<u8> },
    /// Structurally invalid data (counts, offsets or values out of range).
    #[error("{what}: {detail}")]
    Invalid { what: &'static str, detail: String },
}

impl FormatError {
    pub(crate) fn invalid(what: &'static str, detail: impl Into<String>) -> Self {
        Self::Invalid {
            what,
            detail: detail.into(),
        }
    }
}

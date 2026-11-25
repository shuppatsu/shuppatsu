//! Exit codes for the failures a player can act on, so a launcher can say what's wrong in a
//! sentence instead of showing a crash (RecompLauncher's `exit_codes` in recomplauncher.json).
//!
//! An [`Exit`] is attached as context where the failure starts (`.context(Exit::NoData)`);
//! [`code`] finds it anywhere in the error's chain. Other errors exit with 1 and normal quits
//! with 0. The numbers are a contract with recomplauncher.json: keep them.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// No game data where the game looks, or it is incomplete.
    NoData = 10,
    /// The chosen copy is not the 3000-bandai disc, or it is damaged.
    WrongDisc = 11,
    /// No graphics adapter, device or window surface.
    Graphics = 12,
}

impl fmt::Display for Exit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NoData => "no game data",
            Self::WrongDisc => "not the 3000-bandai disc, or a damaged copy",
            Self::Graphics => "graphics could not start",
        })
    }
}

/// The process exit code for `error`.
pub fn code(error: &anyhow::Error) -> u8 {
    // anyhow looks through every context layer for a context of this type.
    error.downcast_ref::<Exit>().map_or(1, |x| *x as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    #[test]
    fn the_code_is_found_anywhere_in_the_chain() {
        let e = anyhow::anyhow!("no adapter")
            .context(Exit::Graphics)
            .context("GPU init");
        assert_eq!(code(&e), 12);
        let e: anyhow::Result<()> = Err(anyhow::anyhow!("x"));
        assert_eq!(code(&e.context(Exit::NoData).unwrap_err()), 10);
        assert_eq!(code(&anyhow::anyhow!("other")), 1);
    }
}

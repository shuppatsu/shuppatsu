//! Proxy DLL loaded into the original exe under Wine as a measurement oracle: logs per-tick
//! state and optionally injects scripted notch/brake inputs (CLAUDE.md, "Oracle and
//! differential testing"). The Windows part builds only for `i686-pc-windows-gnu`; schema and
//! script parsing are platform-independent and tested on the host.

pub mod schema;
pub mod script;
#[cfg(windows)]
mod win;

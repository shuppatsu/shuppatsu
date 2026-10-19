//! Where the log goes on Windows. Release builds use the GUI subsystem (no console window), so
//! a game started from Explorer has no stderr. Then:
//!
//! - by default stdout and stderr go to `shuppatsu.log` in the config directory
//!   (`%APPDATA%\shuppatsu`), overwritten on each start;
//! - `--console` attaches to the console of the shell that started the game, or opens a new one;
//! - a fatal error is also shown in a message box.
//!
//! A redirected stderr (`shuppatsu 2> log.txt`) and debug builds (console subsystem) are left
//! alone. Elsewhere this is a no-op.
//!
//! The only `unsafe` is the kernel32/user32 FFI below.
#![allow(unsafe_code)]

/// Sets up the log as above. Returns the log file's path when stderr goes to it.
#[cfg(windows)]
pub fn init(console: bool) -> Option<std::path::PathBuf> {
    use std::os::windows::io::{AsRawHandle, IntoRawHandle};
    // No handle: null, or INVALID_HANDLE_VALUE (-1).
    let handle = std::io::stderr().as_raw_handle();
    let has_stderr = !handle.is_null() && handle as isize != -1;
    if has_stderr {
        return None;
    }
    if console {
        // SAFETY: plain kernel32 calls without pointers; failure only leaves us without a console.
        let ok = unsafe {
            ffi::AttachConsole(ffi::ATTACH_PARENT_PROCESS) != 0 || ffi::AllocConsole() != 0
        };
        if ok {
            return None;
        }
    }
    let dir = crate::save::config_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("shuppatsu.log");
    let file = std::fs::File::create(&path).ok()?;
    let out = file.try_clone().ok()?;
    // The handles stay open for the life of the process (std writes to them by handle).
    let (err, out) = (file.into_raw_handle(), out.into_raw_handle());
    // SAFETY: both handles are valid, open file handles that are never closed.
    unsafe {
        ffi::SetStdHandle(ffi::STD_ERROR_HANDLE, err);
        ffi::SetStdHandle(ffi::STD_OUTPUT_HANDLE, out);
    }
    Some(path)
}

#[cfg(not(windows))]
pub fn init(_console: bool) -> Option<std::path::PathBuf> {
    None
}

/// With the log in `log` (no visible console): writes the fatal error there, shows it in a
/// message box and exits with status 1. Otherwise returns, and `main` reports it as usual.
#[cfg(windows)]
pub fn fatal(error: &anyhow::Error, log: Option<&std::path::Path>) {
    let Some(log) = log else { return };
    eprintln!("Error: {error:?}");
    let text = format!("{error:#}\n\nLog: {}", log.display());
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (text, caption) = (wide(&text), wide("shuppatsu"));
    // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call; no owner window.
    unsafe {
        ffi::MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            ffi::MB_ICONERROR,
        );
    }
    std::process::exit(1);
}

#[cfg(not(windows))]
pub fn fatal(_error: &anyhow::Error, _log: Option<&std::path::Path>) {}

#[cfg(windows)]
mod ffi {
    use std::ffi::c_void;

    pub const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    pub const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    pub const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    pub const MB_ICONERROR: u32 = 0x10;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn AttachConsole(process_id: u32) -> i32;
        pub fn AllocConsole() -> i32;
        pub fn SetStdHandle(std_handle: u32, handle: *mut c_void) -> i32;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        pub fn MessageBoxW(
            window: *mut c_void,
            text: *const u16,
            caption: *const u16,
            kind: u32,
        ) -> i32;
    }
}

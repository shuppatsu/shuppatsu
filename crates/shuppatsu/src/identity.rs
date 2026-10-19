//! How the game presents itself to the desktop: the window title, the icon (the project's own
//! logo, [`crate::logo`]), and the Wayland app id / X11 `WM_CLASS` a desktop entry can match.

use crate::logo::IconSize;
use std::sync::atomic::{AtomicBool, Ordering};
use winit::window::WindowAttributes;

/// The window title (fixed; the per-frame debug titles need `--debug-title`).
pub const TITLE: &str = "Shuppatsu";
/// Wayland app id, X11 `WM_CLASS` and the desktop entry / icon name.
pub const APP_ID: &str = "shuppatsu";
/// Environment variable that enables the debug titles like `--debug-title`.
pub const DEBUG_TITLE_ENV: &str = "SHUPPATSU_DEBUG_TITLE";

static DEBUG_TITLE: AtomicBool = AtomicBool::new(false);

/// Removes `--debug-title` from `args`, remembering that it was given.
pub fn take_debug_title_flag(args: &mut Vec<String>) {
    let before = args.len();
    args.retain(|a| a != "--debug-title");
    if args.len() != before {
        DEBUG_TITLE.store(true, Ordering::Relaxed);
    }
}

/// Whether the window title shows live debug text (`--debug-title` or `SHUPPATSU_DEBUG_TITLE=1`).
pub fn debug_title() -> bool {
    DEBUG_TITLE.load(Ordering::Relaxed)
        || std::env::var(DEBUG_TITLE_ENV).is_ok_and(|v| !v.is_empty() && v != "0")
}

/// Shows `title` only when debug titles are on (the window otherwise keeps [`TITLE`]).
pub fn set_debug_title(window: &winit::window::Window, title: &str) {
    if debug_title() {
        window.set_title(title);
    }
}

/// The title a new window opens with: [`TITLE`], or `debug` with debug titles on.
pub fn initial_title(debug: &str) -> &str {
    if debug_title() { debug } else { TITLE }
}

/// The window icon of `size` from the embedded logo, or `None` (with a note) if winit
/// refuses it.
fn window_icon(size: IconSize) -> Option<winit::window::Icon> {
    let img = crate::logo::icon(size);
    winit::window::Icon::from_rgba(img.rgba, img.width, img.height)
        .inspect_err(|e| eprintln!("window icon: {e}"))
        .ok()
}

/// Adds the icon (X11, Windows; Wayland ignores it) and the app id / `WM_CLASS`.
pub fn apply(attrs: WindowAttributes) -> WindowAttributes {
    // Windows: the title bar uses the window icon (small), Alt-Tab and the taskbar the big one.
    #[cfg(windows)]
    let attrs = winit::platform::windows::WindowAttributesExtWindows::with_taskbar_icon(
        attrs,
        window_icon(IconSize::Large),
    );
    #[cfg(windows)]
    let attrs = attrs.with_window_icon(window_icon(IconSize::Small));
    // X11 has a single icon for the title bar, taskbar and switcher, scaled by them.
    #[cfg(not(windows))]
    let attrs = attrs.with_window_icon(window_icon(IconSize::Large));
    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "ios", target_os = "android"))
    ))]
    let attrs = {
        use winit::platform::{wayland::WindowAttributesExtWayland, x11::WindowAttributesExtX11};
        let attrs = WindowAttributesExtWayland::with_name(attrs, APP_ID, APP_ID);
        WindowAttributesExtX11::with_name(attrs, APP_ID, APP_ID)
    };
    attrs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_flag_is_taken() {
        let mut args: Vec<String> = ["3", "--debug-title", "--mute"].map(String::from).to_vec();
        take_debug_title_flag(&mut args);
        assert_eq!(args, ["3", "--mute"]);
        assert!(debug_title());
    }
}

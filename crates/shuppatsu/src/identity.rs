//! How the game presents itself to the desktop: the window title, the icon read from the user's
//! `dengo3k.exe`, and the Wayland app id / X11 `WM_CLASS` a desktop entry can match.

use anyhow::{Context, Result};
use shuppatsu_formats::icon::{self, IconData, IconImage};
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

/// The icons of the exe's first icon group, and the one to use (largest, then deepest).
pub fn exe_icon(exe: &[u8]) -> Result<(Vec<IconImage>, IconImage)> {
    let exe = shuppatsu_formats::exe::Exe::parse(exe)?;
    let icons = icon::exe_icons(&exe)?;
    for i in &icons {
        if let IconData::Png(_) = i.data {
            eprintln!(
                "icon #{}: {}x{} is PNG-compressed (not decoded)",
                i.entry.id, i.entry.width, i.entry.height
            );
        }
    }
    let best = icon::best(&icons)
        .cloned()
        .context("dengo3k.exe has no decodable icon")?;
    let images = icons
        .into_iter()
        .filter_map(|i| match i.data {
            IconData::Dib(img) => Some(img),
            IconData::Png(_) => None,
        })
        .collect();
    Ok((images, best))
}

/// The window icon from the user's exe, or `None` (with a note) if it can't be read.
fn window_icon() -> Option<winit::window::Icon> {
    let load = || -> Result<winit::window::Icon> {
        let exe = crate::scene::GameData::from_env()?.exe()?;
        let (_, best) = exe_icon(&exe)?;
        Ok(winit::window::Icon::from_rgba(
            best.rgba,
            best.width,
            best.height,
        )?)
    };
    load().inspect_err(|e| eprintln!("window icon: {e:#}")).ok()
}

/// Adds the icon (X11, Windows; Wayland ignores it) and the app id / `WM_CLASS`.
pub fn apply(attrs: WindowAttributes) -> WindowAttributes {
    let icon = window_icon();
    // Windows: the title bar uses the window icon, Alt-Tab and the taskbar the big one.
    #[cfg(windows)]
    let attrs = winit::platform::windows::WindowAttributesExtWindows::with_taskbar_icon(
        attrs,
        icon.clone(),
    );
    let attrs = attrs.with_window_icon(icon);
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

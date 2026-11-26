//! Issue marks (docs/marks.md), a port debugging aid: F12, or the pad's View button (two squares).
//! Saves the frame on screen and everything needed to get back to it
//! headless in `<data dir>/marks/<YYYYMMDD-HHMMSS>-<n>/` (UTC):
//!
//! - `screenshot.png`: the presented frame at the window's size (before the toast);
//! - `mark.toml`: version, screen, ticks, the run and the train, window, every setting;
//! - `inputs.txt` and `eeprom.dat`: the input script up to the mark (keys and the pad as the
//!   polls read them, [`crate::script::InputLog`]) and the save it started from;
//! - `session-inputs.txt`, `session-eeprom.dat`: for a mark in a run started from the menus,
//!   the whole session from start-up as well;
//! - `reproduce.sh`: the headless command that renders the marked tick again.
//!
//! Nothing is sent anywhere; the folder stays on the user's machine. Taking a mark changes
//! nothing in the game: the input log records what the polls read anyway, and the screenshot
//! is copied from the frame being presented.

use crate::locale::LocaleConfig;
use crate::settings::Settings;
use crate::window::Gpu;
use anyhow::{Context, Result};
use shuppatsu_render::rgba::RgbaImage;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

/// The key that takes a mark (no keyboard mode reads it).
pub const KEY: KeyCode = KeyCode::F12;
/// How long the toast shows.
const TOAST_S: f32 = 2.0;

static REQUESTED: AtomicBool = AtomicBool::new(false);
static COUNT: AtomicU32 = AtomicU32::new(0);

/// Asks for a mark at the next frame (the key, or the pad combination seen by a poll).
pub fn request() {
    REQUESTED.store(true, Ordering::Relaxed);
}

/// Whether a mark was asked for since the last call.
pub fn take_request() -> bool {
    REQUESTED.swap(false, Ordering::Relaxed)
}

/// Asks for a mark when `event` presses [`KEY`] (repeats ignored); true for any [`KEY`]
/// event, which the game never sees.
pub fn key_event(event: &WindowEvent) -> bool {
    let WindowEvent::KeyboardInput {
        event:
            KeyEvent {
                physical_key: PhysicalKey::Code(KEY),
                state,
                repeat,
                ..
            },
        ..
    } = event
    else {
        return false;
    };
    if *state == ElementState::Pressed && !repeat {
        request();
    }
    true
}

/// `<data dir>/marks`: `%APPDATA%\shuppatsu\marks` on Windows, else
/// `$XDG_DATA_HOME/shuppatsu/marks` or `~/.local/share/shuppatsu/marks` (`$SHUPPATSU_HOME/marks`
/// when set, crate::save::HOME_VAR).
pub fn dir() -> Option<PathBuf> {
    dir_in(|k| std::env::var_os(k))
}

/// [`dir`] with the environment looked up by `env` (relative values are ignored).
pub fn dir_in(env: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    let absolute = |v: std::ffi::OsString| Some(PathBuf::from(v)).filter(|p| p.is_absolute());
    if let Some(home) = env(crate::save::HOME_VAR).and_then(absolute) {
        return Some(home.join("marks"));
    }
    let base = if cfg!(windows) {
        env("APPDATA").and_then(absolute)?
    } else if cfg!(target_os = "macos") {
        crate::save::mac_support_dir(&env)?
    } else {
        env("XDG_DATA_HOME").and_then(absolute).or_else(|| {
            env("HOME")
                .and_then(absolute)
                .map(|h| h.join(".local/share"))
        })?
    };
    Some(base.join("shuppatsu").join("marks"))
}

/// A mark being taken: its folder exists and the next presented frame is its screenshot.
#[derive(Debug)]
pub struct Pending {
    pub number: u32,
    pub dir: PathBuf,
    /// Seconds since 1970 (UTC) when it was asked for.
    pub unix: u64,
}

/// Starts a mark: makes its folder and asks `gpu` for the next presented frame.
pub fn begin(gpu: Option<&mut Gpu>) -> Option<Pending> {
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let number = COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    let Some(base) = dir() else {
        eprintln!("mark {number}: no data dir (set XDG_DATA_HOME or HOME); not saved");
        return None;
    };
    let dir = base.join(format!("{}-{number}", stamp(unix, "", "-", "")));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("mark {number}: {}: {e}; not saved", dir.display());
        return None;
    }
    if let Some(g) = gpu {
        g.capture = Some(dir.join("screenshot.png"));
    }
    Some(Pending { number, dir, unix })
}

/// A headless command that reaches the marked tick again.
#[derive(Debug, Clone)]
pub struct Replay {
    /// The arguments after the data dir, before the settings: `play 5 --difficulty 1 …`, or
    /// nothing for the whole game.
    pub command: Vec<String>,
    /// The settings it starts with (every one is passed as a flag).
    pub locale: LocaleConfig,
    pub settings: Settings,
    /// The save it starts from (`--save`, read only).
    pub save: Vec<u8>,
    /// Its input script up to the mark.
    pub inputs: String,
    /// The tick of the mark in the script's count.
    pub tick: u64,
}

/// What the game knows at the mark.
#[derive(Debug)]
pub struct Report {
    /// The screen shown (`play`, `title`, `route_select`, …).
    pub screen: String,
    /// The game's own sections (`ticks`, `run`, `train`, `camera`, `input`).
    pub state: toml::Table,
    /// The settings in effect now.
    pub locale: LocaleConfig,
    pub settings: Settings,
    pub window: Window,
    /// The game's install directory (its `DATA` and `BIN`), for `--data` ([`install_dir`]).
    pub data_dir: PathBuf,
    /// The replay `reproduce.sh` runs by default (`inputs.txt`; `None`: none, a check aid's
    /// run), and the whole session from start-up when that is a different one
    /// (`session-inputs.txt`).
    pub replay: Option<Replay>,
    pub session: Option<Replay>,
}

/// The install directory of the game data directory `data` (`…/DATA`).
pub fn install_dir(data: &Path) -> PathBuf {
    data.parent().unwrap_or(data).to_owned()
}

/// The window and how the 2D layer fills it.
#[derive(Debug, Clone, Copy)]
pub struct Window {
    pub size: (u32, u32),
    pub fullscreen: bool,
    /// The 2D canvas multiple and the canvas size in pixels.
    pub ui_scale: usize,
    pub canvas: (usize, usize),
}

impl Window {
    pub fn of(
        gpu: Option<&Gpu>,
        size: (u32, u32),
        ui_scale: usize,
        canvas: (usize, usize),
    ) -> Self {
        Self {
            size: gpu.map_or(size, Gpu::size),
            fullscreen: gpu.is_some_and(Gpu::is_fullscreen),
            ui_scale,
            canvas,
        }
    }
}

/// Writes the mark's files, shows the toast and logs the folder.
pub fn finish(p: &Pending, r: &Report, gpu: Option<&mut Gpu>, toast: &str) -> Result<()> {
    for (prefix, replay, _) in replays(r) {
        write(
            &p.dir.join(format!("{prefix}inputs.txt")),
            &script_file(p, replay),
        )?;
        std::fs::write(p.dir.join(format!("{prefix}eeprom.dat")), &replay.save)
            .context("writing the save")?;
    }
    write(&p.dir.join("mark.toml"), &mark_toml(p, r)?)?;
    let sh = p.dir.join("reproduce.sh");
    write(&sh, &reproduce_sh(p, r))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&sh, std::fs::Permissions::from_mode(0o755))?;
    }
    eprintln!(
        "mark {} saved: {} ({}{})",
        p.number,
        p.dir.display(),
        r.screen,
        r.replay
            .as_ref()
            .map_or(String::new(), |x| format!(", tick {}", x.tick))
    );
    if let (Some(g), Some(img)) = (gpu, toast_image(toast, r.window.size.1)) {
        g.show_toast(&img, TOAST_S);
    }
    Ok(())
}

fn write(path: &Path, text: &str) -> Result<()> {
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

fn script_file(p: &Pending, r: &Replay) -> String {
    format!(
        "# Shuppatsu issue mark {}: the inputs up to tick {} (docs/marks.md).\n{}",
        p.number, r.tick, r.inputs
    )
}

/// The value of every setting as `config.toml` holds it (`None`: no line, the default that
/// follows the window or no file), in the settings screen's order.
pub fn setting_values(
    cfg: &LocaleConfig,
    s: &Settings,
) -> Vec<(&'static str, Option<toml::Value>)> {
    crate::front::port_settings::ITEMS
        .iter()
        .map(|i| (i.key, i.file_value(cfg, s)))
        .collect()
}

/// The command-line flags that set every setting to `cfg` and `s`.
pub fn setting_flags(cfg: &LocaleConfig, s: &Settings) -> Vec<String> {
    let mut out = Vec::new();
    for (key, value) in setting_values(cfg, s) {
        let flag = Settings::flag(key);
        match value {
            None => {}
            Some(toml::Value::Boolean(true)) => out.push(flag),
            Some(toml::Value::Boolean(false)) => out.push(format!("--no-{}", &flag[2..])),
            Some(toml::Value::String(v)) => out.extend([flag, v]),
            Some(v) => out.extend([flag, v.to_string()]),
        }
    }
    if let Some(d) = &cfg.dir {
        out.extend(["--locale-dir".to_owned(), d.to_string_lossy().into_owned()]);
    }
    out
}

fn mark_toml(p: &Pending, r: &Report) -> Result<String> {
    use toml::{Table, Value};
    let mut t = Table::new();
    let mut mark = Table::new();
    mark.insert("number".into(), Value::Integer(i64::from(p.number)));
    mark.insert(
        "time_utc".into(),
        Value::String(stamp(p.unix, "-", " ", ":")),
    );
    mark.insert(
        "version".into(),
        Value::String(env!("CARGO_PKG_VERSION").into()),
    );
    let rev = env!("SHUPPATSU_GIT_REV");
    if !rev.is_empty() {
        mark.insert("git".into(), Value::String(rev.into()));
    }
    mark.insert("screen".into(), Value::String(r.screen.clone()));
    mark.insert(
        "data_dir".into(),
        Value::String(r.data_dir.to_string_lossy().into_owned()),
    );
    t.insert("mark".into(), Value::Table(mark));
    let w = &r.window;
    let mut window = Table::new();
    window.insert("width".into(), Value::Integer(i64::from(w.size.0)));
    window.insert("height".into(), Value::Integer(i64::from(w.size.1)));
    window.insert("fullscreen".into(), Value::Boolean(w.fullscreen));
    window.insert("ui_scale".into(), Value::Integer(w.ui_scale as i64));
    window.insert(
        "canvas".into(),
        Value::Array(vec![
            Value::Integer(w.canvas.0 as i64),
            Value::Integer(w.canvas.1 as i64),
        ]),
    );
    t.insert("window".into(), Value::Table(window));
    let defaults = setting_values(&LocaleConfig::default(), &Settings::default());
    let mut settings = Table::new();
    let mut changed = Vec::new();
    for ((key, value), (_, default)) in setting_values(&r.locale, &r.settings)
        .into_iter()
        .zip(defaults)
    {
        if value != default {
            changed.push(Value::String(key.into()));
        }
        settings.insert(
            key.into(),
            value.unwrap_or_else(|| Value::String("(default)".into())),
        );
    }
    settings.insert("non_default".into(), Value::Array(changed));
    t.insert("settings".into(), Value::Table(settings));
    let mut repro = Table::new();
    for (prefix, replay, out) in replays(r) {
        let key = if prefix.is_empty() {
            "default"
        } else {
            "session"
        };
        let mut s = "shuppatsu".to_owned();
        for x in args(r, replay, prefix, out) {
            s.push(' ');
            s.push_str(&sh_quote(&x));
        }
        repro.insert(key.into(), Value::String(s));
    }
    t.insert("reproduce".into(), Value::Table(repro));
    // The mark first, the game's sections in their order, then the rest.
    let mut out = String::new();
    let sections = [("mark", &t["mark"])]
        .into_iter()
        .chain(state_sections(&r.state))
        .chain(
            ["window", "settings", "reproduce"]
                .into_iter()
                .map(|k| (k, &t[k])),
        );
    for (name, v) in sections {
        let mut one = Table::new();
        one.insert(name.into(), v.clone());
        out.push_str(&toml::to_string(&one)?);
        out.push('\n');
    }
    Ok(out)
}

/// The game's sections of `mark.toml` in reading order: ticks, run, train, camera, input,
/// then any others.
fn state_sections(state: &toml::Table) -> impl Iterator<Item = (&str, &toml::Value)> {
    const ORDER: [&str; 5] = ["ticks", "run", "train", "camera", "input"];
    let known = ORDER
        .into_iter()
        .filter_map(|k| state.get_key_value(k).map(|(k, v)| (k.as_str(), v)));
    let rest = state
        .iter()
        .filter(|(k, _)| !ORDER.contains(&k.as_str()))
        .map(|(k, v)| (k.as_str(), v));
    known.chain(rest)
}

/// The mark's replays: file prefix, replay and the frame it writes.
fn replays(r: &Report) -> impl Iterator<Item = (&'static str, &Replay, &'static str)> {
    let default = r.replay.as_ref().map(|x| ("", x, "replay.ppm"));
    let session = r.session.as_ref().map(|x| ("session-", x, "session.ppm"));
    default.into_iter().chain(session)
}

/// The replay's arguments after the binary; `prefix` names its files.
fn args(r: &Report, replay: &Replay, prefix: &str, out: &str) -> Vec<String> {
    let mut a = vec![
        "--data".to_owned(),
        r.data_dir.to_string_lossy().into_owned(),
    ];
    a.extend(replay.command.iter().cloned());
    a.extend(["--save".to_owned(), format!("{prefix}eeprom.dat")]);
    a.extend(setting_flags(&replay.locale, &replay.settings));
    let (w, h) = r.window.size;
    a.extend([
        "--size".to_owned(),
        format!("{w}x{h}"),
        "--inputs".to_owned(),
        format!("{prefix}inputs.txt"),
        "--shot".to_owned(),
        format!("{}:{out}", replay.tick),
    ]);
    a
}

fn reproduce_sh(p: &Pending, r: &Report) -> String {
    let exe = std::env::current_exe()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "shuppatsu".to_owned());
    let mut out = format!(
        "#!/bin/sh\n\
         # Shuppatsu issue mark {n} ({time} UTC, {screen}): renders the marked tick again,\n\
         # headless, from the recorded inputs (docs/marks.md). Compare with screenshot.png\n\
         # (the window may have shown a frame between two ticks).\n",
        n = p.number,
        time = stamp(p.unix, "-", " ", ":"),
        screen = r.screen,
    );
    match (&r.replay, &r.session) {
        (Some(_), Some(_)) => out.push_str(
            "#   ./reproduce.sh          the run from its start, writes replay.ppm\n\
             #   ./reproduce.sh session  the whole session from start-up, writes session.ppm\n",
        ),
        (Some(_), None) => out.push_str("#   ./reproduce.sh          writes replay.ppm\n"),
        (None, _) => {
            out.push_str("echo 'this mark has no replay (a check aid run)' >&2\nexit 1\n");
            return out;
        }
    }
    out.push_str(&format!(
        "set -eu\n\
         cd \"$(dirname \"$0\")\"\n\
         # The settings come from the flags below only.\n\
         for v in $(env | sed -n 's/^\\(SHUPPATSU_[A-Z0-9_]*\\)=.*/\\1/p'); do unset \"$v\"; done\n\
         SHUPPATSU=${{SHUPPATSU:-{}}}\n\
         case \"${{1:-}}\" in\n",
        sh_quote(&exe)
    ));
    // The session first: `*)` takes everything else.
    let mut cases: Vec<_> = replays(r).collect();
    cases.reverse();
    for (prefix, replay, file) in cases {
        let pattern = if prefix.is_empty() { "*" } else { "session" };
        // A flag and its value on one line, the subcommand on its own.
        let mut cmd = "\"$SHUPPATSU\"".to_owned();
        let mut values = 1;
        for x in args(r, replay, prefix, file) {
            let flag = x.starts_with("--");
            let new_line = flag || values > 0;
            cmd.push_str(if new_line { " \\\n        " } else { " " });
            cmd.push_str(&sh_quote(&x));
            values = if new_line { 0 } else { values + 1 };
        }
        out.push_str(&format!("{pattern})\n    exec {cmd}\n    ;;\n"));
    }
    out.push_str("esac\n");
    out
}

/// `s` for a POSIX shell: as it is when that is safe, else in single quotes.
fn sh_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:=,+@%".contains(c));
    if safe {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// `unix` (UTC) as `YYYY<d>MM<d>DD<sep>hh<t>mm<t>ss`.
fn stamp(unix: u64, d: &str, sep: &str, t: &str) -> String {
    let (y, mo, day) = civil(unix / 86_400);
    let s = unix % 86_400;
    format!(
        "{y:04}{d}{mo:02}{d}{day:02}{sep}{:02}{t}{:02}{t}{:02}",
        s / 3600,
        s / 60 % 60,
        s % 60
    )
}

/// The calendar date of a day count since 1970-01-01 (proleptic Gregorian).
fn civil(days: u64) -> (i64, u32, u32) {
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(month <= 2), month, day)
}

/// The toast: `text` in white on a dark, slightly see-through box (premultiplied RGBA),
/// sized for a window `window_h` pixels high. `None` without the bundled font.
pub fn toast_image(text: &str, window_h: u32) -> Option<RgbaImage> {
    use shuppatsu_render::text::{Align, GlyphSource, rasterise};
    let font = crate::locale::bundled_font().ok()?;
    let h = (window_h / 30).clamp(18, 80) as usize;
    let (pad_x, pad_y) = (h / 2, h / 6);
    let inner_h = h - 2 * pad_y;
    let text_w = text
        .chars()
        .map(|c| font.advance(c, inner_h as f32 * 0.8))
        .sum::<f32>()
        .ceil() as usize
        + 2;
    let w = text_w + 2 * pad_x;
    let mask = rasterise(text, text_w, inner_h, Align::Center, false, &font);
    // Background: (20, 20, 24) at 80 %, premultiplied.
    let (bg, bg_a) = ([16u32, 16, 19], 204u32);
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let c = match (x.checked_sub(pad_x), y.checked_sub(pad_y)) {
                (Some(mx), Some(my)) if mx < text_w && my < inner_h => {
                    u32::from(mask[my * text_w + mx])
                }
                _ => 0,
            };
            let px = &mut rgba[(y * w + x) * 4..][..4];
            for (i, b) in bg.iter().enumerate() {
                px[i] = ((255 * c + b * (255 - c)) / 255) as u8;
            }
            px[3] = ((255 * c + bg_a * (255 - c)) / 255) as u8;
        }
    }
    Some(RgbaImage {
        width: w as u32,
        height: h as u32,
        rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(stamp(0, "", "-", ""), "19700101-000000");
        assert_eq!(stamp(1_700_000_000, "-", " ", ":"), "2023-11-14 22:13:20");
        assert_eq!(stamp(951_782_400, "", "-", ""), "20000229-000000");
        assert_eq!(stamp(1_792_413_296, "", "-", ""), "20261019-123456");
    }

    #[test]
    fn marks_dir_follows_xdg_and_home() {
        let xdg = |k: &str| match k {
            "XDG_DATA_HOME" => Some("/x/data".into()),
            "APPDATA" => Some("C:\\Users\\a\\AppData\\Roaming".into()),
            "HOME" => Some("/home/a".into()),
            _ => None,
        };
        let home = |k: &str| (k == "HOME").then(|| "/home/a".into());
        if cfg!(windows) {
            assert!(dir_in(xdg).unwrap().ends_with("shuppatsu/marks"));
        } else {
            assert_eq!(dir_in(xdg).unwrap(), Path::new("/x/data/shuppatsu/marks"));
            assert_eq!(
                dir_in(home).unwrap(),
                Path::new("/home/a/.local/share/shuppatsu/marks")
            );
            assert_eq!(dir_in(|_| None), None);
        }
    }

    #[test]
    fn quoting() {
        assert_eq!(sh_quote("--size"), "--size");
        assert_eq!(sh_quote("/a b/it's"), "'/a b/it'\\''s'");
        assert_eq!(sh_quote(""), "''");
    }

    /// The flags set every setting back: parsed as a headless run parses them, they give the
    /// same values, whatever the environment says.
    #[test]
    fn setting_flags_round_trip() {
        let s = Settings {
            fog: false,
            square_pixels: true,
            msaa: 4,
            ..Default::default()
        };
        let cfg = LocaleConfig {
            lang: "en".into(),
            hud_layout: crate::presentation::HudLayout::Enhanced,
            ui_scale_fixed: Some(2),
            ui_scale: 2,
            font: Some("/f/a b.otf".into()),
            ..Default::default()
        };
        let mut args = setting_flags(&cfg, &s);
        let env = |k: &str| (k == "SHUPPATSU_SQUARE_PIXELS").then(|| "0".into());
        let (cfg2, s2) = crate::config::layered(None, env, &mut args).unwrap();
        assert!(args.is_empty(), "{args:?}");
        assert_eq!(s2, s);
        assert_eq!(setting_values(&cfg2, &s2), setting_values(&cfg, &s));
    }

    #[test]
    fn toast_is_a_box_with_text() {
        let img = toast_image("Mark 3 saved", 1600).unwrap();
        assert_eq!(img.height, 53);
        assert!(img.width > img.height * 3);
        let alphas: std::collections::BTreeSet<u8> = img.rgba.chunks(4).map(|p| p[3]).collect();
        assert!(alphas.contains(&204) && alphas.contains(&255), "{alphas:?}");
    }
}

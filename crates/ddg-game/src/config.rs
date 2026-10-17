//! The port's own settings file, `config.toml` in the user's config dir next to `eeprom.dat`
//! (`$XDG_CONFIG_HOME/ddg`, `~/.config/ddg`, `%APPDATA%\ddg`; docs/localisation.md,
//! "Settings").
//!
//! `eeprom.dat` keeps the original game's options byte for byte; this file holds only what
//! the port adds: the language, the fonts, the 2D layout and scaling, and one on/off switch
//! per enhancement ([`Settings`], docs/settings.md). Every key is optional, and a setting is
//! taken from, in order of precedence: the command-line flag, the `DDG_*` environment
//! variable, this file, the built-in default.
//!
//! ```toml
//! lang = "en"               # en, or ja for the original art
//! hud_layout = "original"   # original or enhanced
//! ui_scaling = "smooth"     # smooth, integer or nearest
//! ui_scale = 2              # fixed 2D canvas multiple 1..=4 (default: follows the window)
//! font = "/path/a.otf:/path/b.ttf"
//! display_font = "/path/c.otf"
//! texture_pack = "/path/pack"   # replacement 3D texture pages (docs/texture-packs.md)
//! fog = true                # and the other switches of docs/settings.md
//! ```
//!
//! Unknown keys are ignored when read and kept when the game writes the file. A file that
//! does not parse is reported and ignored (the defaults apply); the game never stops over it.
//! Headless `--shot` runs never read it, so they stay reproducible.

use crate::locale::{Catalogue, IDENTITY_LANG, LocaleConfig};
use crate::presentation::HudLayout;
use crate::settings::Settings;
use anyhow::{Context, Result};
use ddg_render::overlay::UiScaling;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const FILE_NAME: &str = "config.toml";

/// The comment a new file starts with.
const HEADER: &str = "\
# Densha de GO! 2 port settings (docs/settings.md).
# Command-line flags and DDG_* environment variables override these.
";

/// `config.toml`. Values are checked when applied ([`PortConfig::apply`]), so one bad value
/// only loses that setting.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PortConfig {
    /// The language: `en`, or `ja` for the original art (`--lang`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    /// `original` or `enhanced` (`--hud-layout`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hud_layout: Option<String>,
    /// `smooth`, `integer` or `nearest` (`--ui-scaling`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_scaling: Option<String>,
    /// `original`, `2x`, `3x` or `max` (`--draw-distance`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draw_distance: Option<String>,
    /// A fixed canvas multiple, 1..=4 (`--ui-scale`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_scale: Option<i64>,
    /// Font list for translated text (`--font`, `:`-separated).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font: Option<PathBuf>,
    /// Font list for display text (`--display-font`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_font: Option<PathBuf>,
    /// A texture pack directory (`--texture-pack`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub texture_pack: Option<PathBuf>,
    /// The on/off switches ([`Settings::KEYS`], read by [`PortConfig::apply`]) and the keys
    /// this version does not know, kept as they are when the file is written.
    #[serde(flatten)]
    pub other: toml::Table,
}

impl PortConfig {
    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    /// Reads `path`: a missing file is the defaults; an unreadable or corrupt one is reported
    /// and is the defaults too.
    pub fn read(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                eprintln!("warning: {}: {e}; using the defaults", path.display());
                return Self::default();
            }
        };
        Self::parse(&text).unwrap_or_else(|e| {
            let e = e.to_string();
            let e = e.trim_end();
            eprintln!(
                "warning: {} is not valid: {e}\nusing the defaults",
                path.display()
            );
            Self::default()
        })
    }

    /// Sets the values of the file on `cfg` and `settings`; a bad value is reported and
    /// skipped.
    pub fn apply(&self, cfg: &mut LocaleConfig, settings: &mut Settings) {
        settings.apply_table(&self.other, FILE_NAME);
        let warn = |k: &str, want: &str| eprintln!("warning: {FILE_NAME}: {k} must be {want}");
        if let Some(l) = self.lang.as_deref().filter(|l| !l.is_empty()) {
            cfg.lang = l.to_owned();
            cfg.lang_chosen = true;
        }
        if let Some(v) = &self.hud_layout {
            match HudLayout::parse(v) {
                Some(l) => cfg.hud_layout = l,
                None => warn("hud_layout", "\"original\" or \"enhanced\""),
            }
        }
        if let Some(v) = &self.ui_scaling {
            match UiScaling::parse(v) {
                Some(s) => cfg.ui_scaling = s,
                None => warn("ui_scaling", "\"smooth\", \"integer\" or \"nearest\""),
            }
        }
        if let Some(v) = &self.draw_distance {
            match crate::scene::DrawDistance::parse(v) {
                Some(d) => cfg.draw_distance = d,
                None => warn("draw_distance", "\"original\", \"2x\", \"3x\" or \"max\""),
            }
        }
        if let Some(n) = self.ui_scale {
            let max = ddg_render::overlay::MAX_MULTIPLE;
            match usize::try_from(n).ok().filter(|n| (1..=max).contains(n)) {
                Some(n) => {
                    cfg.ui_scale_fixed = Some(n);
                    cfg.ui_scale = n;
                }
                None => warn("ui_scale", &format!("1..={max}")),
            }
        }
        if let Some(f) = &self.font {
            cfg.font = Some(f.clone());
        }
        if let Some(f) = &self.display_font {
            cfg.display_font = Some(f.clone());
        }
        if let Some(d) = self
            .texture_pack
            .as_ref()
            .filter(|d| !d.as_os_str().is_empty())
        {
            cfg.texture_pack = Some(d.clone());
        }
    }
}

/// `config.toml` in the user's config dir.
pub fn path() -> Option<PathBuf> {
    Some(crate::save::config_dir()?.join(FILE_NAME))
}

/// The port's settings for a run: the defaults, `file` (`None` for headless runs), the
/// environment as looked up by `env`, then the flags taken out of `args`. A language from
/// the file that has no catalogue is reported and dropped, so the language screen asks again.
pub fn layered(
    file: Option<&PortConfig>,
    env: impl Fn(&str) -> Option<std::ffi::OsString>,
    args: &mut Vec<String>,
) -> Result<(LocaleConfig, Settings)> {
    let mut cfg = LocaleConfig::default();
    let mut settings = Settings::default();
    if let Some(f) = file {
        f.apply(&mut cfg, &mut settings);
    }
    cfg.apply_vars(&env);
    settings.apply_vars(&env);
    cfg.take_args(args)?;
    settings.take_args(args);
    let from_file = file.and_then(|f| f.lang.as_deref());
    if from_file == Some(cfg.lang.as_str()) && !lang_known(&cfg) {
        eprintln!(
            "warning: {FILE_NAME}: no language {:?}; asking again",
            cfg.lang
        );
        cfg.lang = IDENTITY_LANG.to_owned();
        cfg.lang_chosen = false;
    }
    Ok((cfg, settings))
}

/// [`layered`] with the user's file (unless `headless`) and the process environment.
pub fn load(args: &mut Vec<String>, headless: bool) -> Result<(LocaleConfig, Settings)> {
    let file = if headless {
        None
    } else {
        path().map(|p| PortConfig::read(&p))
    };
    layered(file.as_ref(), |k| std::env::var_os(k), args)
}

/// Whether `cfg.lang` can be drawn: the original's, or one with a catalogue.
fn lang_known(cfg: &LocaleConfig) -> bool {
    cfg.lang == IDENTITY_LANG || Catalogue::load(&cfg.lang, cfg.dir.as_deref()).is_ok()
}

/// Whether the first-run language screen asks: no language from a flag, the environment or
/// the file. Headless runs ask only when `forced` (`--first-run`), as they read no file.
pub fn asks_language(cfg: &LocaleConfig, headless: bool, forced: bool) -> bool {
    forced || (!headless && !cfg.lang_chosen)
}

/// Writes `lang` into the file at `path`, keeping everything else in it (comments too). A
/// file that does not parse is moved to `config.toml.bad` and a new one is started.
pub fn store_lang(path: &Path, lang: &str) -> Result<()> {
    store(path, "lang", Some(toml::Value::String(lang.to_owned())))
}

/// Writes the top-level `key` into the file at `path` (`None` removes it, so the default
/// applies again), keeping everything else in it: comments, the other keys and the keys this
/// version does not know. Atomic: a temporary file renamed over the old one. A file that does
/// not parse is moved to `config.toml.bad` and a new one is started.
pub fn store(path: &Path, key: &str, value: Option<toml::Value>) -> Result<()> {
    let old = match std::fs::read_to_string(path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| path.display().to_string()),
    };
    let text = match old {
        Some(t) if PortConfig::parse(&t).is_ok() => with_key(&t, key, value.as_ref()),
        Some(_) => {
            let bad = path.with_extension("toml.bad");
            std::fs::rename(path, &bad).with_context(|| bad.display().to_string())?;
            eprintln!(
                "warning: {} was not valid; kept as {}",
                path.display(),
                bad.display()
            );
            with_key(HEADER, key, value.as_ref())
        }
        None => with_key(HEADER, key, value.as_ref()),
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).with_context(|| tmp.display().to_string())?;
    std::fs::rename(&tmp, path).with_context(|| path.display().to_string())?;
    Ok(())
}

/// `text` (valid TOML) with the top-level `key` set to `value` (`None`: its line removed): its
/// line replaced (a comment after the value is kept), or a new line before the first table.
/// Falls back to writing the parsed file out again (without comments) if the edit does not
/// read back as intended.
fn with_key(text: &str, key: &str, value: Option<&toml::Value>) -> String {
    let line = value.map(|v| format!("{key} = {v}"));
    let is_key = |l: &str| {
        l.trim_start()
            .strip_prefix(key)
            .is_some_and(|rest| rest.trim_start().starts_with('='))
    };
    let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
    let first_table = lines
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .unwrap_or(lines.len());
    match (lines[..first_table].iter().position(|l| is_key(l)), &line) {
        (Some(i), Some(new)) => {
            let comment = trailing_comment(&lines[i]).map(str::to_owned);
            lines[i] = match comment {
                Some(c) => format!("{new} {c}"),
                None => new.clone(),
            };
        }
        (Some(i), None) => {
            lines.remove(i);
        }
        (None, Some(new)) => lines.insert(first_table, new.clone()),
        (None, None) => {}
    }
    let mut out = lines.join("\n");
    out.push('\n');
    let reads_back = toml::from_str::<toml::Table>(&out)
        .ok()
        .is_some_and(|t| t.get(key) == value);
    if reads_back {
        return out;
    }
    let mut table: toml::Table = toml::from_str(text).unwrap_or_default();
    match value {
        Some(v) => table.insert(key.to_owned(), v.clone()),
        None => table.remove(key),
    };
    toml::to_string(&table).unwrap_or_else(|_| line.map(|l| l + "\n").unwrap_or_default())
}

/// The `# comment` after a `key = value` line's value, when the value holds no `#` itself
/// (the values the game writes never do).
fn trailing_comment(line: &str) -> Option<&str> {
    let (_, value) = line.split_once('=')?;
    let hash = value.find('#')?;
    let before = &value[..hash];
    // A `#` inside a string is not a comment.
    (before.matches('"').count() % 2 == 0 && before.matches('\'').count() % 2 == 0)
        .then(|| &value[hash..])
}

/// Where a setting fixed for the run comes from: a command-line flag or an environment
/// variable (they override `config.toml`, so the settings screen cannot change it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lock {
    /// The flag as given (`--no-fog`, `--lang`).
    Flag(String),
    /// The variable (`DDG_FOG`).
    Env(String),
}

/// The locked settings of a run, by key ([`locks`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Locks(std::collections::BTreeMap<String, Lock>);

impl Locks {
    pub fn get(&self, key: &str) -> Option<&Lock> {
        self.0.get(key)
    }
}

/// The value flags of [`LocaleConfig::take_args`] that are settings, with their keys.
const VALUE_FLAGS: &[(&str, &str)] = &[
    ("--lang", "lang"),
    ("--font", "font"),
    ("--display-font", "display_font"),
    ("--ui-scale", "ui_scale"),
    ("--hud-layout", "hud_layout"),
    ("--ui-scaling", "ui_scaling"),
    ("--texture-pack", "texture_pack"),
    ("--draw-distance", "draw_distance"),
];

/// Every key of docs/settings.md: the values, then the switches.
pub fn all_keys() -> impl Iterator<Item = &'static str> {
    VALUE_FLAGS
        .iter()
        .map(|(_, k)| *k)
        .chain(Settings::KEYS.iter().copied())
        .chain(Settings::LEVEL_KEYS.iter().copied())
}

/// The settings `args` and the environment (as looked up by `env`) fix for this run: a flag
/// over a variable, as in [`layered`]. Call before the flags are taken out of `args`.
pub fn locks(env: impl Fn(&str) -> Option<std::ffi::OsString>, args: &[String]) -> Locks {
    let mut m = std::collections::BTreeMap::new();
    for key in all_keys() {
        let var = Settings::env_var(key);
        if env(&var).is_some_and(|v| !v.is_empty()) {
            m.insert(key.to_owned(), Lock::Env(var));
        }
    }
    for a in args {
        let key = VALUE_FLAGS
            .iter()
            .find(|(f, _)| f == a)
            .map(|(_, k)| *k)
            .or_else(|| Settings::flag_value(a).map(|(k, _)| k))
            .or_else(|| Settings::level_flag(a).map(|(k, _)| k));
        if let Some(k) = key {
            m.insert(k.to_owned(), Lock::Flag(a.clone()));
        }
    }
    Locks(m)
}

/// [`load`] and the settings its flags and environment lock ([`locks`]).
pub fn load_locked(
    args: &mut Vec<String>,
    headless: bool,
) -> Result<(LocaleConfig, Settings, Locks)> {
    let locks = locks(|k| std::env::var_os(k), args);
    let (cfg, settings) = load(args, headless)?;
    Ok((cfg, settings, locks))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::OsString;

    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let m: HashMap<String, OsString> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), OsString::from(v)))
            .collect();
        move |k| m.get(k).cloned()
    }

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ddg-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn flag_over_env_over_file_over_default() {
        let file = PortConfig::parse(
            "lang = \"en\"\nhud_layout = \"enhanced\"\nui_scaling = \"integer\"\nui_scale = 2\n",
        )
        .unwrap();
        // Default.
        let c = layered(None, env(&[]), &mut vec![]).unwrap().0;
        assert_eq!(c.lang, "ja");
        assert!(!c.lang_chosen);
        assert_eq!(c.hud_layout, HudLayout::Original);
        // File.
        let c = layered(Some(&file), env(&[]), &mut vec![]).unwrap().0;
        assert_eq!((c.lang.as_str(), c.lang_chosen), ("en", true));
        assert_eq!(c.hud_layout, HudLayout::Enhanced);
        assert_eq!(c.ui_scaling, UiScaling::Integer);
        assert_eq!(c.ui_scale_fixed, Some(2));
        // Environment over the file.
        let e = env(&[("DDG_LANG", "ja"), ("DDG_HUD_LAYOUT", "original")]);
        let c = layered(Some(&file), e, &mut vec![]).unwrap().0;
        assert_eq!(c.lang, "ja");
        assert_eq!(c.hud_layout, HudLayout::Original);
        assert_eq!(
            c.ui_scaling,
            UiScaling::Integer,
            "the file's other keys stay"
        );
        // Flags over both.
        let e = env(&[("DDG_LANG", "ja"), ("DDG_UI_SCALE", "3")]);
        let mut a = args(&["--lang", "en", "--ui-scale", "1", "x"]);
        let c = layered(Some(&file), e, &mut a).unwrap().0;
        assert_eq!(a, ["x"]);
        assert_eq!((c.lang.as_str(), c.ui_scale_fixed), ("en", Some(1)));
    }

    #[test]
    fn first_run_detection() {
        let none = layered(None, env(&[]), &mut vec![]).unwrap().0;
        assert!(asks_language(&none, false, false));
        assert!(!asks_language(&none, true, false), "headless never asks");
        assert!(asks_language(&none, true, true), "unless forced");
        let ja = PortConfig::parse("lang = \"ja\"").unwrap();
        let c = layered(Some(&ja), env(&[]), &mut vec![]).unwrap().0;
        assert!(!asks_language(&c, false, false), "ja chosen is a choice");
        let c = layered(None, env(&[("DDG_LANG", "en")]), &mut vec![])
            .unwrap()
            .0;
        assert!(!asks_language(&c, false, false));
        let c = layered(None, env(&[]), &mut args(&["--lang", "ja"]))
            .unwrap()
            .0;
        assert!(!asks_language(&c, false, false));
        // Only other settings in the file: still asks.
        let other = PortConfig::parse("hud_layout = \"enhanced\"").unwrap();
        let c = layered(Some(&other), env(&[]), &mut vec![]).unwrap().0;
        assert!(asks_language(&c, false, false));
        // A language without a catalogue in the file: dropped, asks again.
        let xx = PortConfig::parse("lang = \"xx\"").unwrap();
        let c = layered(Some(&xx), env(&[]), &mut vec![]).unwrap().0;
        assert_eq!(c.lang, "ja");
        assert!(asks_language(&c, false, false));
    }

    #[test]
    fn bad_values_are_skipped_one_by_one() {
        let f = PortConfig::parse(
            "lang = \"en\"\nhud_layout = \"wide\"\nui_scaling = \"blurry\"\nui_scale = 9\n",
        )
        .unwrap();
        let c = layered(Some(&f), env(&[]), &mut vec![]).unwrap().0;
        assert_eq!(c.lang, "en");
        assert_eq!(c.hud_layout, HudLayout::Original);
        assert_eq!(c.ui_scaling, UiScaling::Smooth);
        assert_eq!(c.ui_scale_fixed, None);
    }

    /// The user's decided defaults (docs/settings.md), which are the behaviour before the
    /// switches existed: every enhancement on except the three the original's look keeps.
    const DECIDED: &[(&str, bool)] = &[
        ("wide_visibility", true),
        ("square_pixels", false),
        ("stable_track_variants", false),
        ("texture_filtering", false),
        ("ground_variation", false),
        ("ground_detiling", false),
        ("fog", true),
        ("wide_weather", true),
        ("far_traffic", true),
        ("max_lod", true),
        ("early_activation", true),
        ("distance_fade", true),
        ("fine_depth", true),
        ("interpolate", true),
        ("vsync", true),
        ("fullscreen", false),
        ("wide_menus", true),
        ("hud_upscale", false),
        ("one_line_captions", true),
        ("landmark_names", true),
        ("board_names", true),
        ("subtitles", true),
        ("save_on_change", true),
        ("save_timed_out_names", true),
        ("keyboard_with_controller", true),
        ("exit_no_to_menu", true),
    ];

    #[test]
    fn defaults_are_the_decided_ones() {
        let keys: Vec<&str> = DECIDED.iter().map(|(k, _)| *k).collect();
        assert_eq!(Settings::KEYS, keys.as_slice());
        let (c, s) = layered(None, env(&[]), &mut vec![]).unwrap();
        assert_eq!(s, Settings::default());
        for &(k, v) in DECIDED {
            assert_eq!(s.get(k), Some(v), "{k}");
        }
        // The other layers' defaults agree with the switches.
        assert!(crate::gamepad::PadOptions::default().keyboard_with_controller);
        assert!(
            crate::save::Save::new(
                crate::save::Eeprom::from_bytes([0; crate::save::EEPROM_LEN]),
                None
            )
            .save_on_change
        );
        assert_eq!(c.hud_layout, HudLayout::Original);
        assert_eq!(c.ui_scaling, UiScaling::Smooth);
        assert_eq!(c.ui_scale_fixed, None);
    }

    /// Every key of docs/settings.md's table is a setting, and every setting is in it.
    #[test]
    fn the_settings_table_lists_every_key() {
        let doc = include_str!("../../../docs/settings.md");
        let rows: Vec<&str> = doc
            .lines()
            .filter_map(|l| l.strip_prefix("| `")?.split('`').next())
            .collect();
        for k in Settings::KEYS.iter().chain(Settings::LEVEL_KEYS).chain(&[
            "lang",
            "hud_layout",
            "ui_scaling",
            "ui_scale",
            "font",
            "display_font",
            "texture_pack",
            "draw_distance",
        ]) {
            assert!(rows.contains(k), "{k} is not in docs/settings.md");
        }
        for k in Settings::KEYS {
            assert!(
                crate::USAGE.contains(&Settings::flag(k)),
                "{k} is not in USAGE"
            );
        }
        for r in &rows {
            let known = Settings::KEYS.contains(r)
                || Settings::LEVEL_KEYS.contains(r)
                || [
                    "lang",
                    "hud_layout",
                    "ui_scaling",
                    "ui_scale",
                    "font",
                    "display_font",
                    "texture_pack",
                    "draw_distance",
                ]
                .contains(r);
            assert!(known, "docs/settings.md lists {r}, which is no setting");
        }
    }

    #[test]
    fn every_switch_parses_from_file_env_and_flags() {
        for &(k, default) in DECIDED {
            let other = |s: &Settings| {
                Settings::KEYS
                    .iter()
                    .filter(|o| **o != k)
                    .all(|o| s.get(o) == Settings::default().get(o))
            };
            // The file.
            let f = PortConfig::parse(&format!("{k} = {}", !default)).unwrap();
            let (_, s) = layered(Some(&f), env(&[]), &mut vec![]).unwrap();
            assert_eq!(s.get(k), Some(!default), "file {k}");
            assert!(other(&s), "file {k} changed another key");
            // The environment.
            let var = Settings::env_var(k);
            let v = if default { "0" } else { "1" };
            let (_, s) = layered(None, env(&[(&var, v)]), &mut vec![]).unwrap();
            assert_eq!(s.get(k), Some(!default), "{var}");
            assert!(other(&s));
            // The flag pair.
            let flag = Settings::flag(k);
            let no = flag.replacen("--", "--no-", 1);
            for (a, want) in [(&flag, true), (&no, false)] {
                let mut a = args(&[a, "rest"]);
                let (_, s) = layered(None, env(&[]), &mut a).unwrap();
                assert_eq!(s.get(k), Some(want), "{flag}");
                assert_eq!(a, ["rest"]);
                assert!(other(&s));
            }
        }
        // The older flag names.
        for (flag, k) in [
            ("--original-visibility", "wide_visibility"),
            ("--original-weather", "wide_weather"),
            ("--original-traffic", "far_traffic"),
            ("--uncapped", "vsync"),
        ] {
            let (_, s) = layered(None, env(&[]), &mut args(&[flag])).unwrap();
            assert_eq!(s.get(k), Some(false), "{flag}");
        }
        let (_, s) = layered(None, env(&[]), &mut args(&["--square-pixels"])).unwrap();
        assert!(s.square_pixels);
        // Other values are left for the subcommands.
        let mut a = args(&["--no-such-thing", "--fogs", "--speed", "80"]);
        layered(None, env(&[]), &mut a).unwrap();
        assert_eq!(a, ["--no-such-thing", "--fogs", "--speed", "80"]);
    }

    #[test]
    fn levels_parse_from_file_env_and_flags() {
        let d = Settings::default();
        assert_eq!((d.msaa, d.anisotropic), (1, 1), "off by default");
        // The file: a number, false or "off"; a bad step loses only that key.
        let f = PortConfig::parse("msaa = 4\nanisotropic = 16\n").unwrap();
        let (_, s) = layered(Some(&f), env(&[]), &mut vec![]).unwrap();
        assert_eq!((s.msaa, s.anisotropic), (4, 16));
        let f = PortConfig::parse("msaa = 3\nanisotropic = \"off\"\n").unwrap();
        let (_, s) = layered(Some(&f), env(&[]), &mut vec![]).unwrap();
        assert_eq!((s.msaa, s.anisotropic), (1, 1));
        // The environment over the file, the flags over both, in all three spellings.
        let f = PortConfig::parse("msaa = 2\nanisotropic = 2\n").unwrap();
        let e = env(&[("DDG_MSAA", "8"), ("DDG_ANISOTROPIC", "4")]);
        let (_, s) = layered(Some(&f), e, &mut vec![]).unwrap();
        assert_eq!((s.msaa, s.anisotropic), (8, 4));
        for (a, want) in [
            (&["--msaa", "4", "rest"][..], 4),
            (&["--msaa=2", "rest"], 2),
            (&["--no-msaa", "rest"], 1),
        ] {
            let mut a = args(a);
            let e = env(&[("DDG_MSAA", "8")]);
            let (_, s) = layered(Some(&f), e, &mut a).unwrap();
            assert_eq!(s.msaa, want);
            assert_eq!(a, ["rest"]);
        }
        // Both lock the setting on the screen.
        let l = locks(env(&[("DDG_ANISOTROPIC", "8")]), &args(&["--msaa=4"]));
        assert_eq!(l.get("msaa"), Some(&Lock::Flag("--msaa=4".into())));
        assert_eq!(
            l.get("anisotropic"),
            Some(&Lock::Env("DDG_ANISOTROPIC".into()))
        );
        // fine_depth is the one shading switch decided on (renderer.md §6.1).
        assert_eq!(
            Settings {
                fine_depth: false,
                ..Settings::default()
            }
            .shading(),
            ddg_render::Shading::default(),
            "the renderer's default is the original too"
        );
    }

    #[test]
    fn switch_precedence_flag_env_file_default() {
        let file = PortConfig::parse("wide_weather = false\nfog = false\nvsync = false\n").unwrap();
        let (_, s) = layered(Some(&file), env(&[]), &mut vec![]).unwrap();
        assert!(!s.wide_weather && !s.fog && !s.vsync);
        assert!(s.far_traffic, "a key not in the file keeps its default");
        let e = env(&[("DDG_WIDE_WEATHER", "true"), ("DDG_FOG", "on")]);
        let (_, s) = layered(Some(&file), e, &mut vec![]).unwrap();
        assert!(s.wide_weather && s.fog && !s.vsync, "env over file");
        let e = env(&[("DDG_WIDE_WEATHER", "true"), ("DDG_FOG", "on")]);
        let mut a = args(&["--original-weather", "--no-fog", "--vsync"]);
        let (_, s) = layered(Some(&file), e, &mut a).unwrap();
        assert!(
            !s.wide_weather && !s.fog && s.vsync,
            "flags over env and file"
        );
        assert!(a.is_empty());
        // The last of a pair wins, like the other flags.
        let (_, s) = layered(None, env(&[]), &mut args(&["--no-fog", "--fog"])).unwrap();
        assert!(s.fog);
    }

    #[test]
    fn bad_switch_values_lose_only_that_setting() {
        let f =
            PortConfig::parse("lang = \"en\"\nfog = 3\nwide_menus = \"no\"\ninterpolate = false\n")
                .unwrap();
        let (c, s) = layered(Some(&f), env(&[]), &mut vec![]).unwrap();
        assert_eq!(c.lang, "en");
        assert!(s.fog && s.wide_menus, "bad values: the defaults");
        assert!(!s.interpolate);
        let (_, s) = layered(None, env(&[("DDG_FOG", "maybe")]), &mut vec![]).unwrap();
        assert!(s.fog);
        // The switches stay in the file when the game writes it.
        let d = temp_dir("switches");
        let p = d.join(FILE_NAME);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(&p, "fog = false\nexit_no_to_menu = false\n").unwrap();
        store_lang(&p, "en").unwrap();
        let (_, s) = layered(Some(&PortConfig::read(&p)), env(&[]), &mut vec![]).unwrap();
        assert!(!s.fog && !s.exit_no_to_menu);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn corrupt_file_gives_defaults() {
        let d = temp_dir("corrupt");
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join(FILE_NAME);
        for bad in ["lang = ", "lang = 3", "[[[", "ui_scale = \"two\""] {
            std::fs::write(&p, bad).unwrap();
            assert_eq!(PortConfig::read(&p), PortConfig::default(), "{bad}");
        }
        assert_eq!(
            PortConfig::read(&d.join("missing.toml")),
            PortConfig::default()
        );
        // Storing over a corrupt file keeps it aside and starts a new one.
        std::fs::write(&p, "[[[").unwrap();
        store_lang(&p, "en").unwrap();
        assert_eq!(PortConfig::read(&p).lang.as_deref(), Some("en"));
        assert_eq!(
            std::fs::read_to_string(d.join("config.toml.bad")).unwrap(),
            "[[["
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn store_round_trips_and_keeps_the_rest() {
        let d = temp_dir("store");
        let p = d.join("ddg").join(FILE_NAME);
        // A new file (and its directory).
        store_lang(&p, "ja").unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.starts_with('#'), "{text}");
        assert_eq!(PortConfig::read(&p).lang.as_deref(), Some("ja"));
        // An existing file: the line is replaced; comments, other and unknown keys stay.
        let before = "# mine\nhud_layout = \"enhanced\" # wide\nlang = \"ja\"\nfuture = 1\n\
                      \n[later]\nlang = \"not this one\"\n";
        std::fs::write(&p, before).unwrap();
        store_lang(&p, "en").unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text, before.replace("lang = \"ja\"", "lang = \"en\""));
        let c = PortConfig::read(&p);
        assert_eq!(c.lang.as_deref(), Some("en"));
        assert_eq!(c.hud_layout.as_deref(), Some("enhanced"));
        assert_eq!(c.other.get("future"), Some(&toml::Value::Integer(1)));
        assert!(c.other.contains_key("later"));
        // No lang yet: added before the first table.
        std::fs::write(&p, "ui_scale = 2\n[later]\nx = 1\n").unwrap();
        store_lang(&p, "en").unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text, "ui_scale = 2\nlang = \"en\"\n[later]\nx = 1\n");
        // Serde round trip of the struct.
        let c = PortConfig::read(&p);
        let again = PortConfig::parse(&toml::to_string(&c).unwrap()).unwrap();
        assert_eq!(again, c);
        std::fs::remove_dir_all(&d).unwrap();
    }
}

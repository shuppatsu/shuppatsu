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
    /// A fixed canvas multiple, 1..=4 (`--ui-scale`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_scale: Option<i64>,
    /// Font list for translated text (`--font`, `:`-separated).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font: Option<PathBuf>,
    /// Font list for display text (`--display-font`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_font: Option<PathBuf>,
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
    let old = match std::fs::read_to_string(path) {
        Ok(t) => Some(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| path.display().to_string()),
    };
    let text = match old {
        Some(t) if PortConfig::parse(&t).is_ok() => with_key(&t, "lang", lang),
        Some(_) => {
            let bad = path.with_extension("toml.bad");
            std::fs::rename(path, &bad).with_context(|| bad.display().to_string())?;
            eprintln!(
                "warning: {} was not valid; kept as {}",
                path.display(),
                bad.display()
            );
            with_key(HEADER, "lang", lang)
        }
        None => with_key(HEADER, "lang", lang),
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).with_context(|| tmp.display().to_string())?;
    std::fs::rename(&tmp, path).with_context(|| path.display().to_string())?;
    Ok(())
}

/// `text` (valid TOML) with the top-level string `key` set to `value`: its line replaced, or a
/// new line before the first table. Falls back to writing the parsed file out again (without
/// comments) if the edit does not read back as intended.
fn with_key(text: &str, key: &str, value: &str) -> String {
    let line = format!("{key} = {}", toml::Value::String(value.to_owned()));
    let is_key = |l: &str| {
        l.trim_start()
            .strip_prefix(key)
            .is_some_and(|rest| rest.trim_start().starts_with('='))
    };
    let mut lines: Vec<&str> = text.lines().collect();
    let first_table = lines
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .unwrap_or(lines.len());
    match lines[..first_table].iter().position(|l| is_key(l)) {
        Some(i) => lines[i] = &line,
        None => lines.insert(first_table, &line),
    }
    let mut out = lines.join("\n");
    out.push('\n');
    let reads_back = toml::from_str::<toml::Table>(&out)
        .ok()
        .and_then(|t| t.get(key)?.as_str().map(|v| v == value))
        .unwrap_or(false);
    if reads_back {
        return out;
    }
    let mut table: toml::Table = toml::from_str(text).unwrap_or_default();
    table.insert(key.to_owned(), toml::Value::String(value.to_owned()));
    toml::to_string(&table).unwrap_or(line + "\n")
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
        ("fog", true),
        ("wide_weather", true),
        ("far_traffic", true),
        ("interpolate", true),
        ("vsync", true),
        ("wide_menus", true),
        ("one_line_captions", true),
        ("landmark_names", true),
        ("board_names", true),
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
        for k in Settings::KEYS.iter().chain(&[
            "lang",
            "hud_layout",
            "ui_scaling",
            "ui_scale",
            "font",
            "display_font",
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
                || [
                    "lang",
                    "hud_layout",
                    "ui_scaling",
                    "ui_scale",
                    "font",
                    "display_font",
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

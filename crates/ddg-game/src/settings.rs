//! The port's on/off settings: one named switch per enhancement or decided deviation from the
//! original (docs/settings.md). The 2D layout, scaling, language and fonts are in
//! [`crate::locale::LocaleConfig`]; everything that is only on or off is here.
//!
//! Each switch has a `config.toml` key (its name), a `DDG_<NAME>` environment variable
//! (`1`/`0`, `true`/`false`, `on`/`off`) and a flag pair `--<name>` / `--no-<name>` (with `_`
//! as `-`), plus the older flag names kept as aliases (`--original-visibility`,
//! `--original-weather`, `--original-traffic`, `--uncapped`). Precedence: flag > environment >
//! file > default ([`crate::config::layered`]). The defaults are the user's decisions; turning
//! a switch off gives the original's behaviour for that one thing.

macro_rules! settings {
    ($( $(#[$doc:meta])* $name:ident = $default:expr; )*) => {
        /// The port's switches for a run, passed down with the options (no globals).
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct Settings {
            $( $(#[$doc])* pub $name: bool, )*
        }

        impl Default for Settings {
            fn default() -> Self {
                Self { $( $name: $default, )* }
            }
        }

        impl Settings {
            /// Every key, in the order of docs/settings.md.
            pub const KEYS: &[&str] = &[$( stringify!($name) ),*];

            /// The switch named `key`.
            pub fn get_mut(&mut self, key: &str) -> Option<&mut bool> {
                match key {
                    $( stringify!($name) => Some(&mut self.$name), )*
                    _ => None,
                }
            }

            /// The value of the switch named `key`.
            #[cfg(test)]
            pub fn get(&self, key: &str) -> Option<bool> {
                match key {
                    $( stringify!($name) => Some(self.$name), )*
                    _ => None,
                }
            }
        }
    };
}

settings! {
    /// Scenery and side tracks kept for the window's wider Hor+ view (scene.md §5.2); off =
    /// the original's 4:3 visibility lists (`--original-visibility`).
    wide_visibility = true;
    /// Square pixels in the 3D view; off (default) = the original's 7.9 % horizontal stretch
    /// (renderer.md §5).
    square_pixels = false;
    /// Side-track models that do not switch every point (scene.md §2.6); off (default) = the
    /// original's flicker.
    stable_track_variants = false;
    /// Mipmaps and trilinear/anisotropic filtering; off (default) = the original's nearest
    /// sampling.
    texture_filtering = false;
    /// The snow fog, as with DengoSetting's FOGENABLE (`snow_fog`, renderer.md §13.9); off = a
    /// fresh install of the original.
    fog = true;
    /// Raindrops and snow over the whole visible width (renderer.md §13.9); off = the
    /// original's 4:3 area (`--original-weather`).
    wide_weather = true;
    /// AI trains drawn to the far plane (renderer.md §13.9, traffic.md); off = up to 150 m
    /// (`--original-traffic`).
    far_traffic = true;
    /// Camera, AI trains, wiper, snow and speedometer needle between the last two ticks at the
    /// display's rate (renderer.md §13.9); off = every frame shows the last tick.
    interpolate = true;
    /// Present with vsync; off = uncapped (`--uncapped`).
    vsync = true;
    /// Menu backdrops continued to the edges of a wide window (menus.md §14); off = the 4:3
    /// menus centred over black.
    wide_menus = true;
    /// With a translation: captions and banners as one line of text (localisation.md); off =
    /// the sprites translated one by one.
    one_line_captions = true;
    /// With a translation: the landmark's name under its kanji plate (localisation.md); off =
    /// the plate alone.
    landmark_names = true;
    /// With a translation: romaji under the route board's kanji (localisation.md); off = kanji
    /// only.
    board_names = true;
    /// With a translation: subtitles for the Japanese voice lines (station, signal and
    /// speed-limit calls, announcements, navigator, time-up advice; localisation.md); off = the
    /// voices alone, as the original. Never shown in Japanese.
    subtitles = true;
    /// `eeprom.dat` written right after a ranking entry, a clear mark, an options change or a
    /// calibration (menus.md §14.1); off = only at exit, as the original.
    save_on_change = true;
    /// A name entry that times out is saved like a completed one (menus.md §14.1); off = the
    /// original's: shown, never written.
    save_timed_out_names = true;
    /// In controller modes the keyboard's horn (Space) and A / ATS confirm (Enter) work too
    /// (input.md §10.3); off = only Esc, Enter as menu confirm and R, as the original.
    keyboard_with_controller = true;
    /// The exit dialog's いいえ returns to the title menu on ゲーム終了 (menus.md §14.1); off =
    /// attract from the title picture, as the original.
    exit_no_to_menu = true;
}

/// A boolean as written in the environment or on a command line.
pub fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "on" | "yes" => Some(true),
        "0" | "false" | "off" | "no" => Some(false),
        _ => None,
    }
}

/// The older flag names: `(flag, key, value)`.
const ALIASES: &[(&str, &str, bool)] = &[
    ("--original-visibility", "wide_visibility", false),
    ("--original-weather", "wide_weather", false),
    ("--original-traffic", "far_traffic", false),
    ("--uncapped", "vsync", false),
];

impl Settings {
    /// The environment variable of `key`: `DDG_` and the key in capitals.
    pub fn env_var(key: &str) -> String {
        format!("DDG_{}", key.to_ascii_uppercase())
    }

    /// The flag of `key`: `--` and the key with `-` for `_`.
    pub fn flag(key: &str) -> String {
        format!("--{}", key.replace('_', "-"))
    }

    /// Sets the switches from `DDG_<KEY>` as looked up by `get`; a value that is not a boolean
    /// is reported and ignored.
    pub fn apply_vars(&mut self, get: impl Fn(&str) -> Option<std::ffi::OsString>) {
        for key in Self::KEYS {
            let var = Self::env_var(key);
            let Some(v) = get(&var).filter(|v| !v.is_empty()) else {
                continue;
            };
            match parse_bool(&v.to_string_lossy()) {
                Some(b) => *self.get_mut(key).expect("a key") = b,
                None => eprintln!("warning: {var}: expected 1 or 0 (true or false)"),
            }
        }
    }

    /// Takes the switches' flags out of `args` (`--x`, `--no-x` and the aliases) and leaves
    /// everything else.
    pub fn take_args(&mut self, args: &mut Vec<String>) {
        let mut rest = Vec::with_capacity(args.len());
        for a in std::mem::take(args) {
            match Self::flag_value(&a) {
                Some((key, v)) => *self.get_mut(key).expect("a key") = v,
                None => rest.push(a),
            }
        }
        *args = rest;
    }

    /// The switch and value a flag sets, if it is one of theirs.
    fn flag_value(flag: &str) -> Option<(&'static str, bool)> {
        if let Some(&(_, key, v)) = ALIASES.iter().find(|(f, _, _)| *f == flag) {
            return Some((key, v));
        }
        let (flag, v) = match flag.strip_prefix("--no-") {
            Some(name) => (format!("--{name}"), false),
            None => (flag.to_owned(), true),
        };
        Self::KEYS
            .iter()
            .find(|k| Self::flag(k) == flag)
            .map(|k| (*k, v))
    }

    /// Reads the switches from the file's table (`config.toml`); a value that is not a
    /// boolean is reported and loses only that setting.
    pub fn apply_table(&mut self, table: &toml::Table, file: &str) {
        for key in Self::KEYS {
            match table.get(*key) {
                None => {}
                Some(toml::Value::Boolean(b)) => *self.get_mut(key).expect("a key") = *b,
                Some(_) => eprintln!("warning: {file}: {key} must be true or false"),
            }
        }
    }
}

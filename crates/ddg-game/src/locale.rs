//! Localisation of the original's text sprites (docs/localisation.md).
//!
//! The original draws every string as tile art (docs/re/hud.md §4, menus.md §1.2), so the
//! Japanese "source text" is the art itself, read from the user's `tbl.rom`/`cg.rom` at runtime.
//! A catalogue (`locales/<lang>.toml`) maps a sprite's identity to replacement text:
//!
//! * `[tbl]`: a `tbl.rom` tilemap id (the whole map, or one `rect` of it);
//! * `[sheet.<tbl>]`: a label cut out of a string sheet, keyed by its index in the sheet's
//!   descriptor table in the exe ([`SHEETS`]), or by a name plus an explicit `rect` for sheets
//!   without a known table.
//!
//! `ja` is the identity (no overrides), and any sprite missing from a catalogue falls back to
//! the original art. Overrides are drawn by `ddg_render::text` with a font the user supplies
//! (`DDG_FONT` / `--font`); nothing changes unless a language other than `ja` and a font are
//! both configured.

// Not wired into the game yet; see docs/localisation.md for the integration calls.
#![allow(dead_code)]

use anyhow::{Context, Result, bail, ensure};
use ddg_formats::exe::Exe;
use ddg_render::sprites::SpriteBank;
use ddg_render::text::{Align, FontGlyphs, Style, SubRect, TextOverlay, TextOverride, TextPart};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Catalogues compiled into the binary (our own translations; no game data).
const BUILTIN: &[(&str, &str)] = &[("en", include_str!("../../../locales/en.toml"))];

/// The original language: the art as it is.
pub const IDENTITY_LANG: &str = "ja";

/// String sheets with descriptor tables in the exe: `(sheet tbl, [(table VA, entries)])`.
/// A sheet's descriptor index counts through its tables in order. Each descriptor is
/// `{u16 tbl, i8 col, row, w, h}` (menus.md §1.2).
pub const SHEETS: &[(u16, &[(u32, usize)])] = &[
    // Title menu items, 6 tiles high each (0x47c778, menus.md §3).
    (0x513, &[(0x47c778, 5)]),
    // Options labels and values, selected / normal colouring (0x47c584 / 0x47c664, §4).
    (0x4fa, &[(0x47c584, 37)]),
    (0x4fb, &[(0x47c664, 37)]),
    // Pause and dialog texts (0x48bbb0, §8).
    (0x544, &[(0x48bbb0, 29)]),
    // Screen headers: 路線選択 (0x48c584), 電車選択 (0x48c58a) (§6.1).
    (0x54d, &[(0x48c584, 1), (0x48c58a, 1)]),
    // Name-entry legends, keyboard (0x486184) and lever (0x48618a) (§11.2).
    (0x53f, &[(0x486184, 1), (0x48618a, 1)]),
];

fn sheet_len(tbl: u16) -> Option<usize> {
    SHEETS
        .iter()
        .find(|(t, _)| *t == tbl)
        .map(|(_, tables)| tables.iter().map(|(_, n)| n).sum())
}

/// The descriptor sub-rectangles of every sheet in [`SHEETS`], read from the user's exe.
#[derive(Debug, Clone, Default)]
pub struct SheetTables {
    pub descriptors: HashMap<u16, Vec<SubRect>>,
}

impl SheetTables {
    pub fn read(exe: &Exe<'_>) -> Result<Self> {
        let mut descriptors = HashMap::new();
        for &(sheet, tables) in SHEETS {
            let mut rects = Vec::new();
            for &(va, n) in tables {
                let r = exe.reader_at(va, 6 * n, "sheet descriptors")?;
                for i in 0..n {
                    let tbl = r.u16(6 * i)?;
                    ensure!(
                        tbl == sheet,
                        "descriptor {va:#x}+{i} names tbl {tbl:#x}, expected sheet {sheet:#x}"
                    );
                    rects.push(SubRect {
                        col: r.i8(6 * i + 2)?,
                        row: r.i8(6 * i + 3)?,
                        w: r.i8(6 * i + 4)?,
                        h: r.i8(6 * i + 5)?,
                    });
                }
            }
            descriptors.insert(sheet, rects);
        }
        Ok(Self { descriptors })
    }
}

/// Where a catalogue entry applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    /// A tilemap, as a whole or one explicit rectangle of it.
    Tbl { tbl: u16, rect: SubRect },
    /// Descriptor `index` of a sheet in [`SHEETS`].
    Sheet { tbl: u16, index: usize },
    /// A named rectangle of a sheet.
    Rect {
        tbl: u16,
        name: String,
        rect: SubRect,
    },
}

impl Key {
    pub fn tbl(&self) -> u16 {
        match self {
            Key::Tbl { tbl, .. } | Key::Sheet { tbl, .. } | Key::Rect { tbl, .. } => *tbl,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub key: Key,
    pub text: TextOverride,
}

/// A parsed `locales/<lang>.toml`.
#[derive(Debug, Clone, PartialEq)]
pub struct Catalogue {
    pub language: String,
    pub name: String,
    pub entries: Vec<Entry>,
}

impl Catalogue {
    /// Parses and checks a catalogue. Every key must be a valid tbl id or a known sheet
    /// descriptor; unknown fields are errors, so typos do not silently fall back to the art.
    pub fn parse(src: &str) -> Result<Self> {
        let root: toml::Table = src.parse().context("catalogue is not valid TOML")?;
        let mut language = None;
        let mut name = None;
        let mut entries = Vec::new();
        for (section, value) in &root {
            match section.as_str() {
                "meta" => {
                    let t = value.as_table().context("[meta] must be a table")?;
                    for (k, v) in t {
                        let s = v
                            .as_str()
                            .with_context(|| format!("meta.{k} must be a string"));
                        match k.as_str() {
                            "language" => language = Some(s?.to_owned()),
                            "name" => name = Some(s?.to_owned()),
                            _ => bail!("unknown field meta.{k}"),
                        }
                    }
                }
                "tbl" => {
                    let t = value.as_table().context("[tbl] must be a table")?;
                    for (k, v) in t {
                        let tbl = parse_id(k).with_context(|| format!("tbl key {k:?}"))?;
                        let (text, rect) = parse_entry(v).with_context(|| format!("tbl.{k}"))?;
                        let rect = rect.unwrap_or(SubRect::WHOLE).canonical();
                        entries.push(Entry {
                            key: Key::Tbl { tbl, rect },
                            text,
                        });
                    }
                }
                "sheet" => {
                    let sheets = value.as_table().context("[sheet] must be a table")?;
                    for (sk, sv) in sheets {
                        let tbl = parse_id(sk).with_context(|| format!("sheet key {sk:?}"))?;
                        let t = sv.as_table().with_context(|| format!("sheet.{sk}"))?;
                        for (k, v) in t {
                            let ctx = || format!("sheet.{sk}.{k}");
                            let (text, rect) = parse_entry(v).with_context(ctx)?;
                            let key = if let Ok(index) = k.parse::<usize>() {
                                let len = sheet_len(tbl).with_context(|| {
                                    format!("{}: sheet {tbl:#x} has no descriptor table", ctx())
                                })?;
                                ensure!(index < len, "{}: index ≥ {len}", ctx());
                                ensure!(rect.is_none(), "{}: rect on a descriptor", ctx());
                                Key::Sheet { tbl, index }
                            } else {
                                let rect = rect.with_context(|| {
                                    format!("{}: a named sheet entry needs a rect", ctx())
                                })?;
                                Key::Rect {
                                    tbl,
                                    name: k.clone(),
                                    rect: rect.canonical(),
                                }
                            };
                            entries.push(Entry { key, text });
                        }
                    }
                }
                _ => bail!("unknown section [{section}]"),
            }
        }
        Ok(Self {
            language: language.context("meta.language missing")?,
            name: name.context("meta.name missing")?,
            entries,
        })
    }
}

/// `0x206` or `518`.
fn parse_id(s: &str) -> Result<u16> {
    Ok(match s.strip_prefix("0x") {
        Some(hex) => u16::from_str_radix(hex, 16)?,
        None => s.parse()?,
    })
}

fn parse_rect(v: &toml::Value) -> Result<SubRect> {
    let a = v.as_array().context("rect must be [col, row, w, h]")?;
    ensure!(a.len() == 4, "rect must have 4 numbers");
    let n = |i: usize| -> Result<i8> {
        let x = a[i].as_integer().context("rect values must be integers")?;
        Ok(i8::try_from(x)?)
    };
    Ok(SubRect {
        col: n(0)?,
        row: n(1)?,
        w: n(2)?,
        h: n(3)?,
    })
}

/// A plain string, or a table with `text`/`parts` and options.
fn parse_entry(v: &toml::Value) -> Result<(TextOverride, Option<SubRect>)> {
    if let Some(s) = v.as_str() {
        ensure!(!s.trim().is_empty(), "empty text");
        return Ok((
            TextOverride {
                parts: vec![part_with_text(s)],
            },
            None,
        ));
    }
    let t = v.as_table().context("entry must be a string or a table")?;
    let mut rect = None;
    let mut defaults = TextPart {
        outline: true,
        ..TextPart::default()
    };
    let mut text = None;
    let mut parts_value = None;
    for (k, v) in t {
        match k.as_str() {
            "rect" => rect = Some(parse_rect(v)?),
            "text" => text = Some(v.as_str().context("text must be a string")?.to_owned()),
            "parts" => parts_value = Some(v),
            _ => apply_option(&mut defaults, k, v)?,
        }
    }
    let parts = match (text, parts_value) {
        (Some(text), None) => vec![TextPart { text, ..defaults }],
        (None, Some(p)) => {
            let list = p.as_array().context("parts must be an array of tables")?;
            ensure!(!list.is_empty(), "parts is empty");
            list.iter()
                .map(|p| {
                    let t = p.as_table().context("each part must be a table")?;
                    let mut part = defaults.clone();
                    let mut text = None;
                    for (k, v) in t {
                        match k.as_str() {
                            "text" => text = Some(v.as_str().context("text")?.to_owned()),
                            _ => apply_option(&mut part, k, v)?,
                        }
                    }
                    part.text = text.context("part without text")?;
                    ensure!(part.region.is_some(), "a part needs a region");
                    Ok(part)
                })
                .collect::<Result<Vec<_>>>()?
        }
        (Some(_), Some(_)) => bail!("give either text or parts, not both"),
        (None, None) => bail!("entry without text"),
    };
    for p in &parts {
        ensure!(!p.text.trim().is_empty(), "empty text");
    }
    Ok((TextOverride { parts }, rect))
}

fn part_with_text(s: &str) -> TextPart {
    TextPart {
        text: s.to_owned(),
        outline: true,
        ..TextPart::default()
    }
}

fn apply_option(p: &mut TextPart, k: &str, v: &toml::Value) -> Result<()> {
    match k {
        "region" => {
            let a = v
                .as_array()
                .context("region must be [x, y, w, h] in pixels")?;
            ensure!(a.len() == 4, "region must have 4 numbers");
            let mut r = [0u16; 4];
            for (o, x) in r.iter_mut().zip(a) {
                *o = u16::try_from(x.as_integer().context("region values must be integers")?)?;
            }
            ensure!(r[2] > 0 && r[3] > 0, "empty region");
            p.region = Some(r);
        }
        "align" => {
            p.align = match v.as_str() {
                Some("left") => Align::Left,
                Some("center") => Align::Center,
                Some("right") => Align::Right,
                _ => bail!("align must be left, center or right"),
            }
        }
        "style" => {
            p.style = match v.as_str() {
                Some("auto") => Style::Auto,
                Some("plain") => Style::Plain,
                Some("boxed") => Style::Boxed,
                _ => bail!("style must be auto, plain or boxed"),
            }
        }
        "vertical" => p.vertical = v.as_bool().context("vertical must be a boolean")?,
        "outline" => p.outline = v.as_bool().context("outline must be a boolean")?,
        _ => bail!("unknown field {k}"),
    }
    Ok(())
}

/// The overrides of one language, keyed by sprite identity.
#[derive(Debug, Clone, Default)]
pub struct Locale {
    pub language: String,
    map: HashMap<(u16, SubRect), TextOverride>,
}

impl Locale {
    /// The original art everywhere.
    pub fn identity() -> Self {
        Self {
            language: IDENTITY_LANG.to_owned(),
            map: HashMap::new(),
        }
    }

    /// Resolves sheet descriptors to rectangles. Two entries for the same sprite are an error.
    pub fn from_catalogue(cat: &Catalogue, sheets: &SheetTables) -> Result<Self> {
        let mut map = HashMap::new();
        for e in &cat.entries {
            let (tbl, rect) = match &e.key {
                Key::Tbl { tbl, rect } | Key::Rect { tbl, rect, .. } => (*tbl, *rect),
                Key::Sheet { tbl, index } => {
                    let rect = sheets
                        .descriptors
                        .get(tbl)
                        .and_then(|d| d.get(*index))
                        .with_context(|| format!("sheet {tbl:#x} descriptor {index} not read"))?;
                    (*tbl, rect.canonical())
                }
            };
            ensure!(
                map.insert((tbl, rect), e.text.clone()).is_none(),
                "two entries for tbl {tbl:#x} {rect:?} ({:?})",
                e.key
            );
        }
        Ok(Self {
            language: cat.language.clone(),
            map,
        })
    }

    /// Loads `lang`: `ja` is the identity; otherwise `<dir>/<lang>.toml` when a directory is
    /// given and the file exists, else the built-in catalogue.
    pub fn load(lang: &str, dir: Option<&Path>, sheets: &SheetTables) -> Result<Self> {
        if lang == IDENTITY_LANG {
            return Ok(Self::identity());
        }
        let file = dir.map(|d| d.join(format!("{lang}.toml")));
        let src = match file.filter(|f| f.is_file()) {
            Some(f) => {
                std::fs::read_to_string(&f).with_context(|| format!("reading {}", f.display()))?
            }
            None => BUILTIN
                .iter()
                .find(|(l, _)| *l == lang)
                .map(|(_, s)| (*s).to_owned())
                .with_context(|| format!("no catalogue for language {lang:?}"))?,
        };
        let cat = Catalogue::parse(&src).with_context(|| format!("catalogue {lang}"))?;
        ensure!(
            cat.language == lang,
            "catalogue for {lang:?} says language {:?}",
            cat.language
        );
        Self::from_catalogue(&cat, sheets)
    }

    pub fn is_identity(&self) -> bool {
        self.map.is_empty()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// The override for a slot showing `rect` of tilemap `tbl`; `None` = draw the art.
    pub fn lookup(&self, tbl: i16, rect: SubRect) -> Option<&TextOverride> {
        let tbl = u16::try_from(tbl).ok()?;
        self.map.get(&(tbl, rect.canonical()))
    }

    pub fn overrides(&self) -> impl Iterator<Item = (u16, SubRect, &TextOverride)> {
        self.map.iter().map(|(&(t, r), o)| (t, r, o))
    }
}

/// Language and font selection. Off (`ja`, no font) unless configured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocaleConfig {
    pub lang: String,
    /// TTF/OTF font files, `:`-separated like `PATH`: the primary font, then fallbacks for
    /// characters it lacks. No font ships with the project.
    pub font: Option<PathBuf>,
    /// Extra catalogue directory (`<dir>/<lang>.toml` overrides the built-in one).
    pub dir: Option<PathBuf>,
    /// Enhancement `ui_scale`: the 2D layer is composed at 640×480 × this (1 = the original).
    /// The art is enlarged with nearest neighbour; translated text gets the extra pixels.
    pub ui_scale: usize,
}

impl Default for LocaleConfig {
    fn default() -> Self {
        Self {
            lang: IDENTITY_LANG.to_owned(),
            font: None,
            dir: None,
            ui_scale: 1,
        }
    }
}

impl LocaleConfig {
    /// `DDG_LANG`, `DDG_FONT`, `DDG_LOCALE_DIR`, `DDG_UI_SCALE`.
    pub fn from_env() -> Self {
        let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
        let mut c = Self::default();
        if let Some(l) = var("DDG_LANG") {
            c.lang = l.to_string_lossy().into_owned();
        }
        c.font = var("DDG_FONT").map(PathBuf::from);
        c.dir = var("DDG_LOCALE_DIR").map(PathBuf::from);
        if let Some(s) = var("DDG_UI_SCALE").and_then(|v| v.to_str()?.parse().ok()) {
            c.ui_scale = clamp_scale(s);
        }
        c
    }

    /// Takes `--lang <code>`, `--font <file>`, `--locale-dir <dir>` and `--ui-scale <n>` out of `args`
    /// (they override the environment) and leaves everything else.
    pub fn take_args(&mut self, args: &mut Vec<String>) -> Result<()> {
        let mut rest = Vec::with_capacity(args.len());
        let mut it = std::mem::take(args).into_iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--lang" => self.lang = it.next().context("--lang needs a code")?,
                "--font" => self.font = Some(it.next().context("--font needs a file")?.into()),
                "--locale-dir" => {
                    self.dir = Some(it.next().context("--locale-dir needs a dir")?.into())
                }
                "--ui-scale" => {
                    let n = it.next().context("--ui-scale needs 1..=4")?;
                    self.ui_scale = clamp_scale(n.parse().context("--ui-scale needs 1..=4")?);
                }
                _ => rest.push(a),
            }
        }
        *args = rest;
        Ok(())
    }

    /// Whether text overrides will be drawn.
    pub fn active(&self) -> bool {
        self.lang != IDENTITY_LANG && self.font.is_some()
    }
}

/// UI scales beyond 4 only cost memory (a 2560×1920 canvas at 4).
fn clamp_scale(s: usize) -> usize {
    s.clamp(1, 4)
}

/// Builds the overlay for `cfg` and installs it on `bank`. Returns the number of sprites
/// overridden (0 for `ja`). A language without a font keeps the original art, with a warning.
pub fn install(cfg: &LocaleConfig, exe: &Exe<'_>, bank: &mut SpriteBank) -> Result<usize> {
    if cfg.lang == IDENTITY_LANG {
        bank.set_text(None);
        return Ok(0);
    }
    let Some(font_path) = &cfg.font else {
        eprintln!(
            "language {:?} needs a font (DDG_FONT or --font); showing the original art",
            cfg.lang
        );
        bank.set_text(None);
        return Ok(0);
    };
    let sheets = SheetTables::read(exe)?;
    let locale = Locale::load(&cfg.lang, cfg.dir.as_deref(), &sheets)?;
    // A `:`-separated list (`;` on Windows), like PATH: the primary font, then fallbacks.
    let paths: Vec<PathBuf> = std::env::split_paths(font_path).collect();
    let font = FontGlyphs::load_all(&paths)?;
    let overlay = TextOverlay::build_scaled(bank, locale.overrides(), &font, cfg.ui_scale);
    let n = overlay.len();
    bank.set_text(Some(overlay));
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn en() -> Catalogue {
        Catalogue::parse(BUILTIN[0].1).expect("en.toml parses")
    }

    /// Descriptor tables with plausible rectangles, for tests without the exe.
    fn fake_sheets() -> SheetTables {
        let descriptors = SHEETS
            .iter()
            .map(|&(t, tables)| {
                let n: usize = tables.iter().map(|(_, n)| n).sum();
                let rects = (0..n)
                    .map(|i| SubRect {
                        col: 0,
                        row: (2 * i) as i8,
                        w: -1,
                        h: 2,
                    })
                    .collect();
                (t, rects)
            })
            .collect();
        SheetTables { descriptors }
    }

    #[test]
    fn every_catalogue_key_parses() {
        let cat = en();
        assert_eq!(cat.language, "en");
        assert!(cat.entries.len() > 300, "{} entries", cat.entries.len());
        // Resolving catches duplicate sprites.
        let l = Locale::from_catalogue(&cat, &fake_sheets()).unwrap();
        assert_eq!(l.len(), cat.entries.len());
        // Spot checks: a HUD label, a sheet descriptor, a named sheet rect.
        assert!(l.lookup(0x206, SubRect::WHOLE).is_some());
        assert!(
            cat.entries
                .iter()
                .any(|e| matches!(e.key, Key::Sheet { tbl: 0x544, .. }))
        );
        assert!(
            cat.entries
                .iter()
                .any(|e| matches!(e.key, Key::Rect { tbl: 0x54d, .. }))
        );
        for e in &cat.entries {
            for p in &e.text.parts {
                assert!(!p.text.contains('\t'), "{:?}", e.key);
            }
        }
    }

    #[test]
    fn rejects_bad_catalogues() {
        let head = "[meta]\nlanguage = \"xx\"\nname = \"X\"\n";
        for bad in [
            "[tbl]\nzz = \"a\"",
            "[tbl]\n0x10 = { text = \"a\", colour = 3 }",
            "[tbl]\n0x10 = { parts = [{ text = \"a\" }] }",
            "[sheet.0x10]\n0 = \"a\"",
            "[sheet.0x544]\n99 = \"a\"",
            "[sheet.0x54d]\nheader = \"a\"",
            "[tbl]\n0x10 = \"\"",
            "[other]\n",
        ] {
            assert!(Catalogue::parse(&format!("{head}{bad}")).is_err(), "{bad}");
        }
        let dup = format!(
            "{head}[tbl]\n0x10 = \"a\"\n[sheet.0x10]\nx = {{ rect = [0, 0, 0, 0], text = \"b\" }}"
        );
        let cat = Catalogue::parse(&dup).unwrap();
        assert!(Locale::from_catalogue(&cat, &fake_sheets()).is_err());
    }

    #[test]
    fn fallback_to_the_art() {
        let sheets = fake_sheets();
        let en = Locale::load("en", None, &sheets).unwrap();
        // Uncatalogued sprites (digits, pictures) and hidden slots draw the art.
        assert!(en.lookup(0x401, SubRect::WHOLE).is_none());
        assert!(en.lookup(-1, SubRect::WHOLE).is_none());
        // A catalogued map shown through a different sub-rect is not the catalogued sprite.
        let other = SubRect {
            col: 1,
            row: 0,
            w: 1,
            h: 1,
        };
        assert!(en.lookup(0x206, other).is_none());
        // Every spelling of "the whole map" matches.
        let whole = SubRect {
            col: -1,
            row: 0,
            w: -1,
            h: 0,
        };
        assert!(en.lookup(0x206, whole).is_some());
        // Japanese is the identity.
        let ja = Locale::load("ja", None, &sheets).unwrap();
        assert!(ja.is_identity());
        assert!(ja.lookup(0x206, SubRect::WHOLE).is_none());
        assert!(Locale::load("xx", None, &sheets).is_err());
    }

    #[test]
    fn config_from_args() {
        let mut c = LocaleConfig::default();
        assert!(!c.active());
        let mut args: Vec<String> = ["play", "3", "--lang", "en", "--font", "/f.ttf", "--hud"]
            .map(String::from)
            .to_vec();
        c.take_args(&mut args).unwrap();
        assert_eq!(args, ["play", "3", "--hud"]);
        assert_eq!(c.lang, "en");
        assert!(c.active());
        assert!(c.take_args(&mut vec!["--font".into()]).is_err());
    }

    /// Against the user's data: every catalogued tbl id exists in `tbl.rom`, every sheet
    /// descriptor resolves, and every region lies inside its sprite.
    #[test]
    fn catalogue_matches_the_data() {
        let Some(root) = std::env::var_os("DDG_DATA_DIR") else {
            eprintln!("skipped: DDG_DATA_DIR not set");
            return;
        };
        let install = PathBuf::from(root).join("install/Program_Executable_Files");
        let Ok(tbl) = std::fs::read(install.join("DATA/tbl.rom")) else {
            eprintln!("skipped: no tbl.rom under DDG_DATA_DIR");
            return;
        };
        let maps = ddg_formats::cg::parse_tbl(&tbl).unwrap();
        let exe_bytes = std::fs::read(install.join("BIN/dengo3k.exe")).unwrap();
        let exe = Exe::parse(&exe_bytes).unwrap();
        let sheets = SheetTables::read(&exe).unwrap();
        let cat = en();
        let locale = Locale::from_catalogue(&cat, &sheets).unwrap();
        for (tbl, rect, o) in locale.overrides() {
            let map = maps
                .get(usize::from(tbl))
                .unwrap_or_else(|| panic!("tbl {tbl:#x} not in tbl.rom ({})", maps.len()));
            let (_, _, w, h) = rect
                .resolve(map.width, map.height)
                .unwrap_or_else(|| panic!("tbl {tbl:#x}: {rect:?} outside the map"));
            let (pw, ph) = (u16::from(w) * 16, u16::from(h) * 16);
            for p in &o.parts {
                if let Some([x, y, rw, rh]) = p.region {
                    assert!(
                        x + rw <= pw && y + rh <= ph,
                        "tbl {tbl:#x}: region {:?} outside {pw}×{ph}",
                        p.region
                    );
                }
            }
            // Catalogued sprites have art to replace.
            assert!(map.tiles.iter().any(|&t| t != 0), "tbl {tbl:#x} is empty");
        }
    }
}

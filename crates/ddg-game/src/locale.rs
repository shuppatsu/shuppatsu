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
use ddg_render::label::LabelFont;
use ddg_render::sprites::SpriteBank;
use ddg_render::text::{
    Align, Backdrop, FontGlyphs, FontRole, Fonts, Style, SubRect, TextOverlay, TextOverride,
    TextPart,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Catalogues compiled into the binary (our own translations; no game data).
const BUILTIN: &[(&str, &str)] = &[
    ("en", include_str!("../../../locales/en.toml")),
    // Only the port's own strings (`[ui]`): `ja` draws the original art.
    ("ja", include_str!("../../../locales/ja.toml")),
];

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
    // Legend words 選択 / キャンセル / 決定 / おわり / モード切替 of the menu header 0x42a820
    // (0x47c538, §1.2); the route-select header's table 0x47cac4 holds the same five.
    (0x50b, &[(0x47c538, 5)]),
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

    /// The tilemap and canonical source rectangle the entry applies to; sheet descriptors are
    /// looked up in `sheets`.
    pub fn resolve(&self, sheets: &SheetTables) -> Result<(u16, SubRect)> {
        Ok(match self {
            Key::Tbl { tbl, rect } | Key::Rect { tbl, rect, .. } => (*tbl, *rect),
            Key::Sheet { tbl, index } => {
                let rect = sheets
                    .descriptors
                    .get(tbl)
                    .and_then(|d| d.get(*index))
                    .with_context(|| format!("sheet {tbl:#x} descriptor {index} not read"))?;
                (*tbl, rect.canonical())
            }
        })
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
    /// `[station_plate]`: translated station names by vertical plate tilemap (hud.md §3.7),
    /// shown by the horizontal plate layout (`hud::plates_en`), not as sprite overrides.
    pub station_plates: Vec<(u16, String)>,
    /// `[landmark]`: translated landmark names by landmark sprite tilemap (0x466a20, hud.md
    /// §3.13), shown on a line under the original caption (`captions::LandmarkLines`), not as
    /// sprite overrides.
    pub landmarks: Vec<(u16, String)>,
    /// `[caption]`: one-line phrases of the station captions and banners by kind
    /// ([`CAPTION_KINDS`]), with `{name}` for the station name (docs/localisation.md,
    /// "Captions and banners").
    pub captions: Vec<(String, String)>,
    /// `[caption]` `font` and `banner_font`: the font roles of the one-line captions and of
    /// the banners (body unless set).
    pub caption_fonts: [FontRole; 2],
    /// `[caption]` `staff_kanji`: how a staff-roll name's kanji follow it, `{kanji}` for them
    /// (setting `staff_kanji`; `play/staff_roll.rs`), `None` = the kanji alone.
    pub staff_kanji: Option<String>,
    /// `[subtitle]`: templates and lines of the subtitles for the Japanese voice lines, by key
    /// (`subtitles.rs`, docs/localisation.md "Subtitles").
    pub subtitles: Vec<(String, String)>,
    /// `[ui]`: the port's own screens (the settings screen), by string ID
    /// ([`crate::front::port_settings::known_ui_key`]).
    pub ui: Vec<(String, String)>,
}

/// The `[caption]` keys, by caption kind 0–6 (hud.md §3.13: 出発進行, 出発減速, 出発注意,
/// 出発警戒, 通過, 停車, none).
pub const CAPTION_KINDS: [&str; 7] = [
    "depart_clear",
    "depart_reduce",
    "depart_caution",
    "depart_restrict",
    "pass",
    "stop",
    "name",
];

impl Catalogue {
    /// `<dir>/<lang>.toml` when a directory is given and the file exists, else the built-in
    /// catalogue of `lang`.
    pub fn load(lang: &str, dir: Option<&Path>) -> Result<Self> {
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
        let cat = Self::parse(&src).with_context(|| format!("catalogue {lang}"))?;
        ensure!(
            cat.language == lang,
            "catalogue for {lang:?} says language {:?}",
            cat.language
        );
        Ok(cat)
    }

    /// Parses and checks a catalogue. Every key must be a valid tbl id or a known sheet
    /// descriptor; unknown fields are errors, so typos do not silently fall back to the art.
    pub fn parse(src: &str) -> Result<Self> {
        let root: toml::Table = src.parse().context("catalogue is not valid TOML")?;
        let mut language = None;
        let mut name = None;
        let mut entries = Vec::new();
        let mut station_plates = Vec::new();
        let mut landmarks = Vec::new();
        let mut captions = Vec::new();
        let mut caption_fonts = [FontRole::Body; 2];
        let mut staff_kanji = None;
        let mut subtitles = Vec::new();
        let mut ui = Vec::new();
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
                "station_plate" => {
                    let t = value
                        .as_table()
                        .context("[station_plate] must be a table")?;
                    for (k, v) in t {
                        let tbl =
                            parse_id(k).with_context(|| format!("station_plate key {k:?}"))?;
                        let s = v.as_str().with_context(|| format!("station_plate.{k}"))?;
                        ensure!(!s.trim().is_empty(), "station_plate.{k}: empty text");
                        station_plates.push((tbl, s.to_owned()));
                    }
                }
                "landmark" => {
                    let t = value.as_table().context("[landmark] must be a table")?;
                    for (k, v) in t {
                        let tbl = parse_id(k).with_context(|| format!("landmark key {k:?}"))?;
                        let s = v.as_str().with_context(|| format!("landmark.{k}"))?;
                        ensure!(!s.trim().is_empty(), "landmark.{k}: empty text");
                        landmarks.push((tbl, s.to_owned()));
                    }
                }
                "caption" => {
                    let t = value.as_table().context("[caption] must be a table")?;
                    for (k, v) in t {
                        let ctx = || format!("caption.{k}");
                        match k.as_str() {
                            "font" => {
                                caption_fonts[0] = parse_role(v).with_context(ctx)?;
                                continue;
                            }
                            "banner_font" => {
                                caption_fonts[1] = parse_role(v).with_context(ctx)?;
                                continue;
                            }
                            "staff_kanji" => {
                                let s = v.as_str().with_context(ctx)?;
                                ensure!(s.contains("{kanji}"), "caption.staff_kanji: no {{kanji}}");
                                staff_kanji = Some(s.to_owned());
                                continue;
                            }
                            _ => {}
                        }
                        ensure!(
                            CAPTION_KINDS.contains(&k.as_str()),
                            "unknown caption kind {k:?}"
                        );
                        let s = v.as_str().with_context(|| format!("caption.{k}"))?;
                        ensure!(!s.trim().is_empty(), "caption.{k}: empty text");
                        captions.push((k.clone(), s.to_owned()));
                    }
                }
                "subtitle" => {
                    let t = value.as_table().context("[subtitle] must be a table")?;
                    for (k, v) in t {
                        ensure!(crate::subtitles::known_key(k), "unknown subtitle key {k:?}");
                        let s = v.as_str().with_context(|| format!("subtitle.{k}"))?;
                        ensure!(!s.trim().is_empty(), "subtitle.{k}: empty text");
                        subtitles.push((k.clone(), s.to_owned()));
                    }
                }
                "ui" => {
                    let t = value.as_table().context("[ui] must be a table")?;
                    for (k, v) in t {
                        ensure!(
                            crate::front::port_settings::known_ui_key(k),
                            "unknown ui key {k:?}"
                        );
                        let s = v.as_str().with_context(|| format!("ui.{k}"))?;
                        ensure!(!s.trim().is_empty(), "ui.{k}: empty text");
                        ui.push((k.clone(), s.to_owned()));
                    }
                }
                _ => bail!("unknown section [{section}]"),
            }
        }
        Ok(Self {
            language: language.context("meta.language missing")?,
            name: name.context("meta.name missing")?,
            entries,
            station_plates,
            landmarks,
            captions,
            caption_fonts,
            staff_kanji,
            subtitles,
            ui,
        })
    }

    /// The text of every whole-tilemap entry with a single part, by tilemap id (the station
    /// names of the name sprites, among others).
    pub fn tbl_texts(&self) -> HashMap<u16, String> {
        self.entries
            .iter()
            .filter_map(|e| match (&e.key, e.text.parts.as_slice()) {
                (Key::Tbl { tbl, rect }, [p]) if *rect == SubRect::WHOLE.canonical() => {
                    Some((*tbl, p.text.clone()))
                }
                _ => None,
            })
            .collect()
    }

    /// The parts of every whole-tilemap entry, by tilemap id (the ending's staff credits read
    /// their texts, regions and shared groups: `staff_roll`).
    pub fn tbl_parts(&self) -> HashMap<u16, Vec<TextPart>> {
        self.entries
            .iter()
            .filter_map(|e| match &e.key {
                Key::Tbl { tbl, rect } if *rect == SubRect::WHOLE.canonical() => {
                    Some((*tbl, e.text.parts.clone()))
                }
                _ => None,
            })
            .collect()
    }
}

/// One-line captions for a translation: phrases by kind and texts by tilemap id.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CaptionPhrases {
    pub by_kind: [Option<String>; 7],
    pub tbl_texts: HashMap<u16, String>,
    /// Every whole-tilemap entry's parts (the ending's staff credits: `staff_roll`).
    pub tbl_parts: HashMap<u16, Vec<TextPart>>,
    /// Font roles of the captions and of the banners.
    pub fonts: [FontRole; 2],
    /// How a staff-roll name's kanji follow it (`{kanji}`; `[caption]` `staff_kanji`).
    pub staff_kanji: String,
}

/// The `[caption]` phrases of `cfg`'s catalogue, or `None` unless text overrides are drawn
/// ([`LocaleConfig::active`]) and the catalogue has them.
pub fn caption_phrases(cfg: &LocaleConfig) -> Result<Option<CaptionPhrases>> {
    if !cfg.active() {
        return Ok(None);
    }
    let cat = Catalogue::load(&cfg.lang, cfg.dir.as_deref())?;
    if cat.captions.is_empty() {
        return Ok(None);
    }
    let mut by_kind: [Option<String>; 7] = Default::default();
    for (k, v) in cat.captions.iter() {
        if let Some(i) = CAPTION_KINDS.iter().position(|c| c == k) {
            by_kind[i] = Some(v.clone());
        }
    }
    Ok(Some(CaptionPhrases {
        by_kind,
        tbl_texts: cat.tbl_texts(),
        tbl_parts: cat.tbl_parts(),
        fonts: cat.caption_fonts,
        staff_kanji: cat.staff_kanji.unwrap_or_else(|| "{kanji}".to_owned()),
    }))
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
        if let Some(k) = &p.kanji {
            ensure!(
                k.split(" · ").count() == p.text.split(" · ").count(),
                "kanji {k:?}: not one per name of {:?}",
                p.text
            );
        }
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

/// A font role: `"body"` or `"display"`.
fn parse_role(v: &toml::Value) -> Result<FontRole> {
    v.as_str()
        .and_then(FontRole::parse)
        .context("font must be body or display")
}

/// A `[x, y, w, h]` pixel rectangle option (`region`, `ink`).
fn pixel_rect(v: &toml::Value, name: &str) -> Result<[u16; 4]> {
    let a = v
        .as_array()
        .with_context(|| format!("{name} must be [x, y, w, h] in pixels"))?;
    ensure!(a.len() == 4, "{name} must have 4 numbers");
    let mut r = [0u16; 4];
    for (o, x) in r.iter_mut().zip(a) {
        let n = x
            .as_integer()
            .with_context(|| format!("{name} values must be integers"))?;
        *o = u16::try_from(n)?;
    }
    ensure!(r[2] > 0 && r[3] > 0, "empty {name}");
    Ok(r)
}

fn apply_option(p: &mut TextPart, k: &str, v: &toml::Value) -> Result<()> {
    match k {
        "region" => p.region = Some(pixel_rect(v, "region")?),
        "ink" => p.ink = Some(pixel_rect(v, "ink")?),
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
        "group" => {
            let g = v.as_str().context("group must be a string")?;
            ensure!(!g.is_empty(), "empty group");
            p.group = Some(g.to_owned());
        }
        "shared_group" => {
            let g = v.as_str().context("shared_group must be a string")?;
            ensure!(!g.is_empty(), "empty shared_group");
            p.shared_group = Some(g.to_owned());
        }
        "line_height" => {
            let em = v
                .as_float()
                .or_else(|| v.as_integer().map(|i| i as f64))
                .context("line_height must be a number")?;
            ensure!((0.5..=3.0).contains(&em), "line_height must be 0.5 to 3");
            p.line_height = Some(em as f32);
        }
        "font" => p.font = parse_role(v)?,
        "kanji" => {
            let k = v.as_str().context("kanji must be a string")?;
            ensure!(!k.trim().is_empty(), "empty kanji");
            p.kanji = Some(k.to_owned());
        }
        "backdrop" => {
            p.backdrop = match v.as_str() {
                Some("art") => Backdrop::Art,
                Some("solid") => Backdrop::Solid,
                _ => bail!("backdrop must be art or solid"),
            }
        }
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
            let (tbl, rect) = e.key.resolve(sheets)?;
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
        Self::from_catalogue(&Catalogue::load(lang, dir)?, sheets)
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
    /// `lang` came from a flag, the environment or `config.toml` (else the first-run
    /// language screen asks, `crate::config`).
    pub lang_chosen: bool,
    /// TTF/OTF font files, `:`-separated like `PATH`: the primary font, then fallbacks for
    /// characters it lacks. No font ships with the project.
    pub font: Option<PathBuf>,
    /// The display font list (same form), for text marked `font = "display"`: screen
    /// headers, menu titles, banners. `None` draws those in `font` too.
    pub display_font: Option<PathBuf>,
    /// Extra catalogue directory (`<dir>/<lang>.toml` overrides the built-in one).
    pub dir: Option<PathBuf>,
    /// The canvas multiple in use: the 2D layer is composed at 640×480 × this (1 = the
    /// original). The art is enlarged with nearest neighbour; translated text gets the extra
    /// pixels. Follows the window ([`LocaleConfig::multiple_for`]) unless `ui_scale_fixed`.
    pub ui_scale: usize,
    /// `--ui-scale N` / `DDG_UI_SCALE`: a fixed canvas multiple instead of the window's.
    pub ui_scale_fixed: Option<usize>,
    /// `--hud-layout` / `DDG_HUD_LAYOUT` (enhancement `hud_layout`).
    pub hud_layout: crate::presentation::HudLayout,
    /// `--ui-scaling` / `DDG_UI_SCALING`: how the canvas reaches the window.
    pub ui_scaling: ddg_render::overlay::UiScaling,
    /// `--texture-pack` / `DDG_TEXTURE_PACK` (enhancement `texture_pack`, docs/texture-packs.md):
    /// a directory of replacement 3D texture pages; `None` = the original textures.
    pub texture_pack: Option<PathBuf>,
    /// `--draw-distance` / `DDG_DRAW_DISTANCE` (enhancement `draw_distance`, the 3D view's
    /// far plane; kept here with the other valued settings).
    pub draw_distance: crate::scene::DrawDistance,
    /// `--audio-resampler` / `DDG_AUDIO_RESAMPLER` (`audio_resampler`: how each voice is resampled
    /// to the device rate; `sinc` is an enhancement, docs/settings.md).
    pub audio_resampler: ddg_audio::Interpolation,
}

impl Default for LocaleConfig {
    fn default() -> Self {
        Self {
            lang: IDENTITY_LANG.to_owned(),
            lang_chosen: false,
            font: None,
            display_font: None,
            dir: None,
            ui_scale: 1,
            ui_scale_fixed: None,
            hud_layout: Default::default(),
            ui_scaling: Default::default(),
            texture_pack: None,
            draw_distance: Default::default(),
            audio_resampler: Default::default(),
        }
    }
}

impl LocaleConfig {
    /// The canvas multiple for a window `height` pixels high: the fixed one, else
    /// [`ddg_render::overlay::auto_multiple`].
    pub fn multiple_for(&self, height: u32) -> usize {
        self.ui_scale_fixed
            .unwrap_or_else(|| ddg_render::overlay::auto_multiple(height, self.ui_scaling))
    }

    /// Sets [`LocaleConfig::ui_scale`] for a window `height` pixels high (at startup; a
    /// resize goes through `presentation::rescale`).
    pub fn fit_height(&mut self, height: u32) {
        self.ui_scale = self.multiple_for(height);
    }

    /// Whether the enhanced HUD layout is on (enhancement `hud_layout enhanced`).
    pub fn enhanced_hud(&self) -> bool {
        self.hud_layout == crate::presentation::HudLayout::Enhanced
    }

    /// `DDG_LANG`, `DDG_FONT`, `DDG_DISPLAY_FONT`, `DDG_LOCALE_DIR`, `DDG_TEXTURE_PACK`,
    /// `DDG_UI_SCALE`, `DDG_HUD_LAYOUT`, `DDG_UI_SCALING`, `DDG_DRAW_DISTANCE`, `DDG_AUDIO_RESAMPLER` over what is set (the defaults or
    /// `config.toml`; unknown values are ignored with a warning).
    pub fn apply_env(&mut self) {
        self.apply_vars(|k| std::env::var_os(k));
    }

    /// [`LocaleConfig::apply_env`] with the variables looked up by `get` (tests).
    pub fn apply_vars(&mut self, get: impl Fn(&str) -> Option<std::ffi::OsString>) {
        let var = |k: &str| get(k).filter(|v| !v.is_empty());
        let c = self;
        if let Some(l) = var("DDG_LANG") {
            c.lang = l.to_string_lossy().into_owned();
            c.lang_chosen = true;
        }
        if let Some(f) = var("DDG_FONT") {
            c.font = Some(f.into());
        }
        if let Some(f) = var("DDG_DISPLAY_FONT") {
            c.display_font = Some(f.into());
        }
        if let Some(d) = var("DDG_TEXTURE_PACK") {
            c.texture_pack = Some(d.into());
        }
        if let Some(d) = var("DDG_LOCALE_DIR") {
            c.dir = Some(d.into());
        }
        if let Some(s) = var("DDG_UI_SCALE").and_then(|v| v.to_str()?.parse().ok()) {
            c.ui_scale_fixed = Some(clamp_scale(s));
        }
        if let Some(v) = var("DDG_HUD_LAYOUT") {
            match crate::presentation::HudLayout::parse(&v.to_string_lossy()) {
                Some(l) => c.hud_layout = l,
                None => eprintln!("DDG_HUD_LAYOUT: expected original or enhanced"),
            }
        }
        if let Some(v) = var("DDG_UI_SCALING") {
            match ddg_render::overlay::UiScaling::parse(&v.to_string_lossy()) {
                Some(s) => c.ui_scaling = s,
                None => eprintln!("DDG_UI_SCALING: expected smooth, integer or nearest"),
            }
        }
        if let Some(v) = var("DDG_DRAW_DISTANCE") {
            match crate::scene::DrawDistance::parse(&v.to_string_lossy()) {
                Some(d) => c.draw_distance = d,
                None => eprintln!("DDG_DRAW_DISTANCE: expected original, 2x, 3x or max"),
            }
        }
        if let Some(v) = var("DDG_AUDIO_RESAMPLER") {
            match ddg_audio::Interpolation::parse(&v.to_string_lossy()) {
                Some(m) => c.audio_resampler = m,
                None => eprintln!("DDG_AUDIO_RESAMPLER: expected linear, nearest or sinc"),
            }
        }
        c.ui_scale = c.ui_scale_fixed.unwrap_or(1);
    }

    /// Takes `--lang <code>`, `--font <files>`, `--display-font <files>`, `--locale-dir <dir>`,
    /// `--texture-pack <dir>`, `--ui-scale <n>`, `--hud-layout <original|enhanced>` and
    /// `--ui-scaling <smooth|integer|nearest>` out of `args` (they override the environment)
    /// and leaves everything else.
    pub fn take_args(&mut self, args: &mut Vec<String>) -> Result<()> {
        let mut rest = Vec::with_capacity(args.len());
        let mut it = std::mem::take(args).into_iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--lang" => {
                    self.lang = it.next().context("--lang needs a code")?;
                    self.lang_chosen = true;
                }
                "--font" => self.font = Some(it.next().context("--font needs a file")?.into()),
                "--display-font" => {
                    let v = it.next().context("--display-font needs a file")?;
                    self.display_font = Some(v.into());
                }
                "--texture-pack" => {
                    let v = it.next().context("--texture-pack needs a dir")?;
                    self.texture_pack = Some(v.into());
                }
                "--locale-dir" => {
                    self.dir = Some(it.next().context("--locale-dir needs a dir")?.into())
                }
                "--ui-scale" => {
                    let n = it.next().context("--ui-scale needs 1..=4")?;
                    let s = clamp_scale(n.parse().context("--ui-scale needs 1..=4")?);
                    self.ui_scale_fixed = Some(s);
                    self.ui_scale = s;
                }
                "--hud-layout" => {
                    let v = it.next().unwrap_or_default();
                    self.hud_layout = crate::presentation::HudLayout::parse(&v)
                        .context("--hud-layout needs original or enhanced")?;
                }
                "--ui-scaling" => {
                    let v = it.next().unwrap_or_default();
                    self.ui_scaling = ddg_render::overlay::UiScaling::parse(&v)
                        .context("--ui-scaling needs smooth, integer or nearest")?;
                }
                "--draw-distance" => {
                    let v = it.next().unwrap_or_default();
                    self.draw_distance = crate::scene::DrawDistance::parse(&v)
                        .context("--draw-distance needs original, 2x, 3x or max")?;
                }
                "--audio-resampler" => {
                    let v = it.next().unwrap_or_default();
                    self.audio_resampler = ddg_audio::Interpolation::parse(&v)
                        .context("--audio-resampler needs linear, nearest or sinc")?;
                }
                _ => rest.push(a),
            }
        }
        *args = rest;
        Ok(())
    }

    /// Whether text overrides will be drawn (any language but the original; the bundled font
    /// is used when none is given).
    pub fn active(&self) -> bool {
        self.lang != IDENTITY_LANG
    }
}

/// The bundled body font, M+ 2 Medium (SIL OFL 1.1, assets/fonts/OFL-Mplus.txt; docs/localisation.md
/// "Decided: a font"). Used when no `--font` is given, and appended as the last fallback of a
/// user-supplied list so Japanese and macron vowels always render.
const BUNDLED_FONT: &[u8] = include_bytes!("../../../assets/fonts/Mplus2-Medium.otf");

/// Canvas multiples beyond [`ddg_render::overlay::MAX_MULTIPLE`] only cost memory.
fn clamp_scale(s: usize) -> usize {
    s.clamp(1, ddg_render::overlay::MAX_MULTIPLE)
}

/// Builds the overlay for `cfg` and installs it on `bank`. Returns the number of sprites
/// overridden (0 for `ja`). Without `--font` the bundled M+ 2 Medium is used.
pub fn install(cfg: &LocaleConfig, exe: &Exe<'_>, bank: &mut SpriteBank) -> Result<usize> {
    if cfg.lang == IDENTITY_LANG {
        bank.set_text(None);
        bank.set_label_font(None);
        bank.set_display_label_font(None);
        return Ok(0);
    }
    let sheets = SheetTables::read(exe)?;
    let locale = Locale::load(&cfg.lang, cfg.dir.as_deref(), &sheets)?;
    // A `:`-separated list (`;` on Windows), like PATH: the primary font, then fallbacks.
    let load = |list: &Path| {
        let paths: Vec<PathBuf> = std::env::split_paths(list).collect();
        FontGlyphs::load_all(&paths)
    };
    let bundled = || FontGlyphs::from_bytes(BUNDLED_FONT);
    let font = match &cfg.font {
        Some(list) => load(list)?.with_fallback(bundled()?),
        None => bundled()?,
    };
    // The display role (headers, menu titles, banners): its own list, else the body font.
    let display = cfg.display_font.as_deref().map(load).transpose()?;
    let fonts = Fonts {
        body: &font,
        display: display.as_ref().map(|d| d as _),
    };
    let overlay = TextOverlay::build_with(bank, locale.overrides(), fonts, cfg.ui_scale);
    let n = overlay.len();
    bank.set_text(Some(overlay));
    // The same fonts for the free text of port-only widgets (captions, banners, plates).
    bank.set_label_font(Some(LabelFont::new(Box::new(font))));
    bank.set_display_label_font(display.map(|d| LabelFont::new(Box::new(d))));
    Ok(n)
}

/// The bundled body font ([`BUNDLED_FONT`]).
pub fn bundled_font() -> Result<FontGlyphs> {
    Ok(FontGlyphs::from_bytes(BUNDLED_FONT)?)
}

/// Only the bundled font as the label font, no text overrides: the first-run language screen
/// draws its port-only text with it before any language is installed.
pub fn install_label_font(bank: &mut SpriteBank) -> Result<()> {
    let font = bundled_font()?;
    bank.set_label_font(Some(LabelFont::new(Box::new(font))));
    Ok(())
}

/// The translated station names of the vertical plates (`[station_plate]`), by plate tilemap
/// id; empty unless text overrides are drawn ([`LocaleConfig::active`]).
pub fn station_plate_names(cfg: &LocaleConfig) -> Result<HashMap<u16, String>> {
    if !cfg.active() {
        return Ok(HashMap::new());
    }
    let cat = Catalogue::load(&cfg.lang, cfg.dir.as_deref())?;
    Ok(cat.station_plates.into_iter().collect())
}

/// The translated landmark names (`[landmark]`), by landmark sprite tilemap id; empty unless
/// text overrides are drawn ([`LocaleConfig::active`]).
pub fn landmark_names(cfg: &LocaleConfig) -> Result<HashMap<u16, String>> {
    if !cfg.active() {
        return Ok(HashMap::new());
    }
    let cat = Catalogue::load(&cfg.lang, cfg.dir.as_deref())?;
    Ok(cat.landmarks.into_iter().collect())
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
        // The vertical plates are named for the horizontal layout, not overridden as art.
        assert!(cat.station_plates.len() >= 60);
        for (tbl, _) in &cat.station_plates {
            assert!(l.lookup(*tbl as i16, SubRect::WHOLE).is_none(), "{tbl:#x}");
        }
        // The landmark captions keep their kanji; their names go on a line underneath.
        assert_eq!(cat.landmarks.len(), ddg_formats::cab_hud::LANDMARK_COUNT);
        for (tbl, _) in &cat.landmarks {
            assert!((0x222..=0x23f).contains(tbl), "{tbl:#x}");
            assert!(l.lookup(*tbl as i16, SubRect::WHOLE).is_none(), "{tbl:#x}");
        }
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
        // Every caption kind has a one-line phrase naming the station, except kind 6 only
        // naming it.
        for k in CAPTION_KINDS {
            let p = cat.captions.iter().find(|(c, _)| c == k);
            assert!(p.is_some_and(|(_, t)| t.contains("{name}")), "caption {k}");
        }
        assert_eq!(cat.tbl_texts()[&0xe6], "Higashi-Kanagawa");
    }

    /// Every character of the built-in catalogues is in the bundled font: a missing one draws
    /// as an empty box (the name-entry legend's ← → once did; ◀ ▶ are in M+ 2).
    #[test]
    fn bundled_font_draws_every_catalogue_character() {
        let font = FontGlyphs::from_bytes(BUNDLED_FONT).unwrap();
        for (lang, src) in BUILTIN {
            let cat = Catalogue::parse(src).unwrap();
            let texts = cat
                .entries
                .iter()
                .flat_map(|e| e.text.parts.iter())
                .flat_map(|p| std::iter::once(p.text.as_str()).chain(p.kanji.as_deref()))
                .chain(cat.station_plates.iter().map(|(_, t)| t.as_str()))
                .chain(cat.landmarks.iter().map(|(_, t)| t.as_str()))
                .chain(cat.staff_kanji.as_deref())
                .chain(
                    [&cat.captions, &cat.subtitles, &cat.ui]
                        .into_iter()
                        .flatten()
                        .map(|(_, t)| t.as_str()),
                );
            let missing: Vec<String> = texts
                .flat_map(|t| {
                    t.chars()
                        .filter(|&ch| !font.draws(ch))
                        .map(move |ch| format!("{ch:?} ({:#x}) in {t:?}", ch as u32))
                })
                .collect();
            assert!(missing.is_empty(), "{lang}: {missing:#?}");
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
            "[caption]\nleave = \"{name}\"",
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
    fn font_roles_parse() {
        let head = "[meta]\nlanguage = \"xx\"\nname = \"X\"\n";
        let cat = Catalogue::parse(&format!(
            "{head}[tbl]\n0x10 = \"plain\"\n0x11 = {{ text = \"head\", font = \"display\" }}\n\
             0x12 = {{ font = \"display\", parts = [{{ region = [0, 0, 8, 8], text = \"a\" }}, \
             {{ region = [8, 0, 8, 8], text = \"b\", font = \"body\" }}] }}\n\
             [caption]\nstop = \"{{name}}\"\nbanner_font = \"display\"\n"
        ))
        .unwrap();
        let fonts = |tbl: u16| -> Vec<FontRole> {
            let e = cat.entries.iter().find(|e| e.key.tbl() == tbl).unwrap();
            e.text.parts.iter().map(|p| p.font).collect()
        };
        assert_eq!(fonts(0x10), [FontRole::Body]);
        assert_eq!(fonts(0x11), [FontRole::Display]);
        // An entry's role is its parts' default; a part can set its own.
        assert_eq!(fonts(0x12), [FontRole::Display, FontRole::Body]);
        assert_eq!(cat.caption_fonts, [FontRole::Body, FontRole::Display]);
        assert_eq!(cat.captions.len(), 1);
        for bad in [
            "[tbl]\n0x10 = { text = \"a\", font = \"heading\" }",
            "[tbl]\n0x10 = { text = \"a\", font = 1 }",
            "[caption]\nfont = \"bold\"",
        ] {
            assert!(Catalogue::parse(&format!("{head}{bad}")).is_err(), "{bad}");
        }
        // en.toml: headers and titles are display, HUD labels body, banners display.
        let en = en();
        let role = |tbl: u16, name: &str| {
            en.entries
                .iter()
                .find(|e| matches!(&e.key, Key::Rect { tbl: t, name: n, .. } if *t == tbl && n == name))
                .map(|e| e.text.parts[0].font)
        };
        assert_eq!(role(0x54d, "main_menu"), Some(FontRole::Display));
        let tbl = |id: u16| {
            en.entries
                .iter()
                .find(|e| {
                    e.key
                        == Key::Tbl {
                            tbl: id,
                            rect: SubRect::WHOLE.canonical(),
                        }
                })
                .map(|e| e.text.parts[0].font)
        };
        assert_eq!(tbl(0x1af), Some(FontRole::Display));
        assert_eq!(tbl(0x1a0), Some(FontRole::Display));
        assert_eq!(tbl(0x206), Some(FontRole::Body));
        assert_eq!(en.caption_fonts, [FontRole::Body, FontRole::Display]);
    }

    #[test]
    fn display_font_from_args() {
        let mut c = LocaleConfig::default();
        let mut args: Vec<String> = ["--display-font", "/a.ttf:/b.ttf", "--lang", "en"]
            .map(String::from)
            .to_vec();
        c.take_args(&mut args).unwrap();
        assert!(args.is_empty());
        assert_eq!(c.display_font, Some(PathBuf::from("/a.ttf:/b.ttf")));
        // English is on with the bundled body font; Japanese never is.
        assert!(c.active());
        c.lang = IDENTITY_LANG.to_owned();
        assert!(!c.active());
        assert!(c.take_args(&mut vec!["--display-font".into()]).is_err());
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

    /// `--hud-layout`, `--ui-scaling` and `--ui-scale` (docs/localisation.md, "UI scaling").
    #[test]
    fn layout_and_scaling_from_args() {
        use crate::presentation::HudLayout;
        use ddg_render::overlay::UiScaling;
        let mut c = LocaleConfig::default();
        assert_eq!(c.hud_layout, HudLayout::Original);
        assert_eq!(c.ui_scaling, UiScaling::Smooth);
        // The multiple follows the window by default.
        assert_eq!((c.multiple_for(800), c.multiple_for(1080)), (2, 3));
        let mut args: Vec<String> = ["--hud-layout", "enhanced", "--ui-scaling", "integer", "x"]
            .map(String::from)
            .to_vec();
        c.take_args(&mut args).unwrap();
        assert_eq!(args, ["x"]);
        assert!(c.enhanced_hud());
        assert_eq!(c.ui_scaling, UiScaling::Integer);
        assert_eq!((c.multiple_for(800), c.multiple_for(1080)), (1, 2));
        // A fixed multiple overrides the window's, and nothing else.
        let mut args: Vec<String> = ["--ui-scale", "3"].map(String::from).to_vec();
        c.take_args(&mut args).unwrap();
        assert_eq!((c.multiple_for(800), c.ui_scale), (3, 3));
        assert!(c.enhanced_hud());
        let mut c = LocaleConfig::default();
        c.take_args(&mut vec!["--ui-scale".into(), "2".into()])
            .unwrap();
        assert!(
            !c.enhanced_hud(),
            "the multiple no longer switches the layout"
        );
        for bad in [["--hud-layout", "wide"], ["--ui-scaling", "blurry"]] {
            assert!(c.take_args(&mut bad.map(String::from).to_vec()).is_err());
        }
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
                for r in [p.region, p.ink].into_iter().flatten() {
                    let [x, y, rw, rh] = r;
                    assert!(
                        x + rw <= pw && y + rh <= ph,
                        "tbl {tbl:#x}: rectangle {r:?} outside {pw}×{ph}"
                    );
                }
            }
            // Catalogued sprites have art to replace.
            assert!(map.tiles.iter().any(|&t| t != 0), "tbl {tbl:#x} is empty");
        }
    }
}

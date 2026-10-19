//! ポート設定 / Port settings (port only; docs/settings.md, "In the game"): the in-game screen
//! for the port's named settings.
//!
//! Reached from ゲーム設定 (the original's options list, menus.md §4) as a seventh item after
//! the original six, which stay where and as they are; with the analog levers, whose cursor
//! follows the power notch and so cannot reach a seventh row, the horn opens it. It lists the
//! settings in five groups with their current values and a one-line description, and changes
//! them in place: left / right step a value, confirm toggles (or steps it on). Every change is
//! written to `config.toml` at once by the app ([`crate::config::store`]), and applied live
//! where the front end can show it; otherwise the status line says when it takes effect. A
//! setting fixed by a command-line flag or an environment variable (they override the file)
//! is shown locked, with the reason.
//!
//! The look is the options pages': the menu header with its backdrop and button legend
//! (0x42a820), the scroll arrows of the title menu (tbl 0x505 / 0x506) and text in the label
//! font (the bundled M+ 2 Medium unless another is configured), in the menu palette's colours.
//! The strings come from the `[ui]` section of the catalogues (`locales/en.toml`,
//! `locales/ja.toml`), by string ID.

use super::tables::FrontTables;
use super::{Env, PAL_MENU, snd};
use crate::config::{Lock, Locks};
use crate::glyphs::{self, Btn, Hint, Line};
use crate::locale::LocaleConfig;
use crate::movie::MovieScaling;
use crate::presentation::HudLayout;
use crate::scene::DrawDistance;
use crate::screens::{MenuInput, Sfx};
use crate::settings::Settings;
use shuppatsu_audio::Interpolation;
use shuppatsu_formats::cg::Palette;
use shuppatsu_render::overlay::{MAX_MULTIPLE, UiScaling};
use shuppatsu_render::sprites::{Paint, SpriteLayer, TextStyle, WidgetKind};
use shuppatsu_render::text::{Align, FontRole};
use std::collections::HashMap;
use std::path::Path;

/// The five groups, in screen order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Text,
    Display,
    Effects,
    Sound,
    Saves,
}

impl Group {
    /// The heading's string ID.
    pub fn ui_key(self) -> &'static str {
        match self {
            Self::Text => "group_text",
            Self::Display => "group_display",
            Self::Effects => "group_3d",
            Self::Sound => "group_sound",
            Self::Saves => "group_saves",
        }
    }
}

/// What kind of value a setting has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// On / off ([`Settings`]).
    Switch,
    /// `lang`: one of [`LANGS`].
    Lang,
    HudLayout,
    UiScaling,
    /// `ui_scale`: follows the window, or a fixed multiple 1..=[`MAX_MULTIPLE`].
    UiScale,
    /// `font` / `display_font`: file lists, shown but changed only in `config.toml`.
    Font,
    DisplayFont,
    /// `texture_pack`: a directory, shown but changed only in `config.toml`.
    TexturePack,
    /// `msaa` / `anisotropic`: off or one of the level's steps ([`Settings::level_values`]).
    Level,
    /// `draw_distance`: one of [`DrawDistance::ALL`].
    DrawDistance,
    /// `audio_resampler`: one of [`Interpolation::ALL`].
    Resampler,
    /// `movie_scaling`: one of [`MovieScaling::ALL`].
    MovieScaling,
}

/// When a change takes effect, seen from the menus (the only place the screen is reached).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Apply {
    /// At once: the menus show it (language, UI scaling, backdrops) or the app applies it
    /// (the save and input switches, the exit dialog).
    Now,
    /// A run reads the settings when it starts: the next stage has it.
    NextStage,
    /// Set when the window opens (`vsync`) or the fonts load.
    Restart,
}

/// One row of the screen: a `config.toml` key (docs/settings.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Item {
    pub key: &'static str,
    pub group: Group,
    pub kind: Kind,
    pub apply: Apply,
}

const fn item(key: &'static str, group: Group, kind: Kind, apply: Apply) -> Item {
    Item {
        key,
        group,
        kind,
        apply,
    }
}

const fn switch(key: &'static str, group: Group, apply: Apply) -> Item {
    item(key, group, Kind::Switch, apply)
}

/// Every setting of docs/settings.md, grouped, in screen order.
pub const ITEMS: &[Item] = {
    use Apply::*;
    use Group::*;
    &[
        item("lang", Text, Kind::Lang, Now),
        switch("one_line_captions", Text, NextStage),
        switch("staff_kanji", Text, NextStage),
        switch("landmark_names", Text, NextStage),
        switch("board_names", Text, NextStage),
        switch("subtitles", Text, NextStage),
        item("font", Text, Kind::Font, Restart),
        item("display_font", Text, Kind::DisplayFont, Restart),
        item("hud_layout", Display, Kind::HudLayout, NextStage),
        item("ui_scaling", Display, Kind::UiScaling, Now),
        item("ui_scale", Display, Kind::UiScale, Now),
        switch("wide_menus", Display, Now),
        switch("hud_upscale", Display, Now),
        switch("interpolate", Display, NextStage),
        switch("vsync", Display, Restart),
        switch("fullscreen", Display, Now),
        item("movie_scaling", Display, Kind::MovieScaling, Now),
        switch("wide_visibility", Effects, NextStage),
        switch("square_pixels", Effects, NextStage),
        switch("stable_track_variants", Effects, NextStage),
        switch("texture_filtering", Effects, NextStage),
        item("anisotropic", Effects, Kind::Level, NextStage),
        item("msaa", Effects, Kind::Level, NextStage),
        switch("ground_variation", Effects, NextStage),
        switch("ground_detiling", Effects, NextStage),
        item("texture_pack", Effects, Kind::TexturePack, NextStage),
        switch("fog", Effects, NextStage),
        switch("wide_weather", Effects, NextStage),
        switch("wide_coupling_band", Effects, NextStage),
        switch("far_traffic", Effects, NextStage),
        switch("max_lod", Effects, NextStage),
        switch("early_activation", Effects, NextStage),
        item("draw_distance", Effects, Kind::DrawDistance, NextStage),
        switch("distance_fade", Effects, NextStage),
        switch("fine_depth", Effects, NextStage),
        item("audio_resampler", Sound, Kind::Resampler, Now),
        switch("save_on_change", Saves, Now),
        switch("save_timed_out_names", Saves, NextStage),
        switch("hybrid_input", Saves, Now),
        switch("keyboard_with_controller", Saves, Now),
        switch("exit_no_to_menu", Saves, Now),
        switch("movie_in_menu", Saves, Now),
    ]
};

/// The languages, in the order the value steps through them (the language screen's).
pub const LANGS: [&str; 2] = ["en", crate::locale::IDENTITY_LANG];

const SCALINGS: [UiScaling; 3] = [UiScaling::Smooth, UiScaling::Integer, UiScaling::Nearest];

fn scaling_name(s: UiScaling) -> &'static str {
    match s {
        UiScaling::Smooth => "smooth",
        UiScaling::Integer => "integer",
        UiScaling::Nearest => "nearest",
    }
}

fn hud_name(h: HudLayout) -> &'static str {
    match h {
        HudLayout::Original => "original",
        HudLayout::Enhanced => "enhanced",
    }
}

/// `i` moved by `dir` in a cycle of `n`.
fn cycle(i: usize, dir: i32, n: usize) -> usize {
    (i as i64 + i64::from(dir)).rem_euclid(n as i64) as usize
}

impl Item {
    /// Whether the screen can change it (the font lists are paths: `config.toml` only).
    pub fn editable(&self) -> bool {
        !matches!(
            self.kind,
            Kind::Font | Kind::DisplayFont | Kind::TexturePack
        )
    }

    /// Steps the value by `dir` (±1), wrapping; a switch flips either way.
    pub fn step(&self, cfg: &mut LocaleConfig, s: &mut Settings, dir: i32) {
        match self.kind {
            Kind::Switch => {
                if let Some(v) = s.get_mut(self.key) {
                    *v = !*v;
                }
            }
            Kind::Lang => {
                let i = LANGS.iter().position(|l| *l == cfg.lang).unwrap_or(0);
                cfg.lang = LANGS[cycle(i, dir, LANGS.len())].to_owned();
                cfg.lang_chosen = true;
            }
            Kind::HudLayout => {
                cfg.hud_layout = match cfg.hud_layout {
                    HudLayout::Original => HudLayout::Enhanced,
                    HudLayout::Enhanced => HudLayout::Original,
                };
            }
            Kind::UiScaling => {
                let i = SCALINGS.iter().position(|v| *v == cfg.ui_scaling);
                cfg.ui_scaling = SCALINGS[cycle(i.unwrap_or(0), dir, SCALINGS.len())];
            }
            Kind::UiScale => {
                // 0 = follows the window, then 1..=MAX_MULTIPLE.
                let i = cfg.ui_scale_fixed.unwrap_or(0).min(MAX_MULTIPLE);
                let n = cycle(i, dir, MAX_MULTIPLE + 1);
                cfg.ui_scale_fixed = (n != 0).then_some(n);
            }
            Kind::Level => {
                let steps = Settings::level_values(self.key).unwrap_or_default();
                if let Some(v) = s.level_mut(self.key)
                    && !steps.is_empty()
                {
                    let i = steps.iter().position(|x| x == v).unwrap_or(0);
                    *v = steps[cycle(i, dir, steps.len())];
                }
            }
            Kind::Font | Kind::DisplayFont | Kind::TexturePack => {}
            Kind::DrawDistance => {
                let all = DrawDistance::ALL;
                let i = all.iter().position(|d| *d == cfg.draw_distance);
                cfg.draw_distance = all[cycle(i.unwrap_or(0), dir, all.len())];
            }
            Kind::Resampler => {
                let all = Interpolation::ALL;
                let i = all.iter().position(|m| *m == cfg.audio_resampler);
                cfg.audio_resampler = all[cycle(i.unwrap_or(0), dir, all.len())];
            }
            Kind::MovieScaling => {
                let all = MovieScaling::ALL;
                let i = all.iter().position(|m| *m == cfg.movie_scaling);
                cfg.movie_scaling = all[cycle(i.unwrap_or(0), dir, all.len())];
            }
        }
    }

    /// The value as `config.toml` holds it; `None` for no line (the default: `ui_scale`
    /// following the window, no font list).
    pub fn file_value(&self, cfg: &LocaleConfig, s: &Settings) -> Option<toml::Value> {
        use toml::Value;
        let path = |p: &Option<std::path::PathBuf>| {
            p.as_ref()
                .map(|p| Value::String(p.to_string_lossy().into_owned()))
        };
        match self.kind {
            Kind::Switch => s.get(self.key).map(Value::Boolean),
            Kind::Lang => Some(Value::String(cfg.lang.clone())),
            Kind::HudLayout => Some(Value::String(hud_name(cfg.hud_layout).to_owned())),
            Kind::UiScaling => Some(Value::String(scaling_name(cfg.ui_scaling).to_owned())),
            Kind::UiScale => cfg.ui_scale_fixed.map(|n| Value::Integer(n as i64)),
            Kind::Font => path(&cfg.font),
            Kind::DisplayFont => path(&cfg.display_font),
            Kind::TexturePack => path(&cfg.texture_pack),
            Kind::Level => s.level(self.key).map(|v| Value::Integer(i64::from(v))),
            Kind::DrawDistance => Some(Value::String(cfg.draw_distance.name().to_owned())),
            Kind::Resampler => Some(Value::String(cfg.audio_resampler.name().to_owned())),
            Kind::MovieScaling => Some(Value::String(cfg.movie_scaling.name().to_owned())),
        }
    }

    /// The value as the screen shows it.
    pub fn value_text(&self, cfg: &LocaleConfig, s: &Settings, ui: &UiText) -> String {
        let file_name = |p: &Path| {
            std::env::split_paths(p)
                .filter_map(|f| Some(f.file_name()?.to_string_lossy().into_owned()))
                .collect::<Vec<_>>()
                .join(", ")
        };
        match self.kind {
            Kind::Switch => {
                let on = s.get(self.key).unwrap_or(false);
                ui.get(if on { "on" } else { "off" }).to_owned()
            }
            Kind::Lang => match cfg.lang.as_str() {
                "en" => super::language::Language::English.label().to_owned(),
                crate::locale::IDENTITY_LANG => {
                    super::language::Language::Japanese.label().to_owned()
                }
                other => other.to_owned(),
            },
            Kind::HudLayout => ui
                .get(match cfg.hud_layout {
                    HudLayout::Original => "hud_original",
                    HudLayout::Enhanced => "hud_enhanced",
                })
                .to_owned(),
            Kind::UiScaling => ui
                .get(match cfg.ui_scaling {
                    UiScaling::Smooth => "scaling_smooth",
                    UiScaling::Integer => "scaling_integer",
                    UiScaling::Nearest => "scaling_nearest",
                })
                .to_owned(),
            Kind::UiScale => match cfg.ui_scale_fixed {
                Some(n) => format!("{n}×"),
                None => ui.get("scale_auto").to_owned(),
            },
            Kind::Font => match &cfg.font {
                Some(p) => file_name(p),
                None => ui.get("font_bundled").to_owned(),
            },
            Kind::DisplayFont => match &cfg.display_font {
                Some(p) => file_name(p),
                None => ui.get("display_font_same").to_owned(),
            },
            Kind::TexturePack => match &cfg.texture_pack {
                Some(p) => p.file_name().map_or_else(
                    || p.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                ),
                None => ui.get("texture_pack_none").to_owned(),
            },
            Kind::Level => match s.level(self.key) {
                Some(v) if v > 1 => format!("{v}×"),
                _ => ui.get("off").to_owned(),
            },
            Kind::DrawDistance => ui
                .get(match cfg.draw_distance {
                    DrawDistance::Original => "distance_original",
                    DrawDistance::Double => "distance_2x",
                    DrawDistance::Triple => "distance_3x",
                    DrawDistance::Max => "distance_max",
                })
                .to_owned(),
            Kind::Resampler => ui
                .get(match cfg.audio_resampler {
                    Interpolation::Linear => "resampler_linear",
                    Interpolation::Nearest => "resampler_nearest",
                    Interpolation::Sinc => "resampler_sinc",
                })
                .to_owned(),
            Kind::MovieScaling => ui
                .get(match cfg.movie_scaling {
                    MovieScaling::Smooth => "movie_smooth",
                    MovieScaling::Sharp => "movie_sharp",
                    MovieScaling::Nearest => "movie_nearest",
                })
                .to_owned(),
        }
    }
}

/// The string IDs of the screen besides `<key>` and `<key>_desc` of every item.
const UI_IDS: &[&str] = &[
    "title",
    "options_item",
    "group_text",
    "group_display",
    "group_3d",
    "group_sound",
    "group_saves",
    "on",
    "off",
    "hud_original",
    "hud_enhanced",
    "scaling_smooth",
    "scaling_integer",
    "scaling_nearest",
    "scale_auto",
    "font_bundled",
    "display_font_same",
    "texture_pack_none",
    "distance_original",
    "distance_2x",
    "distance_3x",
    "distance_max",
    "resampler_linear",
    "resampler_nearest",
    "resampler_sinc",
    "movie_smooth",
    "movie_sharp",
    "movie_nearest",
    "status_now",
    "status_stage",
    "status_restart",
    "status_flag",
    "status_env",
    "status_file",
    "legend_keys",
    "legend_pad",
    "legend_levers",
    "hint_select",
    "hint_confirm",
    "hint_cancel",
    "hint_back",
    "hint_change",
    "hint_toggle",
    "hint_next",
    "hint_back_to_menu",
    "hint_resume",
    "pad_title",
    "pad_power_dpad",
    "pad_power_shoulders",
    "pad_brake_dpad",
    "pad_brake_stick",
    "pad_brake_shoulders",
    "pad_a",
    "pad_x",
    "pad_b",
    "pad_menu",
    "pad_y",
    "pad_note",
];

/// Whether `k` is a string ID of the `[ui]` section.
pub fn known_ui_key(k: &str) -> bool {
    UI_IDS.contains(&k)
        || ITEMS
            .iter()
            .any(|i| k == i.key || k.strip_suffix("_desc") == Some(i.key))
}

/// Every string ID of the `[ui]` section.
#[cfg(test)]
pub fn ui_ids() -> impl Iterator<Item = String> {
    UI_IDS.iter().map(|s| (*s).to_owned()).chain(
        ITEMS
            .iter()
            .flat_map(|i| [i.key.to_owned(), format!("{}_desc", i.key)]),
    )
}

/// The `[ui]` strings of a language, with English for any it lacks.
#[derive(Debug, Clone, Default)]
pub struct UiText {
    text: HashMap<String, String>,
    fallback: HashMap<String, String>,
}

impl UiText {
    /// The strings of `lang` (`dir` as [`crate::locale::Catalogue::load`]); a catalogue that
    /// does not load is reported and English is used.
    pub fn load(lang: &str, dir: Option<&Path>) -> Self {
        let read = |l: &str| match crate::locale::Catalogue::load(l, dir) {
            Ok(c) => c.ui.into_iter().collect(),
            Err(e) => {
                eprintln!("warning: ui text {l}: {e:#}");
                HashMap::new()
            }
        };
        Self {
            text: read(lang),
            fallback: if lang == "en" {
                HashMap::new()
            } else {
                read("en")
            },
        }
    }

    /// The text of `id`; the ID itself when no catalogue has it.
    pub fn get<'a>(&'a self, id: &'a str) -> &'a str {
        self.text
            .get(id)
            .or_else(|| self.fallback.get(id))
            .map_or(id, String::as_str)
    }
}

/// Colours from the menu palette (nearest to the RGB wanted), read by the app.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    pub bright: Paint,
    pub dim: Paint,
    /// Group headings and the status line: the gold of the legend arrows.
    pub gold: Paint,
    /// The cursor bar: the blue of the options header's stripe.
    pub bar: Paint,
    /// The description panel, and the outline of the larger text (the art's shadow).
    pub panel: Paint,
}

impl Look {
    pub fn new(menu: Option<&Palette>) -> Self {
        let paint = |rgb| Paint {
            palette: PAL_MENU,
            index: menu.map_or(1, |p| super::language::nearest(p, rgb)),
        };
        Self {
            bright: paint([255, 255, 255]),
            dim: paint([150, 150, 150]),
            gold: paint([216, 184, 64]),
            bar: paint([16, 104, 160]),
            panel: paint([0, 0, 0]),
        }
    }
}

/// The options list's header page (0x42a820(8)): this screen keeps its bar and legend.
const HEADER_PAGE: usize = 8;
const LAYER: u8 = 3;
/// The list: first row, row height and rows on screen (original pixels).
const LIST_Y: f32 = 118.0;
const ROW_H: f32 = 23.0;
pub const VISIBLE: usize = 12;
/// Columns: the list's box, headings, labels and values.
const BOX_X: f32 = 32.0;
const BOX_W: f32 = 576.0;
const HEADING_X: f32 = 40.0;
const LABEL_X: f32 = 60.0;
const VALUE_X: f32 = 352.0;
const VALUE_W: f32 = 248.0;
/// The panel under the list: description, status and the change legend.
const PANEL_Y: f32 = 400.0;
const PANEL_H: f32 = 76.0;
/// Text sizes (capital height, original pixels).
const TITLE_CAP: f32 = 26.0;
const ROW_CAP: f32 = 11.5;
const PANEL_CAP: f32 = 11.0;
const LEGEND_CAP: f32 = 9.5;
/// The seventh item of the options list (0x42a730's rows, 0x30 apart from y 0xa0).
const OPTIONS_ITEM_Y: i16 = 0xa0 + 0x30 * 6;
const OPTIONS_ITEM_CAP: f32 = 24.0;

/// A row of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Heading(Group),
    Item(usize),
}

/// The rows: each group's heading, then its items.
pub fn rows() -> Vec<Row> {
    let mut out = Vec::new();
    let mut group = None;
    for (i, it) in ITEMS.iter().enumerate() {
        if group != Some(it.group) {
            out.push(Row::Heading(it.group));
            group = Some(it.group);
        }
        out.push(Row::Item(i));
    }
    out
}

fn row_of(item: usize) -> usize {
    rows()
        .iter()
        .position(|r| *r == Row::Item(item))
        .unwrap_or(0)
}

/// What the screen asks of the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Item `n` of [`ITEMS`] changed (already set on the config): write and apply it.
    Changed(usize),
    /// Back to the options list.
    Back,
}

/// The screen's state.
#[derive(Debug, Clone, Default)]
pub struct PortSettings {
    cursor: usize,
    /// The first row on screen.
    top: usize,
    /// The levers' (power, brake) notches of the previous tick.
    levers: Option<(u8, u8)>,
}

impl PortSettings {
    pub fn new() -> Self {
        Self::default()
    }

    /// The item under the cursor (an index into [`ITEMS`]).
    #[cfg(test)]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    #[cfg(test)]
    pub fn top(&self) -> usize {
        self.top
    }

    /// One tick. Up / down move the cursor (no wrap); with the levers, each notch the power
    /// handle is pushed on moves it down and each notch the brake handle is pulled on moves it
    /// up (the power notch alone cannot address 24 rows). Left / right step the value,
    /// confirm steps it on; a locked or read-only setting answers with the cancel sound.
    /// Cancel goes back.
    pub fn tick(
        &mut self,
        input: &MenuInput,
        locks: &Locks,
        cfg: &mut LocaleConfig,
        settings: &mut Settings,
        sfx: &mut Vec<Sfx>,
    ) -> Option<Step> {
        if input.cancel {
            sfx.push(Sfx::Play(snd::CANCEL));
            return Some(Step::Back);
        }
        let mut moved = 0i32;
        if input.up {
            moved -= 1;
        }
        if input.down {
            moved += 1;
        }
        if let (Some(p), Some(b)) = (input.notch, input.brake) {
            if let Some((pp, pb)) = self.levers {
                moved += i32::from(p.saturating_sub(pp));
                moved -= i32::from(b.saturating_sub(pb));
            }
            self.levers = Some((p, b));
        }
        if moved != 0 {
            let last = ITEMS.len() as i32 - 1;
            let c = (self.cursor as i32 + moved).clamp(0, last) as usize;
            if c != self.cursor {
                self.cursor = c;
                sfx.push(Sfx::Play(snd::MOVE));
            }
            self.scroll();
            return None;
        }
        let dir = if input.left {
            -1
        } else if input.right || input.confirm {
            1
        } else {
            return None;
        };
        let it = &ITEMS[self.cursor];
        if !it.editable() || locks.get(it.key).is_some() {
            sfx.push(Sfx::Play(snd::CANCEL));
            return None;
        }
        it.step(cfg, settings, dir);
        sfx.push(Sfx::Play(snd::DECIDE_SOFT));
        Some(Step::Changed(self.cursor))
    }

    /// Keeps the cursor's row on screen, with its group heading when it is the group's first.
    fn scroll(&mut self) {
        let row = row_of(self.cursor);
        let want_top = if row > 0 && matches!(rows()[row - 1], Row::Heading(_)) {
            row - 1
        } else {
            row
        };
        if want_top < self.top {
            self.top = want_top;
        } else if row >= self.top + VISIBLE {
            self.top = row + 1 - VISIBLE;
        }
        self.top = self.top.min(rows().len().saturating_sub(VISIBLE));
    }

    /// The status line of the item under the cursor.
    pub fn status(&self, locks: &Locks, ui: &UiText) -> String {
        let it = &ITEMS[self.cursor];
        match locks.get(it.key) {
            Some(Lock::Flag(f)) => ui.get("status_flag").replace("{src}", f),
            Some(Lock::Env(v)) => ui.get("status_env").replace("{src}", v),
            None if !it.editable() => ui.get("status_file").to_owned(),
            None => ui
                .get(match it.apply {
                    Apply::Now => "status_now",
                    Apply::NextStage => "status_stage",
                    Apply::Restart => "status_restart",
                })
                .to_owned(),
        }
    }

    /// The menu header with this screen's title, the rows on screen with the cursor bar,
    /// the scroll arrows, and the panel with the description, status and legend.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &self,
        look: &Look,
        ui: &UiText,
        locks: &Locks,
        cfg: &LocaleConfig,
        settings: &Settings,
        env: &Env,
        t: &FrontTables,
        l: &mut SpriteLayer,
    ) {
        super::draw_menu_header(l, t, env, HEADER_PAGE);
        super::hide(l, super::slot::TITLE);
        let x0 = f32::from(t.origin.0);
        let y0 = f32::from(t.origin.1);
        let mut title = TextStyle::plain(look.bright, Some(TITLE_CAP));
        title.align = Align::Left;
        title.font = FontRole::Display;
        title.outline = Some(look.panel);
        l.add_styled_text(
            LAYER,
            [x0 + 96.0, y0 + 32.0, 270.0, 48.0],
            ui.get("title"),
            title,
        );

        let rows = rows();
        for (n, row) in rows.iter().skip(self.top).take(VISIBLE).enumerate() {
            let y = y0 + LIST_Y + ROW_H * n as f32;
            match *row {
                Row::Heading(g) => {
                    let mut s = TextStyle::plain(look.gold, Some(ROW_CAP));
                    s.align = Align::Left;
                    s.font = FontRole::Display;
                    let rect = [x0 + HEADING_X, y, BOX_W - 16.0, ROW_H];
                    l.add_styled_text(LAYER, rect, ui.get(g.ui_key()), s);
                }
                Row::Item(i) => {
                    let it = &ITEMS[i];
                    let selected = i == self.cursor;
                    let locked = locks.get(it.key).is_some() || !it.editable();
                    if selected {
                        let bar = WidgetKind::Fill(look.bar);
                        l.add_widget(LAYER, [x0 + BOX_X, y + 1.0, BOX_W, ROW_H - 2.0], bar);
                    }
                    let paint = if selected { look.bright } else { look.dim };
                    let mut s = TextStyle::plain(paint, Some(ROW_CAP));
                    s.align = Align::Left;
                    let rect = [x0 + LABEL_X, y, VALUE_X - LABEL_X - 8.0, ROW_H];
                    l.add_styled_text(LAYER, rect, ui.get(it.key), s);
                    let value = it.value_text(cfg, settings, ui);
                    let value = if selected && !locked {
                        format!("◀  {value}  ▶")
                    } else {
                        value
                    };
                    let paint = if selected && !locked {
                        look.bright
                    } else {
                        look.dim
                    };
                    let s = TextStyle::plain(paint, Some(ROW_CAP));
                    l.add_styled_text(LAYER, [x0 + VALUE_X, y, VALUE_W, ROW_H], &value, s);
                }
            }
        }
        // ▲ / ▼ in the left margin when rows are hidden above / below (the title menu's
        // arrow sprites are too large for these rows).
        let mut s = TextStyle::plain(look.gold, Some(ROW_CAP * 0.8));
        s.align = Align::Center;
        if self.top > 0 {
            let rect = [x0 + 10.0, y0 + LIST_Y, 20.0, ROW_H];
            l.add_styled_text(LAYER, rect, "▲", s);
        }
        if self.top + VISIBLE < rows.len() {
            let y = y0 + LIST_Y + ROW_H * (VISIBLE - 1) as f32;
            l.add_styled_text(LAYER, [x0 + 10.0, y, 20.0, ROW_H], "▼", s);
        }

        let panel = WidgetKind::Fill(look.panel);
        l.add_widget(LAYER, [x0 + BOX_X, y0 + PANEL_Y, BOX_W, PANEL_H], panel);
        let it = &ITEMS[self.cursor];
        let line = |l: &mut SpriteLayer, y: f32, h: f32, text: &str, paint, cap, align| {
            let mut s = TextStyle::plain(paint, Some(cap));
            s.align = align;
            let rect = [x0 + BOX_X + 12.0, y0 + y, BOX_W - 24.0, h];
            l.add_styled_text(LAYER, rect, text, s);
        };
        let desc = format!("{}_desc", it.key);
        line(
            l,
            PANEL_Y + 4.0,
            22.0,
            ui.get(&desc),
            look.bright,
            PANEL_CAP,
            Align::Left,
        );
        let status = self.status(locks, ui);
        line(
            l,
            PANEL_Y + 27.0,
            20.0,
            &status,
            look.gold,
            PANEL_CAP,
            Align::Left,
        );
        let legend = match env.hint {
            Hint::Pad => "legend_pad",
            Hint::Levers => "legend_levers",
            Hint::Keys => "legend_keys",
            Hint::Glyphs => {
                // `hybrid_input` with a gamepad: the same line with the Xbox glyphs.
                Line::new()
                    .btn(glyphs::DPAD_LR)
                    .text(ui.get("hint_change"))
                    .gap(2.0)
                    .btn(Btn::A)
                    .text(ui.get("hint_toggle"))
                    .gap(2.0)
                    .btn(Btn::B)
                    .text(ui.get("hint_back"))
                    .draw(
                        l,
                        LAYER,
                        [x0 + BOX_X + 12.0, y0 + PANEL_Y + 50.0, BOX_W - 24.0, 24.0],
                        look.dim,
                        LEGEND_CAP,
                        Align::Center,
                    );
                return;
            }
        };
        line(
            l,
            PANEL_Y + 52.0,
            20.0,
            ui.get(legend),
            look.dim,
            LEGEND_CAP,
            Align::Center,
        );
    }
}

/// The options list's seventh row (port only): "Port settings" under the original six, at
/// their x and spacing (0x42a730), bright under the cursor like their selected sheet.
pub fn draw_options_item(
    l: &mut SpriteLayer,
    t: &FrontTables,
    look: &Look,
    ui: &UiText,
    selected: bool,
) {
    let (x0, y0) = t.origin;
    let paint = if selected { look.bright } else { look.dim };
    let mut s = TextStyle::plain(paint, Some(OPTIONS_ITEM_CAP));
    s.align = Align::Left;
    s.outline = Some(look.panel);
    let rect = [
        f32::from(x0 + 0x60),
        f32::from(y0 + OPTIONS_ITEM_Y - 4),
        300.0,
        32.0,
    ];
    l.add_styled_text(LAYER, rect, ui.get("options_item"), s);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{PortConfig, layered, locks};
    use std::ffi::OsString;

    fn press(f: impl FnOnce(&mut MenuInput)) -> MenuInput {
        let mut i = MenuInput::NONE;
        f(&mut i);
        i
    }

    fn index(key: &str) -> usize {
        ITEMS.iter().position(|i| i.key == key).unwrap()
    }

    fn no_env(_: &str) -> Option<OsString> {
        None
    }

    #[test]
    fn every_setting_is_listed_once_in_groups() {
        let mut keys: Vec<&str> = ITEMS.iter().map(|i| i.key).collect();
        let all: Vec<&str> = crate::config::all_keys().collect();
        assert_eq!(keys.len(), all.len());
        keys.sort_unstable();
        let mut sorted = all.clone();
        sorted.sort_unstable();
        assert_eq!(keys, sorted);
        // Groups are contiguous: each heading once.
        let headings = rows()
            .iter()
            .filter(|r| matches!(r, Row::Heading(_)))
            .count();
        assert_eq!(headings, 5);
    }

    #[test]
    fn navigation_is_clamped_and_scrolls() {
        let (mut cfg, mut s) = (LocaleConfig::default(), Settings::default());
        let locks = Locks::default();
        let mut p = PortSettings::new();
        let mut sfx = Vec::new();
        let up = press(|i| i.up = true);
        let down = press(|i| i.down = true);
        assert_eq!(p.tick(&up, &locks, &mut cfg, &mut s, &mut sfx), None);
        assert_eq!((p.cursor(), sfx.len()), (0, 0), "no wrap at the top");
        p.tick(&down, &locks, &mut cfg, &mut s, &mut sfx);
        assert_eq!(p.cursor(), 1);
        assert_eq!(sfx, [Sfx::Play(snd::MOVE)]);
        for _ in 0..40 {
            p.tick(&down, &locks, &mut cfg, &mut s, &mut sfx);
        }
        assert_eq!(p.cursor(), ITEMS.len() - 1, "no wrap at the bottom");
        let rows = rows().len();
        assert_eq!(p.top(), rows - VISIBLE, "the last row on screen");
        // Back to the first item of a group: its heading comes on screen with it.
        while ITEMS[p.cursor()].key != "hud_layout" {
            p.tick(&up, &locks, &mut cfg, &mut s, &mut sfx);
        }
        assert_eq!(p.top(), row_of(p.cursor()) - 1);
        assert_eq!(rows_on_screen(&p)[0], Row::Heading(Group::Display));
        // Nothing changed on the way.
        assert_eq!((cfg, s), (LocaleConfig::default(), Settings::default()));
    }

    fn rows_on_screen(p: &PortSettings) -> Vec<Row> {
        rows().into_iter().skip(p.top()).take(VISIBLE).collect()
    }

    #[test]
    fn levers_push_down_and_pull_up() {
        let (mut cfg, mut s) = (LocaleConfig::default(), Settings::default());
        let locks = Locks::default();
        let mut p = PortSettings::new();
        let mut sfx = Vec::new();
        let lever = |power, brake| {
            press(|i| {
                i.notch = Some(power);
                i.brake = Some(brake);
            })
        };
        p.tick(&lever(0, 0), &locks, &mut cfg, &mut s, &mut sfx);
        assert_eq!(p.cursor(), 0, "the first tick only reads the levers");
        p.tick(&lever(3, 0), &locks, &mut cfg, &mut s, &mut sfx);
        assert_eq!(p.cursor(), 3);
        p.tick(&lever(0, 0), &locks, &mut cfg, &mut s, &mut sfx);
        assert_eq!(p.cursor(), 3, "back to P0 keeps the row");
        p.tick(&lever(5, 0), &locks, &mut cfg, &mut s, &mut sfx);
        assert_eq!(p.cursor(), 8);
        p.tick(&lever(5, 2), &locks, &mut cfg, &mut s, &mut sfx);
        assert_eq!(p.cursor(), 6, "the brake pulls it up");
        // Confirm steps the value on.
        let i = index("hud_layout");
        while p.cursor() != i {
            p.tick(
                &press(|x| x.down = true),
                &locks,
                &mut cfg,
                &mut s,
                &mut sfx,
            );
        }
        let r = p.tick(
            &press(|x| x.confirm = true),
            &locks,
            &mut cfg,
            &mut s,
            &mut sfx,
        );
        assert_eq!(r, Some(Step::Changed(i)));
        assert_eq!(cfg.hud_layout, HudLayout::Enhanced);
    }

    #[test]
    fn values_cycle_both_ways() {
        let (mut cfg, mut s) = (LocaleConfig::default(), Settings::default());
        let at = |k| &ITEMS[index(k)];
        // A switch flips either way.
        at("fog").step(&mut cfg, &mut s, 1);
        assert!(!s.fog);
        at("fog").step(&mut cfg, &mut s, -1);
        assert!(s.fog);
        // Language: en ⇄ ja, and it counts as chosen.
        at("lang").step(&mut cfg, &mut s, 1);
        assert_eq!((cfg.lang.as_str(), cfg.lang_chosen), ("en", true));
        at("lang").step(&mut cfg, &mut s, 1);
        assert_eq!(cfg.lang, "ja");
        // Scaling: three values, wrapping both ways.
        at("ui_scaling").step(&mut cfg, &mut s, -1);
        assert_eq!(cfg.ui_scaling, UiScaling::Nearest);
        at("ui_scaling").step(&mut cfg, &mut s, 1);
        assert_eq!(cfg.ui_scaling, UiScaling::Smooth);
        // Scale: auto, 1..=4, auto.
        let mut seen = Vec::new();
        for _ in 0..=MAX_MULTIPLE {
            at("ui_scale").step(&mut cfg, &mut s, 1);
            seen.push(cfg.ui_scale_fixed);
        }
        assert_eq!(seen, [Some(1), Some(2), Some(3), Some(4), None]);
        at("ui_scale").step(&mut cfg, &mut s, -1);
        assert_eq!(cfg.ui_scale_fixed, Some(4));
        // Fonts are not changed here.
        let before = cfg.clone();
        at("font").step(&mut cfg, &mut s, 1);
        assert_eq!(cfg, before);
    }

    #[test]
    fn left_right_and_confirm_change_and_report() {
        let (mut cfg, mut s) = (LocaleConfig::default(), Settings::default());
        let locks = Locks::default();
        let mut p = PortSettings::new();
        let mut sfx = Vec::new();
        let r = p.tick(
            &press(|i| i.right = true),
            &locks,
            &mut cfg,
            &mut s,
            &mut sfx,
        );
        assert_eq!(r, Some(Step::Changed(0)));
        assert_eq!(cfg.lang, "en");
        assert_eq!(sfx, [Sfx::Play(snd::DECIDE_SOFT)]);
        let r = p.tick(
            &press(|i| i.left = true),
            &locks,
            &mut cfg,
            &mut s,
            &mut sfx,
        );
        assert_eq!(r, Some(Step::Changed(0)));
        assert_eq!(cfg.lang, "ja");
        let r = p.tick(
            &press(|i| i.cancel = true),
            &locks,
            &mut cfg,
            &mut s,
            &mut sfx,
        );
        assert_eq!(r, Some(Step::Back));
    }

    #[test]
    fn flags_and_variables_lock() {
        let args: Vec<String> = ["--no-fog", "--lang", "en", "--uncapped", "--size", "1x1"]
            .map(String::from)
            .to_vec();
        let env = |k: &str| (k == "SHUPPATSU_WIDE_WEATHER").then(|| OsString::from("0"));
        let locks = locks(env, &args);
        assert_eq!(locks.get("fog"), Some(&Lock::Flag("--no-fog".into())));
        assert_eq!(locks.get("lang"), Some(&Lock::Flag("--lang".into())));
        assert_eq!(locks.get("vsync"), Some(&Lock::Flag("--uncapped".into())));
        assert_eq!(
            locks.get("wide_weather"),
            Some(&Lock::Env("SHUPPATSU_WIDE_WEATHER".into()))
        );
        assert_eq!(locks.get("interpolate"), None);
        // A flag wins over a variable in the reason, as in the precedence.
        let env = |k: &str| (k == "SHUPPATSU_FOG").then(|| OsString::from("1"));
        assert_eq!(
            crate::config::locks(env, &args).get("fog"),
            Some(&Lock::Flag("--no-fog".into()))
        );
        // A locked setting does not change; the cancel sound answers.
        let (mut cfg, mut s) = (LocaleConfig::default(), Settings::default());
        let mut p = PortSettings::new();
        while ITEMS[p.cursor()].key != "fog" {
            p.tick(
                &press(|i| i.down = true),
                &locks,
                &mut cfg,
                &mut s,
                &mut Vec::new(),
            );
        }
        let mut sfx = Vec::new();
        for input in [
            press(|i| i.confirm = true),
            press(|i| i.left = true),
            press(|i| i.right = true),
        ] {
            assert_eq!(p.tick(&input, &locks, &mut cfg, &mut s, &mut sfx), None);
        }
        assert!(s.fog);
        assert_eq!(sfx, [Sfx::Play(snd::CANCEL); 3]);
        let ui = UiText::load("en", None);
        assert_eq!(
            p.status(&locks, &ui),
            "Locked: set by --no-fog on the command line"
        );
        // Read-only fonts answer the same way.
        while ITEMS[p.cursor()].key != "font" {
            p.tick(
                &press(|i| i.up = true),
                &locks,
                &mut cfg,
                &mut s,
                &mut Vec::new(),
            );
        }
        let r = p.tick(
            &press(|i| i.confirm = true),
            &locks,
            &mut cfg,
            &mut s,
            &mut sfx,
        );
        assert_eq!(r, None);
        assert_eq!(p.status(&locks, &ui), "Change it in config.toml");
    }

    /// Every editable setting, stepped away from its default, written with the file writer and
    /// read back through the layers, has the stepped value; the rest of the file stays.
    #[test]
    fn config_round_trip_for_every_key() {
        let dir =
            std::env::temp_dir().join(format!("shuppatsu-port-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(crate::config::FILE_NAME);
        std::fs::create_dir_all(&dir).unwrap();
        let start = "# mine\nfuture_key = 7 # kept\n\n[later]\nx = 1\n";
        std::fs::write(&path, start).unwrap();
        let (mut cfg, mut s) = (LocaleConfig::default(), Settings::default());
        for it in ITEMS.iter().filter(|i| i.editable()) {
            it.step(&mut cfg, &mut s, 1);
            crate::config::store(&path, it.key, it.file_value(&cfg, &s)).unwrap();
            let file = PortConfig::read(&path);
            let (c2, s2) = layered(Some(&file), no_env, &mut vec![]).unwrap();
            assert_eq!(
                it.file_value(&c2, &s2),
                it.file_value(&cfg, &s),
                "{} read back",
                it.key
            );
        }
        // Everything at once, and the file's own lines kept.
        let file = PortConfig::read(&path);
        let (c2, s2) = layered(Some(&file), no_env, &mut vec![]).unwrap();
        assert_eq!(s2, s);
        assert_eq!(
            (
                c2.lang.as_str(),
                c2.hud_layout,
                c2.ui_scaling,
                c2.ui_scale_fixed
            ),
            ("en", HudLayout::Enhanced, UiScaling::Integer, Some(1))
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with("# mine\nfuture_key = 7 # kept\n"),
            "{text}"
        );
        assert!(text.ends_with("[later]\nx = 1\n"), "{text}");
        assert_eq!(file.other.get("future_key"), Some(&toml::Value::Integer(7)));
        // ui_scale back to following the window removes its line.
        let at = &ITEMS[index("ui_scale")];
        for _ in 0..MAX_MULTIPLE {
            at.step(&mut cfg, &mut s, 1);
        }
        assert_eq!(cfg.ui_scale_fixed, None);
        crate::config::store(&path, "ui_scale", at.file_value(&cfg, &s)).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("ui_scale ="), "{text}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn both_catalogues_have_every_string() {
        for lang in ["en", "ja"] {
            let cat = crate::locale::Catalogue::load(lang, None).unwrap();
            let have: Vec<&str> = cat.ui.iter().map(|(k, _)| k.as_str()).collect();
            for id in ui_ids() {
                assert!(have.contains(&id.as_str()), "{lang}: ui.{id} missing");
            }
        }
        let ja = UiText::load("ja", None);
        assert_eq!(ja.get("title"), "ポート設定");
        assert_eq!(ja.get("on"), "オン");
        assert!(known_ui_key("fog_desc") && !known_ui_key("fog_description"));
    }

    #[test]
    fn values_and_status_text() {
        let ui = UiText::load("en", None);
        let (cfg, s) = (LocaleConfig::default(), Settings::default());
        let text = |k| ITEMS[index(k)].value_text(&cfg, &s, &ui);
        assert_eq!(text("lang"), "日本語");
        assert_eq!(text("fog"), "On");
        assert_eq!(text("square_pixels"), "Off");
        assert_eq!(text("ui_scale"), "Auto");
        assert_eq!(text("font"), "M+ 2 Medium (bundled)");
        let mut p = PortSettings::new();
        let locks = Locks::default();
        assert_eq!(p.status(&locks, &ui), "Applies now");
        p.cursor = index("vsync");
        assert_eq!(p.status(&locks, &ui), "Applies after a restart");
        p.cursor = index("fog");
        assert_eq!(p.status(&locks, &ui), "Applies from the next stage");
    }

    #[test]
    fn draws_rows_bar_and_panel() {
        let t = crate::front::tests_support::tables();
        let look = Look::new(None);
        let ui = UiText::load("en", None);
        let p = PortSettings::new();
        let mut l = SpriteLayer::default();
        let (cfg, s) = (LocaleConfig::default(), Settings::default());
        let env = Env::default();
        p.draw(&look, &ui, &Locks::default(), &cfg, &s, &env, &t, &mut l);
        let texts: Vec<&str> = l
            .widgets
            .iter()
            .filter_map(|w| match &w.kind {
                WidgetKind::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts[0], "Port Settings");
        assert_eq!(texts[1], "Language & text");
        assert_eq!(texts[2], "Language");
        assert_eq!(texts[3], "◀  日本語  ▶");
        assert!(texts.contains(&"Applies now"));
        assert!(texts.contains(&"◀ ▶ Change     Enter Toggle     Esc Back"));
        let fills = l
            .widgets
            .iter()
            .filter(|w| matches!(w.kind, WidgetKind::Fill(_)))
            .count();
        assert_eq!(fills, 2, "the cursor bar and the panel");
        assert!(
            texts.contains(&"▼") && !texts.contains(&"▲"),
            "more below only"
        );
        assert_eq!(l.slots[super::super::slot::TITLE].tilemap, -1);
    }
}

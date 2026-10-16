//! Enhancement `wide_plates`: the station plates of the driving HUD as horizontal boxes, with a
//! translation drawn and the enhanced HUD layout (docs/localisation.md, "Station plates").
//! Japanese and the original layout keep the original vertical plates.
//!
//! The original shows two tall plates with vertical kanji in the top right (hud.md §3.7). Here
//! the same corner holds two stacked boxes, "from" on top and "next stop" below with a small
//! arrow between them. Each box shows the station's original horizontal name sprite (the one
//! the captions use), recoloured to the plates' yellow, and, when a translation with a font is
//! active, the translated name in small text underneath. On a wide canvas (enhancement
//! `wide_hud`) the boxes grow to the left by up to [`MAX_EXTRA`], into the room the right
//! group gains; the vehicle picture moves left with them.
//!
//! The plate slots keep the original's state (their images are set and cleared exactly as in
//! arcade mode) but are parked off screen; the boxes are drawn from that state every tick as
//! widgets, so they appear and disappear with the plates.

use ddg_formats::cg::Palette;
use ddg_formats::hud::HudTables;
use ddg_render::sprites::{Paint, SpriteBank, SpriteLayer, WidgetKind};
use std::collections::HashMap;

/// Port-only palette slot for the recoloured name sprites; the original's init and the port's
/// screens use slots below 0x20.
pub const PAL_PLATES: u8 = 0x7f;
/// The box colour, #303030.
const GREY: [u8; 3] = [0x30, 0x30, 0x30];

// Layout in 640×480 pixels, inside the original plates' footprint: "from" (493, 8) and "next"
// (552, 8), 32×96 each, and the arrow at (530, 48). Boxes grow to the left by `extra`.
const BACKDROP: [f32; 4] = [490.0, 7.0, 98.0, 100.0];
const BOX_X: f32 = 492.0;
const BOX_W: f32 = 94.0;
const BOX_H: f32 = 42.0;
const BOX_Y: [f32; 2] = [10.0, 62.0];
const ARROW_Y: f32 = 53.0;
const ARROW_W: f32 = 11.0;
const ARROW_H: f32 = 9.0;
/// The most the boxes widen on a wide canvas (to 150 px, 1.6× the 4:3 width).
pub const MAX_EXTRA: f32 = 56.0;
/// The romaji line under the kanji, and the gap between them; both grow with the boxes.
const ROMAJI_H: f32 = 10.0;
const ROMAJI_H_WIDE: f32 = 12.0;
/// Capital height of the romaji (it shrinks for long names), 4:3 and fully widened.
const ROMAJI_CAP: f32 = 5.0;
const ROMAJI_CAP_WIDE: f32 = 7.0;
const GAP: f32 = 2.0;
/// Side margin of the romaji in a box (the kanji may use the full width).
const MARGIN: f32 = 2.0;

struct Name {
    /// The plain horizontal name sprite and its inked columns `[x, w]`.
    sprite: u16,
    ink_x: [u16; 2],
    romaji: Option<String>,
}

/// The colour indices the layout uses, read from the art at load.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Colours {
    /// The plates' palette slot.
    palette: u8,
    /// Plate background (black) and lettering (yellow).
    plate_bg: u8,
    plate_fg: u8,
    /// The name sprites' lettering (green in the captions).
    letter: u8,
    /// An index the name sprites do not use, set to [`GREY`] in [`PAL_PLATES`].
    grey: u8,
}

pub struct EnglishPlates {
    /// By plate tilemap id.
    names: HashMap<i16, Name>,
    /// Inked rows `[y, h]` common to all name sprites.
    ink_y: [u16; 2],
    colours: Colours,
}

/// Index counts of a tilemap's art (index 0, transparency, is not counted).
fn histogram(bank: &SpriteBank, tbl: u16, counts: &mut [u32; 256]) {
    if let Some((_, _, px)) = bank.art(tbl) {
        for v in px.into_iter().filter(|&v| v != 0) {
            counts[usize::from(v)] += 1;
        }
    }
}

/// Indices by count, most common first.
fn ranked(counts: &[u32; 256]) -> Vec<u8> {
    let mut r: Vec<u8> = (1..=255u8)
        .filter(|&i| counts[usize::from(i)] > 0)
        .collect();
    r.sort_by_key(|&i| std::cmp::Reverse(counts[usize::from(i)]));
    r
}

impl EnglishPlates {
    /// `station_plates`: plate tilemap by station-name id (`0x46645c`); `romaji`: translated
    /// names by plate tilemap id; `palette`: the plates' palette slot. `None` when the art has
    /// no usable plates or name sprites.
    pub fn new(
        bank: &SpriteBank,
        tables: &HudTables,
        station_plates: &[i16],
        palette: u8,
        romaji: &HashMap<u16, String>,
    ) -> Option<Self> {
        let mut plate_counts = [0u32; 256];
        let mut name_counts = [0u32; 256];
        let mut names = HashMap::new();
        let (mut y0, mut y1) = (u16::MAX, 0u16);
        // Only names some stage uses: the plate table runs past the real plates.
        let used: std::collections::HashSet<usize> = tables
            .stage_station_ids
            .iter()
            .flatten()
            .map(|&i| usize::from(i))
            .collect();
        for (id, &plate) in station_plates.iter().enumerate() {
            let (Ok(plate_tbl), Some(n)) = (u16::try_from(plate), tables.station_names.get(id))
            else {
                continue;
            };
            if !used.contains(&id) {
                continue;
            }
            if names.contains_key(&plate) {
                continue;
            }
            let Some((w, h, px)) = bank.art(n.plain) else {
                continue;
            };
            histogram(bank, plate_tbl, &mut plate_counts);
            let (mut x0, mut x1) = (w, 0);
            for y in 0..h {
                for x in 0..w {
                    let v = px[y * w + x];
                    if v != 0 {
                        name_counts[usize::from(v)] += 1;
                        x0 = x0.min(x);
                        x1 = x1.max(x + 1);
                        y0 = y0.min(y as u16);
                        y1 = y1.max(y as u16 + 1);
                    }
                }
            }
            if x0 >= x1 {
                continue;
            }
            names.insert(
                plate,
                Name {
                    sprite: n.plain,
                    ink_x: [x0 as u16, (x1 - x0) as u16],
                    romaji: romaji.get(&plate_tbl).cloned(),
                },
            );
        }
        let plate = ranked(&plate_counts);
        let (&plate_bg, &plate_fg) = (plate.first()?, plate.get(1)?);
        let letter = ranked(&name_counts).into_iter().find(|&i| i != plate_bg)?;
        let grey = (1..=255u8)
            .rev()
            .find(|&i| name_counts[usize::from(i)] == 0 && i != plate_bg && i != plate_fg)?;
        Some(Self {
            names,
            ink_y: [y0, y1.saturating_sub(y0)],
            colours: Colours {
                palette,
                plate_bg,
                plate_fg,
                letter,
                grey,
            },
        })
    }

    /// Draws the boxes for the plate slots' current images (`None` = no plate), `extra`
    /// pixels wider than on 4:3 ([`extra_width`]), and sets up [`PAL_PLATES`] from the
    /// plates' palette as it is this tick.
    pub fn draw(
        &self,
        layer: &mut SpriteLayer,
        l: u8,
        from: Option<i16>,
        next: Option<i16>,
        extra: f32,
    ) {
        if from.is_none() && next.is_none() {
            return;
        }
        let c = self.colours;
        let Some(Some(src)) = layer.palettes.get(usize::from(c.palette)) else {
            return;
        };
        let mut pal: Palette = src.clone();
        pal.0[usize::from(c.letter)] = src.0[usize::from(c.plate_fg)];
        // Palette entries are bytes B, R, G (cg::Palette).
        pal.0[usize::from(c.grey)] = u32::from_le_bytes([GREY[2], GREY[0], GREY[1], 0]);
        layer.palettes[usize::from(PAL_PLATES)] = Some(pal);
        let paint = |index| Paint {
            palette: PAL_PLATES,
            index,
        };
        let extra = extra.clamp(0.0, MAX_EXTRA);
        let [bx, by, bw, bh] = BACKDROP;
        let backdrop = [bx - extra, by, bw + extra, bh];
        let (box_x, box_w) = (BOX_X - extra, BOX_W + extra);
        let arrow = [box_x + (box_w - ARROW_W) / 2.0, ARROW_Y, ARROW_W, ARROW_H];
        layer.add_widget(l, backdrop, WidgetKind::Fill(paint(c.plate_bg)));
        layer.add_widget(l, arrow, WidgetKind::DownArrow(paint(c.plate_fg)));
        let t = extra / MAX_EXTRA;
        let romaji = (
            ROMAJI_H + (ROMAJI_H_WIDE - ROMAJI_H) * t,
            ROMAJI_CAP + (ROMAJI_CAP_WIDE - ROMAJI_CAP) * t,
        );
        for (plate, y) in [(from, BOX_Y[0]), (next, BOX_Y[1])] {
            let Some(name) = plate.and_then(|p| self.names.get(&p)) else {
                continue;
            };
            let rect = [box_x, y, box_w, BOX_H];
            layer.add_widget(l, rect, WidgetKind::Fill(paint(c.grey)));
            self.name(layer, l, name, rect, paint(c.plate_fg), romaji);
        }
    }

    /// The kanji sprite and the romaji under it, centred in `rect` as a group. The sprite is
    /// drawn with whole canvas pixels per art pixel: 1 up to `ui_scale` 2, 2 at 3 and 4 (so at
    /// 2× it is half the original size, the lettering size of the other HUD text).
    /// `romaji`: the romaji line's height and capital height.
    fn name(
        &self,
        layer: &mut SpriteLayer,
        l: u8,
        name: &Name,
        rect: [f32; 4],
        paint: Paint,
        (romaji_h, romaji_cap): (f32, f32),
    ) {
        let s = layer.scale.max(1);
        let zoom = s.div_ceil(2);
        let kanji_h = f32::from(self.ink_y[1]) * zoom as f32 / s as f32;
        // No romaji without a translation and its font (the names are then empty).
        let romaji = name.romaji.as_deref().filter(|r| !r.is_empty());
        let group_h = kanji_h
            + if romaji.is_some() {
                GAP + romaji_h
            } else {
                0.0
            };
        let [x, y, w, h] = rect;
        let top = y + ((h - group_h) / 2.0).max(0.0);
        layer.add_widget(
            l,
            [x, top, w, kanji_h],
            WidgetKind::Sprite {
                tilemap: name.sprite,
                src: [name.ink_x[0], self.ink_y[0], name.ink_x[1], self.ink_y[1]],
                zoom: zoom as u8,
                palette: PAL_PLATES,
            },
        );
        if let Some(r) = romaji {
            let ry = top + kanji_h + GAP;
            let rect = [x + MARGIN, ry, w - 2.0 * MARGIN, romaji_h];
            layer.add_text(l, rect, r, paint, Some(romaji_cap));
        }
    }

    /// Names whose kanji sprite is wider than a box at `ui_scale` `s` (clipped at both ends):
    /// `(plate tilemap, ink width in art pixels)`.
    pub fn too_wide(&self, s: usize) -> Vec<(i16, u16)> {
        let s = s.max(1);
        let room = (BOX_W * s as f32) as usize;
        let mut v: Vec<(i16, u16)> = self
            .names
            .iter()
            .filter(|(_, n)| usize::from(n.ink_x[1]) * s.div_ceil(2) > room)
            .map(|(&p, n)| (p, n.ink_x[1]))
            .collect();
        v.sort_unstable();
        v
    }
}

/// How much wider the boxes can be on this canvas: the room the right group gained over the
/// centred 4:3 layout (0 at 4:3), up to [`MAX_EXTRA`], in whole original pixels.
pub fn extra_width(layer: &SpriteLayer) -> f32 {
    layer.side_room().floor().clamp(0.0, MAX_EXTRA)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::GameData;
    use ddg_formats::cab_hud::CabTables;
    use ddg_formats::exe::Exe;

    /// Against the user's data: the colours found in the art, every plate a stage shows has a
    /// name sprite and a catalogued English name, and every kanji sprite fits a box at 2×.
    #[test]
    fn plates_from_the_data() {
        if std::env::var_os("DDG_DATA_DIR").is_none() {
            eprintln!("skipped: DDG_DATA_DIR not set");
            return;
        }
        let data = GameData::from_env().unwrap();
        let exe_bytes = data.exe().unwrap();
        let exe = Exe::parse(&exe_bytes).unwrap();
        let bank = SpriteBank::new(
            &ddg_formats::cg::CgFile::parse(&data.read("cg.rom").unwrap()).unwrap(),
            ddg_formats::cg::parse_tbl(&data.read("tbl.rom").unwrap()).unwrap(),
        );
        let tables = HudTables::read(&exe).unwrap();
        let cab = CabTables::read(&exe).unwrap();
        let en =
            crate::locale::Catalogue::parse(include_str!("../../../../locales/en.toml")).unwrap();
        let romaji: HashMap<u16, String> = en.station_plates.into_iter().collect();
        let p = EnglishPlates::new(&bank, &tables, &cab.station_plates, 4, &romaji).unwrap();
        // Plates: black (1) and yellow (0xa); names: green lettering 0x1c (hud.md §3.7, §3.13).
        assert_eq!((p.colours.plate_bg, p.colours.plate_fg), (1, 0xa));
        assert_eq!(p.colours.letter, 0x1c);
        for ids in &tables.stage_station_ids {
            for &id in ids {
                // Stations the trains pass have no plate (−1).
                let plate = cab.station_plates[usize::from(id)];
                if plate < 0 {
                    continue;
                }
                let name = &p.names[&plate];
                assert!(
                    name.romaji.is_some(),
                    "plate {plate:#x} has no English name"
                );
            }
        }
        assert_eq!(p.too_wide(2), []);
        // Drawing: backdrop, arrow, and per box a fill, the sprite and the romaji.
        let mut layer = SpriteLayer {
            scale: 2,
            ..SpriteLayer::default()
        };
        layer.palettes[4] = Some(Palette([0x0010_2030; 256]));
        p.draw(&mut layer, 2, Some(0x4e1), Some(0x4e2), MAX_EXTRA);
        assert_eq!(layer.widgets.len(), 2 + 2 * 3);
        let pal = layer.palettes[usize::from(PAL_PLATES)].as_ref().unwrap();
        assert_eq!(pal.rgba(p.colours.grey), [0x30, 0x30, 0x30, 255]);
    }
}

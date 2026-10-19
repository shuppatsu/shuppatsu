//! Enhancement `board_names`: romaji under the station names of the pre-departure route board
//! (docs/localisation.md, "Route board"), with a translation drawn.
//!
//! The board's diagram (slot 0x7b, hud.md §3.7) is one sprite per stage and stations served:
//! a white board with the line and its station dots across the top and each station's name in
//! vertical kanji under its dot, black for stops and grey for the stations passed. The kanji
//! stay; this module finds the name columns in the art at load and draws each station's romaji
//! centred under its column, in the column's own lettering colour.
//!
//! A diagram starts at the segment's start station: column `i` is station `station + i` of the
//! stage.
//!
//! Placement, left to right: each name goes on the first line under its own kanji where it
//! touches no other column's kanji and no name already placed (so on a crowded board the
//! names of neighbouring columns step down in turn); a name that fits nowhere on one line is
//! broken at a hyphen or space onto two. A name whose kanji reach the bottom edge is turned
//! 90° beside them, in the gap to the next column. Failing all that the name is made smaller
//! (down to [`SHRINK`]), and last of all narrowed to the room directly under its kanji.

use shuppatsu_render::sprites::{Paint, SpriteBank, SpriteLayer, TextStyle, WidgetKind};
use shuppatsu_render::text::Align;
use std::collections::HashMap;

/// Capital height of the romaji, original pixels, and at least this many canvas pixels (so the
/// text stays legible at `ui_scale` 1).
const CAP: f32 = 5.5;
const MIN_CANVAS_CAP: f32 = 6.0;
/// Line height as a multiple of the capital height, for one line and for a broken name.
const LINE: f32 = 2.2;
const LINE2: f32 = 3.8;
/// Gap under the kanji, and the least space between a name and anything else.
const GAP: f32 = 1.0;
const SPACE: f32 = 3.0;
/// Smallest capitals of a name turned beside its kanji.
const MIN_TURNED_CAP: f32 = 3.0;
/// The smallest a name under its kanji is made to fit beside its neighbours.
const SHRINK: f32 = 0.7;
/// Columns of lettering this close (art pixels) are one column (strokes of 川, 小).
const MERGE: usize = 6;
/// Narrower ink is not a column (a stray pixel).
const MIN_COLUMN_W: usize = 4;

/// One station-name column of a diagram, in art pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Column {
    /// Ink from `left` to `right`.
    left: f32,
    right: f32,
    /// First row under the kanji.
    bottom: u16,
    /// The lettering colour (black for a stop, grey for a station passed).
    ink: u8,
}

impl Column {
    fn centre(&self) -> f32 {
        (self.left + self.right) / 2.0
    }
}

/// The name columns of one diagram sprite, left to right, the first row of the names and the
/// first row of the board's bottom edge.
#[derive(Debug, Clone, PartialEq)]
struct Diagram {
    columns: Vec<Column>,
    top: u16,
    floor: u16,
    width: u16,
}

/// A translated name: the text, its two-line form, and the widths of both (widest line) with
/// capitals 1 px high.
struct Name {
    text: String,
    two: String,
    width: f32,
    two_width: f32,
}

/// Where a name goes: `[x, y, w, h]` in art pixels of the diagram, on two lines or not,
/// turned 90° or not, with capitals `cap` high.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Spot {
    rect: [f32; 4],
    two: bool,
    turned: bool,
    cap: f32,
}

pub struct BoardNames {
    /// By diagram tilemap.
    diagrams: HashMap<u16, Diagram>,
    /// By station-name id.
    names: HashMap<usize, Name>,
}

impl BoardNames {
    /// `diagrams`: the diagram tilemaps the stages use; `names`: romaji by station-name id.
    pub fn new(
        bank: &SpriteBank,
        diagrams: impl IntoIterator<Item = u16>,
        names: &HashMap<usize, String>,
    ) -> Self {
        // Each diagram is read from its art on its own core (tens of ms in all on one).
        let tbls: Vec<u16> = diagrams.into_iter().collect();
        let diagrams = shuppatsu_render::par::par_map(&tbls, |&t| Some((t, Diagram::of(bank, t)?)))
            .into_iter()
            .flatten()
            .collect();
        let names = names
            .iter()
            .filter_map(|(&id, n)| {
                let two = broken(n);
                Some((
                    id,
                    Name {
                        width: bank.label_width(n, 1.0)?,
                        two_width: bank.label_width(&two, 1.0)?,
                        text: n.clone(),
                        two,
                    },
                ))
            })
            .collect();
        Self { diagrams, names }
    }

    /// Draws the romaji for diagram `tilemap` placed at `pos` (the slot's position), on sprite
    /// layer `l` in palette `palette`; `name_at(i)` is the station-name id of column `i`.
    pub fn draw(
        &self,
        layer: &mut SpriteLayer,
        tilemap: i16,
        pos: (i16, i16),
        (l, palette): (u8, u8),
        name_at: &dyn Fn(usize) -> Option<usize>,
    ) {
        let Some(d) = u16::try_from(tilemap)
            .ok()
            .and_then(|t| self.diagrams.get(&t))
        else {
            return;
        };
        let cap = CAP.max(MIN_CANVAS_CAP / layer.scale.max(1) as f32);
        let labels: Vec<(usize, &Name)> = (0..d.columns.len())
            .filter_map(|i| Some((i, self.names.get(&name_at(i)?)?)))
            .collect();
        let widths: Vec<(usize, f32, f32)> = labels
            .iter()
            .map(|&(i, n)| (i, n.width * cap, n.two_width * cap))
            .collect();
        let (x0, y0) = (f32::from(pos.0), f32::from(pos.1));
        for ((i, name), spot) in labels.iter().zip(place(d, &widths, cap)) {
            let paint = Paint {
                palette,
                index: d.columns[*i].ink,
            };
            let [x, y, w, h] = spot.rect;
            let text = if spot.two { &name.two } else { &name.text }.clone();
            let style = TextStyle {
                align: if spot.turned {
                    Align::Left
                } else {
                    Align::Center
                },
                vertical: spot.turned,
                ..TextStyle::plain(paint, Some(spot.cap))
            };
            layer.add_widget(l, [x0 + x, y0 + y, w, h], WidgetKind::Text { text, style });
        }
    }
}

/// Places the names `(column, one-line width, two-line width)` of diagram `d` (see the module
/// notes), capitals `cap` high.
fn place(d: &Diagram, names: &[(usize, f32, f32)], cap: f32) -> Vec<Spot> {
    let floor = f32::from(d.floor);
    let top = f32::from(d.top);
    let overlaps = |a: [f32; 4], b: [f32; 4]| {
        a[0] < b[0] + b[2] + SPACE
            && b[0] < a[0] + a[2] + SPACE
            && a[1] < b[1] + b[3]
            && b[1] < a[1] + a[3]
    };
    let kanji: Vec<[f32; 4]> = d
        .columns
        .iter()
        .map(|c| [c.left, top, c.right - c.left, f32::from(c.bottom) - top])
        .collect();
    let mut placed: Vec<[f32; 4]> = Vec::new();
    let mut spots = Vec::new();
    for &(i, one, two) in names {
        let c = d.columns[i];
        let under = f32::from(c.bottom) + GAP;
        let free = |r: [f32; 4]| {
            r[0] >= 0.0
                && r[0] + r[2] <= f32::from(d.width)
                && r[1] + r[3] <= floor
                && !kanji
                    .iter()
                    .enumerate()
                    .any(|(j, &k)| j != i && overlaps(r, k))
                && !placed.iter().any(|&p| overlaps(r, p))
        };
        // Lines under the kanji, one line height apart, down to the bottom edge; one pixel of
        // slack each side so the fitted text is not shrunk by rounding.
        let under_at = |w: f32, lines: f32, cap: f32, two: bool| {
            let h = cap * lines;
            let mut y = under;
            while y + h <= floor {
                let rect = [c.centre() - w / 2.0 - 1.0, y, w + 2.0, h];
                if free(rect) {
                    return Some(Spot {
                        rect,
                        two,
                        turned: false,
                        cap,
                    });
                }
                y += cap * LINE;
            }
            None
        };
        // Turned beside the kanji, in the gap to the next column, from the names' top.
        let beside = || {
            let next = d.columns.get(i + 1).map_or(f32::from(d.width), |n| n.left);
            let (x, w) = (c.right + GAP, next - c.right - 2.0 * GAP);
            let turned = cap.min(w / LINE);
            let rect = [x, top, w, one * turned / cap + 2.0];
            // In the gap by construction: only the bottom edge and the names placed so far.
            let clear = rect[1] + rect[3] <= floor && !placed.iter().any(|&p| overlaps(rect, p));
            (turned >= MIN_TURNED_CAP && clear).then_some(Spot {
                rect,
                two: false,
                turned: true,
                cap: turned,
            })
        };
        let no_room_under = under + cap * LINE > floor;
        let spot = under_at(one, LINE, cap, false)
            .or_else(|| (two < one).then(|| under_at(two, LINE2, cap, true))?)
            .or_else(|| no_room_under.then(beside)?)
            .or_else(|| {
                // Smaller, down to SHRINK of the size.
                (1..=3)
                    .map(|k| 1.0 - (1.0 - SHRINK) * k as f32 / 3.0)
                    .find_map(|s| under_at(one * s, LINE, cap * s, false))
            })
            .unwrap_or_else(|| {
                // Directly under, narrowed (and shrunk) to what room there is.
                let room = pitch(d, i) - SPACE;
                let h = (cap * LINE).min(floor - under).max(0.0);
                Spot {
                    rect: [c.centre() - room / 2.0, under, room, h],
                    two: false,
                    turned: false,
                    cap,
                }
            });
        placed.push(spot.rect);
        spots.push(spot);
    }
    spots
}

/// The room a column's label has across: the distance to the nearer neighbour, or for a lone
/// column twice the distance to the nearer board edge.
fn pitch(d: &Diagram, i: usize) -> f32 {
    let columns = &d.columns;
    let c = columns[i].centre();
    let left = i.checked_sub(1).map(|j| c - columns[j].centre());
    let right = columns.get(i + 1).map(|n| n.centre() - c);
    match (left, right) {
        (Some(a), Some(b)) => a.min(b),
        (Some(a), None) | (None, Some(a)) => a,
        (None, None) => 2.0 * c.min(f32::from(d.width) - c),
    }
}

/// `name` on two lines, broken at its middlemost hyphen or space (the hyphen stays on the first
/// line); unchanged without one.
fn broken(name: &str) -> String {
    let mid = name.chars().count() / 2;
    let best = name
        .char_indices()
        .enumerate()
        .filter(|(_, (_, c))| *c == '-' || *c == ' ')
        .min_by_key(|(n, _)| n.abs_diff(mid));
    match best {
        Some((_, (b, '-'))) => format!("{}\n{}", &name[..=b], &name[b + 1..]),
        Some((_, (b, _))) => format!("{}\n{}", &name[..b], &name[b + 1..]),
        None => name.to_owned(),
    }
}

impl Diagram {
    /// Finds the name columns in diagram `tbl`'s art: the board is its commonest colour; rows
    /// and columns opaque almost throughout are its frame; the first rows mostly inked are the
    /// line with its dots, and the names start at the first blank row below the dots. Ink
    /// columns closer than [`MERGE`] belong to one name.
    fn of(bank: &SpriteBank, tbl: u16) -> Option<Self> {
        let (w, h, px) = bank.art(tbl)?;
        let mut counts = [0u32; 256];
        for &v in &px {
            counts[usize::from(v)] += 1;
        }
        let board = (1..=255u8).max_by_key(|&i| counts[usize::from(i)])?;
        let ink = |x: usize, y: usize| {
            let v = px[y * w + x];
            v != 0 && v != board
        };
        let frame_col: Vec<bool> = (0..w)
            .map(|x| (0..h).filter(|&y| ink(x, y)).count() * 10 >= h * 9)
            .collect();
        let inner: Vec<usize> = (0..w).filter(|&x| !frame_col[x]).collect();
        let row_ink = |y: usize| inner.iter().filter(|&&x| ink(x, y)).count();
        let frame_row = |y: usize| row_ink(y) * 10 >= inner.len() * 9;
        // The line: the first rows inked over half the width (not the frame); the names start
        // at the first blank row under it and end at the bottom edge.
        let line = (0..h).find(|&y| !frame_row(y) && row_ink(y) * 2 > inner.len())?;
        let names_top = (line..h).find(|&y| row_ink(y) == 0)?;
        let floor = (names_top..h).find(|&y| frame_row(y)).unwrap_or(h);
        let inked = |x: usize| !frame_col[x] && (names_top..floor).any(|y| ink(x, y));
        let mut columns = Vec::new();
        let mut x = 0;
        while x < w {
            if !inked(x) {
                x += 1;
                continue;
            }
            let start = x;
            let mut end = x + 1;
            let mut gap = 0;
            x += 1;
            while x < w && gap <= MERGE {
                if inked(x) {
                    end = x + 1;
                    gap = 0;
                } else {
                    gap += 1;
                }
                x += 1;
            }
            x = end;
            if end - start < MIN_COLUMN_W {
                continue;
            }
            let mut colours = [0u32; 256];
            let mut bottom = names_top;
            for y in names_top..floor {
                for cx in start..end {
                    if ink(cx, y) {
                        colours[usize::from(px[y * w + cx])] += 1;
                        bottom = bottom.max(y + 1);
                    }
                }
            }
            let ink = (1..=255u8).max_by_key(|&i| colours[usize::from(i)])?;
            columns.push(Column {
                left: start as f32,
                right: end as f32,
                bottom: bottom as u16,
                ink,
            });
        }
        Some(Self {
            columns,
            top: names_top as u16,
            floor: floor as u16,
            width: w as u16,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_break_at_the_middle_hyphen_or_space() {
        assert_eq!(broken("Ugo-Sakai"), "Ugo-\nSakai");
        assert_eq!(broken("Hokuhoku-Ōshima"), "Hokuhoku-\nŌshima");
        assert_eq!(broken("Akita"), "Akita");
        assert_eq!(broken("Ōu Main"), "Ōu\nMain");
    }

    fn column(centre: f32, bottom: u16) -> Column {
        Column {
            left: centre - 12.0,
            right: centre + 12.0,
            bottom,
            ink: 1,
        }
    }

    fn board(columns: Vec<Column>) -> Diagram {
        Diagram {
            columns,
            top: 24,
            floor: 136,
            width: 352,
        }
    }

    #[test]
    fn pitch_is_the_nearer_neighbour() {
        let d = board(vec![column(40.0, 50), column(72.0, 50), column(136.0, 50)]);
        assert_eq!(pitch(&d, 0), 32.0);
        assert_eq!(pitch(&d, 1), 32.0);
        assert_eq!(pitch(&d, 2), 64.0);
        assert_eq!(pitch(&board(vec![column(40.0, 50)]), 0), 80.0);
    }

    #[test]
    fn names_sit_under_their_kanji_and_step_down_when_crowded() {
        // Two far apart: each directly under its own kanji, centred on it.
        let d = board(vec![column(76.0, 99), column(268.0, 73)]);
        let s = place(&d, &[(0, 40.0, 40.0), (1, 30.0, 30.0)], 5.0);
        assert_eq!(s[0].rect, [76.0 - 21.0, 99.0 + GAP, 42.0, 5.0 * LINE]);
        assert_eq!(s[1].rect[1], 73.0 + GAP);
        // Three columns 32 apart with the same depth: the middle name is too wide beside its
        // neighbour, so it steps down one line; the third fits beside the first line again.
        let d = board(vec![column(44.0, 98), column(76.0, 98), column(108.0, 98)]);
        let s = place(
            &d,
            &[(0, 26.0, 26.0), (1, 40.0, 40.0), (2, 20.0, 20.0)],
            5.0,
        );
        assert_eq!(s[0].rect[1], 98.0 + GAP);
        assert_eq!(s[1].rect[1], 98.0 + GAP + 5.0 * LINE);
        assert_eq!(s[2].rect[1], 98.0 + GAP);
        assert!(s.iter().all(|s| !s.two));
        // A long name with no room on one line anywhere goes on two lines.
        let d = board(vec![
            column(28.0, 130),
            column(76.0, 60),
            column(124.0, 130),
        ]);
        let s = place(&d, &[(1, 70.0, 36.0)], 5.0);
        assert!(s[0].two);
    }
}

#[cfg(test)]
mod data_tests {
    use super::*;

    /// Against the user's data: every diagram has name columns under its line, on the board.
    #[test]
    fn diagram_columns_from_the_data() {
        if std::env::var_os("SHUPPATSU_DATA_DIR").is_none() {
            eprintln!("skipped: SHUPPATSU_DATA_DIR not set");
            return;
        }
        let data = crate::scene::GameData::from_env().unwrap();
        let exe_bytes = data.exe().unwrap();
        let exe = shuppatsu_formats::exe::Exe::parse(&exe_bytes).unwrap();
        let bank = SpriteBank::new(
            &shuppatsu_formats::cg::CgFile::parse(&data.read("cg.rom").unwrap()).unwrap(),
            shuppatsu_formats::cg::parse_tbl(&data.read("tbl.rom").unwrap()).unwrap(),
        );
        let cab = shuppatsu_formats::cab_hud::CabTables::read(&exe).unwrap();
        let hud = crate::departure_hud::DepartureHud::new(&cab);
        // Akita – Ōmagari, all nine stations; Naoetsu, Kuroi, Saigata (the lesson).
        let n = |t| Diagram::of(&bank, t).map(|d| d.columns.len());
        assert_eq!(n(0x32a), Some(9));
        assert_eq!(n(0x31e), Some(3));
        let mut found = 0;
        for t in hud.diagram_tilemaps() {
            let Some(d) = Diagram::of(&bank, t) else {
                continue;
            };
            found += usize::from(!d.columns.is_empty());
            for c in &d.columns {
                assert!(c.left < c.right && c.right <= f32::from(d.width), "{t:#x}");
                assert!(d.top < c.bottom && c.bottom <= d.floor, "{t:#x}");
            }
        }
        assert!(found > 40, "{found} diagrams");
    }
}

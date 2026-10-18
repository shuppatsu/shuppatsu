//! Exe tables of the front end (docs/re/menus.md §2–§7), read from the user's `dengo3k.exe`.
//! Only addresses, counts and the tilemap ids that the code passes as immediates are
//! hardcoded; every descriptor, list and placement is read at runtime. Everything is taken for
//! resolution mode 1 (640×480), the mode the sprite layer is composed in.

use crate::screens::tables::{Dialog, Placement, TextDesc, dialog, placement_list};
use anyhow::{Context, Result, ensure};
use ddg_formats::exe::Exe;
use ddg_formats::hud::{MODE_640, RES_MODES, parse_slot_layout};

const WHAT: &str = "front-end tables";

/// Per-mode origin of screens laid out for 640×480 (`0x463d78` x, `0x463d88` y; menus.md §1.3).
const ORIGIN_X: u32 = 0x463d78;
const ORIGIN_Y: u32 = 0x463d88;

/// Menu header (0x42a820): background-bar tilemap per page (`0x47c50c`, i16) and the page title
/// cut from tbl 0x54d (`0x47c4c8`, 6-byte descriptors). Pages 0 (title), 1–6 (value pages),
/// 8 (options list) are used.
const HEADER_TBL: u32 = 0x47c50c;
const HEADER_TEXT: u32 = 0x47c4c8;
pub const HEADER_PAGES: usize = 11;
/// Legend descriptors of 0x42a820 (keyboard / pad / lever variants) and of the route select's
/// 0x436210. Each address is one 6-byte descriptor.
const TITLE_LEGENDS: Legends<u32> = Legends {
    word_keys: 0x47c524,
    word_pad: 0x47c52a,
    word_lever: 0x47c530,
    upper_keys: 0x47c558,
    upper_pad: 0x47c564,
    lower_keys: 0x47c55e,
    lower_pad: 0x47c56a,
    up_down: 0x47c538,
    upper_label: 0x47c53e,
    lower_label: 0x47c544,
};
const ROUTE_LEGENDS: Legends<u32> = Legends {
    word_keys: 0x47cab0,
    word_pad: 0x47cab6,
    word_lever: 0x47cabc,
    upper_keys: 0x47cae4,
    upper_pad: 0x47caf0,
    lower_keys: 0x47caea,
    lower_pad: 0x47caf6,
    up_down: 0x47cac4,
    upper_label: 0x47caca,
    lower_label: 0x47cad0,
};
/// Route select, train level only: the R-key legend (0x436210, slots 0x9c / 0x9d).
const ROUTE_R_LABEL: u32 = 0x47cadc;
const ROUTE_R_KEY: u32 = 0x47cb08;
/// Title items 1–5 on tbl 0x513 (`0x47c778`, 5 descriptors; there is none for item 6, the
/// Komachi movie).
const TITLE_ITEMS: u32 = 0x47c778;
pub const TITLE_ITEM_COUNT: usize = 5;
/// Options sheets: selected (tbl 0x4fa) and normal (tbl 0x4fb) colouring, 38 descriptors each.
const OPTION_SELECTED: u32 = 0x47c584;
const OPTION_NORMAL: u32 = 0x47c664;
const OPTION_TEXT_COUNT: usize = 38;
/// One pointer per option item (`0x48766c`): label index, value indices, −1.
const OPTION_LISTS: u32 = 0x48766c;
pub const OPTION_COUNT: usize = 6;
/// Controller pictures of the control-mode page per mode 0–5 (`0x487684`, i16; mode 6 draws two
/// fixed pictures, 0x42ad10).
const CONTROL_PICTURES: u32 = 0x487684;
pub const CONTROL_MODES: usize = 7;

/// Route select (0x435510): group boxes `{i32 header, i32 thumb[4]}` × 5 at `0x47cb10`, their
/// stages (6 × i32, −1 padded) at `0x47cb74`, and the stage table `0x4656ac` (21 × 32 bytes).
const GROUPS: u32 = 0x47cb10;
pub const GROUP_COUNT: usize = 5;
pub const GROUP_THUMBS: usize = 4;
const GROUP_STAGES: u32 = 0x47cb74;
pub const GROUP_STAGE_MAX: usize = 6;
const STAGE_TABLE: u32 = 0x4656ac;
pub const SELECT_STAGES: usize = 21;
/// Placements `{x, y, layer, palette, slot}` relative to a row's top (0x435510): four frame
/// pieces at `0x463d98`, the group header at `0x463dc0`, four thumbnails at `0x463e10`, and the
/// train rows (16 records, −1 terminated) at `0x463e6c`.
const GROUP_FRAME: u32 = 0x463d98;
const GROUP_HEADER: u32 = 0x463dc0;
const GROUP_THUMB_POS: u32 = 0x463e10;
const TRAIN_ROW: u32 = 0x463e6c;
const TRAIN_ROW_RECORDS: usize = 16;
/// Route-select page titles 路線選択 / 電車選択 (`0x48c584`, 2 descriptors on tbl 0x54d).
const ROUTE_TITLES: u32 = 0x48c584;

/// Lever check (0x4137e0): layout rows × 4 resolution modes at `0x463fb0`.
const LEVER_LAYOUT: u32 = 0x463fb0;
const LEVER_LAYOUT_ROWS: usize = 8;
/// Departure board (0x4139c0): placements at `0x46425c` (x0/y0-relative), the flap lists
/// (series `0x466fa0`, train type `0x466fd8`, stations `0x466ff8`, i32, −1 terminated) and the
/// per-route entries (`0x467040`, 20 × i32 per route, the first four used).
const BOARD_LAYOUT: u32 = 0x46425c;
const BOARD_SERIES: u32 = 0x466fa0;
const BOARD_TYPES: u32 = 0x466fd8;
const BOARD_STATIONS: u32 = 0x466ff8;
const BOARD_ROUTES: u32 = 0x467040;
const BOARD_ROUTE_STRIDE: u32 = 20 * 4;
pub const ROUTE_COUNT: usize = 20;
/// Calibration page titles (0x434d20, `0x48c478`, 2 descriptors).
const CALIBRATION_TITLES: u32 = 0x48c478;
/// Exit confirmation 終了しますか? (`Dialog_Run(0x47c848, 1)` in 0x42bb90).
const EXIT_DIALOG: u32 = 0x47c848;

/// The legend descriptors of one header style, by role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Legends<T> {
    /// Slot 0x88: the cursor-key / pad / lever picture.
    pub word_keys: T,
    pub word_pad: T,
    pub word_lever: T,
    /// Slot 0x8c: the upper key picture (Esc in the keyboard set), or the joystick-mode set.
    pub upper_keys: T,
    pub upper_pad: T,
    /// Slot 0x8e: the lower key picture (Enter).
    pub lower_keys: T,
    pub lower_pad: T,
    /// Slots 0x89 / 0x8b / 0x8d: 選択 / キャンセル / 決定 labels (palette 4).
    pub up_down: T,
    pub upper_label: T,
    pub lower_label: T,
}

/// One entry of the select-stage table 0x4656ac (menus.md §6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectStage {
    pub thumb: i16,
    pub caption: i16,
    pub from: i16,
    pub to: i16,
    pub train_type: i16,
    /// Difficulty 1–5 (stars, the rank badge, the stage kind of rules.md).
    pub difficulty: i32,
    /// Route index written to `0x8570a4`; 20 for the route-10 variant (select stage 20).
    pub route: i32,
}

#[derive(Debug, Clone)]
pub struct FrontTables {
    pub origin: (i16, i16),
    /// Tilemap heights in tiles (`(tbl, rows)`), for the legend pictures whose descriptor
    /// takes the whole map height (`h <= 0`); filled from the sprite bank by the app
    /// ([`FrontTables::picture_rows`]).
    pub map_rows: Vec<(i16, i8)>,
    pub header_tbl: [i16; HEADER_PAGES],
    pub header_text: [TextDesc; HEADER_PAGES],
    pub title_legends: Legends<TextDesc>,
    pub route_legends: Legends<TextDesc>,
    pub route_r_label: TextDesc,
    pub route_r_key: TextDesc,
    pub title_items: [TextDesc; TITLE_ITEM_COUNT],
    pub option_selected: Vec<TextDesc>,
    pub option_normal: Vec<TextDesc>,
    /// Per item: label index, then the value indices.
    pub option_lists: [Vec<i8>; OPTION_COUNT],
    pub control_pictures: [i16; CONTROL_MODES - 1],
    pub groups: [(i16, [i16; GROUP_THUMBS]); GROUP_COUNT],
    pub group_stages: [Vec<usize>; GROUP_COUNT],
    pub stages: [SelectStage; SELECT_STAGES],
    pub group_frame: Vec<Placement>,
    pub group_header: Placement,
    pub group_thumbs: Vec<Placement>,
    pub train_row: Vec<Placement>,
    pub route_titles: [TextDesc; 2],
    pub lever_layout: Vec<Placement>,
    pub board_layout: Vec<Placement>,
    pub board_series: Vec<i16>,
    pub board_types: Vec<i16>,
    pub board_stations: Vec<i16>,
    pub board_routes: [[i32; 4]; ROUTE_COUNT],
    pub calibration_titles: [TextDesc; 2],
    pub exit_dialog: Dialog,
}

pub fn desc(exe: &Exe<'_>, va: u32) -> Result<TextDesc> {
    let r = exe.reader_at(va, 6, WHAT)?;
    Ok(TextDesc {
        tbl: r.i16(0)?,
        col: r.i8(2)?,
        row: r.i8(3)?,
        w: r.i8(4)?,
        h: r.i8(5)?,
    })
}

fn descs<const N: usize>(exe: &Exe<'_>, va: u32) -> Result<[TextDesc; N]> {
    let mut out = [TextDesc::default(); N];
    for (i, d) in out.iter_mut().enumerate() {
        *d = desc(exe, va + 6 * i as u32)?;
    }
    Ok(out)
}

/// A −1 terminated i32 list of tilemap ids.
fn id_list(exe: &Exe<'_>, va: u32, max: usize) -> Result<Vec<i16>> {
    let r = exe.reader_at(va, 4 * max, WHAT)?;
    let mut out = Vec::new();
    for i in 0..max {
        let v = r.i32(4 * i)?;
        if v == -1 {
            return Ok(out);
        }
        out.push(i16::try_from(v)?);
    }
    anyhow::bail!("id list at {va:#x} has no terminator")
}

fn placement(exe: &Exe<'_>, va: u32) -> Result<Placement> {
    let r = exe.reader_at(va, 10, WHAT)?;
    Ok(Placement {
        x: r.i16(0)?,
        y: r.i16(2)?,
        layer: r.i16(4)?,
        palette: r.i16(6)?,
        slot: r.i16(8)?,
    })
}

impl<T: Copy> Legends<T> {
    fn map<U>(self, mut f: impl FnMut(T) -> Result<U>) -> Result<Legends<U>> {
        Ok(Legends {
            word_keys: f(self.word_keys)?,
            word_pad: f(self.word_pad)?,
            word_lever: f(self.word_lever)?,
            upper_keys: f(self.upper_keys)?,
            upper_pad: f(self.upper_pad)?,
            lower_keys: f(self.lower_keys)?,
            lower_pad: f(self.lower_pad)?,
            up_down: f(self.up_down)?,
            upper_label: f(self.upper_label)?,
            lower_label: f(self.lower_label)?,
        })
    }
}

impl FrontTables {
    /// The height in tiles of the picture `d` cuts: its own, or its map's.
    pub fn picture_rows(&self, d: TextDesc) -> i8 {
        if d.h > 0 {
            return d.h;
        }
        self.map_rows
            .iter()
            .find(|&&(t, _)| t == d.tbl)
            .map_or(2, |&(_, r)| r - d.row)
    }

    /// The legend pictures' tilemaps (title, options and route-select headers).
    pub fn legend_maps(&self) -> Vec<i16> {
        let l = |g: &Legends<TextDesc>| [g.word_keys.tbl, g.upper_keys.tbl, g.lower_keys.tbl];
        let mut v: Vec<i16> = l(&self.title_legends)
            .into_iter()
            .chain(l(&self.route_legends))
            .chain([self.route_r_key.tbl])
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// The sixth title label, LOVE特急こまち (the Komachi movie). The exe has no descriptor for
    /// it (0x47c778 holds five), but it is on the sheet right under the fifth, in the same
    /// 32×6-tile cell: rows 0, 6, 12, 18, 24, then 30 (tbl 0x513 is 36 rows high).
    pub fn movie_item(&self) -> TextDesc {
        let last = self.title_items[TITLE_ITEM_COUNT - 1];
        TextDesc {
            row: last.row + last.h,
            ..last
        }
    }

    pub fn read(exe: &Exe<'_>) -> Result<Self> {
        let mode_i16 = |va: u32| -> Result<i16> {
            let r = exe.reader_at(va, 4 * RES_MODES, WHAT)?;
            Ok(i16::try_from(r.i32(4 * MODE_640)?)?)
        };
        let header_tbl = {
            let r = exe.reader_at(HEADER_TBL, 2 * HEADER_PAGES, WHAT)?;
            let mut out = [0i16; HEADER_PAGES];
            for (i, v) in out.iter_mut().enumerate() {
                *v = r.i16(2 * i)?;
            }
            out
        };
        let option_text = |va: u32| -> Result<Vec<TextDesc>> {
            (0..OPTION_TEXT_COUNT)
                .map(|i| desc(exe, va + 6 * i as u32))
                .collect()
        };
        let mut option_lists: [Vec<i8>; OPTION_COUNT] = Default::default();
        for (i, list) in option_lists.iter_mut().enumerate() {
            let ptr = exe.u32_at(OPTION_LISTS + 4 * i as u32)?;
            let bytes = exe.bytes_at(ptr, 16).context("option list")?;
            *list = bytes
                .iter()
                .map(|&b| b as i8)
                .take_while(|&b| b != -1)
                .collect();
            ensure!(
                list.len() >= 2
                    && list
                        .iter()
                        .all(|&v| (0..OPTION_TEXT_COUNT as i8).contains(&v)),
                "option list {i} at {ptr:#x}: {list:?}"
            );
        }
        let control_pictures = {
            let r = exe.reader_at(CONTROL_PICTURES, 2 * (CONTROL_MODES - 1), WHAT)?;
            let mut out = [0i16; CONTROL_MODES - 1];
            for (i, v) in out.iter_mut().enumerate() {
                *v = r.i16(2 * i)?;
            }
            out
        };
        let mut groups = [(0i16, [0i16; GROUP_THUMBS]); GROUP_COUNT];
        let mut group_stages: [Vec<usize>; GROUP_COUNT] = Default::default();
        {
            let g = exe.reader_at(GROUPS, 20 * GROUP_COUNT, WHAT)?;
            let s = exe.reader_at(GROUP_STAGES, 4 * GROUP_STAGE_MAX * GROUP_COUNT, WHAT)?;
            for i in 0..GROUP_COUNT {
                groups[i].0 = i16::try_from(g.i32(20 * i)?)?;
                for k in 0..GROUP_THUMBS {
                    groups[i].1[k] = i16::try_from(g.i32(20 * i + 4 + 4 * k)?)?;
                }
                // 0x435510 counts entries until −1, at most six.
                for k in 0..GROUP_STAGE_MAX {
                    let v = s.i32(4 * (GROUP_STAGE_MAX * i + k))?;
                    if v == -1 {
                        break;
                    }
                    let st = usize::try_from(v)?;
                    ensure!(st < SELECT_STAGES, "group {i} stage {st}");
                    group_stages[i].push(st);
                }
            }
        }
        let mut stages = [SelectStage {
            thumb: 0,
            caption: 0,
            from: 0,
            to: 0,
            train_type: 0,
            difficulty: 0,
            route: 0,
        }; SELECT_STAGES];
        {
            let r = exe.reader_at(STAGE_TABLE, 32 * SELECT_STAGES, WHAT)?;
            for (i, st) in stages.iter_mut().enumerate() {
                let f = |k: usize| r.i32(32 * i + 4 * k);
                *st = SelectStage {
                    thumb: i16::try_from(f(0)?)?,
                    caption: i16::try_from(f(1)?)?,
                    from: i16::try_from(f(2)?)?,
                    to: i16::try_from(f(3)?)?,
                    train_type: i16::try_from(f(4)?)?,
                    difficulty: f(6)?,
                    route: f(7)?,
                };
                ensure!(
                    (1..=5).contains(&st.difficulty) && (0..=20).contains(&st.route),
                    "select stage {i}: {st:?}"
                );
            }
        }
        let records = |va: u32, n: usize| -> Result<Vec<Placement>> {
            (0..n).map(|i| placement(exe, va + 10 * i as u32)).collect()
        };
        let lever_layout = parse_slot_layout(exe.bytes_at(LEVER_LAYOUT, 40 * LEVER_LAYOUT_ROWS)?)?
            .iter()
            .map(|r| r.at_640())
            .collect();
        let mut board_routes = [[0i32; 4]; ROUTE_COUNT];
        for (route, row) in board_routes.iter_mut().enumerate() {
            let r = exe.reader_at(BOARD_ROUTES + BOARD_ROUTE_STRIDE * route as u32, 16, WHAT)?;
            for (k, v) in row.iter_mut().enumerate() {
                *v = r.i32(4 * k)?;
            }
        }
        let t = Self {
            origin: (mode_i16(ORIGIN_X)?, mode_i16(ORIGIN_Y)?),
            map_rows: Vec::new(),
            header_tbl,
            header_text: descs(exe, HEADER_TEXT)?,
            title_legends: TITLE_LEGENDS.map(|va| desc(exe, va))?,
            route_legends: ROUTE_LEGENDS.map(|va| desc(exe, va))?,
            route_r_label: desc(exe, ROUTE_R_LABEL)?,
            route_r_key: desc(exe, ROUTE_R_KEY)?,
            title_items: descs(exe, TITLE_ITEMS)?,
            option_selected: option_text(OPTION_SELECTED)?,
            option_normal: option_text(OPTION_NORMAL)?,
            option_lists,
            control_pictures,
            groups,
            group_stages,
            stages,
            group_frame: records(GROUP_FRAME, 4)?,
            group_header: placement(exe, GROUP_HEADER)?,
            group_thumbs: records(GROUP_THUMB_POS, GROUP_THUMBS)?,
            train_row: records(TRAIN_ROW, TRAIN_ROW_RECORDS)?,
            route_titles: descs(exe, ROUTE_TITLES)?,
            lever_layout,
            board_layout: placement_list(exe, BOARD_LAYOUT, 8)?,
            board_series: id_list(exe, BOARD_SERIES, 32)?,
            board_types: id_list(exe, BOARD_TYPES, 32)?,
            board_stations: id_list(exe, BOARD_STATIONS, 32)?,
            board_routes,
            calibration_titles: descs(exe, CALIBRATION_TITLES)?,
            exit_dialog: dialog(exe, EXIT_DIALOG)?,
        };
        ensure!(
            t.board_layout.len() == 5,
            "departure-board layout has {} records",
            t.board_layout.len()
        );
        ensure!(
            !t.board_series.is_empty() && !t.board_types.is_empty() && t.board_stations.len() > 2,
            "departure-board lists"
        );
        Ok(t)
    }

    /// The value indices of option item `i` (its list without the label).
    pub fn option_values(&self, item: usize) -> &[i8] {
        self.option_lists.get(item).map_or(&[], |l| &l[1..])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_front_tables() {
        let Ok(data) = crate::scene::GameData::from_env() else {
            eprintln!("DDG_DATA_DIR unset; skipping");
            return;
        };
        let bytes = data.exe().unwrap();
        let exe = Exe::parse(&bytes).unwrap();
        let t = FrontTables::read(&exe).unwrap();
        assert_eq!(t.origin, (0, 0));
        // menus.md §3: five title items on tbl 0x513, rows 0, 6, 12, 18, 24.
        assert!(t.title_items.iter().all(|d| d.tbl == 0x513));
        assert_eq!(
            t.title_items.map(|d| d.row),
            [0, 6, 12, 18, 24],
            "title rows"
        );
        let m = t.movie_item();
        assert_eq!(
            (m.tbl, m.col, m.row, m.w, m.h),
            (0x513, 0, 30, -1, 6),
            "the movie label"
        );
        // §4: option values.
        assert_eq!(t.option_values(1), &[6, 7, 8, 9]);
        assert_eq!(t.option_values(3), &[14, 15]);
        assert_eq!(t.option_values(4), &[16, 7, 19]);
        assert_eq!(t.option_values(0).len(), 7);
        // §6.1: groups and their stages.
        assert_eq!(t.group_stages[0], [5, 6, 8, 9, 7]);
        assert_eq!(t.group_stages[1], [0, 3, 17, 18, 4, 1]);
        assert_eq!(t.group_stages[2], [11, 10, 20, 16]);
        assert_eq!(t.group_stages[3], [13, 15, 12, 14]);
        assert_eq!(t.group_stages[4], [2, 2, 2, 2]);
        assert_eq!(t.groups[0].0, 0x126);
        assert_eq!(t.stages[20].route, 20);
        assert_eq!(t.stages[12].route, 12);
        assert_eq!(t.train_row.len(), TRAIN_ROW_RECORDS);
        assert_eq!(t.train_row[15].slot, 28);
        assert_eq!(t.group_header.slot, 13);
        assert_eq!(t.board_layout[4].slot, 58);
        assert_eq!(t.board_series.len(), 13);
        assert_eq!(t.board_types.len(), 5);
        assert_eq!(t.board_stations.len(), 15);
        assert_eq!(t.lever_layout.len(), 7);
        assert_eq!(t.exit_dialog.frame, 0x542);
        assert_eq!(t.exit_dialog.items.len(), 2);
    }
}

//! Exe tables of the stage-flow screens (docs/re/menus.md §8–§11), read from the user's
//! `dengo3k.exe`. Only addresses and counts are hardcoded; every position, tilemap id and dialog
//! layout is read at runtime. Everything is taken for resolution mode 1 (640×480), the mode the
//! sprite layer is composed in.

use anyhow::{Context, Result, ensure};
use ddg_formats::exe::Exe;
use ddg_formats::hud::{MODE_640, RES_MODES, SlotPlacement, parse_slot_layout};

const WHAT: &str = "screen tables";

/// Screen width / height per resolution mode (`0x48c1e0` / `0x48c1f0`, read by 0x42b930).
const MODE_WIDTHS: u32 = 0x48c1e0;
const MODE_HEIGHTS: u32 = 0x48c1f0;
/// Per-mode origin of screens laid out for 640×480 (`0x463d78` x, `0x463d88` y; menus.md §1.3).
const ORIGIN_X: u32 = 0x463d78;
const ORIGIN_Y: u32 = 0x463d88;
/// Station-result layout rows (0x413f30) and the lesson-stage variant: 4 modes × 10 bytes.
const RESULT_LAYOUT: u32 = 0x464298;
const LESSON_RESULT_LAYOUT: u32 = 0x4646a8;
/// Both result layouts are 0x410 bytes apart from the next table (0x4646a8, 0x464ab8): at most
/// 26 rows including the terminator.
const RESULT_LAYOUT_MAX_ROWS: usize = 26;
/// Report-card placements `{x, y, layer, palette, slot}` × 1 (0x4149d0), x = −1 terminated.
const REPORT_LAYOUT: u32 = 0x464ab8;
const REPORT_LAYOUT_MAX: usize = 48;
/// Remaining-time digits 0–9 of the HUD (`0x4653d4`, u32 each).
const TIME_DIGITS: u32 = 0x4653d4;
/// Result / report digits 0–9 (`0x465458`, u32 each).
const RESULT_DIGITS: u32 = 0x465458;
/// Overrun banners by overrun factor 0–3 (`0x4654ec`, u32 each; 1–3 = "m×N秒").
const OVERRUN_BANNERS: u32 = 0x4654ec;
/// "Press START" / continue verdict position (0x415ba0) and the run-end message (0x415f00).
const PROMPT_X: u32 = 0x466ee8;
const PROMPT_Y: u32 = 0x466ef8;
/// "Continue chosen" banner position (0x415ba0).
const CHOSEN_X: u32 = 0x466f48;
const CHOSEN_Y: u32 = 0x466f58;
/// Conductor picture of the cleared banner (0x415f00).
const GIRL_X: u32 = 0x467680;
const GIRL_Y: u32 = 0x467690;
/// Pause dialogs (0x42b7f0): main, quit, return to the first station.
const DIALOG_MAIN: u32 = 0x47c810;
const DIALOG_QUIT: u32 = 0x47c820;
const DIALOG_RETRY: u32 = 0x47c830;
/// Text descriptors on the dialog sheet tbl 0x544 (`0x48bbb0`, 6 bytes each, 29 entries).
const DIALOG_TEXT: u32 = 0x48bbb0;
const DIALOG_TEXT_COUNT: usize = 29;

/// A label cut out of a string sheet: `{u16 tbl, i8 col, i8 row, i8 w, i8 h}` in 16-px tiles
/// (menus.md §1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextDesc {
    pub tbl: i16,
    pub col: i8,
    pub row: i8,
    pub w: i8,
    pub h: i8,
}

/// A list dialog (`Dialog_Run` 0x42b930 descriptor, menus.md §8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dialog {
    /// Frame tilemap, centred on the screen.
    pub frame: i16,
    /// Title text index into [`ScreenTables::dialog_text`] and its y below the frame top.
    pub title: i8,
    pub title_y: i8,
    /// Star cursor x from the frame's left edge.
    pub cursor_x: i8,
    pub items_y: i8,
    pub spacing: i8,
    /// Item text indices (the highlighted variant is the next index).
    pub items: Vec<i8>,
}

/// A 640×480 placement `{x, y, layer, palette, slot}` (0x431850 arguments).
pub type Placement = SlotPlacement;

#[derive(Debug, Clone)]
pub struct ScreenTables {
    pub screen: (i16, i16),
    pub origin: (i16, i16),
    pub result_layout: Vec<Placement>,
    pub lesson_result_layout: Vec<Placement>,
    pub report_layout: Vec<Placement>,
    pub time_digits: [i16; 10],
    pub result_digits: [i16; 10],
    pub overrun_banners: [i16; 4],
    pub prompt_pos: (i16, i16),
    pub chosen_pos: (i16, i16),
    pub girl_pos: (i16, i16),
    pub dialog_main: Dialog,
    pub dialog_quit: Dialog,
    pub dialog_retry: Dialog,
    pub dialog_text: Vec<TextDesc>,
    pub ranking: super::ranking::RankingTables,
}

impl ScreenTables {
    pub fn read(exe: &Exe<'_>) -> Result<Self> {
        let mode_i32 = |va: u32| -> Result<i16> {
            let r = exe.reader_at(va, 4 * RES_MODES, WHAT)?;
            Ok(i16::try_from(r.i32(4 * MODE_640)?)?)
        };
        let ids = |va: u32| -> Result<[i16; 10]> {
            let r = exe.reader_at(va, 40, WHAT)?;
            let mut out = [0i16; 10];
            for (i, v) in out.iter_mut().enumerate() {
                *v = i16::try_from(r.i32(4 * i)?)?;
            }
            Ok(out)
        };
        let result_layout = |va: u32| -> Result<Vec<Placement>> {
            let rows = parse_slot_layout(exe.bytes_at(va, 40 * RESULT_LAYOUT_MAX_ROWS)?)?;
            Ok(rows.iter().map(|r| r.at_640()).collect())
        };
        let banners = exe.reader_at(OVERRUN_BANNERS, 16, WHAT)?;
        let mut overrun_banners = [0i16; 4];
        for (i, v) in overrun_banners.iter_mut().enumerate() {
            *v = i16::try_from(banners.i32(4 * i)?)?;
        }
        let dialog_text = {
            let r = exe.reader_at(DIALOG_TEXT, 6 * DIALOG_TEXT_COUNT, WHAT)?;
            (0..DIALOG_TEXT_COUNT)
                .map(|i| {
                    Ok(TextDesc {
                        tbl: r.i16(6 * i)?,
                        col: r.i8(6 * i + 2)?,
                        row: r.i8(6 * i + 3)?,
                        w: r.i8(6 * i + 4)?,
                        h: r.i8(6 * i + 5)?,
                    })
                })
                .collect::<Result<Vec<_>>>()?
        };
        let t = Self {
            screen: (mode_i32(MODE_WIDTHS)?, mode_i32(MODE_HEIGHTS)?),
            origin: (mode_i32(ORIGIN_X)?, mode_i32(ORIGIN_Y)?),
            result_layout: result_layout(RESULT_LAYOUT)?,
            lesson_result_layout: result_layout(LESSON_RESULT_LAYOUT)?,
            report_layout: report_layout(exe)?,
            time_digits: ids(TIME_DIGITS)?,
            result_digits: ids(RESULT_DIGITS)?,
            overrun_banners,
            prompt_pos: (mode_i32(PROMPT_X)?, mode_i32(PROMPT_Y)?),
            chosen_pos: (mode_i32(CHOSEN_X)?, mode_i32(CHOSEN_Y)?),
            girl_pos: (mode_i32(GIRL_X)?, mode_i32(GIRL_Y)?),
            dialog_main: dialog(exe, DIALOG_MAIN)?,
            dialog_quit: dialog(exe, DIALOG_QUIT)?,
            dialog_retry: dialog(exe, DIALOG_RETRY)?,
            dialog_text,
            ranking: super::ranking::RankingTables::read(exe)?,
        };
        for d in [&t.dialog_main, &t.dialog_quit, &t.dialog_retry] {
            let max = d.items.iter().copied().max().unwrap_or(0);
            ensure!(
                (d.title as usize) < DIALOG_TEXT_COUNT && (max as usize) + 1 < DIALOG_TEXT_COUNT,
                "dialog text index out of range"
            );
        }
        ensure!(
            t.report_layout.len() >= REPORT_SLOTS,
            "report-card layout has {} rows, expected {REPORT_SLOTS}",
            t.report_layout.len()
        );
        Ok(t)
    }

    #[cfg(test)]
    /// The three total-digit placements (records 30–32) and the background (record 33).
    pub fn report_total(&self) -> &[Placement] {
        &self.report_layout[REPORT_TOTAL..REPORT_TOTAL + 3]
    }

    #[cfg(test)]
    pub fn report_background(&self) -> &Placement {
        &self.report_layout[REPORT_TOTAL + 3]
    }
}

/// Report layout records: five rows of six, three total digits, the background.
const REPORT_ROW_STRIDE: usize = 6;
const REPORT_TOTAL: usize = 5 * REPORT_ROW_STRIDE;
const REPORT_SLOTS: usize = REPORT_TOTAL + 4;

fn report_layout(exe: &Exe<'_>) -> Result<Vec<Placement>> {
    placement_list(exe, REPORT_LAYOUT, REPORT_LAYOUT_MAX)
}

/// A list of single-mode placements `{x, y, layer, palette, slot}` (10 bytes each), x = −1
/// terminated, at most `max` records.
pub(crate) fn placement_list(exe: &Exe<'_>, va: u32, max: usize) -> Result<Vec<Placement>> {
    let r = exe.reader_at(va, 10 * max, WHAT)?;
    let mut out = Vec::new();
    for i in 0..max {
        let o = 10 * i;
        let x = r.i16(o)?;
        if x == -1 {
            return Ok(out);
        }
        out.push(SlotPlacement {
            x,
            y: r.i16(o + 2)?,
            layer: r.i16(o + 4)?,
            palette: r.i16(o + 6)?,
            slot: r.i16(o + 8)?,
        });
    }
    anyhow::bail!("placement list at {va:#x} has no terminator")
}

fn dialog(exe: &Exe<'_>, va: u32) -> Result<Dialog> {
    let r = exe.reader_at(va, 12, WHAT)?;
    let count = r.i8(7)?;
    ensure!((1..=8).contains(&count), "dialog at {va:#x}: {count} items");
    let list = r.u32(8)?;
    let items = exe
        .bytes_at(list, count as usize)
        .with_context(|| format!("dialog items at {list:#x}"))?
        .iter()
        .map(|&b| b as i8)
        .collect();
    Ok(Dialog {
        frame: r.i16(0)?,
        title: r.i8(2)?,
        title_y: r.i8(3)?,
        cursor_x: r.i8(4)?,
        items_y: r.i8(5)?,
        spacing: r.i8(6)?,
        items,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_screen_tables() {
        let Ok(data) = crate::scene::GameData::from_env() else {
            eprintln!("DDG_DATA_DIR unset; skipping");
            return;
        };
        let bytes = data.exe().unwrap();
        let exe = Exe::parse(&bytes).unwrap();
        let t = ScreenTables::read(&exe).unwrap();
        eprintln!("{t:#?}");
        assert_eq!(t.screen, (640, 480));
        assert_eq!(t.origin, (0, 0));
        // menus.md §9: overrun banners m×1秒 / m×2秒 / m×3秒.
        assert_eq!(&t.overrun_banners[1..], &[0x1a3, 0x1a4, 0x1a5]);
        assert_eq!(t.dialog_main.frame, 0x541);
        assert_eq!(t.dialog_main.items.len(), 3);
        assert_eq!(t.dialog_quit.items.len(), 2);
        assert_eq!(t.report_background().slot, 43);
        assert_eq!(
            t.report_total().iter().map(|p| p.slot).collect::<Vec<_>>(),
            [40, 41, 42]
        );
    }
}

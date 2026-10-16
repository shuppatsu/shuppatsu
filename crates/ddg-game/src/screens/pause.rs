//! Pause dialogs (`PauseMenu_Run` 0x42b7f0, `Dialog_Run` 0x42b930; menus.md §8).
//!
//! One [`PauseMenu::tick`] is one `Task_Tick` of the original's blocking loops, all with the
//! simulation frozen (`0x855ffc & 4`). A dialog's first tick only places the frame and title;
//! from the next one it reads the button edges and draws the items and the star cursor. Pause
//! (Esc) inside a dialog `longjmp`s straight out (resume). After the last dialog closes, three
//! more frozen ticks run (0x425490(3)) before the stage loop goes on.

use super::tables::{Dialog, ScreenTables, TextDesc};
use super::{MenuInput, Sfx, SlotSet};

/// Dialog slots (0x42b930): star cursor, title, items from 0x89, frame.
const SLOT_CURSOR: usize = 0x87;
const SLOT_TITLE: usize = 0x88;
const SLOT_ITEMS: usize = 0x89;
const SLOT_FRAME: usize = 0x90;
const PALETTE: u8 = 4;
/// Layer 0: on top of everything.
const LAYER: u8 = 0;
/// Every dialog move, confirm and cancel (sent straight to the sound thread, 0x430100).
const SND_DIALOG: u16 = 0x26;
/// Frozen ticks after the dialogs close (0x42b7f0: 0x425490(3)).
const TAIL_TICKS: u32 = 3;
/// Text index of the star cursor on the dialog sheet.
const TEXT_STAR: usize = 0;

/// What the pause ended with (0x42b7f0's return value).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseResult {
    /// 0: resume driving.
    Resume,
    /// 1: 運転中止 + はい: back to the title (`0x855ffc |= 0x10`).
    Quit,
    /// 2: 始発駅に戻る + はい: restart the route from station 0 without the departure board.
    Retry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    Main,
    Quit,
    Retry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// The dialog was just opened: its first tick places frame and title.
    Opening,
    Open,
    /// Frozen ticks left after the close, then the result.
    Closing(u32, PauseResult),
}

/// A dialog descriptor with its frame's pixel size (the cached map size at 0x850aa8).
#[derive(Debug, Clone)]
pub struct DialogDef {
    pub dialog: Dialog,
    pub frame_size: (i16, i16),
}

/// The dialogs and texts the pause menu needs.
#[derive(Debug, Clone)]
pub struct PauseDefs {
    pub main: DialogDef,
    pub quit: DialogDef,
    pub retry: DialogDef,
    pub text: Vec<TextDesc>,
    pub screen: (i16, i16),
}

impl PauseDefs {
    /// `map_size(tbl)` gives a tilemap's size in pixels.
    pub fn new(t: &ScreenTables, map_size: impl Fn(i16) -> (i16, i16)) -> Self {
        let def = |d: &Dialog| DialogDef {
            dialog: d.clone(),
            frame_size: map_size(d.frame),
        };
        Self {
            main: def(&t.dialog_main),
            quit: def(&t.dialog_quit),
            retry: def(&t.dialog_retry),
            text: t.dialog_text.clone(),
            screen: t.screen,
        }
    }
}

pub struct PauseMenu {
    defs: PauseDefs,
    which: Which,
    cursor: i32,
    state: State,
    slots: SlotSet,
}

impl PauseMenu {
    /// Opens the main dialog with the cursor on 運転再開 (0x42b930(0x47c810, 0)).
    pub fn new(defs: PauseDefs) -> Self {
        Self {
            defs,
            which: Which::Main,
            cursor: 0,
            state: State::Opening,
            slots: SlotSet::default(),
        }
    }

    fn def(&self) -> &DialogDef {
        match self.which {
            Which::Main => &self.defs.main,
            Which::Quit => &self.defs.quit,
            Which::Retry => &self.defs.retry,
        }
    }

    /// Top-left of the centred frame.
    fn frame_origin(&self) -> (i16, i16) {
        let (w, h) = self.def().frame_size;
        ((self.defs.screen.0 - w) / 2, (self.defs.screen.1 - h) / 2)
    }

    fn text(&self, i: i32) -> TextDesc {
        usize::try_from(i)
            .ok()
            .and_then(|i| self.defs.text.get(i))
            .copied()
            .unwrap_or_default()
    }

    fn open(&mut self, which: Which, cursor: i32) {
        self.which = which;
        self.cursor = cursor;
        self.state = State::Opening;
        self.slots.clear();
    }

    fn close(&mut self, result: PauseResult) {
        // 0x42b910 clears the dialog; this tick is the first of the three frozen ones.
        self.slots.clear();
        self.state = State::Closing(TAIL_TICKS - 1, result);
    }

    /// One frozen tick. Returns the result on the last tick of the pause; the next tick is a
    /// normal simulation tick.
    pub fn tick(&mut self, input: &MenuInput, sfx: &mut Vec<Sfx>) -> Option<PauseResult> {
        match self.state {
            State::Closing(0, r) => return Some(r),
            State::Closing(n, r) => {
                self.state = State::Closing(n - 1, r);
                return if n == 1 { Some(r) } else { None };
            }
            State::Opening => {
                self.place_frame();
                self.state = State::Open;
                return None;
            }
            State::Open => {}
        }
        if input.pause {
            // longjmp out of the dialog loop: resume, no sound.
            self.close(PauseResult::Resume);
            return None;
        }
        if input.confirm || input.cancel {
            sfx.push(Sfx::Play(SND_DIALOG));
            let choice = if input.confirm { self.cursor } else { -1 };
            self.choose(choice);
            return None;
        }
        let last = self.def().dialog.items.len() as i32 - 1;
        if let Some(notch) = input.notch {
            // Analog levers: one step per tick toward the power notch (0x42b930).
            let notch = i32::from(notch);
            if self.cursor >= 1 && self.cursor > notch {
                sfx.push(Sfx::Play(SND_DIALOG));
                self.cursor -= 1;
            } else if self.cursor < last && self.cursor < notch {
                sfx.push(Sfx::Play(SND_DIALOG));
                self.cursor += 1;
            }
            self.cursor = self.cursor.clamp(0, last.max(0));
        } else if input.up && self.cursor >= 1 {
            sfx.push(Sfx::Play(SND_DIALOG));
            self.cursor -= 1;
        } else if input.down && self.cursor < last {
            sfx.push(Sfx::Play(SND_DIALOG));
            self.cursor += 1;
        }
        self.place_items();
        None
    }

    /// The dispatch of 0x42b7f0 on a dialog's return value (−1 = cancel).
    fn choose(&mut self, choice: i32) {
        match (self.which, choice) {
            (Which::Main, 1) => self.open(Which::Quit, 1),
            (Which::Main, 2) => self.open(Which::Retry, 1),
            (Which::Main, _) => self.close(PauseResult::Resume),
            (Which::Quit, 0) => self.close(PauseResult::Quit),
            (Which::Retry, 0) => self.close(PauseResult::Retry),
            (_, _) => self.open(Which::Main, 0),
        }
        if self.state == State::Opening {
            // The next dialog's first Task_Tick is this tick.
            self.place_frame();
            self.state = State::Open;
        }
    }

    fn place_frame(&mut self) {
        let (fx, fy) = self.frame_origin();
        let d = self.def().dialog.clone();
        self.slots.full(SLOT_FRAME, d.frame, fx, fy, PALETTE, LAYER);
        let title = self.text(i32::from(d.title));
        let x = (self.defs.screen.0 - i16::from(title.w) * 16) / 2;
        self.slots.text(
            SLOT_TITLE,
            title,
            x,
            fy + i16::from(d.title_y),
            PALETTE,
            LAYER,
        );
    }

    fn place_items(&mut self) {
        let (fx, fy) = self.frame_origin();
        let d = self.def().dialog.clone();
        let first = self.text(i32::from(d.items.first().copied().unwrap_or(0)));
        let x = (self.defs.screen.0 - i16::from(first.w) * 16) / 2;
        for (i, &item) in d.items.iter().enumerate() {
            let highlighted = i as i32 == self.cursor;
            let text = self.text(i32::from(item) + i32::from(highlighted));
            let y = i16::from(d.spacing) * i as i16 + i16::from(d.items_y) + fy;
            self.slots.text(SLOT_ITEMS + i, text, x, y, PALETTE, LAYER);
        }
        let star = self.text(TEXT_STAR as i32);
        let y = i16::from(d.spacing) * self.cursor as i16 + i16::from(d.items_y) + fy;
        self.slots.text(
            SLOT_CURSOR,
            star,
            i16::from(d.cursor_x) + fx,
            y,
            PALETTE,
            LAYER,
        );
    }

    pub fn slots(&self) -> &SlotSet {
        &self.slots
    }

    #[cfg(test)]
    pub fn cursor(&self) -> i32 {
        self.cursor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dialog(frame: i16, title: i8, items: Vec<i8>) -> DialogDef {
        DialogDef {
            dialog: Dialog {
                frame,
                title,
                title_y: 16,
                cursor_x: 24,
                items_y: 64,
                spacing: 40,
                items,
            },
            frame_size: (320, 192),
        }
    }

    fn defs() -> PauseDefs {
        PauseDefs {
            main: dialog(0x541, 1, vec![3, 5, 7]),
            quit: dialog(0x542, 9, vec![11, 13]),
            retry: dialog(0x542, 15, vec![11, 13]),
            text: (0..29)
                .map(|i| TextDesc {
                    tbl: 0x544,
                    col: 0,
                    row: i,
                    w: 8,
                    h: 2,
                })
                .collect(),
            screen: (640, 480),
        }
    }

    const NONE: MenuInput = MenuInput::NONE;

    fn press(f: impl FnOnce(&mut MenuInput)) -> MenuInput {
        let mut i = NONE;
        f(&mut i);
        i
    }

    /// Ticks until the pause returns, counting the returning tick.
    fn run_out(p: &mut PauseMenu) -> (u32, PauseResult) {
        let mut sfx = Vec::new();
        for n in 1..10 {
            if let Some(r) = p.tick(&NONE, &mut sfx) {
                return (n, r);
            }
        }
        panic!("pause did not end");
    }

    #[test]
    fn first_tick_places_frame_only() {
        let mut p = PauseMenu::new(defs());
        let mut sfx = Vec::new();
        // Input on the opening tick is not read.
        assert_eq!(p.tick(&press(|i| i.confirm = true), &mut sfx), None);
        assert!(sfx.is_empty());
        assert_eq!(
            p.slots().get(SLOT_FRAME).map(|s| (s.x, s.y)),
            Some((160, 144))
        );
        assert!(p.slots().get(SLOT_ITEMS).is_none());
        p.tick(&NONE, &mut sfx);
        // Item 0 highlighted (index + 1), star at the first row.
        assert_eq!(p.slots().get(SLOT_ITEMS).map(|s| s.row), Some(4));
        assert_eq!(p.slots().get(SLOT_ITEMS + 1).map(|s| s.row), Some(5));
        assert_eq!(
            p.slots().get(SLOT_CURSOR).map(|s| (s.x, s.y)),
            Some((184, 208))
        );
    }

    #[test]
    fn cursor_clamps_and_beeps() {
        let mut p = PauseMenu::new(defs());
        let mut sfx = Vec::new();
        p.tick(&NONE, &mut sfx);
        p.tick(&press(|i| i.up = true), &mut sfx);
        assert_eq!((p.cursor(), sfx.len()), (0, 0));
        for _ in 0..4 {
            p.tick(&press(|i| i.down = true), &mut sfx);
        }
        assert_eq!((p.cursor(), sfx.len()), (2, 2));
        // Up wins over down.
        p.tick(
            &press(|i| {
                i.up = true;
                i.down = true
            }),
            &mut sfx,
        );
        assert_eq!(p.cursor(), 1);
    }

    #[test]
    fn resume_takes_three_frozen_ticks() {
        let mut p = PauseMenu::new(defs());
        let mut sfx = Vec::new();
        p.tick(&NONE, &mut sfx);
        assert_eq!(p.tick(&press(|i| i.confirm = true), &mut sfx), None);
        assert_eq!(sfx, [Sfx::Play(SND_DIALOG)]);
        assert!(p.slots().get(SLOT_FRAME).is_none());
        assert_eq!(run_out(&mut p), (2, PauseResult::Resume));
    }

    #[test]
    fn pause_key_resumes_silently_and_cancel_resumes() {
        let mut p = PauseMenu::new(defs());
        let mut sfx = Vec::new();
        p.tick(&NONE, &mut sfx);
        // Esc gives pause and cancel together; pause is checked first.
        p.tick(
            &press(|i| {
                i.pause = true;
                i.cancel = true
            }),
            &mut sfx,
        );
        assert!(sfx.is_empty());
        assert_eq!(run_out(&mut p).1, PauseResult::Resume);

        let mut p = PauseMenu::new(defs());
        p.tick(&NONE, &mut sfx);
        p.tick(&press(|i| i.cancel = true), &mut sfx);
        assert_eq!(run_out(&mut p).1, PauseResult::Resume);
    }

    #[test]
    fn quit_and_retry_need_yes() {
        let mut sfx = Vec::new();
        let down = press(|i| i.down = true);
        let ok = press(|i| i.confirm = true);
        // Quit, default いいえ → back to the main dialog with the cursor reset.
        let mut p = PauseMenu::new(defs());
        p.tick(&NONE, &mut sfx);
        p.tick(&down, &mut sfx);
        p.tick(&ok, &mut sfx);
        assert_eq!((p.which, p.cursor()), (Which::Quit, 1));
        assert_eq!(p.slots().get(SLOT_FRAME).map(|s| s.tilemap), Some(0x542));
        p.tick(&NONE, &mut sfx);
        p.tick(&ok, &mut sfx);
        assert_eq!((p.which, p.cursor()), (Which::Main, 0));
        // Quit + はい.
        p.tick(&down, &mut sfx);
        p.tick(&ok, &mut sfx);
        p.tick(&press(|i| i.up = true), &mut sfx);
        p.tick(&ok, &mut sfx);
        assert_eq!(run_out(&mut p).1, PauseResult::Quit);
        // Retry + はい.
        let mut p = PauseMenu::new(defs());
        p.tick(&NONE, &mut sfx);
        p.tick(&down, &mut sfx);
        p.tick(&down, &mut sfx);
        p.tick(&ok, &mut sfx);
        assert_eq!(p.which, Which::Retry);
        p.tick(&press(|i| i.up = true), &mut sfx);
        p.tick(&ok, &mut sfx);
        assert_eq!(run_out(&mut p).1, PauseResult::Retry);
    }
}

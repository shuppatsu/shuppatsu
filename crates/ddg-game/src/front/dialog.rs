//! 終了しますか?: the exit confirmation of the title menu (`ExitConfirm_Run` 0x42bb90 around
//! `Dialog_Run(0x47c848, 1)` 0x42b930, menus.md §3).
//!
//! The dialog clears slots 0x87..0x90 (the menu header), pauses the sound (0x4301e0(1)) and
//! freezes everything but the render list (`0x855ffc |= 4`). Its first tick places the frame and
//! the title; from the next one it reads the edges. Afterwards the dialog is cleared and three
//! more frozen ticks run before the result: はい (item 0) is true, いいえ, cancel and the pause
//! key (a `longjmp` out of the dialog) are false.

use super::snd;
use crate::screens::tables::{Dialog, TextDesc};
use crate::screens::{MenuInput, Sfx};
use ddg_render::sprites::SpriteLayer;

const SLOT_STAR: usize = 0x87;
const SLOT_TITLE: usize = 0x88;
const SLOT_ITEMS: usize = 0x89;
const SLOT_FRAME: usize = 0x90;
/// 0x42b910 clears 0x87..0x90.
const SLOT_CLEAR: std::ops::Range<usize> = 0x87..0x91;
const PAL: u8 = 4;
const LAYER: u8 = 0;
const TAIL_TICKS: u32 = 3;
/// The cursor starts on いいえ.
const START_CURSOR: i32 = 1;
const SCREEN: (i16, i16) = (640, 480);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Opening,
    Open,
    Closing(u32, bool),
}

#[derive(Debug, Clone)]
pub struct ExitConfirm {
    dialog: Dialog,
    frame_size: (i16, i16),
    cursor: i32,
    state: State,
}

fn clear(l: &mut SpriteLayer) {
    for s in SLOT_CLEAR {
        super::hide(l, s);
    }
}

impl ExitConfirm {
    /// `frame_size` is the frame tilemap's size in pixels (the cache at 0x850aa8).
    pub fn new(dialog: Dialog, frame_size: (i16, i16), l: &mut SpriteLayer) -> Self {
        clear(l);
        Self {
            dialog,
            frame_size,
            cursor: START_CURSOR,
            state: State::Opening,
        }
    }

    fn origin(&self) -> (i16, i16) {
        (
            (SCREEN.0 - self.frame_size.0) / 2,
            (SCREEN.1 - self.frame_size.1) / 2,
        )
    }

    fn close(&mut self, yes: bool, l: &mut SpriteLayer) {
        clear(l);
        self.state = State::Closing(TAIL_TICKS, yes);
    }

    /// One frozen tick. Returns `Some(yes)` on the last one. Clicks are 0x26, sent straight to
    /// the sound thread (play them ungated).
    pub fn tick(
        &mut self,
        input: &MenuInput,
        text: &[TextDesc],
        l: &mut SpriteLayer,
        sfx: &mut Vec<Sfx>,
    ) -> Option<bool> {
        let t = |i: i32| {
            usize::try_from(i)
                .ok()
                .and_then(|i| text.get(i))
                .copied()
                .unwrap_or_default()
        };
        let (fx, fy) = self.origin();
        let d = self.dialog.clone();
        match self.state {
            State::Closing(n, yes) => {
                if n <= 1 {
                    return Some(yes);
                }
                self.state = State::Closing(n - 1, yes);
                return None;
            }
            State::Opening => {
                super::put(l, SLOT_FRAME, d.frame, fx, fy, PAL, LAYER);
                let title = t(i32::from(d.title));
                let x = (SCREEN.0 - i16::from(title.w) * 16) / 2;
                super::text(
                    l,
                    SLOT_TITLE,
                    title,
                    x,
                    fy + i16::from(d.title_y),
                    PAL,
                    LAYER,
                );
                self.state = State::Open;
                return None;
            }
            State::Open => {}
        }
        if input.pause {
            self.close(false, l);
            return None;
        }
        if input.confirm {
            sfx.push(Sfx::Play(snd::DIALOG));
            self.close(self.cursor == 0, l);
            return None;
        }
        if input.cancel {
            sfx.push(Sfx::Play(snd::DIALOG));
            self.close(false, l);
            return None;
        }
        let last = d.items.len() as i32 - 1;
        let mut list = super::ScrollList {
            cursor: self.cursor,
            ..Default::default()
        };
        if list.step(input, last) {
            sfx.push(Sfx::Play(snd::DIALOG));
        }
        self.cursor = list.cursor;
        let first = t(i32::from(d.items.first().copied().unwrap_or(0)));
        let x = (SCREEN.0 - i16::from(first.w) * 16) / 2;
        let y0 = fy + i16::from(d.items_y);
        for (i, &item) in d.items.iter().enumerate() {
            let hl = i32::from(i as i32 == self.cursor);
            let y = y0 + i16::from(d.spacing) * i as i16;
            super::text(l, SLOT_ITEMS + i, t(i32::from(item) + hl), x, y, PAL, LAYER);
        }
        let y = y0 + i16::from(d.spacing) * self.cursor as i16;
        super::text(
            l,
            SLOT_STAR,
            t(0),
            fx + i16::from(d.cursor_x),
            y,
            PAL,
            LAYER,
        );
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dialog() -> ExitConfirm {
        let d = Dialog {
            frame: 0x542,
            title: 10,
            title_y: 24,
            cursor_x: 96,
            items_y: 72,
            spacing: 48,
            items: vec![12, 14],
        };
        ExitConfirm::new(d, (320, 192), &mut SpriteLayer::default())
    }

    fn run(d: &mut ExitConfirm, input: MenuInput) -> (Option<bool>, Vec<Sfx>) {
        let text = vec![TextDesc::default(); 29];
        let mut sfx = vec![];
        let r = d.tick(&input, &text, &mut SpriteLayer::default(), &mut sfx);
        (r, sfx)
    }

    #[test]
    fn yes_needs_the_cursor_moved_up() {
        let mut d = dialog();
        run(&mut d, MenuInput::NONE);
        let up = MenuInput {
            up: true,
            ..MenuInput::NONE
        };
        assert_eq!(run(&mut d, up).1, [Sfx::Play(snd::DIALOG)]);
        let ok = MenuInput {
            confirm: true,
            ..MenuInput::NONE
        };
        assert_eq!(run(&mut d, ok).0, None);
        assert_eq!(run(&mut d, MenuInput::NONE).0, None);
        assert_eq!(run(&mut d, MenuInput::NONE).0, None);
        assert_eq!(run(&mut d, MenuInput::NONE).0, Some(true));

        let mut d = dialog();
        run(&mut d, ok);
        let mut r = None;
        for _ in 0..4 {
            r = run(&mut d, ok).0.or(r);
        }
        assert_eq!(r, Some(false), "the default is いいえ");
    }
}

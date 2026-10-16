//! The screens between the route select and the run (`Stage_Run` 0x4080d0, menus.md §7): the
//! 仕業検査 lever check of the analog-lever modes (0x4137e0 / 0x413900 / 0x413940) and the
//! 発車案内 departure board (0x4139c0 / 0x413b10 / 0x413d30).

use super::tables::FrontTables;
use super::{PAL_MENU, snd};
use crate::screens::{MenuInput, Sfx};
use ddg_render::sprites::SpriteLayer;

/// Lever check: "仕業検査" (slot 10), "各レバーをスタート位置に合わせて下さい" (slot 11) and the
/// controller picture (slot 0x11 at x0 + 0x9c, y0 + 0xa0), 0x4137e0 immediates.
const LEVER_TITLE: (usize, i16) = (10, 0x249);
const LEVER_PROMPT: (usize, i16) = (11, 0x248);
const LEVER_PICTURE: (usize, i16, i16, i16) = (0x11, 0x50a, 0x9c, 0xa0);
/// The check ends after 50 ticks with the brake past B8 and power 0, or after 400 ticks.
const LEVER_OK_TICKS: i32 = 50;
const LEVER_MAX_TICKS: i32 = 400;

/// Departure board slots: four flaps 0x36..0x39 and the board 0x3a (0x4139c0).
const SLOT_FLAPS: usize = 0x36;
const SLOT_BOARD: usize = 0x3a;
/// Blank flap, the two spin frames and the board (0x4139c0 / 0x413b10 immediates).
const TBL_FLAP_BLANK: i16 = 0x3d5;
const TBL_SPIN: [i16; 2] = [0x3d6, 0x3d7];
const TBL_BOARD: i16 = 0x3d8;
/// Flap k starts spinning at tick 10 + 8k and lands after 32 ticks; the board stays 70 ticks
/// after the last landing.
const BOARD_START: i32 = 10;
const FLAP_STAGGER: i32 = 8;
const FLAP_SPIN: i32 = 32;
const BOARD_HOLD: i32 = 70;
/// The flap forced to station entry 2 for the route-10 variant (`0x48ddf0`).
const VARIANT_FLAP: usize = 3;
const VARIANT_ENTRY: usize = 2;

/// 仕業検査 (only created in control modes 5/6; the digital modes force P0 / EB instead).
#[derive(Debug, Clone, Default)]
pub struct LeverCheck {
    /// +0x34: ticks run.
    ticks: i32,
    /// +0x38: ticks with the levers at the start position (not necessarily consecutive).
    ok: i32,
}

impl LeverCheck {
    pub fn new(t: &FrontTables, l: &mut SpriteLayer) -> Self {
        for p in &t.lever_layout {
            super::place(l, p, 0, 0);
        }
        super::image(l, LEVER_TITLE.0, LEVER_TITLE.1);
        super::image(l, LEVER_PROMPT.0, LEVER_PROMPT.1);
        let (slot, tbl, x, y) = LEVER_PICTURE;
        let (x0, y0) = t.origin;
        super::put(l, slot, tbl, x0 + x, y0 + y, PAL_MENU, 3);
        Self::default()
    }

    /// One tick of 0x413900 with the notches `(power, brake)`; true when it ended (the
    /// destroy 0x413940 clears the sprites).
    pub fn tick(&mut self, input: &MenuInput, l: &mut SpriteLayer) -> bool {
        if input.brake.unwrap_or(0) > 8 && input.notch.unwrap_or(1) == 0 {
            self.ok += 1;
        }
        if self.ticks < LEVER_MAX_TICKS && self.ok < LEVER_OK_TICKS {
            self.ticks += 1;
            return false;
        }
        super::clear_all(l);
        true
    }
}

/// 発車案内: the departure-board roulette (0x413b10).
#[derive(Debug, Clone)]
pub struct DepartureBoard {
    /// +0x34.
    counter: i32,
    /// +0x38: ticks since all four flaps landed.
    landed: i32,
    route: usize,
    variant: bool,
}

impl DepartureBoard {
    pub fn new(t: &FrontTables, route: usize, variant: bool, l: &mut SpriteLayer) -> Self {
        let (x0, y0) = t.origin;
        for p in &t.board_layout {
            super::place(l, p, x0, y0);
        }
        for k in 0..4 {
            super::image(l, SLOT_FLAPS + k, TBL_FLAP_BLANK);
        }
        super::image(l, SLOT_BOARD, TBL_BOARD);
        Self {
            counter: 0,
            landed: 0,
            route,
            variant,
        }
    }

    fn list(t: &FrontTables, k: usize) -> &[i16] {
        match k {
            0 => &t.board_series,
            1 => &t.board_types,
            _ => &t.board_stations,
        }
    }

    /// One tick; true when the screen ended (the destroy 0x413d30 hides its slots).
    pub fn tick(&mut self, t: &FrontTables, l: &mut SpriteLayer, sfx: &mut Vec<Sfx>) -> bool {
        if self.counter == BOARD_START {
            sfx.push(Sfx::Play(snd::FLAPS));
        }
        if self.counter >= BOARD_START {
            let mut done = 0;
            for k in 0..4 {
                let s = self.counter - BOARD_START - FLAP_STAGGER * k as i32;
                let list = Self::list(t, k);
                if s < FLAP_SPIN {
                    if s >= 0 {
                        let tbl = match s % 3 {
                            0 => TBL_SPIN[0],
                            1 => TBL_SPIN[1],
                            _ => list[(s / 3) as usize % list.len()],
                        };
                        super::image(l, SLOT_FLAPS + k, tbl);
                    }
                    continue;
                }
                let mut entry = t
                    .board_routes
                    .get(self.route)
                    .map_or(0, |r| r[k].max(0) as usize);
                if self.variant && k == VARIANT_FLAP {
                    entry = VARIANT_ENTRY;
                }
                super::image(l, SLOT_FLAPS + k, list.get(entry).copied().unwrap_or(-1));
                done += 1;
            }
            if done == 4 {
                sfx.push(Sfx::Stop(snd::FLAPS));
                self.landed += 1;
            }
        }
        if self.landed < BOARD_HOLD {
            self.counter += 1;
            return false;
        }
        for k in 0..=4 {
            super::hide(l, SLOT_FLAPS + k);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::front::tests_support::tables;

    #[test]
    fn lever_check_counts_start_position_ticks() {
        let t = tables();
        let mut l = SpriteLayer::default();
        let mut c = LeverCheck::new(&t, &mut l);
        let ready = MenuInput {
            notch: Some(0),
            brake: Some(9),
            ..MenuInput::NONE
        };
        let not = MenuInput {
            notch: Some(1),
            brake: Some(9),
            ..MenuInput::NONE
        };
        let mut n = 0;
        for i in 0.. {
            n += 1;
            if c.tick(if i % 2 == 0 { &ready } else { &not }, &mut l) {
                break;
            }
        }
        assert_eq!(n, 99, "50 ready ticks, not consecutive");
        let mut c = LeverCheck::new(&t, &mut l);
        let mut n = 0;
        while !c.tick(&not, &mut l) {
            n += 1;
        }
        assert_eq!(n, 400);
    }

    #[test]
    fn board_timing_and_landing() {
        let t = tables();
        let mut l = SpriteLayer::default();
        let mut sfx = vec![];
        let mut b = DepartureBoard::new(&t, 3, false, &mut l);
        let mut ticks = 1;
        while !b.tick(&t, &mut l, &mut sfx) {
            ticks += 1;
            if ticks == 68 {
                // All four landed on the route's entries.
                for k in 0..4 {
                    let e = t.board_routes[3][k] as usize;
                    assert_eq!(
                        l.slots[SLOT_FLAPS + k].tilemap,
                        DepartureBoard::list(&t, k)[e]
                    );
                }
            }
        }
        assert_eq!(ticks, 136);
        assert_eq!(sfx.first(), Some(&Sfx::Play(snd::FLAPS)));
        assert_eq!(
            sfx.iter().filter(|s| **s == Sfx::Stop(snd::FLAPS)).count(),
            70
        );
    }
}

//! Boot and attract (`Attract_Run` 0x402700 over the item table 0x484e30, menus.md §2).
//!
//! The PC build runs in free play (`Credits_Init` 0x401910 sets `0x8573e0 = 1`), so the arcade
//! "press START" screen 0x4122d0 lasts one tick and START is the menu-confirm edge (0x1000,
//! Enter / pad A). Attract walks its item table until START. START while the title picture
//! shows (`0x855ffc & 0x20`) plays 0x1e and goes to the title menu; START during any other item
//! jumps to the title picture (`0x855ffc |= 0x10` restarts the walk at item 2).
//!
//! Built: item 2 (0x402e80) as its title-picture branch 0x4037f0 (tbl 0x410 full screen,
//! palettes 0x15–0x17 fading in, title call 0x1024 at tick 100, 300 ticks), and item 3
//! (0x404e10) as its Komachi-movie check (the movie itself is postponed, decision D2). The other
//! items (the 3D opening fly-by 0x402fb0 that item 2 plays on a cold start, the difficulty logo
//! 0x402880, the demo route and drive 0x40e450 / 0x40e4b0, 0x402e60, 0x40e6c0, 0x403f40,
//! 0x403da0, 0x40e6e0) are not built and take no time, so attract shows the title picture in a
//! loop.

use super::snd;
use crate::screens::{MenuInput, Sfx};
use anyhow::Result;
use ddg_formats::cg::Palette;
use ddg_formats::exe::Exe;
use ddg_render::sprites::SpriteLayer;

/// The attract item table (null-terminated function pointers).
const ITEMS: u32 = 0x484e30;
const ITEMS_MAX: usize = 32;
/// Item functions with a port.
const ITEM_TITLE: u32 = 0x402e80;
const ITEM_RANKING_MOVIE: u32 = 0x404e10;
/// With `0x855ffc & 0x10` the walk starts at item 2.
const RESTART_ITEM: usize = 2;
/// The title picture (0x4037f0): tbl 0x410 in res modes 0/1/3 at (x0, y0), palette 0x17.
const TBL_TITLE: i16 = 0x410;
const SLOT_TITLE: usize = 0xf;
const PAL_TITLE: u8 = 0x17;
const TITLE_TICKS: i32 = 300;
const TITLE_CALL_TICK: i32 = 100;
/// Title call (sequence 0x24).
const SND_TITLE_CALL: u16 = 0x1024;
/// Palette fades started by 0x4037f0: `0x4367c0(slot, ticks, target)`.
const FADES: [(u8, i32, u32); 3] = [
    (0x17, 0x40, 0x48b290),
    (0x16, 0x50, 0x488e90),
    (0x15, 0x50, 0x488290),
];
/// At the end of the table: sprites cleared, then three ticks (0x402700).
const TABLE_END_TICKS: i32 = 3;
/// START from the title picture to the title menu: the attract task is killed between two
/// `Task_Tick`s of `Game_MainFlow`.
const START_TICKS: i32 = 2;

/// The exe data attract needs.
#[derive(Debug, Clone)]
pub struct AttractTables {
    items: Vec<u32>,
    fade_targets: Vec<(u8, i32, Palette)>,
}

impl AttractTables {
    pub fn read(exe: &Exe<'_>) -> Result<Self> {
        let mut items = Vec::new();
        for i in 0..ITEMS_MAX {
            let f = exe.u32_at(ITEMS + 4 * i as u32)?;
            if f == 0 {
                break;
            }
            items.push(f);
        }
        anyhow::ensure!(
            items.get(RESTART_ITEM) == Some(&ITEM_TITLE),
            "attract table: {items:x?}"
        );
        let fade_targets = FADES
            .iter()
            .map(|&(slot, n, va)| Ok((slot, n, Palette::parse(exe.bytes_at(va, Palette::BYTES)?)?)))
            .collect::<Result<_>>()?;
        Ok(Self {
            items,
            fade_targets,
        })
    }
}

/// `0x4367c0`: a palette fading from black to `target` over `total` ticks (task 0x436820:
/// each tick scales by `(total − remaining) / total`, the last tick copies exactly).
#[derive(Debug, Clone)]
pub struct Fade {
    slot: u8,
    total: i32,
    remaining: i32,
    target: Palette,
}

impl Fade {
    pub fn new(slot: u8, total: i32, target: Palette, l: &mut SpriteLayer) -> Self {
        if let Some(p) = l.palettes.get_mut(usize::from(slot)) {
            *p = Some(Palette([0; 256]));
        }
        Self {
            slot,
            total,
            remaining: total,
            target,
        }
    }

    /// One tick; false once finished.
    pub fn tick(&mut self, l: &mut SpriteLayer) -> bool {
        if self.remaining < 1 {
            return false;
        }
        let p = if self.remaining < 2 {
            self.target.clone()
        } else {
            let f = f64::from(self.total - self.remaining) / f64::from(self.total);
            let scale = |c: u32| (f64::from(c & 0xff) * f) as u32 & 0xff;
            Palette(
                self.target
                    .0
                    .map(|c| (scale(c >> 16) << 16) | (scale(c >> 8) << 8) | scale(c)),
            )
        };
        if let Some(slot) = l.palettes.get_mut(usize::from(self.slot)) {
            *slot = Some(p);
        }
        self.remaining -= 1;
        true
    }
}

#[derive(Debug, Clone)]
enum Phase {
    /// About to start item n.
    Item(usize),
    /// The title picture, at its counter.
    Picture(i32),
    /// Ticks before item n starts.
    Gap(i32, usize),
    /// START taken from the title picture: ticks left before the title menu.
    Leaving(i32),
}

/// What the attract tick asks of its owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttractStep {
    Running,
    /// Item 3's check passed: the Komachi movie would play now (postponed, D2).
    KomachiMovie,
    /// Go to the title menu.
    Title,
}

#[derive(Debug, Clone)]
pub struct Attract {
    phase: Phase,
    fades: Vec<Fade>,
    /// `0x8573c8`, counted per walk of the table.
    pub counter: u32,
}

impl Attract {
    /// `Attract_Run`: from item 0, or from the title picture after a menu cancel / a quit
    /// (`0x855ffc & 0x10`).
    pub fn new(skip_opening: bool) -> Self {
        Self {
            phase: Phase::Item(if skip_opening { RESTART_ITEM } else { 0 }),
            fades: Vec::new(),
            counter: 0,
        }
    }

    #[cfg(test)]
    pub fn showing_title(&self) -> bool {
        matches!(self.phase, Phase::Picture(_))
    }

    /// One tick. `komachi` is the result of 0x404cf0 (every select stage cleared).
    pub fn tick(
        &mut self,
        input: &MenuInput,
        t: &AttractTables,
        komachi: bool,
        l: &mut SpriteLayer,
        sfx: &mut Vec<Sfx>,
    ) -> AttractStep {
        self.fades.retain_mut(|f| f.tick(l));
        let mut step = AttractStep::Running;
        // Items that take no time run back to back within this tick.
        for _ in 0..=t.items.len() {
            match self.phase.clone() {
                Phase::Leaving(n) => {
                    if n <= 1 {
                        return AttractStep::Title;
                    }
                    self.phase = Phase::Leaving(n - 1);
                    return AttractStep::Running;
                }
                Phase::Gap(n, next) => {
                    if input.confirm {
                        // START outside the title picture: restart at the picture.
                        self.phase = Phase::Item(RESTART_ITEM);
                        continue;
                    }
                    self.phase = if n <= 1 {
                        Phase::Item(next)
                    } else {
                        Phase::Gap(n - 1, next)
                    };
                    return step;
                }
                Phase::Picture(c) => {
                    if input.confirm {
                        sfx.push(Sfx::Play(snd::START));
                        super::clear_all(l);
                        self.fades.clear();
                        self.phase = Phase::Leaving(START_TICKS);
                        return AttractStep::Running;
                    }
                    if c == TITLE_CALL_TICK {
                        sfx.push(Sfx::Play(SND_TITLE_CALL));
                    }
                    if c >= TITLE_TICKS - 1 {
                        // The task ends; its destroy 0x403d20 clears the sprites.
                        super::clear_all(l);
                        self.phase = Phase::Gap(1, RESTART_ITEM + 1);
                    } else {
                        self.phase = Phase::Picture(c + 1);
                    }
                    return step;
                }
                Phase::Item(i) => {
                    let Some(&f) = t.items.get(i) else {
                        super::clear_all(l);
                        self.counter += 1;
                        self.phase = Phase::Gap(TABLE_END_TICKS, 0);
                        return step;
                    };
                    match f {
                        ITEM_TITLE => {
                            super::put(l, SLOT_TITLE, TBL_TITLE, 0, 0, PAL_TITLE, 3);
                            self.fades = t
                                .fade_targets
                                .iter()
                                .map(|(slot, n, p)| Fade::new(*slot, *n, p.clone(), l))
                                .collect();
                            self.fades.retain_mut(|f| f.tick(l));
                            self.phase = Phase::Picture(1);
                            return step;
                        }
                        ITEM_RANKING_MOVIE if komachi => step = AttractStep::KomachiMovie,
                        _ => {}
                    }
                    // Not built: the item ends at once; this tick is the Task_Tick after it.
                    self.phase = Phase::Item(i + 1);
                    return step;
                }
            }
        }
        step
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables() -> AttractTables {
        AttractTables {
            items: vec![0x402e60, 0x402880, ITEM_TITLE, ITEM_RANKING_MOVIE, 0x40e450],
            fade_targets: vec![(0x17, 0x40, Palette([0x00ff_8040; 256]))],
        }
    }

    fn run(a: &mut Attract, t: &AttractTables, confirm: bool) -> (AttractStep, Vec<Sfx>) {
        let mut l = SpriteLayer::default();
        let mut sfx = vec![];
        let input = MenuInput {
            confirm,
            ..MenuInput::NONE
        };
        (a.tick(&input, t, false, &mut l, &mut sfx), sfx)
    }

    #[test]
    fn start_on_the_picture_goes_to_the_title() {
        let t = tables();
        let mut a = Attract::new(false);
        for _ in 0..3 {
            run(&mut a, &t, false);
        }
        assert!(a.showing_title());
        let mut call = false;
        for _ in 0..150 {
            call |= run(&mut a, &t, false)
                .1
                .contains(&Sfx::Play(SND_TITLE_CALL));
        }
        assert!(call);
        let (s, sfx) = run(&mut a, &t, true);
        assert_eq!(
            (s, sfx),
            (AttractStep::Running, vec![Sfx::Play(snd::START)])
        );
        assert_eq!(run(&mut a, &t, false).0, AttractStep::Running);
        assert_eq!(run(&mut a, &t, false).0, AttractStep::Title);
    }

    #[test]
    fn picture_loops_and_start_elsewhere_jumps_to_it() {
        let t = tables();
        let mut a = Attract::new(true);
        run(&mut a, &t, false);
        assert!(a.showing_title());
        let mut n = 0;
        while a.showing_title() {
            run(&mut a, &t, false);
            n += 1;
        }
        assert_eq!(n, TITLE_TICKS - 1);
        // In a gap, START restarts at the picture.
        run(&mut a, &t, true);
        assert!(a.showing_title());
    }

    #[test]
    fn fade_reaches_the_target() {
        let target = Palette([0x00ff_8040; 256]);
        let mut l = SpriteLayer::default();
        let mut f = Fade::new(0x17, 4, target.clone(), &mut l);
        f.tick(&mut l);
        assert_eq!(l.palettes[0x17].as_ref().unwrap().0[1], 0);
        f.tick(&mut l);
        assert_eq!(l.palettes[0x17].as_ref().unwrap().0[1], 0x003f_2010);
        f.tick(&mut l);
        f.tick(&mut l);
        assert_eq!(l.palettes[0x17].as_ref(), Some(&target));
        assert!(!f.tick(&mut l));
    }
}

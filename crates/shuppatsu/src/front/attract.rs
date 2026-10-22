//! Boot and attract (`Attract_Run` 0x402700 over the item table 0x484e30, menus.md §2).
//!
//! The PC build runs in free play (`Credits_Init` 0x401910 sets `0x8573e0 = 1`), so the arcade
//! "press START" screen 0x4122d0 lasts one tick and START is the menu-confirm edge (0x1000,
//! Enter / pad A). Attract walks its item table until START. START while the title picture
//! shows (`0x855ffc & 0x20`) plays 0x1e and goes to the title menu; START during any other item
//! jumps to the title picture (`0x855ffc |= 0x10` restarts the walk at item 2), except in the
//! logo item, which handles START itself (`0x855ffc & 0x100`).
//!
//! The table: 0x402e60 (assets of area 0x15, instant), 0x402880 (the TAITO logo with two
//! locomotives that spell the difficulty), 0x402e80 (the title: the cold-start opening, then
//! the title picture 0x4037f0), 0x404e10 (the Komachi-movie check), 0x40e450 (picks the demo
//! route), 0x40e4b0 (demo drive on it), 0x40e6c0 (assets of stage 5, instant), 0x403f40 ×4
//! (photos), 0x403da0 (a poster), 0x40e6e0 (the stage-5 lesson demo). The demo drives are run
//! by the owner ([`AttractStep::Demo`]), and so are the cold-start opening
//! ([`AttractStep::Opening`]) and the Komachi movie ([`AttractStep::KomachiMovie`], played by
//! `crate::movie` once every select stage has a clear mark).

use super::snd;
use crate::screens::{MenuInput, Sfx};
use anyhow::Result;
use shuppatsu_formats::cg::Palette;
use shuppatsu_formats::exe::Exe;
use shuppatsu_formats::hud::{MODE_640, RES_MODES};
use shuppatsu_render::sprites::SpriteLayer;

/// The attract item table (null-terminated function pointers).
const ITEMS: u32 = 0x484e30;
const ITEMS_MAX: usize = 32;
/// Item functions with a port.
const ITEM_LOGO: u32 = 0x402880;
const ITEM_TITLE: u32 = 0x402e80;
const ITEM_RANKING_MOVIE: u32 = 0x404e10;
const ITEM_DEMO_ROUTE: u32 = 0x40e450;
const ITEM_DEMO_DRIVE: u32 = 0x40e4b0;
const ITEM_PHOTO: u32 = 0x403f40;
const ITEM_POSTER: u32 = 0x403da0;
const ITEM_LESSON_DEMO: u32 = 0x40e6e0;
/// With `0x855ffc & 0x10` the walk starts at item 2.
const RESTART_ITEM: usize = 2;
/// The title picture (0x4037f0): tbl 0x410 in res modes 0/1/3 at (x0, y0), palette 0x17.
pub(super) const TBL_TITLE: i16 = 0x410;
/// Enhancement `wide_menus` (menus.md §14): the title picture has no tile pattern, so a wide
/// canvas continues it with paper synthesised from its plain edges ([`Paper`]). Measured on a
/// 640×480 shot of tbl 0x410, palette 0x17:
/// - the patch: rows 256–479 of the 128 outermost columns at each side, clear of the art
///   ("PRESS START BUTTON", the TAITO logo and the copyright lines span x ≈ 140–500 there);
/// - the ramp: the 24 outermost columns. In the logo band (rows 48–255) the glow left of 電
///   reaches x ≈ 15 and the shadow of 番台 / the signal head x ≈ 631, but the art itself
///   (outlines, the 高速編 box, the signal) stays inside x = 32–609, so only glow and shadow
///   fade out;
/// - the light: 12 columns from each edge, which the glow and shadow barely reach (their
///   mean in the band is within 2 levels of the rows outside it on the left, 4 on the
///   right); the outermost column is skipped, the right one is lighter than the paper next
///   to it (mean grey 194 against 187 outside the band).
///
/// [`Paper`]: shuppatsu_render::sprites::Paper
pub(super) const TITLE_PAPER: shuppatsu_render::sprites::Paper = shuppatsu_render::sprites::Paper {
    source_rows: (256, 480),
    source_cols: 128,
    ramp: 24,
    edge_skip: 1,
    edge_cols: 12,
};
const SLOT_TITLE: usize = 0xf;
const PAL_TITLE: u8 = 0x17;
const TITLE_TICKS: i32 = 300;
const TITLE_CALL_TICK: i32 = 100;
/// The opening's BGM, released by the title task's destroy 0x403d20.
const OPENING_MUSIC: u16 = 0x1a1;
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
/// START during another item: the item's task ends in the next `Task_Tick`, then 0x402700
/// runs one more before it restarts at the title.
const ABORT_TICKS: i32 = 2;

/// The logo item (0x402880, task 0x402a70): slots 0x11 / 0x12 = two locomotives, 0xd = the
/// TAITO logo, placed at (0x280, 0x13e) / (0x300, 0x13e) palette 4 and (0x380, 0x66) palette
/// 0xb. Locomotive k shows bit k of the difficulty `0x829d10` (immediates of 0x402880, res
/// modes 0/1: 0x433 / 0x436), the logo is tbl 0x160.
const SLOT_LOCO: [usize; 2] = [0x11, 0x12];
const SLOT_LOGO: usize = 0xd;
const TBL_LOCOS: [i16; 2] = [0x433, 0x436];
const TBL_LOGO: i16 = 0x160;
const PAL_LOCO: u8 = 4;
const PAL_LOGO: u8 = 0xb;
const LOGO_FIRST: [(i16, i16); 3] = [(0x280, 0x13e), (0x300, 0x13e), (0x380, 0x66)];
/// Parked locomotives while the logo stands alone (state 3).
const LOGO_PARKED: (i16, i16) = (0x800, 0x13e);
/// Pixels per tick of every slide, and the pauses (states 1, 3, 5, 7: counter > 0x13, 0x3b,
/// 0x13, 4).
const LOGO_SPEED: i32 = 5;
const LOGO_WAITS: [i32; 4] = [0x13, 0x3b, 0x13, 4];
/// The locomotives' horn when they stop (states 1 and 5).
const SND_LOGO_HORN: u16 = 0x2a;
/// Per resolution mode: locomotive width (0x4579c0), logo width (0x4579e0), locomotive y
/// (0x457a00), logo y (0x457a10), screen width (0x48c1e0).
const LOGO_TABLES: [u32; 5] = [0x4579c0, 0x4579e0, 0x457a00, 0x457a10, 0x48c1e0];

/// Photos (0x403f40, task 0x404030): `{tbl, w, h}` i16 × 35 at 0x4578ac, picked by the photo
/// counter `0x8573cc`, centred on (320, 240) in slot 0x10, palette 7; 102 ticks each.
const PHOTOS: u32 = 0x4578ac;
const PHOTO_COUNT: usize = 0x23;
const SLOT_PHOTO: usize = 0x10;
const PAL_PHOTO: u8 = 7;
const PHOTO_LAST: i32 = 0x65;
/// Poster (0x403da0, task 0x403ec0): `{tbl, palette}` i16 × 2 at 0x4578a0 by the walk counter
/// parity, full screen in slot 0xf; the 3000番台 badge tbl 0x122 in slot 7 at (0x196, 0xf0)
/// (even) or (0x34, 0x78) (odd), palette 4; 202 ticks.
const POSTERS: u32 = 0x4578a0;
const SLOT_POSTER: usize = 0xf;
const SLOT_BADGE: usize = 7;
const TBL_BADGE: i16 = 0x122;
const BADGE_POS: [(i16, i16); 2] = [(0x196, 0xf0), (0x34, 0x78)];
const POSTER_LAST: i32 = 0xc9;
/// The routes attract may demo: bit r of `0x8500b0` (`mov [0x8500b0], 0x3f9da` in 0x41e520),
/// routes 0–12 (0x40e450).
const DEMO_ROUTES: u32 = 0x3f9da;
const DEMO_ROUTE_COUNT: u32 = 13;
/// The lesson demo's stage (0x40e6e0: `0x8570a4 = 5`).
const LESSON_STAGE: usize = 5;
/// The stage whose data the opening runs on (AKITA1C in its route part).
pub const OPENING_STAGE: usize = 1;

/// The exe data attract needs.
#[derive(Debug, Clone)]
pub struct AttractTables {
    items: Vec<u32>,
    fade_targets: Vec<(u8, i32, Palette)>,
    /// Locomotive width, logo width, locomotive y, logo y, screen width (res mode 1).
    logo: [i32; 5],
    photos: Vec<(i16, i16, i16)>,
    posters: [(i16, u8); 2],
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
        let mut logo = [0; 5];
        for (v, va) in logo.iter_mut().zip(LOGO_TABLES) {
            *v = exe
                .reader_at(va, 4 * RES_MODES, "logo tables")?
                .i32(4 * MODE_640)?;
        }
        let r = exe.reader_at(PHOTOS, 6 * PHOTO_COUNT, "attract photos")?;
        let photos = (0..PHOTO_COUNT)
            .map(|i| Ok((r.i16(6 * i)?, r.i16(6 * i + 2)?, r.i16(6 * i + 4)?)))
            .collect::<Result<_>>()?;
        let r = exe.reader_at(POSTERS, 8, "attract posters")?;
        let posters = [(r.i16(0)?, r.i16(2)? as u8), (r.i16(4)?, r.i16(6)? as u8)];
        Ok(Self {
            items,
            fade_targets,
            logo,
            photos,
            posters,
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

/// The attract globals that outlive one walk: the walk counter `0x8573c8` and the photo
/// counter `0x8573cc`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AttractCounters {
    pub walk: u32,
    pub photo: u32,
}

/// Which demo drive the owner runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoKind {
    /// 0x40e4b0: the demo route, 1600 ticks, notches from the table 0x467b20.
    Route,
    /// 0x40e6e0: stage 5 with the lesson course events 0x459338, 3250 ticks, notches from
    /// 0x46857c.
    Lesson,
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
    /// The logo item: state (task+0x34) and counter (task+0x30).
    Logo(u8, i32),
    /// A photo or the poster, at its counter.
    Photo(i32),
    Poster(i32),
    /// The owner runs a demo drive; item n follows.
    Demo(usize),
    /// The owner runs the opening; the title picture of item n follows.
    Opening(usize),
    /// The owner plays the Komachi movie of item n.
    Movie(usize),
}

/// What the attract tick asks of its owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttractStep {
    Running,
    /// Item 3's check passed: play the Komachi movie, then call [`Attract::movie_done`].
    KomachiMovie,
    /// Go to the title menu.
    Title,
    /// Run the cold-start opening, then call [`Attract::opening_done`].
    Opening,
    /// Run a demo drive of `stage`, then call [`Attract::demo_done`].
    Demo {
        kind: DemoKind,
        stage: usize,
    },
}

/// The machine state attract reads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AttractEnv {
    /// 0x404cf0: every select stage cleared.
    pub komachi: bool,
    /// The difficulty `0x829d10` (0–3).
    pub difficulty: u8,
}

#[derive(Debug, Clone)]
pub struct Attract {
    phase: Phase,
    fades: Vec<Fade>,
    pub counters: AttractCounters,
    /// The route item 4 picked (`0x8570a4`).
    demo_stage: usize,
    /// The item being run.
    current: usize,
    /// `0x855ffc & 0x10`: the next title item skips the cold-start opening.
    pub skip_opening: bool,
}

impl Attract {
    /// `Attract_Run`: from item 0, or from the title picture after a menu cancel / a quit
    /// (`0x855ffc & 0x10`).
    pub fn new(skip_opening: bool, counters: AttractCounters) -> Self {
        Self {
            phase: Phase::Item(if skip_opening { RESTART_ITEM } else { 0 }),
            fades: Vec::new(),
            counters,
            demo_stage: 1,
            current: 0,
            skip_opening,
        }
    }

    #[cfg(test)]
    pub fn showing_title(&self) -> bool {
        matches!(self.phase, Phase::Picture(_))
    }

    /// What attract shows, for the app's START harness and issue marks.
    pub fn shown(&self) -> &'static str {
        match self.phase {
            Phase::Picture(_) => "picture",
            Phase::Logo(..) => "logo",
            Phase::Photo(_) => "photo",
            Phase::Poster(_) => "poster",
            Phase::Leaving(_) => "leaving",
            _ => "attract",
        }
    }

    /// The opening has ended: by START (the walk restarts at the title, which then skips it)
    /// or into the title picture at once.
    pub fn opening_done(&mut self, start: bool) {
        if let Phase::Opening(i) = self.phase {
            if start {
                self.restart_at_title();
            } else {
                self.phase = Phase::Item(i);
            }
        }
    }

    /// The movie asked for by [`AttractStep::KomachiMovie`] has ended. At its end the item
    /// ends and the walk goes on (0x404e10 clears `0x855ff8 & 4`); skipped with the confirm
    /// edge (0x428f00 sets `0x855ffc |= 0x10` and leaves the item running), the walk sees that
    /// same edge as START at once and restarts at the title picture, like any item aborted
    /// by START (0x402700).
    pub fn movie_done(&mut self, skipped: bool, l: &mut SpriteLayer) {
        if let Phase::Movie(i) = self.phase {
            if skipped {
                self.abort(l);
            } else {
                self.phase = Phase::Item(i + 1);
            }
        }
    }

    /// The stage of the run (a demo drive, or the opening on stage 1) that attract starts
    /// next, if no other screen item comes before it: what its owner can load ahead while the
    /// current item shows. A guess only for what the items do (the owner checks what it
    /// loaded against the run it starts).
    pub fn next_run(&self, t: &AttractTables) -> Option<usize> {
        let from = match self.phase {
            Phase::Item(i) | Phase::Gap(_, i) => i,
            Phase::Logo(..) | Phase::Photo(_) | Phase::Poster(_) | Phase::Picture(_) => {
                self.current + 1
            }
            Phase::Leaving(_) | Phase::Demo(_) | Phase::Opening(_) | Phase::Movie(_) => {
                return None;
            }
        };
        let mut stage = self.demo_stage;
        for &f in t.items.get(from..)? {
            match f {
                ITEM_DEMO_ROUTE => stage = demo_route(self.counters.walk),
                ITEM_DEMO_DRIVE => return Some(stage),
                ITEM_LESSON_DEMO => return Some(LESSON_STAGE),
                ITEM_TITLE if !self.skip_opening => return Some(OPENING_STAGE),
                _ => return None,
            }
        }
        None
    }

    /// The demo drive asked for by [`AttractStep::Demo`] has ended, by START or by its time.
    pub fn demo_done(&mut self, start: bool) {
        if let Phase::Demo(next) = self.phase {
            if start {
                self.restart_at_title();
            } else {
                self.phase = Phase::Gap(1, next);
            }
        }
    }

    /// START outside the title picture and the logo: the item ends, the walk restarts at the
    /// title (0x402700).
    fn abort(&mut self, l: &mut SpriteLayer) {
        super::clear_all(l);
        self.restart_at_title();
    }

    /// 0x402700 after START ended an item (0x402829–0x402853): the walk counter `0x8573c8`
    /// goes up and, the title picture not being up, `0x855ffc |= 0x10`, so the walk restarts
    /// at item 2 and 0x402e80 goes straight to the title picture 0x4037f0, no opening.
    fn restart_at_title(&mut self) {
        self.counters.walk += 1;
        self.skip_opening = true;
        self.phase = Phase::Gap(ABORT_TICKS, RESTART_ITEM);
    }

    /// One tick.
    pub fn tick(
        &mut self,
        input: &MenuInput,
        t: &AttractTables,
        env: AttractEnv,
        l: &mut SpriteLayer,
        sfx: &mut Vec<Sfx>,
    ) -> AttractStep {
        self.fades.retain_mut(|f| f.tick(l));
        let step = AttractStep::Running;
        match self.phase.clone() {
            Phase::Leaving(n) => {
                if n <= 1 {
                    return AttractStep::Title;
                }
                self.phase = Phase::Leaving(n - 1);
                AttractStep::Running
            }
            Phase::Gap(n, next) => {
                if input.confirm && next != RESTART_ITEM {
                    // START outside the title picture: restart at the picture.
                    self.abort(l);
                    return step;
                }
                self.phase = if n <= 1 {
                    Phase::Item(next)
                } else {
                    Phase::Gap(n - 1, next)
                };
                step
            }
            Phase::Demo(_) | Phase::Opening(_) | Phase::Movie(_) => step,
            Phase::Picture(c) => {
                if input.confirm {
                    sfx.push(Sfx::Play(snd::START));
                    // Destroy 0x403d20 releases the opening's BGM.
                    sfx.push(Sfx::Stop(OPENING_MUSIC));
                    super::clear_all(l);
                    self.fades.clear();
                    // 0x402829 counts the walk on before the 0x20 test.
                    self.counters.walk += 1;
                    self.phase = Phase::Leaving(START_TICKS);
                    return AttractStep::Running;
                }
                if c == TITLE_CALL_TICK {
                    sfx.push(Sfx::Play(SND_TITLE_CALL));
                }
                if c >= TITLE_TICKS - 1 {
                    // The task ends; its destroy 0x403d20 clears the sprites.
                    sfx.push(Sfx::Stop(OPENING_MUSIC));
                    super::clear_all(l);
                    self.phase = Phase::Gap(1, RESTART_ITEM + 1);
                } else {
                    self.phase = Phase::Picture(c + 1);
                }
                step
            }
            Phase::Logo(state, c) => {
                self.logo_tick(state, c, input, t, l, sfx);
                step
            }
            Phase::Photo(c) => {
                if input.confirm {
                    self.abort(l);
                } else if c >= PHOTO_LAST {
                    // Destroy 0x404060.
                    super::hide(l, SLOT_PHOTO);
                    self.phase = Phase::Gap(1, self.current + 1);
                } else {
                    self.phase = Phase::Photo(c + 1);
                }
                step
            }
            Phase::Poster(c) => {
                if input.confirm {
                    self.abort(l);
                } else if c >= POSTER_LAST {
                    // Destroy 0x403ef0.
                    super::hide(l, SLOT_POSTER);
                    super::hide(l, SLOT_BADGE);
                    self.phase = Phase::Gap(1, self.current + 1);
                } else {
                    self.phase = Phase::Poster(c + 1);
                }
                step
            }
            Phase::Item(i) => {
                let Some(&f) = t.items.get(i) else {
                    super::clear_all(l);
                    self.counters.walk += 1;
                    self.phase = Phase::Gap(TABLE_END_TICKS, 0);
                    return step;
                };
                self.current = i;
                match f {
                    ITEM_TITLE => {
                        if !self.skip_opening {
                            // The cold-start opening (0x402fb0 chain) first; the owner runs
                            // it, then the title picture in the same task.
                            self.skip_opening = true;
                            self.phase = Phase::Opening(i);
                            return AttractStep::Opening;
                        }
                        self.skip_opening = false;
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
                    ITEM_LOGO => {
                        for (k, &(x, y)) in LOGO_FIRST.iter().enumerate() {
                            let (slot, tbl, pal) = match k {
                                2 => (SLOT_LOGO, TBL_LOGO, PAL_LOGO),
                                _ => (
                                    SLOT_LOCO[k],
                                    TBL_LOCOS[usize::from((env.difficulty >> k) & 1)],
                                    PAL_LOCO,
                                ),
                            };
                            super::put(l, slot, tbl, x, y, pal, 3);
                        }
                        self.logo_tick(0, 0, input, t, l, sfx);
                        return step;
                    }
                    ITEM_PHOTO => {
                        let k = self.counters.photo as usize % PHOTO_COUNT;
                        self.counters.photo += 1;
                        if let Some(&(tbl, w, h)) = t.photos.get(k) {
                            let (x, y) = (0x140 - w / 2, 0xf0 - h / 2);
                            super::put(l, SLOT_PHOTO, tbl, x, y, PAL_PHOTO, 3);
                        }
                        self.phase = Phase::Photo(1);
                        return step;
                    }
                    ITEM_POSTER => {
                        let k = (self.counters.walk % 2) as usize;
                        let (tbl, pal) = t.posters[k];
                        super::put(l, SLOT_POSTER, tbl, 0, 0, pal, 3);
                        let (x, y) = BADGE_POS[k];
                        super::put(l, SLOT_BADGE, TBL_BADGE, x, y, 4, 3);
                        self.phase = Phase::Poster(1);
                        return step;
                    }
                    ITEM_DEMO_ROUTE => self.demo_stage = demo_route(self.counters.walk),
                    ITEM_DEMO_DRIVE | ITEM_LESSON_DEMO => {
                        let (kind, stage) = if f == ITEM_DEMO_DRIVE {
                            (DemoKind::Route, self.demo_stage)
                        } else {
                            (DemoKind::Lesson, LESSON_STAGE)
                        };
                        self.phase = Phase::Demo(i + 1);
                        return AttractStep::Demo { kind, stage };
                    }
                    ITEM_RANKING_MOVIE if env.komachi => {
                        self.phase = Phase::Movie(i);
                        return AttractStep::KomachiMovie;
                    }
                    _ => {}
                }
                // Instant items: the item ends at once; this tick is the Task_Tick after it.
                self.phase = Phase::Item(i + 1);
                step
            }
        }
    }

    /// One update of the logo task 0x402a70 (state machine on task+0x34, counter task+0x30).
    fn logo_tick(
        &mut self,
        mut state: u8,
        mut c: i32,
        input: &MenuInput,
        t: &AttractTables,
        l: &mut SpriteLayer,
        sfx: &mut Vec<Sfx>,
    ) {
        let [a, b, loco_y, logo_y, w] = t.logo.map(|v| v as i16);
        let centre = (w - b) / 2;
        let far = 2 * a - centre + w;
        let x = (c * LOGO_SPEED) as i16;
        let mut ended = false;
        match state {
            0 | 6 => {
                let x = x.min(far);
                if x >= far {
                    (c, state) = (0, state + 1);
                }
                let p = if state <= 1 {
                    w - x
                } else {
                    centre + x - 2 * a
                };
                super::move_to(l, SLOT_LOCO[0], p, loco_y);
                super::move_to(l, SLOT_LOCO[1], p + a, loco_y);
                super::move_to(l, SLOT_LOGO, p + 2 * a, logo_y);
            }
            1 | 5 => {
                if c > LOGO_WAITS[usize::from(state / 2)] {
                    (c, state) = (0, state + 1);
                    if state == 2 {
                        sfx.push(Sfx::Play(SND_LOGO_HORN));
                    }
                }
            }
            2 => {
                let x = x.min(centre);
                if x >= centre {
                    (c, state) = (0, 3);
                }
                let p = centre - a - x;
                super::move_to(l, SLOT_LOCO[0], p - a, loco_y);
                super::move_to(l, SLOT_LOCO[1], p, loco_y);
            }
            3 => {
                if c > LOGO_WAITS[1] {
                    (c, state) = (0, 4);
                }
                super::move_to(l, SLOT_LOCO[0], LOGO_PARKED.0, LOGO_PARKED.1);
                super::move_to(l, SLOT_LOCO[1], LOGO_PARKED.0, LOGO_PARKED.1);
                super::move_to(l, SLOT_LOGO, centre, logo_y);
            }
            4 => {
                let x = x.min(centre);
                if x >= centre {
                    sfx.push(Sfx::Play(SND_LOGO_HORN));
                    (c, state) = (0, 5);
                }
                let p = x - a;
                super::move_to(l, SLOT_LOCO[0], p - a, loco_y);
                super::move_to(l, SLOT_LOCO[1], p, loco_y);
            }
            _ => ended = c > LOGO_WAITS[3],
        }
        if !ended && input.confirm {
            if state < 3 {
                // No count this update (0x402a70 returns before `+0x30 += 1`).
                self.phase = Phase::Logo(3, 0);
                return;
            } else if state > 3 {
                // `0x855ffc |= 0x10`: the walk goes on to the title, which skips the opening.
                ended = true;
                self.skip_opening = true;
            }
        }
        if ended {
            // Destroy 0x402e00.
            for s in [SLOT_LOGO, SLOT_LOCO[0], SLOT_LOCO[1]] {
                super::hide(l, s);
            }
            self.phase = Phase::Gap(1, self.current + 1);
            return;
        }
        self.phase = Phase::Logo(state, c + 1);
    }
}

/// 0x40e450: the routes 0–12 with their bit set in `0x8500b0`, entry `walk mod count`.
fn demo_route(walk: u32) -> usize {
    let routes: Vec<u32> = (0..DEMO_ROUTE_COUNT)
        .filter(|r| DEMO_ROUTES & (1 << r) != 0)
        .collect();
    routes[(walk as usize) % routes.len()] as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables() -> AttractTables {
        AttractTables {
            items: vec![
                0x402e60,
                ITEM_LOGO,
                ITEM_TITLE,
                ITEM_RANKING_MOVIE,
                ITEM_DEMO_ROUTE,
                ITEM_DEMO_DRIVE,
                0x40e6c0,
                ITEM_PHOTO,
                ITEM_POSTER,
            ],
            fade_targets: vec![(0x17, 0x40, Palette([0x00ff_8040; 256]))],
            logo: [128, 208, 318, 102, 640],
            photos: vec![(0x2e1, 240, 192)],
            posters: [(0x412, 26), (0x40f, 13)],
        }
    }

    fn run(a: &mut Attract, t: &AttractTables, confirm: bool) -> (AttractStep, Vec<Sfx>) {
        let mut l = SpriteLayer::default();
        let mut sfx = vec![];
        let input = MenuInput {
            confirm,
            ..MenuInput::NONE
        };
        (
            a.tick(&input, t, AttractEnv::default(), &mut l, &mut sfx),
            sfx,
        )
    }

    #[test]
    fn start_on_the_picture_goes_to_the_title() {
        let t = tables();
        let mut a = Attract::new(true, AttractCounters::default());
        run(&mut a, &t, false);
        assert!(a.showing_title());
        let mut call = false;
        for _ in 0..150 {
            call |= run(&mut a, &t, false)
                .1
                .contains(&Sfx::Play(SND_TITLE_CALL));
        }
        assert!(call);
        let walk = a.counters.walk;
        let (s, sfx) = run(&mut a, &t, true);
        assert_eq!(a.counters.walk, walk + 1);
        assert_eq!(
            (s, sfx),
            (
                AttractStep::Running,
                vec![Sfx::Play(snd::START), Sfx::Stop(OPENING_MUSIC)]
            )
        );
        assert_eq!(run(&mut a, &t, false).0, AttractStep::Running);
        assert_eq!(run(&mut a, &t, false).0, AttractStep::Title);
    }

    #[test]
    fn the_walk_runs_logo_title_demo_and_photos() {
        let t = tables();
        let mut a = Attract::new(false, AttractCounters::default());
        let mut horns = 0;
        let mut n = 0;
        while !a.showing_title() {
            let (step, sfx) = run(&mut a, &t, false);
            horns += sfx
                .iter()
                .filter(|s| **s == Sfx::Play(SND_LOGO_HORN))
                .count();
            if step == AttractStep::Opening {
                a.opening_done(false);
            }
            n += 1;
            assert!(n < 1000);
        }
        assert_eq!(horns, 2);
        // The logo: slide in (≈ 100), 20, slide out (≈ 22), 60, back (≈ 22), 20, off (≈ 100), 5.
        assert!((400..600).contains(&n), "{n}");
        while a.showing_title() {
            run(&mut a, &t, false);
        }
        let demo = loop {
            if let AttractStep::Demo { kind, stage } = run(&mut a, &t, false).0 {
                break (kind, stage);
            }
        };
        assert_eq!(demo, (DemoKind::Route, 1));
        for _ in 0..10 {
            assert_eq!(run(&mut a, &t, false).0, AttractStep::Running);
        }
        a.demo_done(false);
        let mut photo = 0;
        for _ in 0..400 {
            run(&mut a, &t, false);
            photo += usize::from(matches!(a.phase, Phase::Photo(_)));
        }
        assert_eq!(photo, PHOTO_LAST as usize);
        assert_eq!(a.counters.photo, 1);
    }

    #[test]
    fn start_in_the_logo_skips_to_the_logo_then_leaves() {
        let t = tables();
        let mut a = Attract::new(false, AttractCounters::default());
        run(&mut a, &t, false);
        run(&mut a, &t, false);
        run(&mut a, &t, true);
        assert!(matches!(a.phase, Phase::Logo(3, 0)));
        for _ in 0..70 {
            run(&mut a, &t, false);
        }
        run(&mut a, &t, true);
        assert!(a.skip_opening);
    }

    /// Ticks without input until `pred` holds, the owner's opening and demos running to
    /// their end at once.
    fn walk_to(a: &mut Attract, t: &AttractTables, pred: impl Fn(&Phase) -> bool) {
        for _ in 0..5000 {
            if pred(&a.phase) {
                return;
            }
            let step = run(a, t, false).0;
            if pred(&a.phase) {
                return;
            }
            match step {
                AttractStep::Opening => a.opening_done(false),
                AttractStep::Demo { .. } => a.demo_done(false),
                _ => {}
            }
        }
        panic!("not reached: {:?}", a.phase);
    }

    /// START now: the walk counts on and, within the abort gap, the title picture shows
    /// with no opening asked for (0x402700 sets `0x855ffc |= 0x10`; 0x402e80 then goes
    /// straight to 0x4037f0).
    fn start_lands_on_the_picture(a: &mut Attract, t: &AttractTables) {
        let walk = a.counters.walk;
        match a.phase {
            Phase::Opening(_) => a.opening_done(true),
            Phase::Demo(_) => a.demo_done(true),
            _ => assert_eq!(run(a, t, true).0, AttractStep::Running),
        }
        assert_eq!(a.counters.walk, walk + 1, "{:?}", a.phase);
        for _ in 0..ABORT_TICKS + 1 {
            assert_eq!(
                run(a, t, false).0,
                AttractStep::Running,
                "the opening again"
            );
        }
        assert!(a.showing_title(), "{:?}", a.phase);
    }

    /// START in every item but the title picture and the logo, in the first walk of a cold
    /// boot (after the opening has played) and after a restart: always the title picture,
    /// never the opening again.
    #[test]
    fn start_in_any_item_restarts_at_the_picture_without_the_opening() {
        let t = tables();
        type Pred = fn(&Phase) -> bool;
        let cases: [(&str, Pred); 5] = [
            ("opening", |p| matches!(p, Phase::Opening(_))),
            ("demo", |p| matches!(p, Phase::Demo(_))),
            ("photo", |p| matches!(p, Phase::Photo(50))),
            ("poster", |p| matches!(p, Phase::Poster(150))),
            (
                "gap",
                |p| matches!(p, Phase::Gap(1, n) if *n > RESTART_ITEM),
            ),
        ];
        for (name, pred) in cases {
            for cold in [true, false] {
                let mut a = Attract::new(!cold, AttractCounters::default());
                walk_to(&mut a, &t, pred);
                eprintln!("{name}, cold {cold}");
                start_lands_on_the_picture(&mut a, &t);
                // The restarted walk goes on from the picture, and START again in the same
                // item lands on the picture again.
                walk_to(&mut a, &t, |p| !matches!(p, Phase::Picture(_)));
                if name != "opening" {
                    walk_to(&mut a, &t, pred);
                    start_lands_on_the_picture(&mut a, &t);
                }
            }
        }
    }

    /// Without START the walk returns to item 0 at the table's end, and its title item plays
    /// the opening again (`0x855ffc & 0x10` clear: 0x402e80 consumed it).
    #[test]
    fn a_full_walk_plays_the_opening_again() {
        let t = tables();
        let mut a = Attract::new(true, AttractCounters::default());
        let mut openings = 0;
        let mut n = 0;
        while a.counters.walk == 0 || !a.showing_title() {
            n += 1;
            assert!(n < 5000);
            match run(&mut a, &t, false).0 {
                AttractStep::Opening => {
                    openings += 1;
                    a.opening_done(false);
                }
                AttractStep::Demo { .. } => a.demo_done(false),
                _ => {}
            }
        }
        assert_eq!(openings, 1);
        assert_eq!(a.counters.walk, 1);
    }

    /// START in the logo's states 4 to 7 ends the item with `0x855ffc |= 0x10`: the next
    /// item, the title, shows the picture at once.
    #[test]
    fn late_start_in_the_logo_goes_to_the_picture() {
        let t = tables();
        let mut a = Attract::new(false, AttractCounters::default());
        walk_to(&mut a, &t, |p| matches!(p, Phase::Logo(5, _)));
        let walk = a.counters.walk;
        run(&mut a, &t, true);
        for _ in 0..2 {
            assert_eq!(run(&mut a, &t, false).0, AttractStep::Running);
        }
        assert!(a.showing_title(), "{:?}", a.phase);
        // Not an abort of the walk: the counter stays.
        assert_eq!(a.counters.walk, walk);
    }

    #[test]
    fn demo_routes_follow_the_mask() {
        let r: Vec<usize> = (0..9).map(demo_route).collect();
        assert_eq!(r, [1, 3, 4, 6, 7, 8, 11, 12, 1]);
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

    /// Item 3 with every stage cleared: the movie, then the walk goes on (its end) or
    /// restarts at the title picture (a skip).
    #[test]
    fn the_movie_item_plays_and_skips_like_the_original() {
        let t = tables();
        let env = AttractEnv {
            komachi: true,
            difficulty: 0,
        };
        let mut l = SpriteLayer::default();
        let mut sfx = vec![];
        let none = MenuInput::NONE;
        let to_movie = |a: &mut Attract, l: &mut SpriteLayer, sfx: &mut Vec<Sfx>| {
            for _ in 0..400 {
                if a.tick(&none, &t, env, l, sfx) == AttractStep::KomachiMovie {
                    return;
                }
            }
            panic!("no movie");
        };
        // Natural end: the demo route item, then its drive.
        let mut a = Attract::new(true, AttractCounters::default());
        to_movie(&mut a, &mut l, &mut sfx);
        assert_eq!(
            a.tick(&none, &t, env, &mut l, &mut sfx),
            AttractStep::Running
        );
        a.movie_done(false, &mut l);
        let walk = a.counters.walk;
        let mut step = AttractStep::Running;
        for _ in 0..3 {
            step = a.tick(&none, &t, env, &mut l, &mut sfx);
            if step != AttractStep::Running {
                break;
            }
        }
        assert!(matches!(step, AttractStep::Demo { .. }), "{step:?}");
        assert_eq!(a.counters.walk, walk);
        // Skipped: the walk counts on and the title picture comes back.
        let mut a = Attract::new(true, AttractCounters::default());
        to_movie(&mut a, &mut l, &mut sfx);
        let walk = a.counters.walk;
        a.movie_done(true, &mut l);
        assert_eq!(a.counters.walk, walk + 1);
        for _ in 0..ABORT_TICKS + 1 {
            a.tick(&none, &t, env, &mut l, &mut sfx);
        }
        assert!(a.showing_title());
        // Without the clear marks there is no movie.
        let mut a = Attract::new(true, AttractCounters::default());
        for _ in 0..400 {
            let s = a.tick(&none, &t, AttractEnv::default(), &mut l, &mut sfx);
            assert_ne!(s, AttractStep::KomachiMovie);
        }
    }
}

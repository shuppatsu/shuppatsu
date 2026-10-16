//! Attract demo drives in the cab view (docs/re/menus.md §2): a [`Play`] whose drive runs
//! [`ddg_engine::drive::Drive::start_demo`] instead of the stage flow, with the attract
//! overlay: "スタートボタンで運転開始 / PUSH ENTER KEY" and "一日乗車券 FREE PLAY" blinking
//! (0x4023b0, task 0x4024a0), the 3000番台 badge, and in the route demo the GMQ logo animated
//! by the rail-joint clicks (0x402590, task 0x402630). START (the menu confirm edge) ends the
//! demo; so does the end of its task.

use super::Play;
use crate::cab::{Snap, TickInput};
use crate::slots;
use anyhow::Result;
use ddg_engine::demo::{DemoKind, DemoTables};
use ddg_engine::drive::DriverInput;
use ddg_formats::exe::Exe;
use ddg_formats::hud::{MODE_640, RES_MODES};

/// 0x4023b0 (free play, `0x8573e0 != 0`): slot 6 = tbl 0x25a at (0xe0, 0xac), slot 8 = tbl
/// 0x25b at (0xf0, 0xc4), palette 4; task 0x4024a0 shows slot 6 in the first and slot 8 in the
/// third 30-tick quarter of every 120 ticks.
const BLINK: [(usize, i16, (i16, i16)); 2] = [(6, 0x25a, (0xe0, 0xac)), (8, 0x25b, (0xf0, 0xc4))];
const BLINK_PERIOD: u32 = 0x1e;
/// The 3000番台 badge, slot 0x26 = tbl 0x123 at (0x4579a0, 0x4579b0)[res], set every update.
const SLOT_BADGE: usize = 0x26;
const TBL_BADGE: i16 = 0x123;
const BADGE_POS: [u32; 2] = [0x4579a0, 0x4579b0];
/// The GMQ logo (route demo only): slot 7 at (0x457980, 0x457990)[res], palette 0xb; frame
/// `[0x304, 0x305 ×3 pairs, 0x306, 0x307 ×3 pairs][0x84c3dc % 12]` (stack table of 0x402630).
const SLOT_LOGO: usize = 7;
const LOGO_POS: [u32; 2] = [0x457980, 0x457990];
const LOGO_FRAMES: [i16; 12] = [
    0x304, 0x305, 0x304, 0x305, 0x304, 0x305, 0x306, 0x307, 0x306, 0x307, 0x306, 0x307,
];
const PAL_LOGO: u8 = 0xb;
/// The lesson demo lights the departure lamp at its count 200 (0x40e860: `0x434fb0(1)`).
const LESSON_LAMP_AT: u32 = 200;

/// The demo's own state in [`Play`].
#[derive(Debug, Clone)]
pub(super) struct DemoView {
    kind: DemoKind,
    /// Overlay task counter (task+0x30 of 0x4024a0).
    blink: u32,
    badge: (i16, i16),
    logo: (i16, i16),
    /// START ended the demo.
    started: bool,
}

impl Play {
    /// Turns this freshly built run (station 0) into attract demo `kind`.
    pub fn start_demo(&mut self, kind: DemoKind) -> Result<()> {
        let exe_bytes = self.data.exe()?;
        let exe = Exe::parse(&exe_bytes)?;
        let tables = DemoTables::read(&exe)?;
        let pos = |va: [u32; 2]| -> Result<(i16, i16)> {
            let x = exe.reader_at(va[0], 4 * RES_MODES, "demo overlay")?;
            let y = exe.reader_at(va[1], 4 * RES_MODES, "demo overlay")?;
            Ok((x.i32(4 * MODE_640)? as i16, y.i32(4 * MODE_640)? as i16))
        };
        self.drive.start_demo(kind, &tables);
        self.cab
            .demo_setup(&Snap::of(&self.drive), kind == DemoKind::Route);
        for (slot, _, (x, y)) in BLINK {
            slots::place(&mut self.layer, slot, x, y, 3, 4);
        }
        let logo = pos(LOGO_POS)?;
        if kind == DemoKind::Route {
            slots::place(&mut self.layer, SLOT_LOGO, logo.0, logo.1, 3, PAL_LOGO);
        }
        self.demo = Some(DemoView {
            kind,
            blink: 0,
            badge: pos(BADGE_POS)?,
            logo,
            started: false,
        });
        Ok(())
    }

    /// The demo was ended by START (the owner goes to the title picture).
    pub fn demo_ended_by_start(&self) -> bool {
        self.demo.as_ref().is_some_and(|d| d.started)
    }

    /// One tick of a demo drive.
    pub(super) fn demo_tick(&mut self) -> Result<()> {
        self.ticks += 1;
        let Some(mut d) = self.demo.take() else {
            return Ok(());
        };
        if self.menu_input.confirm || self.drive.demo_finished() {
            // START clears 0x855ff8 & 4; the demo task ends and its destroy 0x40e670 stops
            // the sounds and clears the sprites.
            d.started |= self.menu_input.confirm;
            if let Some(s) = self.sound.as_mut() {
                s.stop_all();
            }
            self.leave = true;
            self.demo = Some(d);
            return Ok(());
        }
        let before = crate::sound::Before::of(&self.drive);
        let out = self.drive.tick(DriverInput::default());
        let controls = self.drive.demo_controls().unwrap_or_default();
        self.scene.traffic = self.drive.traffic_poses(1.0);
        self.scene.signal_model = usize::from(self.drive.rules().signal_model);
        if let Some(s) = self.sound.as_mut() {
            s.events(&out.events);
            s.after_tick(&self.drive, before, controls.horn_held, false);
        }
        if d.kind == DemoKind::Lesson && self.drive.demo_counter() == Some(LESSON_LAMP_AT + 1) {
            self.cab.light_lamp();
        }
        self.interpolate = true;
        self.last_events = out.events;
        self.poll();
        self.demo_overlay(&mut d);
        self.update_hud(TickInput {
            tolerance_m: self.tolerance_m,
            ..TickInput::default()
        });
        self.demo = Some(d);
        Ok(())
    }

    /// The overlay tasks' updates (0x4024a0, 0x402630).
    fn demo_overlay(&mut self, d: &mut DemoView) {
        let quarter = (d.blink / BLINK_PERIOD) % 4;
        let l = &mut self.layer;
        match quarter {
            0 => slots::image(l, BLINK[0].0, BLINK[0].1),
            1 => slots::image(l, BLINK[0].0, -1),
            2 => slots::image(l, BLINK[1].0, BLINK[1].1),
            _ => slots::image(l, BLINK[1].0, -1),
        }
        slots::set(l, SLOT_BADGE, TBL_BADGE, d.badge, 4, 3);
        if d.kind == DemoKind::Route {
            let clicks = self
                .sound
                .as_ref()
                .and_then(|s| s.joint_clicks())
                .unwrap_or(0);
            let frame = LOGO_FRAMES[(clicks % 12) as usize];
            slots::set(l, SLOT_LOGO, frame, d.logo, PAL_LOGO, 3);
        }
        d.blink += 1;
    }
}

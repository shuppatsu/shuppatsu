//! `ddg-game play`: drive a stage. The simulation is `ddg_engine::drive` at the original's
//! fixed 33 ms tick; rendering interpolates the camera along the track between the last two
//! ticks. Menus and result screens are not built yet: arrivals advance after a short hold, and
//! Enter continues after a game over.

use crate::hud::{Hud, HudState};
use crate::input::{Bindings, ButtonState, Keyboard};
use crate::scene::{GameData, RouteScene, Visibility};
use crate::window::Gpu;
use anyhow::{Context, Result};
use ddg_engine::drive::{Drive, DrivePhase, DriverInput, LoadError};
use ddg_engine::physics::TrainTables;
use ddg_engine::rules::RuleTables;
use ddg_formats::exe::Exe;
use ddg_formats::route::UNITS_PER_POINT;
use ddg_formats::scene::{DRIVING_FOV_Y_DEG, TrackPos};
use ddg_formats::stages::StageTables;
use ddg_render::FrameGeometry;
use ddg_render::camera::ORIGINAL_STRETCH;
use ddg_render::sprites::{CANVAS_H, CANVAS_W, SpriteBank, SpriteLayer};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::WindowId;

/// The original's fixed tick (timeSetEvent(33, …) at 0x436f30).
const TICK: Duration = Duration::from_millis(33);
/// The original's 3D viewport starts at virtual y 52 of 400 (renderer.md §5).
pub const HUD_BAND_TOP: f32 = 52.0 / 400.0;
/// Stand-in for the station result screen until it exists: ticks before `finish_station`.
const RESULT_HOLD_TICKS: u32 = 150;
/// Visibility is widened for at least this aspect, so resizing up to 21:9 needs no rebuild.
const MIN_WIDE_ASPECT: f32 = 21.0 / 9.0;

pub struct PlayOptions {
    pub stage: usize,
    pub difficulty: usize,
    pub station: usize,
    pub keyboard_mode: usize,
    pub size: (u32, u32),
    pub uncapped: bool,
    pub square_pixels: bool,
    pub original_visibility: bool,
    pub stable_track_variants: bool,
    pub texture_filtering: bool,
    pub fog: bool,
}

pub fn run(opts: PlayOptions) -> Result<()> {
    let data = GameData::from_env()?;
    let exe_bytes = data.exe()?;
    let exe = Exe::parse(&exe_bytes)?;
    let stages = StageTables::read(&exe)?;
    let train_tables = TrainTables::read(&exe)?;
    let rule_tables = RuleTables::read(&exe)?;
    let tolerance_m = rule_tables
        .stop_tolerance_m
        .get(opts.stage, opts.difficulty);
    let loader_data = GameData::from_env()?;
    let loader = move |name: &str| -> Result<Vec<u8>, LoadError> {
        loader_data
            .read(name)
            .map_err(|e| LoadError::from(format!("{e:#}")))
    };
    let drive = Drive::new(
        &train_tables,
        &rule_tables,
        &stages,
        opts.stage,
        opts.difficulty,
        opts.station,
        loader,
    )
    .context("starting the stage")?;
    eprintln!(
        "stage {} difficulty {} from station {} ({})",
        opts.stage,
        opts.difficulty,
        opts.station,
        drive.station_name(opts.station).unwrap_or("?")
    );

    let keyboard = Keyboard::new(&Bindings::read(&exe)?, opts.keyboard_mode)
        .context("keyboard modes are 0..=3")?;
    let cg_bytes = data.read("cg.rom")?;
    let bank = SpriteBank::new(
        &ddg_formats::cg::CgFile::parse(&cg_bytes)?,
        ddg_formats::cg::parse_tbl(&data.read("tbl.rom")?)?,
    );
    let mut layer = SpriteLayer::default();
    Hud::install_palettes(&exe, &mut layer)?;
    let hud = Hud::new(&exe, opts.stage)?;
    hud.create(&mut layer);

    let visibility = visibility(&opts);
    let scene = load_scene(&data, drive.com_name(), visibility, &opts)?;
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let now = Instant::now();
    let mut app = Play {
        scene_com: drive.com(),
        drive,
        data,
        visibility,
        scene,
        keyboard,
        latched: DriverInput::default(),
        down: [false; 256],
        hud,
        layer,
        bank,
        canvas: vec![0; CANVAS_W * CANVAS_H * 4],
        canvas_dirty: true,
        panel_on: true,
        tolerance_m,
        hold: 0,
        paused: false,
        interpolate: false,
        frame: FrameGeometry::default(),
        gpu: None,
        accumulator: Duration::ZERO,
        last_frame: now,
        title_at: now,
        error: None,
        opts,
    };
    event_loop.run_app(&mut app)?;
    match app.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn visibility(opts: &PlayOptions) -> Visibility {
    if opts.original_visibility {
        return Visibility::Original;
    }
    let mut p = ddg_render::Projection {
        fov_tenths: (DRIVING_FOV_Y_DEG * 10.0) as u16,
        ..Default::default()
    };
    if opts.square_pixels {
        p.stretch = 1.0;
    }
    let band_aspect = opts.size.0 as f32 / (opts.size.1 as f32 * (1.0 - HUD_BAND_TOP));
    Visibility::Wide {
        tan_half_fov_x: p.tan_half_horizontal(band_aspect.max(MIN_WIDE_ASPECT)),
    }
}

fn load_scene(
    data: &GameData,
    com: &str,
    visibility: Visibility,
    opts: &PlayOptions,
) -> Result<RouteScene> {
    let t = Instant::now();
    let mut scene = RouteScene::load(data, com, visibility)?;
    scene.stable_track_variants = opts.stable_track_variants;
    eprintln!("{com}: loaded in {:.2?}", t.elapsed());
    Ok(scene)
}

/// Track units from the main-line origin.
fn units(p: TrackPos) -> f32 {
    p.engine_index as f32 * UNITS_PER_POINT + p.t
}

fn track_pos(units: f32) -> TrackPos {
    let engine_index = (units / UNITS_PER_POINT).floor().max(0.0) as usize;
    TrackPos {
        engine_index,
        t: units - engine_index as f32 * UNITS_PER_POINT,
    }
}

struct Play {
    opts: PlayOptions,
    data: GameData,
    drive: Drive,
    visibility: Visibility,
    scene: RouteScene,
    scene_com: u32,
    keyboard: Keyboard,
    /// Controls polled at the end of the last tick (the physics reads them this tick).
    latched: DriverInput,
    /// Keys down now, by DirectInput scan code.
    down: [bool; 256],
    hud: Hud,
    layer: SpriteLayer,
    bank: SpriteBank,
    canvas: Vec<u8>,
    canvas_dirty: bool,
    /// Distance panel shown (1) or "Professional" plate (0), toggled by Enter (`0x856c70`).
    panel_on: bool,
    tolerance_m: i32,
    /// Ticks spent in the current arrival hold.
    hold: u32,
    paused: bool,
    /// False right after a segment change: the position jumped, don't interpolate.
    interpolate: bool,
    frame: FrameGeometry,
    gpu: Option<Gpu>,
    accumulator: Duration,
    last_frame: Instant,
    title_at: Instant,
    error: Option<anyhow::Error>,
}

impl Play {
    fn tick(&mut self) -> Result<()> {
        let out = self.drive.tick(self.latched);
        self.interpolate = !out.segment_started;
        if out.segment_started {
            // Digital modes force P0 / EB at each station start (0x4332c0, input.md §4.1).
            self.keyboard.reset_levers();
        }
        for e in &out.events {
            eprintln!("[{}] {e:?}", self.drive.tick_count());
        }

        // The input poll at the end of `Task_Tick`: next tick's controls.
        let down = self.down;
        let buttons = self.keyboard.poll(|c| down[usize::from(c)]);
        self.latched = DriverInput {
            power: self.keyboard.power,
            brake: self.keyboard.brake,
            horn_held: buttons.horn_held,
            horn_pressed: buttons.horn_pressed,
            confirm_pressed: buttons.confirm_pressed,
        };
        self.flow(buttons)?;

        if self.drive.com() != self.scene_com {
            self.scene = load_scene(
                &self.data,
                self.drive.com_name(),
                self.visibility,
                &self.opts,
            )?;
            self.scene_com = self.drive.com();
            self.interpolate = false;
        }
        self.update_hud(buttons);
        Ok(())
    }

    /// Stand-ins for the result and continue screens.
    fn flow(&mut self, buttons: ButtonState) -> Result<()> {
        match self.drive.phase() {
            DrivePhase::Arrived => {
                self.hold += 1;
                if self.hold >= RESULT_HOLD_TICKS {
                    self.hold = 0;
                    let r = self.drive.finish_station()?;
                    eprintln!("station result: {r:?}");
                }
            }
            DrivePhase::GameOver if buttons.confirm_pressed => {
                if self.drive.can_continue() {
                    eprintln!("continue");
                    self.drive.continue_run()?;
                } else {
                    eprintln!("game over, no continue");
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn update_hud(&mut self, buttons: ButtonState) {
        let rules = self.drive.rules();
        let phase = self.drive.phase();
        let driving = phase == DrivePhase::Driving;
        if driving && buttons.confirm_pressed && rules.ats == 0 {
            self.panel_on = !self.panel_on;
        }
        let train = self.drive.train();
        let state = HudState {
            power: u8::try_from(train.prev_power).unwrap_or(0),
            brake: u8::try_from(train.prev_brake).unwrap_or(0),
            remaining_s: rules.remaining_s,
            pending_s: rules.pending_s,
            hud_counter: rules.hud_counter,
            clock: rules.clock,
            scheduled: rules.scheduled,
            clocks_visible: !matches!(phase, DrivePhase::Loading | DrivePhase::Cleared),
            remaining_visible: phase != DrivePhase::Cleared,
            panel_active: driving,
            panel_on: self.panel_on,
            next_is_stop: rules.target_stops,
            dist_m: self.drive.dist_to_stop_m(),
            dist_round_m: rules.dist_round_m,
            tolerance_m: self.tolerance_m,
            speed_kmh: self.drive.speed_kmh(),
        };
        self.layer.reset_transient();
        self.hud.update(&state, &mut self.layer);
        self.layer.compose(&self.bank, &mut self.canvas);
        self.canvas_dirty = true;
    }

    fn redraw(&mut self) -> Result<()> {
        let now = Instant::now();
        let dt = now - self.last_frame;
        self.last_frame = now;
        if !self.paused {
            self.accumulator += dt.min(Duration::from_millis(250));
            while self.accumulator >= TICK {
                self.accumulator -= TICK;
                self.tick()?;
            }
        }
        // Interpolate the track distance between the last two ticks (along the track, so the
        // camera follows curves), then evaluate the original's cab camera there.
        let (prev, cur) = (units(self.drive.prev_pos()), units(self.drive.pos()));
        let alpha = self.accumulator.as_secs_f32() / TICK.as_secs_f32();
        let pos = if self.interpolate {
            track_pos(prev + (cur - prev) * alpha)
        } else {
            self.drive.pos()
        };

        let Some(gpu) = self.gpu.as_mut() else {
            return Ok(());
        };
        let (width, height) = gpu.size();
        let band_aspect = width as f32 / (height as f32 * (1.0 - HUD_BAND_TOP)).max(1.0);
        self.scene.compose(&mut self.frame, pos, false, band_aspect);
        let camera = self.scene.cab_camera(pos);
        let Some(frame) = gpu.acquire() else {
            return Ok(());
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        gpu.renderer
            .set_geometry(&gpu.device, &gpu.queue, &self.frame);
        gpu.renderer
            .render(&gpu.device, &gpu.queue, &view, (width, height), &camera);
        if std::mem::take(&mut self.canvas_dirty) {
            gpu.overlay.upload(&gpu.queue, &self.canvas);
        }
        gpu.overlay.render(
            &gpu.device,
            &gpu.queue,
            &view,
            ddg_render::overlay::canvas_rect(width, height),
        );
        gpu.window.pre_present_notify();
        gpu.queue.present(frame);

        if now - self.title_at >= Duration::from_millis(500) {
            self.title_at = now;
            let (station, _) = self.drive.next_station();
            gpu.window.set_title(&format!(
                "ddg-game play | {:?}{} | {:.1} km/h | next {} | {} m",
                self.drive.phase(),
                if self.paused { " (paused)" } else { "" },
                self.drive.speed_kmh(),
                self.drive.station_name(station).unwrap_or("?"),
                self.drive.rules().dist_round_m,
            ));
        }
        Ok(())
    }

    fn init_gpu(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let mut gpu = Gpu::create(
            event_loop,
            "ddg-game play",
            self.opts.size,
            self.opts.uncapped,
        )?;
        let r = &mut gpu.renderer;
        r.texture_filtering = self.opts.texture_filtering;
        r.fog.enabled = self.opts.fog;
        r.projection.fov_tenths = (DRIVING_FOV_Y_DEG * 10.0) as u16;
        r.projection.stretch = if self.opts.square_pixels {
            1.0
        } else {
            ORIGINAL_STRETCH
        };
        r.band_top = HUD_BAND_TOP;
        gpu.renderer
            .upload_pages(&gpu.device, &gpu.queue, &self.scene.library.pages);
        self.gpu = Some(gpu);
        Ok(())
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, e: anyhow::Error) {
        self.error = Some(e);
        event_loop.exit();
    }
}

impl ApplicationHandler for Play {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        match self.init_gpu(event_loop) {
            Ok(()) => {
                self.last_frame = Instant::now();
                if let Some(g) = &self.gpu {
                    g.window.request_redraw();
                }
            }
            Err(e) => self.fail(event_loop, e.context("GPU init")),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(g) = self.gpu.as_mut() {
                    g.resize(size.width, size.height);
                }
            }
            WindowEvent::Focused(false) => self.down = [false; 256],
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        physical_key: PhysicalKey::Code(code),
                        state,
                        repeat,
                        ..
                    },
                ..
            } => {
                let pressed = state == ElementState::Pressed;
                if pressed && !repeat {
                    match code {
                        // Esc pauses (input.md §6.1); Q quits while paused.
                        KeyCode::Escape => self.paused = !self.paused,
                        KeyCode::KeyQ if self.paused => event_loop.exit(),
                        _ => {}
                    }
                }
                if code != KeyCode::Escape
                    && let Some(d) = crate::input::dik(code)
                {
                    self.down[usize::from(d)] = pressed;
                }
            }
            WindowEvent::RedrawRequested => {
                if let Err(e) = self.redraw() {
                    self.fail(event_loop, e);
                    return;
                }
                if let Some(g) = &self.gpu {
                    g.window.request_redraw();
                }
            }
            _ => {}
        }
    }
}

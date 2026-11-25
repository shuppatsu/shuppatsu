//! The game-data setup screen (port only; docs/re/disc-import.md): shown instead of the game
//! when no game data is found, so a player can pick their disc image, a copy of the disc or an
//! installed game without the command line.
//!
//! Nothing of the original can be drawn yet (its art and sounds are the data being looked
//! for), so the screen uses the port's own look: the station-board logo, the bundled M+ font,
//! white and grey text with gold headings on black, as the language and port settings screens.
//! It is drawn on the CPU into one RGBA image the size of the window and shown with an
//! [`Overlay`].
//!
//! A small file browser lists folders, disc images and the places to start from (home, the
//! system's removable media, the drives on Windows); a folder that is a copy of the disc or an
//! installed game gets a "Use this folder" row. Keys: arrows, Page Up/Down, Home/End, Enter
//! (or Right) to open or use, Backspace (or Left) for the parent folder, a letter to jump,
//! Esc to quit. A controller (any gilrs pad): D-pad or left stick to choose (held: repeats),
//! A / Cross or Start to open or use, B / Circle for the parent folder, the shoulders to page,
//! View / Share to quit; the hints name PlayStation buttons when `RECOMPLAUNCHER_PAD` says so.
//! The mouse: point, click to select, double-click to open, wheel to scroll. A
//! file or folder dropped onto the window is used directly. The import itself
//! ([`crate::import`]) runs on a worker thread behind a progress bar.

use crate::import::{self, Progress, Source};
use crate::window::Gpu;
use anyhow::Result;
use shuppatsu_render::overlay::{Overlay, UiScaling};
use shuppatsu_render::rgba::RgbaImage;
use shuppatsu_render::text::{Align, FontGlyphs, rasterise_sized};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};

/// The layout's design size; it is scaled to the window's height (and narrower windows).
const DESIGN: (f32, f32) = (640.0, 480.0);
/// The list's first row, row height, and rows shown.
const LIST_Y: f32 = 184.0;
const ROW_H: f32 = 16.0;
const ROWS: usize = 15;
/// The list's left and right edges.
const LIST_X: (f32, f32) = (36.0, 604.0);
const DOUBLE_CLICK: Duration = Duration::from_millis(450);
/// A held D-pad direction or stick: first repeat after this, then every [`PAD_REPEAT`].
const PAD_DELAY: Duration = Duration::from_millis(400);
const PAD_REPEAT: Duration = Duration::from_millis(90);
/// How far the stick must be pushed to step (of 1).
const PAD_STICK: f32 = 0.5;

/// The screen's string IDs in the catalogues' `[ui]` sections.
pub const UI_IDS: &[&str] = &[
    "setup_title",
    "setup_intro",
    "setup_intro2",
    "setup_places",
    "setup_empty",
    "setup_keys",
    "setup_mouse",
    "setup_pad",
    "setup_pad_ps",
    "setup_importing",
    "setup_checking",
    "setup_failed",
    "setup_back",
    "setup_done",
    "setup_use_here",
    "setup_tag_installed",
    "setup_tag_disc",
    "setup_tag_parent",
    "setup_tag_image",
    "setup_tag_archive",
    "setup_archive",
    "setup_unknown",
];

const WHITE: [u8; 3] = [255, 255, 255];
const GREY: [u8; 3] = [140, 140, 140];
const DARK: [u8; 3] = [90, 90, 90];
/// Headings and the path line: the gold of the menus' legend arrows.
const GOLD: [u8; 3] = [240, 192, 64];
/// The cursor bar: the blue of the options header's stripe.
const BAR: [u8; 3] = [30, 60, 150];
const PANEL: [u8; 3] = [16, 24, 40];
const RED: [u8; 3] = [255, 110, 90];

/// One row of the browser.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Row {
    /// The folder being shown is usable as it is.
    UseHere(Source),
    Parent,
    /// A start place (the root view): label and folder.
    Place(String, PathBuf),
    Dir(String),
    /// A file, with what it is.
    File(String, FileKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileKind {
    Image,
    Archive,
    Other,
}

/// What a key or button does on the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Up,
    Down,
    PageUp,
    PageDown,
    First,
    Last,
    /// Open a folder, use a file or folder, dismiss a message.
    Activate,
    /// The parent folder, or dismiss a message.
    Back,
    Quit,
    /// The next row starting with this letter.
    Jump(char),
}

/// The controllers (gilrs, as the game's own input): any pad steers the screen.
struct Pad {
    gilrs: gilrs::Gilrs,
    /// The direction held and when it repeats next.
    held: Option<(Action, Instant)>,
}

/// The screen's state.
enum Phase {
    Browse,
    Importing {
        progress: Arc<Mutex<Progress>>,
        worker: JoinHandle<Result<PathBuf>>,
        installed: bool,
    },
    /// A message, then back to browsing.
    Failed(String),
    /// Done: the game can start.
    Done,
}

/// The setup screen and its window.
pub struct Setup {
    gpu: Option<Gpu>,
    overlay: Option<Overlay>,
    size: (u32, u32),
    fullscreen: bool,
    ui: crate::front::port_settings::UiText,
    font: Option<FontGlyphs>,
    board: RgbaImage,
    /// `None`: the places view.
    dir: Option<PathBuf>,
    rows: Vec<Row>,
    cursor: usize,
    top: usize,
    phase: Phase,
    /// Mouse position in window pixels, and the last click (row, when).
    mouse: (f64, f64),
    last_click: Option<(usize, Instant)>,
    /// `None` when gilrs can't start (no access to input devices).
    pad: Option<Pad>,
    /// The button names to show: PlayStation's when the launcher says that pad is plugged in
    /// (`RECOMPLAUNCHER_PAD`), else Xbox's.
    playstation: bool,
    /// The window wants a redraw.
    dirty: bool,
    /// Set when the player quits from here.
    pub quit: bool,
}

impl Setup {
    /// `lang`: the language for the screen's text (`en`, `ja`).
    pub fn new(size: (u32, u32), fullscreen: bool, lang: &str) -> Self {
        let mut s = Self {
            gpu: None,
            overlay: None,
            size,
            fullscreen,
            ui: crate::front::port_settings::UiText::load(lang, None),
            font: crate::locale::bundled_font().ok(),
            board: crate::logo::board_rgba(),
            dir: None,
            rows: Vec::new(),
            cursor: 0,
            top: 0,
            phase: Phase::Browse,
            mouse: (-1.0, -1.0),
            last_click: None,
            pad: gilrs::Gilrs::new()
                .map_err(|e| eprintln!("setup screen: no controllers ({e})"))
                .ok()
                .map(|gilrs| Pad { gilrs, held: None }),
            playstation: std::env::var("RECOMPLAUNCHER_PAD").is_ok_and(|p| p == "playstation"),
            dirty: true,
            quit: false,
        };
        let start = home().filter(|h| h.is_dir());
        s.open(start);
        s
    }

    /// Whether the data is in place and the game can start.
    pub fn done(&self) -> bool {
        matches!(self.phase, Phase::Done)
    }

    /// Opens the window (first `resumed`).
    pub fn resumed(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        if self.gpu.is_some() {
            return Ok(());
        }
        let window = Gpu::open_window(event_loop, "shuppatsu", self.size, self.fullscreen)?;
        window.set_cursor_visible(true);
        let gpu = Gpu::with_window(window, false, 1)?;
        let mut overlay = Overlay::new(&gpu.device, gpu.config.format);
        overlay.scaling = UiScaling::Nearest;
        gpu.window.request_redraw();
        self.overlay = Some(overlay);
        self.gpu = Some(gpu);
        Ok(())
    }

    /// Closes the window (before the game opens its own).
    pub fn close(&mut self) {
        self.overlay = None;
        self.gpu = None;
    }

    pub fn window_event(&mut self, event: &WindowEvent) {
        if let Some(gpu) = self.gpu.as_mut()
            && gpu.fullscreen_key(event).is_some()
        {
            self.dirty = true;
            return;
        }
        match event {
            WindowEvent::CloseRequested => self.quit = true,
            WindowEvent::Resized(size) => {
                if let Some(gpu) = self.gpu.as_mut() {
                    gpu.resize(size.width, size.height);
                }
                self.dirty = true;
            }
            WindowEvent::RedrawRequested => {
                if let Err(e) = self.draw() {
                    eprintln!("setup screen: {e:#}");
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                self.key(event);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse = (position.x, position.y);
                if matches!(self.phase, Phase::Browse)
                    && let Some(i) = self.row_at(self.mouse)
                    && i != self.cursor
                {
                    self.cursor = i;
                    self.dirty = true;
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => self.click(),
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -y.round() as i32 * 3,
                    MouseScrollDelta::PixelDelta(p) => -(p.y / 40.0).round() as i32,
                };
                self.scroll(lines);
            }
            WindowEvent::DroppedFile(path) => self.choose(path.clone()),
            _ => {}
        }
    }

    /// Per loop turn: follows the import and redraws when needed.
    pub fn about_to_wait(&mut self) {
        self.poll_pad();
        if let Phase::Importing { worker, .. } = &self.phase {
            if worker.is_finished() {
                let Phase::Importing {
                    worker, installed, ..
                } = std::mem::replace(&mut self.phase, Phase::Browse)
                else {
                    unreachable!()
                };
                self.phase = match worker.join() {
                    Ok(Ok(dir)) => {
                        if installed {
                            remember(&dir);
                        }
                        Phase::Done
                    }
                    Ok(Err(e)) => Phase::Failed(format!("{e:#}")),
                    Err(_) => Phase::Failed("the import stopped unexpectedly".into()),
                };
            }
            self.dirty = true;
        }
        if self.dirty
            && let Some(gpu) = &self.gpu
        {
            gpu.window.request_redraw();
        }
    }

    // --- browsing -------------------------------------------------------------------------

    /// Shows `dir` (`None`: the places).
    fn open(&mut self, dir: Option<PathBuf>) {
        let previous = self.dir.clone();
        self.rows = match &dir {
            None => places()
                .into_iter()
                .map(|(label, path)| Row::Place(label, path))
                .collect(),
            Some(d) => listing(d),
        };
        self.dir = dir;
        // Coming back up: the cursor on the folder we were in.
        self.cursor = previous
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|name| {
                let name = name.to_string_lossy();
                self.rows
                    .iter()
                    .position(|r| matches!(r, Row::Dir(n) if *n == name))
            })
            .unwrap_or(0);
        self.top = self.cursor.saturating_sub(ROWS / 2);
        self.clamp_view();
        self.dirty = true;
    }

    fn up(&mut self) {
        let parent = self
            .dir
            .as_ref()
            .and_then(|d| d.parent())
            .map(Path::to_owned);
        match (&self.dir, parent) {
            (None, _) => {}
            (Some(_), Some(p)) => self.open(Some(p)),
            (Some(_), None) => self.open(None),
        }
    }

    fn activate(&mut self) {
        let Some(row) = self.rows.get(self.cursor).cloned() else {
            return;
        };
        match row {
            Row::UseHere(source) => self.start(source),
            Row::Parent => self.up(),
            Row::Place(_, path) => self.open(Some(path)),
            Row::Dir(name) => {
                if let Some(d) = &self.dir {
                    self.open(Some(d.join(name)));
                }
            }
            Row::File(name, _) => {
                if let Some(d) = &self.dir {
                    self.choose(d.join(name));
                }
            }
        }
    }

    /// A file or folder picked (or dropped): imported when usable, else explained; a plain
    /// folder is opened.
    fn choose(&mut self, path: PathBuf) {
        if !matches!(self.phase, Phase::Browse | Phase::Failed(_)) {
            return;
        }
        self.phase = Phase::Browse;
        let source = import::classify(&path);
        match source {
            s if s.usable() => self.start(s),
            Source::Unknown if path.is_dir() => self.open(Some(path)),
            Source::Archive(ext) => {
                self.phase = Phase::Failed(self.ui.get("setup_archive").replace("{ext}", &ext));
            }
            _ => self.phase = Phase::Failed(self.ui.get("setup_unknown").to_owned()),
        }
        self.dirty = true;
    }

    fn start(&mut self, source: Source) {
        let installed = matches!(source, Source::Installed(_));
        let progress = Arc::new(Mutex::new(Progress::default()));
        let shared = progress.clone();
        let worker = std::thread::spawn(move || {
            import::import(&source, &mut |p| {
                *shared.lock().unwrap_or_else(|e| e.into_inner()) = p.clone();
            })
        });
        self.phase = Phase::Importing {
            progress,
            worker,
            installed,
        };
        self.dirty = true;
    }

    fn key(&mut self, event: &KeyEvent) {
        let PhysicalKey::Code(code) = event.physical_key else {
            return;
        };
        let action = match code {
            KeyCode::ArrowUp => Action::Up,
            KeyCode::ArrowDown => Action::Down,
            KeyCode::PageUp => Action::PageUp,
            KeyCode::PageDown => Action::PageDown,
            KeyCode::Home => Action::First,
            KeyCode::End => Action::Last,
            KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::ArrowRight | KeyCode::Space => {
                Action::Activate
            }
            KeyCode::Backspace | KeyCode::ArrowLeft => Action::Back,
            KeyCode::Escape => Action::Quit,
            _ => match &event.logical_key {
                Key::Character(c) => match c.chars().next() {
                    Some(ch) => Action::Jump(ch),
                    None => return,
                },
                Key::Named(NamedKey::Space) => Action::Activate,
                _ => return,
            },
        };
        self.act(action);
    }

    /// One action from the keyboard or a controller.
    fn act(&mut self, action: Action) {
        match &self.phase {
            // A message: confirm, back or quit dismisses it (quit too, so a stray press
            // never closes the game from an error).
            Phase::Failed(_) => {
                if matches!(action, Action::Activate | Action::Back | Action::Quit) {
                    self.phase = Phase::Browse;
                    self.dirty = true;
                }
                return;
            }
            Phase::Importing { .. } | Phase::Done => return,
            Phase::Browse => {}
        }
        let last = self.rows.len().saturating_sub(1);
        match action {
            Action::Up => self.cursor = self.cursor.saturating_sub(1),
            Action::Down => self.cursor = (self.cursor + 1).min(last),
            Action::PageUp => self.cursor = self.cursor.saturating_sub(ROWS - 1),
            Action::PageDown => self.cursor = (self.cursor + ROWS - 1).min(last),
            Action::First => self.cursor = 0,
            Action::Last => self.cursor = last,
            Action::Activate => self.activate(),
            Action::Back => self.up(),
            Action::Quit => self.quit = true,
            Action::Jump(ch) => self.jump(ch),
        }
        self.clamp_view();
        self.dirty = true;
    }

    /// The controllers' events since the last turn, and the held direction's repeat.
    fn poll_pad(&mut self) {
        let Some(pad) = self.pad.as_mut() else {
            return;
        };
        let mut actions = Vec::new();
        let now = Instant::now();
        while let Some(gilrs::Event { event, .. }) = pad.gilrs.next_event() {
            use gilrs::{Axis, Button, EventType};
            match event {
                EventType::ButtonPressed(b, _) => {
                    let held = match b {
                        Button::DPadUp => Some(Action::Up),
                        Button::DPadDown => Some(Action::Down),
                        Button::LeftTrigger | Button::LeftTrigger2 => Some(Action::PageUp),
                        Button::RightTrigger | Button::RightTrigger2 => Some(Action::PageDown),
                        _ => None,
                    };
                    if let Some(a) = held {
                        pad.held = Some((a, now + PAD_DELAY));
                        actions.push(a);
                    }
                    match b {
                        Button::South | Button::Start | Button::DPadRight => {
                            actions.push(Action::Activate)
                        }
                        Button::East | Button::DPadLeft => actions.push(Action::Back),
                        Button::Select => actions.push(Action::Quit),
                        _ => {}
                    }
                }
                EventType::ButtonReleased(..) | EventType::Disconnected => pad.held = None,
                EventType::AxisChanged(Axis::LeftStickY, v, _) => {
                    // Up is positive; a half push steps, holding repeats.
                    let dir = if v > PAD_STICK {
                        Some(Action::Up)
                    } else if v < -PAD_STICK {
                        Some(Action::Down)
                    } else {
                        None
                    };
                    match (dir, pad.held) {
                        (Some(a), Some((h, _))) if a == h => {}
                        (Some(a), _) => {
                            pad.held = Some((a, now + PAD_DELAY));
                            actions.push(a);
                        }
                        (None, Some((Action::Up | Action::Down, _))) => pad.held = None,
                        (None, _) => {}
                    }
                }
                _ => {}
            }
        }
        if let Some((a, next)) = pad.held
            && now >= next
        {
            pad.held = Some((a, now + PAD_REPEAT));
            actions.push(a);
        }
        for a in actions {
            self.act(a);
        }
    }

    /// Whether a controller is being read (the event loop then wakes to poll it).
    pub fn wants_polling(&self) -> bool {
        self.pad.is_some()
    }

    /// Moves to the next row whose name starts with `ch` (case-insensitive), wrapping.
    fn jump(&mut self, ch: char) {
        let ch = ch.to_lowercase().next().unwrap_or(ch);
        let n = self.rows.len();
        for k in 1..=n {
            let i = (self.cursor + k) % n;
            let name = match &self.rows[i] {
                Row::Dir(s) | Row::File(s, _) | Row::Place(s, _) => s,
                _ => continue,
            };
            if name.to_lowercase().starts_with(ch) {
                self.cursor = i;
                return;
            }
        }
    }

    fn scroll(&mut self, lines: i32) {
        if !matches!(self.phase, Phase::Browse) {
            return;
        }
        let max_top = self.rows.len().saturating_sub(ROWS);
        self.top = (self.top as i32 + lines).clamp(0, max_top as i32) as usize;
        self.cursor = self.cursor.clamp(
            self.top,
            (self.top + ROWS - 1).min(self.rows.len().saturating_sub(1)),
        );
        self.dirty = true;
    }

    fn clamp_view(&mut self) {
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if self.cursor >= self.top + ROWS {
            self.top = self.cursor + 1 - ROWS;
        }
        self.top = self.top.min(self.rows.len().saturating_sub(ROWS));
    }

    fn click(&mut self) {
        match self.phase {
            Phase::Failed(_) => {
                self.phase = Phase::Browse;
                self.dirty = true;
            }
            Phase::Browse => {
                let Some(i) = self.row_at(self.mouse) else {
                    return;
                };
                let now = Instant::now();
                let double = self
                    .last_click
                    .is_some_and(|(r, t)| r == i && now.duration_since(t) < DOUBLE_CLICK);
                self.cursor = i;
                self.dirty = true;
                if double {
                    self.last_click = None;
                    self.activate();
                } else {
                    self.last_click = Some((i, now));
                }
            }
            _ => {}
        }
    }

    // --- drawing --------------------------------------------------------------------------

    /// Window pixels per design unit, and the design origin in window pixels.
    fn frame(&self) -> (f32, f32, f32) {
        frame_for(self.window_size())
    }

    fn window_size(&self) -> (u32, u32) {
        self.gpu
            .as_ref()
            .map_or(self.size, |g| (g.config.width, g.config.height))
    }
}

/// Window pixels per design unit and the design origin for a window of `(w, h)`.
fn frame_for((w, h): (u32, u32)) -> (f32, f32, f32) {
    let s = (h as f32 / DESIGN.1).min(w as f32 / DESIGN.0);
    let x0 = (w as f32 - DESIGN.0 * s) / 2.0;
    let y0 = (h as f32 - DESIGN.1 * s) / 2.0;
    (s, x0, y0)
}

impl Setup {
    fn row_at(&self, (mx, my): (f64, f64)) -> Option<usize> {
        let (s, x0, y0) = self.frame();
        let (x, y) = ((mx as f32 - x0) / s, (my as f32 - y0) / s);
        if !(LIST_X.0..LIST_X.1).contains(&x) || y < LIST_Y {
            return None;
        }
        let k = ((y - LIST_Y) / ROW_H) as usize;
        (k < ROWS)
            .then_some(self.top + k)
            .filter(|&i| i < self.rows.len())
    }

    fn draw(&mut self) -> Result<()> {
        self.dirty = false;
        let (w, h) = self.window_size();
        if self.gpu.is_none() {
            return Ok(());
        }
        let c = self.compose(w, h);
        let gpu = self.gpu.as_mut().expect("checked above");
        let overlay = self.overlay.as_mut().expect("overlay with the window");
        overlay.resize(&gpu.device, (w as usize, h as usize));
        overlay.upload(&gpu.queue, &c.rgba);
        let Some(frame) = gpu.acquire() else {
            self.dirty = true;
            return Ok(());
        };
        let view = gpu.target(&frame);
        overlay.render(
            &gpu.device,
            &gpu.queue,
            &view,
            [0.0, 0.0, w as f32, h as f32],
        );
        gpu.present(frame);
        Ok(())
    }

    /// The screen drawn into an image of the window's size (CPU only).
    fn compose(&self, w: u32, h: u32) -> Canvas {
        let (s, x0, y0) = frame_for((w, h));
        let mut c = Canvas::new(w, h, s, x0, y0);

        // The board, scaled from its 4× art.
        let (bw, bh) = ((308.0 * s) as u32, (66.0 * s) as u32);
        c.image(&self.board.scaled(bw.max(1), bh.max(1)), 166.0, 22.0);

        let font = self.font.as_ref();
        let ui = &self.ui;
        let text = |c: &mut Canvas, t: &str, x, y, w, h, cap, col, align| {
            if let Some(f) = font {
                c.text(f, t, [x, y, w, h], cap, col, align);
            }
        };
        text(
            &mut c,
            ui.get("setup_title"),
            0.0,
            96.0,
            640.0,
            24.0,
            15.0,
            GOLD,
            Align::Center,
        );
        text(
            &mut c,
            ui.get("setup_intro"),
            36.0,
            124.0,
            568.0,
            14.0,
            8.0,
            WHITE,
            Align::Center,
        );
        text(
            &mut c,
            ui.get("setup_intro2"),
            36.0,
            140.0,
            568.0,
            14.0,
            8.0,
            GREY,
            Align::Center,
        );

        match &self.phase {
            Phase::Browse => {
                let path = self.dir.as_ref().map_or_else(
                    || ui.get("setup_places").to_owned(),
                    |d| d.display().to_string(),
                );
                text(
                    &mut c,
                    &path,
                    LIST_X.0,
                    162.0,
                    LIST_X.1 - LIST_X.0,
                    14.0,
                    8.0,
                    GOLD,
                    Align::Left,
                );
                c.rect(
                    LIST_X.0 - 4.0,
                    LIST_Y - 2.0,
                    LIST_X.1 - LIST_X.0 + 8.0,
                    ROW_H * ROWS as f32 + 4.0,
                    PANEL,
                );
                for (k, row) in self.rows.iter().enumerate().skip(self.top).take(ROWS) {
                    let y = LIST_Y + (k - self.top) as f32 * ROW_H;
                    if k == self.cursor {
                        c.rect(LIST_X.0 - 2.0, y, LIST_X.1 - LIST_X.0 + 4.0, ROW_H, BAR);
                    }
                    let (name, tag, col) = describe(row, ui);
                    let name_w = if tag.is_empty() { 560.0 } else { 420.0 };
                    text(
                        &mut c,
                        &name,
                        LIST_X.0 + 6.0,
                        y + 2.0,
                        name_w,
                        ROW_H - 4.0,
                        8.0,
                        col,
                        Align::Left,
                    );
                    if !tag.is_empty() {
                        text(
                            &mut c,
                            &tag,
                            LIST_X.1 - 150.0,
                            y + 2.0,
                            144.0,
                            ROW_H - 4.0,
                            7.0,
                            GREY,
                            Align::Right,
                        );
                    }
                }
                if self.rows.is_empty() {
                    text(
                        &mut c,
                        ui.get("setup_empty"),
                        LIST_X.0,
                        LIST_Y + 4.0,
                        568.0,
                        12.0,
                        8.0,
                        DARK,
                        Align::Center,
                    );
                }
                // Scroll markers beside the list: more rows above or below.
                if self.top > 0 {
                    c.triangle(616.0, LIST_Y + 2.0, 8.0, true, GREY);
                }
                if self.top + ROWS < self.rows.len() {
                    c.triangle(616.0, LIST_Y + ROW_H * ROWS as f32 - 8.0, 8.0, false, GREY);
                }
                text(
                    &mut c,
                    ui.get("setup_keys"),
                    0.0,
                    428.0,
                    640.0,
                    12.0,
                    7.0,
                    GREY,
                    Align::Center,
                );
                text(
                    &mut c,
                    ui.get(if self.playstation {
                        "setup_pad_ps"
                    } else {
                        "setup_pad"
                    }),
                    0.0,
                    442.0,
                    640.0,
                    12.0,
                    7.0,
                    GREY,
                    Align::Center,
                );
                text(
                    &mut c,
                    ui.get("setup_mouse"),
                    0.0,
                    456.0,
                    640.0,
                    12.0,
                    7.0,
                    DARK,
                    Align::Center,
                );
            }
            Phase::Importing {
                progress,
                installed,
                ..
            } => {
                let p = progress.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let title = if *installed {
                    "setup_checking"
                } else {
                    "setup_importing"
                };
                text(
                    &mut c,
                    ui.get(title),
                    0.0,
                    220.0,
                    640.0,
                    18.0,
                    11.0,
                    WHITE,
                    Align::Center,
                );
                let frac = if p.total == 0 {
                    0.0
                } else {
                    p.done as f32 / p.total as f32
                };
                c.rect(120.0, 250.0, 400.0, 14.0, PANEL);
                c.rect(122.0, 252.0, 396.0 * frac, 10.0, GOLD);
                text(
                    &mut c,
                    &p.file,
                    0.0,
                    272.0,
                    640.0,
                    12.0,
                    8.0,
                    GREY,
                    Align::Center,
                );
            }
            Phase::Failed(msg) => {
                c.rect(60.0, 200.0, 520.0, 120.0, PANEL);
                text(
                    &mut c,
                    ui.get("setup_failed"),
                    60.0,
                    210.0,
                    520.0,
                    18.0,
                    11.0,
                    RED,
                    Align::Center,
                );
                text(
                    &mut c,
                    &wrap(msg, 70),
                    76.0,
                    236.0,
                    488.0,
                    56.0,
                    8.0,
                    WHITE,
                    Align::Center,
                );
                text(
                    &mut c,
                    ui.get("setup_back"),
                    60.0,
                    300.0,
                    520.0,
                    12.0,
                    7.0,
                    GREY,
                    Align::Center,
                );
            }
            Phase::Done => {
                text(
                    &mut c,
                    ui.get("setup_done"),
                    0.0,
                    240.0,
                    640.0,
                    18.0,
                    11.0,
                    WHITE,
                    Align::Center,
                );
            }
        }

        c
    }
}

/// A row's name, its tag on the right, and its colour.
fn describe(row: &Row, ui: &crate::front::port_settings::UiText) -> (String, String, [u8; 3]) {
    match row {
        Row::UseHere(source) => {
            let tag = match source {
                Source::Installed(_) => ui.get("setup_tag_installed"),
                _ => ui.get("setup_tag_disc"),
            };
            (ui.get("setup_use_here").to_owned(), tag.to_owned(), GOLD)
        }
        Row::Parent => ("..".into(), ui.get("setup_tag_parent").to_owned(), GREY),
        Row::Place(label, path) => (label.clone(), path.display().to_string(), WHITE),
        Row::Dir(name) => (format!("{name}/"), String::new(), WHITE),
        Row::File(name, FileKind::Image) => {
            (name.clone(), ui.get("setup_tag_image").to_owned(), WHITE)
        }
        Row::File(name, FileKind::Archive) => {
            (name.clone(), ui.get("setup_tag_archive").to_owned(), DARK)
        }
        Row::File(name, FileKind::Other) => (name.clone(), String::new(), DARK),
    }
}

/// The rows of folder `dir`: "use this folder" when it is usable, `..`, then folders and
/// files by name (hidden ones left out).
fn listing(dir: &Path) -> Vec<Row> {
    let mut rows = Vec::new();
    let here = import::classify(dir);
    if matches!(here, Source::Installed(_) | Source::DiscFolder(_)) {
        rows.push(Row::UseHere(here));
    }
    rows.push(Row::Parent);
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let is_dir = e.path().is_dir();
        if is_dir {
            dirs.push(name);
        } else {
            let ext = Path::new(&name)
                .extension()
                .map(|x| x.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            let kind = if import::IMAGE_EXTENSIONS.contains(&ext.as_str()) {
                FileKind::Image
            } else if matches!(import::classify(&e.path()), Source::Archive(_)) {
                FileKind::Archive
            } else {
                FileKind::Other
            };
            files.push((name, kind));
        }
    }
    let key = |s: &String| s.to_lowercase();
    dirs.sort_by_key(key);
    files.sort_by_key(|(n, _)| n.to_lowercase());
    rows.extend(dirs.into_iter().map(Row::Dir));
    rows.extend(files.into_iter().map(|(n, k)| Row::File(n, k)));
    rows
}

fn home() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

/// Where to start looking: home, its Downloads, the removable media, the root (or the
/// drives on Windows).
fn places() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    if let Some(h) = home() {
        out.push(("Home".to_owned(), h.clone()));
        for sub in ["Downloads", "Desktop", "Documents"] {
            if h.join(sub).is_dir() {
                out.push((sub.to_owned(), h.join(sub)));
            }
        }
    }
    if cfg!(windows) {
        for letter in b'A'..=b'Z' {
            let root = PathBuf::from(format!("{}:\\", letter as char));
            if root.exists() {
                out.push((format!("{}:", letter as char), root));
            }
        }
    } else {
        let user = std::env::var("USER").unwrap_or_default();
        let mut media = vec![PathBuf::from("/media"), PathBuf::from("/mnt")];
        if !user.is_empty() {
            media.insert(0, PathBuf::from("/run/media").join(&user));
            media.insert(1, PathBuf::from("/media").join(&user));
        }
        for m in media {
            for e in std::fs::read_dir(&m).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() && !out.iter().any(|(_, q)| *q == p) && Some(&p) != home().as_ref() {
                    out.push((e.file_name().to_string_lossy().into_owned(), p));
                }
            }
        }
        out.push(("/".to_owned(), PathBuf::from("/")));
    }
    out
}

/// An installed game picked here is remembered in `config.toml` (`data_dir`), so later starts
/// find it; an import needs nothing, its folder is looked in by itself.
fn remember(dir: &Path) {
    crate::data_dir::choose(dir.to_owned());
    if let Some(path) = crate::config::path() {
        let value = toml::Value::String(dir.display().to_string());
        if let Err(e) = crate::config::store(&path, "data_dir", Some(value)) {
            eprintln!(
                "warning: could not write data_dir to {}: {e:#}",
                path.display()
            );
        }
    }
}

/// Breaks `s` into lines of at most `width` characters at spaces.
fn wrap(s: &str, width: usize) -> String {
    let mut out = String::new();
    let mut line = 0;
    for word in s.split(' ') {
        let n = word.chars().count();
        if line > 0 && line + 1 + n > width {
            out.push('\n');
            line = 0;
        } else if line > 0 {
            out.push(' ');
            line += 1;
        }
        out.push_str(word);
        line += n;
    }
    out
}

/// The RGBA image being drawn, with the design-to-window mapping.
struct Canvas {
    w: u32,
    h: u32,
    s: f32,
    x0: f32,
    y0: f32,
    rgba: Vec<u8>,
}

impl Canvas {
    fn new(w: u32, h: u32, s: f32, x0: f32, y0: f32) -> Self {
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for px in rgba.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        Self {
            w,
            h,
            s,
            x0,
            y0,
            rgba,
        }
    }

    /// Design units to a window pixel rectangle, clipped.
    fn px(&self, x: f32, y: f32, w: f32, h: f32) -> (i64, i64, i64, i64) {
        let l = (self.x0 + x * self.s).round() as i64;
        let t = (self.y0 + y * self.s).round() as i64;
        let r = (self.x0 + (x + w) * self.s).round() as i64;
        let b = (self.y0 + (y + h) * self.s).round() as i64;
        (l, t, r, b)
    }

    fn blend(&mut self, x: i64, y: i64, col: [u8; 3], a: u8) {
        if x < 0 || y < 0 || x >= i64::from(self.w) || y >= i64::from(self.h) || a == 0 {
            return;
        }
        let i = ((y as u32 * self.w + x as u32) * 4) as usize;
        let a = u32::from(a);
        for k in 0..3 {
            let d = u32::from(self.rgba[i + k]);
            self.rgba[i + k] = ((u32::from(col[k]) * a + d * (255 - a)) / 255) as u8;
        }
    }

    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, col: [u8; 3]) {
        let (l, t, r, b) = self.px(x, y, w, h);
        for yy in t.max(0)..b.min(i64::from(self.h)) {
            for xx in l.max(0)..r.min(i64::from(self.w)) {
                self.blend(xx, yy, col, 255);
            }
        }
    }

    /// A triangle `size` design units wide with its top-left at `(x, y)`, pointing up or down.
    fn triangle(&mut self, x: f32, y: f32, size: f32, up: bool, col: [u8; 3]) {
        let (l, t, r, b) = self.px(x, y, size, size * 0.6);
        let (w, h) = ((r - l).max(1) as f32, (b - t).max(1));
        for yy in 0..h {
            // Row width grows towards the base.
            let f = (yy as f32 + 0.5) / h as f32;
            let f = if up { f } else { 1.0 - f };
            let half = w * f / 2.0;
            let mid = l as f32 + w / 2.0;
            for xx in (mid - half).round() as i64..(mid + half).round() as i64 {
                self.blend(xx, t + yy, col, 255);
            }
        }
    }

    /// `img` (already at window scale) with its top-left at design `(x, y)`.
    fn image(&mut self, img: &RgbaImage, x: f32, y: f32) {
        let (l, t, _, _) = self.px(x, y, 0.0, 0.0);
        for yy in 0..img.height as i64 {
            for xx in 0..img.width as i64 {
                let i = ((yy as u32 * img.width + xx as u32) * 4) as usize;
                let p = &img.rgba[i..i + 4];
                self.blend(l + xx, t + yy, [p[0], p[1], p[2]], p[3]);
            }
        }
    }

    /// Text fitted into the design box `[x, y, w, h]` at capital height `cap` (design units).
    #[allow(clippy::too_many_arguments)]
    fn text(
        &mut self,
        font: &FontGlyphs,
        text: &str,
        [x, y, w, h]: [f32; 4],
        cap: f32,
        col: [u8; 3],
        align: Align,
    ) {
        if text.is_empty() {
            return;
        }
        let (l, t, r, b) = self.px(x, y, w, h);
        let (bw, bh) = ((r - l).max(1) as usize, (b - t).max(1) as usize);
        let mask = rasterise_sized(text, bw, bh, align, false, font, Some(cap * self.s));
        for yy in 0..bh {
            for xx in 0..bw {
                self.blend(l + xx as i64, t + yy as i64, col, mask[yy * bw + xx]);
            }
        }
    }
}

/// The language of the screen: the configured one, else the system's (Japanese when the
/// locale says so), else English.
pub fn language(cfg: &crate::locale::LocaleConfig) -> String {
    if cfg.lang_chosen {
        return if cfg.lang == crate::locale::IDENTITY_LANG {
            "ja".into()
        } else {
            cfg.lang.clone()
        };
    }
    let system = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .unwrap_or_default();
    if system.starts_with("ja") {
        "ja".into()
    } else {
        "en".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Draws the screen's phases at 16:10 and 4:3 and checks text was drawn; with
    /// `SHUPPATSU_SETUP_SHOTS=<dir>` the images are written there as PPM for a look.
    #[test]
    fn composes_every_phase() {
        let tmp = std::env::temp_dir().join(format!("shuppatsu-setup-draw-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("Games")).unwrap();
        for f in [
            "Densha de Go! 2 Kosoku-hen 3000.zip",
            "DENGO3K.mdf",
            "DENGO3K.mds",
            "Cover.jpg",
        ] {
            std::fs::write(tmp.join(f), b"").unwrap();
        }
        let out = std::env::var_os("SHUPPATSU_SETUP_SHOTS").map(PathBuf::from);
        let mut s = Setup::new((1280, 800), false, "en");
        s.open(Some(tmp.clone()));
        s.cursor = 3;
        let progress = Arc::new(Mutex::new(Progress {
            done: 120 << 20,
            total: 214 << 20,
            file: "DATA/komachi.dat".into(),
        }));
        let phases: Vec<(&str, Phase)> = vec![
            ("browse", Phase::Browse),
            (
                "importing",
                Phase::Importing {
                    progress,
                    worker: std::thread::spawn(|| Ok(PathBuf::new())),
                    installed: false,
                },
            ),
            (
                "failed",
                Phase::Failed(s.ui.get("setup_archive").replace("{ext}", "zip")),
            ),
        ];
        for (name, phase) in phases {
            s.phase = phase;
            for (w, h) in [(1280, 800), (1024, 768)] {
                let c = s.compose(w, h);
                let lit = c
                    .rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .filter(|p| p[0] > 200)
                    .count();
                assert!(lit > 1000, "{name} {w}x{h}: text drawn");
                if let Some(dir) = &out {
                    let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
                    for p in c.rgba.as_chunks::<4>().0 {
                        ppm.extend_from_slice(&p[..3]);
                    }
                    std::fs::write(dir.join(format!("setup-{name}-{w}x{h}.ppm")), ppm).unwrap();
                }
            }
        }
        s.open(None);
        s.phase = Phase::Browse;
        if let Some(dir) = &out {
            let c = s.compose(1280, 800);
            let mut ppm = b"P6\n1280 800\n255\n".to_vec();
            for p in c.rgba.as_chunks::<4>().0 {
                ppm.extend_from_slice(&p[..3]);
            }
            std::fs::write(dir.join("setup-places-1280x800.ppm"), ppm).unwrap();
        }
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    /// The keyboard and the controller share these: steps, pages, back, and a message that
    /// any confirm, back or quit dismisses without quitting.
    #[test]
    fn actions_move_and_dismiss() {
        let tmp = std::env::temp_dir().join(format!("shuppatsu-setup-act-{}", std::process::id()));
        for i in 0..40 {
            std::fs::create_dir_all(tmp.join(format!("d{i:02}"))).unwrap();
        }
        let mut s = Setup::new((1280, 800), false, "en");
        s.pad = None;
        s.open(Some(tmp.clone()));
        assert_eq!(s.cursor, 0);
        s.act(Action::Down);
        s.act(Action::Down);
        assert_eq!(s.cursor, 2);
        s.act(Action::PageDown);
        assert_eq!(s.cursor, 2 + ROWS - 1);
        assert!(s.top > 0, "the view follows the cursor");
        s.act(Action::Last);
        assert_eq!(s.cursor, s.rows.len() - 1);
        s.act(Action::Activate); // opens d39
        assert_eq!(s.dir.as_deref(), Some(tmp.join("d39").as_path()));
        s.act(Action::Back);
        assert_eq!(s.dir.as_deref(), Some(tmp.as_path()));
        assert_eq!(
            s.rows[s.cursor],
            Row::Dir("d39".into()),
            "back to the folder we were in"
        );
        s.phase = Phase::Failed("x".into());
        s.act(Action::Quit);
        assert!(!s.quit && matches!(s.phase, Phase::Browse));
        s.act(Action::Quit);
        assert!(s.quit);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn wraps_at_spaces() {
        assert_eq!(wrap("aa bb cc", 5), "aa bb\ncc");
        assert_eq!(wrap("one", 2), "one");
    }

    #[test]
    fn lists_folders_then_files_with_kinds() {
        let tmp = std::env::temp_dir().join(format!("shuppatsu-setup-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("b-dir")).unwrap();
        std::fs::create_dir_all(tmp.join(".hidden")).unwrap();
        for f in ["Game.MDS", "a.zip", "notes.txt"] {
            std::fs::write(tmp.join(f), b"").unwrap();
        }
        let rows = listing(&tmp);
        assert_eq!(
            rows,
            vec![
                Row::Parent,
                Row::Dir("b-dir".into()),
                Row::File("a.zip".into(), FileKind::Archive),
                Row::File("Game.MDS".into(), FileKind::Image),
                Row::File("notes.txt".into(), FileKind::Other),
            ]
        );
        std::fs::create_dir_all(tmp.join("BIN")).unwrap();
        std::fs::create_dir_all(tmp.join("DATA")).unwrap();
        assert!(matches!(
            listing(&tmp)[0],
            Row::UseHere(Source::Installed(_))
        ));
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn every_string_is_in_both_catalogues() {
        let en = crate::front::port_settings::UiText::load("en", None);
        let ja = crate::front::port_settings::UiText::load("ja", None);
        for &id in UI_IDS {
            assert_ne!(en.get(id), id, "en lacks {id}");
            assert_ne!(ja.get(id), en.get(id), "ja lacks {id}");
        }
    }
}

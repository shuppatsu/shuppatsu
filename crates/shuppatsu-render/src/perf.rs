//! The frame trace (env `SHUPPATSU_FRAME_TRACE=<file>`, docs/performance.md): per presented
//! frame, the wall time, the interval since the previous frame and the CPU time of each part of
//! the frame, as one tab-separated line. `shuppatsu-tools frame-report` summarises the file.
//!
//! Sections are timed exclusively: a section entered inside another (a compose inside a tick)
//! pauses the outer one, so the columns add up to the frame's tracked time. Without the
//! variable every call is a cheap check of a flag.
//!
//! The lines are written by a thread of their own: a `write` to the file can block for a
//! refresh or more while the kernel writes dirty pages back (every 5 s on ext4), which showed
//! up as a missed refresh every 600 frames that was the trace's own doing.

use std::cell::RefCell;
use std::io::Write;
use std::sync::OnceLock;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// A part of a frame's CPU time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    /// Simulation and screen ticks, without the 2D compose inside them.
    Tick,
    /// The 2D layer composed on the CPU (`SpriteLayer::compose` and backdrop fills).
    Compose,
    /// The 2D canvas copied to the GPU (`Overlay::upload`).
    Upload2d,
    /// The 3D frame's geometry built on the CPU (scene compose, effects).
    Build3d,
    /// The 3D geometry copied to the GPU (`Renderer::set_geometry`).
    Upload3d,
    /// Render passes encoded and submitted (3D, overlay, clears).
    Render,
    /// Waiting for the next surface texture.
    Acquire,
    /// Presenting the frame.
    Present,
    /// The movie's decoding and picture.
    Movie,
}

const SECTIONS: [Section; 9] = [
    Section::Tick,
    Section::Compose,
    Section::Upload2d,
    Section::Build3d,
    Section::Upload3d,
    Section::Render,
    Section::Acquire,
    Section::Present,
    Section::Movie,
];

impl Section {
    fn name(self) -> &'static str {
        match self {
            Section::Tick => "tick",
            Section::Compose => "compose",
            Section::Upload2d => "upload2d",
            Section::Build3d => "build3d",
            Section::Upload3d => "upload3d",
            Section::Render => "render",
            Section::Acquire => "acquire",
            Section::Present => "present",
            Section::Movie => "movie",
        }
    }
}

/// A counted event of a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Counter {
    /// 33 ms ticks run in the frame.
    Ticks,
    /// Full 2D composes.
    Composes,
    /// Bytes copied to the 2D canvas texture.
    Upload2dBytes,
    /// 2D composes that drew the whole canvas.
    FullComposes,
}

/// What the writer thread is sent.
enum Out {
    Line(String),
    /// Write out everything sent before, then answer.
    Flush(mpsc::Sender<()>),
}

/// The writer thread's loop: lines are written as they come and the file is flushed whenever
/// nothing more is waiting, so a run that is killed keeps its trace.
fn write_lines(mut out: std::io::BufWriter<std::fs::File>, rx: mpsc::Receiver<Out>) {
    while let Ok(mut msg) = rx.recv() {
        loop {
            match msg {
                Out::Line(line) => {
                    let _ = writeln!(out, "{line}");
                }
                Out::Flush(done) => {
                    let _ = out.flush();
                    let _ = done.send(());
                }
            }
            match rx.try_recv() {
                Ok(m) => msg = m,
                Err(_) => break,
            }
        }
        let _ = out.flush();
    }
}

struct Trace {
    out: mpsc::Sender<Out>,
    start: Instant,
    last_frame: Option<Instant>,
    frames: u64,
    stack: Vec<(Section, Instant)>,
    acc: [Duration; SECTIONS.len()],
    counters: [u64; 4],
}

thread_local! {
    static TRACE: RefCell<Option<Trace>> = const { RefCell::new(None) };
}

fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let Some(path) = std::env::var_os("SHUPPATSU_FRAME_TRACE").filter(|p| !p.is_empty()) else {
            return false;
        };
        let file = match std::fs::File::create(&path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("frame trace {}: {e}", path.to_string_lossy());
                return false;
            }
        };
        let mut out = std::io::BufWriter::new(file);
        let mut header = String::from("frame\tt_ms\tinterval_ms");
        for s in SECTIONS {
            header.push('\t');
            header.push_str(s.name());
        }
        header.push_str("\tticks\tcomposes\tupload2d_kb\tfull_composes\tscreen\tcanvas");
        let _ = writeln!(out, "{header}");
        let (tx, rx) = mpsc::channel();
        if let Err(e) = std::thread::Builder::new()
            .name("frame-trace".into())
            .spawn(move || write_lines(out, rx))
        {
            eprintln!("frame trace thread: {e}");
            return false;
        }
        TRACE.with(|t| {
            *t.borrow_mut() = Some(Trace {
                out: tx,
                start: Instant::now(),
                last_frame: None,
                frames: 0,
                stack: Vec::new(),
                acc: [Duration::ZERO; SECTIONS.len()],
                counters: [0; 4],
            })
        });
        eprintln!("frame trace: {}", path.to_string_lossy());
        true
    })
}

fn index(s: Section) -> usize {
    SECTIONS.iter().position(|&x| x == s).unwrap_or(0)
}

/// Times `section` until the guard is dropped.
#[must_use]
pub fn scope(section: Section) -> Scope {
    if !enabled() {
        return Scope { on: false };
    }
    let now = Instant::now();
    TRACE.with(|t| {
        if let Some(t) = t.borrow_mut().as_mut() {
            if let Some(&(outer, since)) = t.stack.last() {
                t.acc[index(outer)] += now - since;
            }
            t.stack.push((section, now));
        }
    });
    Scope { on: true }
}

/// The guard of [`scope`].
pub struct Scope {
    on: bool,
}

impl Drop for Scope {
    fn drop(&mut self) {
        if !self.on {
            return;
        }
        let now = Instant::now();
        TRACE.with(|t| {
            if let Some(t) = t.borrow_mut().as_mut()
                && let Some((s, since)) = t.stack.pop()
            {
                t.acc[index(s)] += now - since;
                if let Some(top) = t.stack.last_mut() {
                    top.1 = now;
                }
            }
        });
    }
}

/// Opens the trace now (call it first thing in `main`, on the thread that draws): `t_ms`
/// then counts from the program's start, so the first frame's is the start-up time.
pub fn start() {
    enabled();
}

/// Runs `f` timed as `section`.
pub fn time<R>(section: Section, f: impl FnOnce() -> R) -> R {
    let _g = scope(section);
    f()
}

/// Adds `n` to a counter of the current frame.
pub fn count(counter: Counter, n: u64) {
    if !enabled() {
        return;
    }
    TRACE.with(|t| {
        if let Some(t) = t.borrow_mut().as_mut() {
            t.counters[counter as usize] += n;
        }
    });
}

/// Ends a presented frame: writes its line, `screen` naming what it showed and `canvas` the
/// 2D canvas size.
pub fn frame(screen: &str, canvas: (usize, usize)) {
    if !enabled() {
        return;
    }
    let now = Instant::now();
    TRACE.with(|t| {
        let mut t = t.borrow_mut();
        let Some(t) = t.as_mut() else { return };
        let interval = t
            .last_frame
            .map_or(0.0, |l| (now - l).as_secs_f64() * 1000.0);
        t.last_frame = Some(now);
        let mut line = format!(
            "{}\t{:.3}\t{interval:.3}",
            t.frames,
            (now - t.start).as_secs_f64() * 1000.0
        );
        for a in &mut t.acc {
            line.push_str(&format!("\t{:.3}", a.as_secs_f64() * 1000.0));
            *a = Duration::ZERO;
        }
        let [ticks, composes, bytes, full] = std::mem::take(&mut t.counters);
        line.push_str(&format!(
            "\t{ticks}\t{composes}\t{}\t{full}\t{screen}\t{}x{}",
            bytes / 1024,
            canvas.0,
            canvas.1
        ));
        let _ = t.out.send(Out::Line(line));
        t.frames += 1;
    });
}

/// Writes out what is buffered (at exit).
pub fn flush() {
    if !enabled() {
        return;
    }
    TRACE.with(|t| {
        if let Some(t) = t.borrow_mut().as_mut() {
            let (done, wait) = mpsc::channel();
            if t.out.send(Out::Flush(done)).is_ok() {
                let _ = wait.recv();
            }
        }
    });
}

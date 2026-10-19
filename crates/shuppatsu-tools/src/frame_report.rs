//! `frame-report`: summarises frame traces of the game (`SHUPPATSU_FRAME_TRACE`,
//! docs/performance.md) per screen, as Markdown: frame intervals against the display's refresh
//! period, missed refreshes, and the CPU time of each part of the frame.

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

/// The columns of one frame line that are times in milliseconds, after `interval_ms`.
const SECTIONS: [&str; 9] = [
    "tick", "compose", "upload2d", "build3d", "upload3d", "render", "acquire", "present", "movie",
];

struct Frame {
    interval: f64,
    sections: [f64; SECTIONS.len()],
    ticks: u32,
    composes: u32,
    full_composes: u32,
    upload_kb: u64,
}

impl Frame {
    /// The CPU time spent on the frame's own work: everything but waiting for the display.
    fn work(&self) -> f64 {
        self.sections
            .iter()
            .zip(SECTIONS)
            .filter(|(_, n)| !matches!(*n, "acquire" | "present"))
            .map(|(v, _)| v)
            .sum()
    }
}

fn parse(text: &str) -> Result<Vec<(String, String, Frame)>> {
    let mut lines = text.lines();
    let header: Vec<&str> = lines.next().context("empty trace")?.split('\t').collect();
    let col = |name: &str| {
        header
            .iter()
            .position(|h| *h == name)
            .with_context(|| format!("trace has no column {name}"))
    };
    let interval = col("interval_ms")?;
    let sections = SECTIONS.map(|s| col(s).ok());
    let full = col("full_composes").ok();
    let (ticks, composes, upload, screen, canvas) = (
        col("ticks")?,
        col("composes")?,
        col("upload2d_kb")?,
        col("screen")?,
        col("canvas")?,
    );
    let mut frames = Vec::new();
    for (i, line) in lines.enumerate() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < header.len() {
            continue; // a line cut off by a kill
        }
        let num = |c: usize| -> Result<f64> {
            f[c].parse()
                .with_context(|| format!("line {}: column {c}", i + 2))
        };
        let mut s = [0.0; SECTIONS.len()];
        for (v, c) in s.iter_mut().zip(sections) {
            if let Some(c) = c {
                *v = num(c)?;
            }
        }
        frames.push((
            f[screen].to_owned(),
            f[canvas].to_owned(),
            Frame {
                interval: num(interval)?,
                sections: s,
                ticks: num(ticks)? as u32,
                composes: num(composes)? as u32,
                full_composes: match full {
                    Some(c) => num(c)? as u32,
                    None => num(composes)? as u32,
                },
                upload_kb: num(upload)? as u64,
            },
        ));
    }
    Ok(frames)
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[i]
}

struct Stats {
    avg: f64,
    p50: f64,
    p99: f64,
    max: f64,
}

fn stats(mut v: Vec<f64>) -> Stats {
    v.sort_by(f64::total_cmp);
    let avg = if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f64>() / v.len() as f64
    };
    Stats {
        avg,
        p50: percentile(&v, 0.5),
        p99: percentile(&v, 0.99),
        max: v.last().copied().unwrap_or(0.0),
    }
}

/// Prints the report of the traces in `args` (`[--hz N] <trace>...`).
pub fn run(args: &[String]) -> Result<()> {
    let mut hz = 120.0;
    let mut files = Vec::new();
    let mut rest = args.iter();
    while let Some(a) = rest.next() {
        match a.as_str() {
            "--hz" => hz = rest.next().context("--hz needs a rate")?.parse()?,
            _ => files.push(a.clone()),
        }
    }
    if files.is_empty() {
        bail!("usage: frame-report [--hz <refresh rate>] <trace.tsv>...");
    }
    let mut out = String::new();
    for file in &files {
        out.push_str(&report(Path::new(file), hz)?);
    }
    print!("{out}");
    Ok(())
}

fn report(path: &Path, hz: f64) -> Result<String> {
    let text = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
    // The first frames (start-up, first presents) are not representative.
    let frames: Vec<_> = parse(&text)?.into_iter().skip(3).collect();
    let period = 1000.0 / hz;
    let mut by_screen: BTreeMap<String, Vec<&Frame>> = BTreeMap::new();
    let mut canvases: BTreeMap<String, String> = BTreeMap::new();
    let mut order: Vec<String> = Vec::new();
    for (screen, canvas, f) in &frames {
        if !by_screen.contains_key(screen) {
            order.push(screen.clone());
        }
        by_screen.entry(screen.clone()).or_default().push(f);
        canvases.insert(screen.clone(), canvas.clone());
    }
    let mut s = String::new();
    let _ = writeln!(
        s,
        "\n### {}\n\n{} frames at {hz} Hz (period {period:.2} ms). Interval and work in ms; \
         missed = refreshes skipped (interval rounded to whole periods, minus one).\n",
        path.display(),
        frames.len()
    );
    let _ = writeln!(
        s,
        "| screen | canvas | frames | interval avg | p50 | p99 | max | missed | missed % | 1/2/3/4+ periods | work avg | work p99 | work max |"
    );
    let _ = writeln!(s, "|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    let mut all: Vec<&Frame> = Vec::new();
    let mut rows = Vec::new();
    for name in order.iter().map(String::as_str).chain(["(all)"]) {
        let list: Vec<&Frame> = if name == "(all)" {
            all.clone()
        } else {
            let l = by_screen[name].clone();
            all.extend(&l);
            l
        };
        rows.push((name.to_owned(), list));
    }
    for (name, list) in &rows {
        let iv = stats(list.iter().map(|f| f.interval).collect());
        let work = stats(list.iter().map(|f| f.work()).collect());
        let mut hist = [0usize; 4];
        let mut missed = 0usize;
        for f in list {
            let periods = (f.interval / period).round().max(1.0) as usize;
            hist[periods.min(4) - 1] += 1;
            missed += periods - 1;
        }
        let total = list.len() + missed;
        let _ = writeln!(
            s,
            "| {name} | {} | {} | {:.2} | {:.2} | {:.2} | {:.2} | {missed} | {:.1} | {}/{}/{}/{} | {:.2} | {:.2} | {:.2} |",
            canvases.get(name).map_or("", String::as_str),
            list.len(),
            iv.avg,
            iv.p50,
            iv.p99,
            iv.max,
            100.0 * missed as f64 / total.max(1) as f64,
            hist[0],
            hist[1],
            hist[2],
            hist[3],
            work.avg,
            work.p99,
            work.max,
        );
    }
    let _ = writeln!(
        s,
        "\nCPU time per section, avg / p99 / max (ms); composes and upload per frame (avg):\n"
    );
    let mut head = String::from("| screen |");
    let mut rule = String::from("|---|");
    for n in SECTIONS {
        let _ = write!(head, " {n} |");
        rule.push_str("---|");
    }
    let _ = writeln!(s, "{head} ticks | composes | full composes | upload KB |");
    let _ = writeln!(s, "{rule}---|---|---|---|");
    for (name, list) in &rows {
        let _ = write!(s, "| {name} |");
        for i in 0..SECTIONS.len() {
            let st = stats(list.iter().map(|f| f.sections[i]).collect());
            let _ = write!(s, " {:.2} / {:.2} / {:.2} |", st.avg, st.p99, st.max);
        }
        let n = list.len().max(1) as f64;
        let _ = writeln!(
            s,
            " {:.2} | {:.2} | {:.2} | {:.0} |",
            list.iter().map(|f| f64::from(f.ticks)).sum::<f64>() / n,
            list.iter().map(|f| f64::from(f.composes)).sum::<f64>() / n,
            list.iter().map(|f| f64::from(f.full_composes)).sum::<f64>() / n,
            list.iter().map(|f| f.upload_kb as f64).sum::<f64>() / n,
        );
    }
    Ok(s)
}

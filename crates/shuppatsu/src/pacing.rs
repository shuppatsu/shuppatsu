//! The time a run's simulation advances per redraw (docs/performance.md, "Frame pacing").
//!
//! With vsync every present is shown on a display refresh, evenly spaced, but the redraw that
//! renders it runs at a jittery moment before (±0.5 ms typical, ±2 ms at worst, measured at
//! 120 Hz). Timed at the redraws, the 33 ms ticks and the interpolation between them inherit
//! that jitter, and the camera moves unevenly from frame to frame. [`FrameClock`] advances by
//! whole refresh periods instead (the display's own rate), phase-locked to the wall clock: a
//! tenth of the remaining offset is taken up per frame, so the simulation keeps exact time, and
//! an offset of more than half a period (a stall, a wrong or variable rate) jumps to the wall
//! clock. Without vsync, or without a known rate, it is the redraws' own time.

use std::time::{Duration, Instant};

/// The share of the offset from the wall clock taken up per frame.
const PULL: f64 = 0.1;

#[derive(Debug, Clone)]
pub struct FrameClock {
    origin: Instant,
    /// The previous redraw's time (seconds from `origin`); `None` after a reset.
    last: Option<f64>,
    on_grid: bool,
}

impl FrameClock {
    /// A clock on the refresh grid (`on_grid`: presenting with vsync) or on the redraws.
    pub fn new(on_grid: bool) -> Self {
        Self {
            origin: Instant::now(),
            last: None,
            on_grid,
        }
    }

    /// The next redraw advances by nothing (a new run, a resumed window).
    pub fn reset(&mut self) {
        self.last = None;
    }

    /// The time since the previous redraw, for a redraw at `now` on a display refreshing every
    /// `period` (`None`: unknown).
    pub fn advance(&mut self, now: Instant, period: Option<Duration>) -> Duration {
        let wall = now.saturating_duration_since(self.origin).as_secs_f64();
        let Some(last) = self.last else {
            self.last = Some(wall);
            return Duration::ZERO;
        };
        let t = match period.map(|p| p.as_secs_f64()).filter(|&p| p > 0.0) {
            Some(p) if self.on_grid => {
                // Whole refreshes since the last redraw (Fifo shows each frame on one).
                let k = ((wall - last) / p).round().max(1.0);
                let t = last + k * p;
                let off = wall - t;
                if off.abs() > p / 2.0 {
                    wall
                } else {
                    t + off * PULL
                }
            }
            _ => wall,
        };
        let t = t.max(last);
        self.last = Some(t);
        Duration::from_secs_f64(t - last)
    }
}

/// How the event loop waits between redraws (docs/performance.md, "Frame pacing"). Polling
/// keeps a core busy; sleeping until the next redraw lets the core clock down, and the frame
/// after it runs slower. With a margin, the loop sleeps until `margin` before the next redraw
/// is due (the previous one plus the shortest recent interval) and polls from there.
#[derive(Debug, Clone)]
pub struct IdleWait {
    margin: Option<Duration>,
    last: Option<Instant>,
    /// The shortest of the recent redraw intervals, and how many redraws it has stood.
    period: Option<(Duration, u32)>,
}

/// Redraws after which the shortest interval is measured afresh.
const PERIOD_WINDOW: u32 = 240;

/// Polling starts this long (ms) before the next redraw is due. Measured on the 120 Hz TV
/// (docs/performance.md): driving misses as few refreshes as with polling throughout and
/// every section of the frame takes as long, at half the CPU time; sleeping to the redraw
/// itself (0) made the frame work about 40 % slower.
pub const DEFAULT_MARGIN_MS: f64 = 1.0;

impl IdleWait {
    /// The margin is [`DEFAULT_MARGIN_MS`]; `SHUPPATSU_IDLE_WAIT=<ms>` sets another, `poll`
    /// makes the loop poll throughout.
    pub fn from_env() -> Self {
        let margin = match std::env::var("SHUPPATSU_IDLE_WAIT") {
            Ok(v) if v == "poll" => None,
            Ok(v) => v
                .parse::<f64>()
                .ok()
                .filter(|ms| ms.is_finite() && *ms >= 0.0)
                .or(Some(DEFAULT_MARGIN_MS)),
            Err(_) => Some(DEFAULT_MARGIN_MS),
        }
        .map(|ms| Duration::from_secs_f64(ms / 1000.0));
        Self {
            margin,
            last: None,
            period: None,
        }
    }

    /// A redraw starts at `now`.
    pub fn redraw(&mut self, now: Instant) {
        if let Some(last) = self.last {
            let d = now.saturating_duration_since(last);
            self.period = Some(match self.period {
                Some((p, n)) if n < PERIOD_WINDOW && p <= d => (p, n + 1),
                _ => (d, 0),
            });
        }
        self.last = Some(now);
    }

    /// The control flow until the next event.
    pub fn control_flow(&self, now: Instant) -> winit::event_loop::ControlFlow {
        use winit::event_loop::ControlFlow;
        let (Some(margin), Some(last), Some((period, _))) = (self.margin, self.last, self.period)
        else {
            return ControlFlow::Poll;
        };
        // An interval far off any display rate (a stall) caps the sleep.
        let period = period.min(Duration::from_millis(50));
        match (last + period).checked_sub(margin) {
            Some(t) if t > now => ControlFlow::WaitUntil(t),
            // A redraw long overdue (a hidden window gets no frame callbacks): sleep until an
            // event comes.
            _ if now > last + 4 * period => ControlFlow::Wait,
            _ => ControlFlow::Poll,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Redraws jittering around a 120 Hz grid advance by (almost exactly) a refresh period
    /// and keep wall time; without vsync they advance by their own intervals.
    #[test]
    fn redraws_advance_by_refresh_periods_on_the_grid() {
        let start = Instant::now();
        let period = 1.0 / 120.0;
        let p = Some(Duration::from_secs_f64(period));
        let jitter = [
            0.0004, -0.0011, 0.0019, -0.0003, 0.0, 0.0008, -0.0017, 0.0012,
        ];
        let at =
            |k: usize| start + Duration::from_secs_f64(0.5 + k as f64 * period + jitter[k % 8]);
        let mut grid = FrameClock::new(true);
        grid.origin = start;
        let mut plain = FrameClock::new(false);
        plain.origin = start;
        let (mut worst, mut worst_plain, mut total): (f64, f64, f64) = (0.0, 0.0, 0.0);
        for k in 0..600 {
            let (g, q) = (grid.advance(at(k), p), plain.advance(at(k), p));
            total += g.as_secs_f64();
            if k == 0 {
                continue; // the first redraw only starts the clock
            }
            worst = worst.max((g.as_secs_f64() - period).abs());
            worst_plain = worst_plain.max((q.as_secs_f64() - period).abs());
        }
        assert!(worst < 4e-4, "grid intervals off by {worst} s");
        assert!(worst_plain > 2e-3, "the redraws themselves jitter");
        // Phase-locked: the simulated time stays within the jitter of the wall time.
        let wall = 599.0 * period + jitter[599 % 8] - jitter[0];
        assert!((total - wall).abs() < 3e-3, "{total} against {wall}");
    }

    /// Two refreshes between redraws advance two periods; a stall jumps to the wall clock.
    #[test]
    fn skips_and_stalls() {
        let start = Instant::now();
        let p = Duration::from_secs_f64(1.0 / 60.0);
        let mut c = FrameClock::new(true);
        c.origin = start;
        let s = |t: f64| start + Duration::from_secs_f64(t);
        c.advance(s(1.0), Some(p));
        let two = c
            .advance(s(1.0 + 2.0 / 60.0 + 0.001), Some(p))
            .as_secs_f64();
        assert!((two - 2.0 / 60.0).abs() < 2e-4, "{two}");
        let stall = c.advance(s(1.5), Some(p)).as_secs_f64();
        assert!(stall > 0.45, "{stall}");
    }

    /// The loop sleeps until the margin before the next redraw, polls from there, and sleeps
    /// for events once a redraw is long overdue; without a margin it always polls.
    #[test]
    fn idle_wait_sleeps_until_the_margin() {
        use winit::event_loop::ControlFlow;
        let start = Instant::now();
        let ms = |v: f64| Duration::from_secs_f64(v / 1000.0);
        let mut w = IdleWait {
            margin: Some(ms(1.0)),
            last: None,
            period: None,
        };
        assert_eq!(w.control_flow(start), ControlFlow::Poll);
        for (k, jitter) in [0.0, 0.3, -0.2, 0.1].into_iter().enumerate() {
            w.redraw(start + ms(k as f64 * 8.333 + jitter));
        }
        // The shortest interval: 8.333 − 0.5 ms.
        let last = start + ms(3.0 * 8.333 + 0.1);
        let period = ms(8.333 - 0.5);
        assert_eq!(
            w.control_flow(last + ms(1.0)),
            ControlFlow::WaitUntil(last + period - ms(1.0))
        );
        assert_eq!(w.control_flow(last + period - ms(0.5)), ControlFlow::Poll);
        assert_eq!(w.control_flow(last + 5 * period), ControlFlow::Wait);
        let poll = IdleWait {
            margin: None,
            ..w.clone()
        };
        assert_eq!(poll.control_flow(last + ms(1.0)), ControlFlow::Poll);
    }
}

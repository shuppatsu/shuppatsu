//! Issue marks in the whole game ([`crate::marks`]): the screen and the ticks for
//! `mark.toml`, the whole session as a replay from start-up, and a run's own replay when
//! the mark falls in one.

use super::{App, Screen};
use crate::marks::{Replay, Report, Window};
use toml::{Table, Value};

impl App {
    /// The screen shown, for `mark.toml`.
    fn mark_screen(&self) -> String {
        match &self.screen {
            Screen::Language(_) => "language".into(),
            Screen::Attract(a) => format!("attract_{}", a.shown()),
            Screen::Title(_) => "title".into(),
            Screen::Options(_) => "options".into(),
            Screen::Control(_) => "options_control".into(),
            Screen::Value(_) => "options_value".into(),
            Screen::PortSettings(_) => "port_settings".into(),
            Screen::Records(_) => "records".into(),
            Screen::Calibrate(_) => "calibrate".into(),
            Screen::Exit(_) => "exit_dialog".into(),
            Screen::RouteSelect(_) => "route_select".into(),
            Screen::LeverCheck(..) => "lever_check".into(),
            Screen::Board(..) => "departure_board".into(),
            Screen::Wait(..) => "between_screens".into(),
            Screen::Play(p) => p.mark_screen().into(),
            Screen::Movie(_) => "movie".into(),
            Screen::Quit => "quit".into(),
        }
    }

    /// Writes the waiting mark once its frame was presented (the state it reports is the one
    /// that frame shows).
    pub(super) fn finish_mark(&mut self) {
        if self.mark.is_none() || self.gpu().is_some_and(|g| g.capture.is_some()) {
            return;
        }
        let Some(p) = self.mark.take() else {
            return;
        };
        let report = self.mark_report();
        let toast = self
            .ui
            .get("mark_saved")
            .replace("{n}", &p.number.to_string());
        if let Err(e) = crate::marks::finish(&p, &report, self.gpu_mut(), &toast) {
            eprintln!("mark {}: {e:#}", p.number);
        }
    }

    fn mark_report(&self) -> Report {
        let (log, data_dir) = match (&self.screen, self.res.as_ref()) {
            (Screen::Play(p), _) => (p.log(), p.data_dir()),
            (_, Some(r)) => (&r.log, r.data.data_dir()),
            (_, None) => unreachable!("the resources are lent only to a run"),
        };
        let session = Replay {
            command: Vec::new(),
            locale: self.boot.locale.clone(),
            settings: self.boot.settings,
            save: self.boot.eeprom.clone(),
            inputs: log.script(0, self.ticks, None),
            tick: self.ticks,
        };
        let mut ticks = Table::new();
        ticks.insert("app_tick".into(), Value::Integer(self.ticks as i64));
        let mut state = Table::new();
        let window = match &self.screen {
            Screen::Play(p) => {
                state = p.mark_state();
                p.mark_window()
            }
            _ => {
                state.insert("ticks".into(), Value::Table(ticks));
                let r = self.res.as_ref();
                Window::of(
                    self.gpu(),
                    self.opts.size,
                    r.map_or(1, |r| r.layer.scale),
                    self.canvas_size(),
                )
            }
        };
        // A run replays alone from its start; the session from start-up is the fallback.
        let run = match &self.screen {
            Screen::Play(p) => p.run_replay(),
            _ => None,
        };
        let (replay, session) = match run {
            Some(run) => (run, Some(session)),
            None => (session, None),
        };
        Report {
            screen: self.mark_screen(),
            state,
            locale: self.opts.locale.clone(),
            settings: self.opts.settings,
            window,
            data_dir: crate::marks::install_dir(data_dir),
            replay: Some(replay),
            session,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::AppOptions;
    use super::*;
    use crate::gamepad::{GenericInputs, PadOptions, generic_state};
    use crate::play::mark::tests::{headless_run, run_state};
    use crate::script::parse_script;
    use std::path::PathBuf;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("shuppatsu-marks-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    /// A headless session (English, enhanced layout) with an input script and a save.
    fn session(inputs: Option<PathBuf>, save_file: Option<PathBuf>, pad: Option<bool>) -> App {
        let opts = AppOptions {
            size: (1280, 800),
            settings: crate::settings::Settings::default(),
            mute: true,
            pad: PadOptions {
                no_pad: true,
                scripted_pad: pad,
                ..Default::default()
            },
            inputs,
            save_file,
            shot: Some((0, std::env::temp_dir().join("shuppatsu-unused.ppm"))),
            locale: crate::locale::LocaleConfig {
                lang: "en".into(),
                hud_layout: crate::presentation::HudLayout::Enhanced,
                ..Default::default()
            },
            locks: crate::config::Locks::default(),
            first_run: false,
        };
        App::new(opts).unwrap()
    }

    /// One tick as the headless `--shot` loop runs it.
    fn step(app: &mut App) {
        if matches!(app.screen, Screen::Play(_)) {
            app.step_play().unwrap();
        } else {
            app.tick().unwrap();
        }
    }

    fn state(app: &App) -> String {
        match &app.screen {
            Screen::Play(p) => format!("{} {}", app.mark_screen(), run_state(p)),
            _ => app.mark_screen(),
        }
    }

    /// A session played live from start-up into a run (keys as the window leaves them, a
    /// pad connected from the start), marked during the run: the whole session replayed from
    /// its recorded inputs and boot save reaches the same screen and run state, and so does
    /// the run replayed alone (`shuppatsu play`, the default of `reproduce.sh`).
    #[test]
    fn a_marked_session_and_its_run_replay() {
        if std::env::var_os("SHUPPATSU_DATA_DIR").is_none() {
            eprintln!("skipped: SHUPPATSU_DATA_DIR not set");
            return;
        }
        // The live pad: connected, the left stick pulled for power once driving.
        let pulled = GenericInputs {
            left_y: -0.9,
            ..Default::default()
        };
        let rest = GenericInputs::default();
        // Fed below as a device would be read; recorded like one.
        let mut live = session(None, None, Some(true));
        let feed = |app: &mut App, sample| match &mut app.screen {
            Screen::Play(p) => p.feed_pad(Some(sample)),
            _ => app.res().controls.feed_pad(Some(sample)),
        };
        // Into the run: Enter every 40 ticks.
        let mut ticks = 0;
        while live.mark_screen() != "play" {
            ticks += 1;
            assert!(ticks < 8000, "no run, at {}", live.mark_screen());
            live.down[0x1c] = live.ticks % 40 == 39;
            feed(&mut live, Default::default());
            step(&mut live);
        }
        live.down = [false; 256];
        let start = live.ticks;
        while live.ticks < start + 500 {
            let t = live.ticks - start;
            live.down[0xcb] = (10..=40).contains(&t); // brake off
            live.down[0xd0] = (60..=61).contains(&t); // power
            live.down[0x39] = (100..=130).contains(&t); // horn
            let stick = if (200..=215).contains(&t) {
                pulled
            } else {
                rest
            };
            feed(&mut live, (generic_state(&stick, &[0xff; 14]), stick));
            step(&mut live);
        }
        let Screen::Play(p) = &live.screen else {
            panic!("left the run");
        };
        let speed = p.mark_state()["train"]["speed_kmh"].as_float().unwrap();
        assert!(speed > 1.0, "{}", state(&live));
        let report = live.mark_report();
        let run = report.replay.as_ref().unwrap();
        assert_eq!(run.command[0], "play");
        let whole = report.session.as_ref().unwrap();
        assert_eq!(whole.tick, live.ticks);

        // The session from start-up.
        let (inputs, save) = (temp("session-inputs.txt"), temp("session-eeprom.dat"));
        std::fs::write(&inputs, &whole.inputs).unwrap();
        std::fs::write(&save, &whole.save).unwrap();
        let mut again = session(Some(inputs), Some(save), None);
        while again.ticks < whole.tick {
            step(&mut again);
        }
        assert_eq!(state(&again), state(&live));

        // The run alone.
        let script = parse_script(&run.inputs).unwrap();
        let pad = script.has_pad().then(|| script.pad_at(0).is_some());
        let stage: usize = run.command[1].parse().unwrap();
        let mut alone = headless_run(stage, Some(&run.save), script, pad).unwrap();
        let g = run
            .command
            .iter()
            .position(|a| a == "--global-tick")
            .unwrap();
        alone.set_global_tick(run.command[g + 1].parse().unwrap());
        while alone.ticks() < run.tick {
            alone.step().unwrap();
        }
        assert_eq!(run_state(&alone), run_state(p));
    }
}

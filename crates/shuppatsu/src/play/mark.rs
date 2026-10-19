//! Issue marks of a run ([`crate::marks`]): what the run reports for `mark.toml`, and the
//! command that replays it alone, `shuppatsu play` from its start.

use super::{Play, TICK, WorldView, units};
use crate::marks::{Replay, Report, Window};
use toml::{Table, Value};

fn int(v: impl Into<i64>) -> Value {
    Value::Integer(v.into())
}

fn float(v: f32) -> Value {
    Value::Float(f64::from(v))
}

fn text(v: impl std::fmt::Display) -> Value {
    Value::String(v.to_string())
}

fn vec3(v: glam::Vec3) -> Value {
    Value::Array(v.to_array().into_iter().map(float).collect())
}

impl Play {
    /// The screen the run shows, for `mark.toml`.
    pub fn mark_screen(&self) -> &'static str {
        if self.demo.is_some() {
            "attract_demo"
        } else if self.opening.is_some() {
            "attract_opening"
        } else if self.intro.is_some() {
            "stage_intro"
        } else if self.ending.is_some() {
            "ending"
        } else if self.drive.bonus().is_some() {
            "bonus_game"
        } else if self.flow.paused() {
            "pause"
        } else if self.world == WorldView::ResultReplay {
            "station_result"
        } else {
            "play"
        }
    }

    /// The run's sections of `mark.toml`: ticks, the run, the train, the cab camera, input.
    pub fn mark_state(&self) -> Table {
        let mut ticks = Table::new();
        ticks.insert("play_tick".into(), int(self.ticks as i64));
        if let Some(b) = self.tick_base {
            ticks.insert("app_tick".into(), int((b + self.ticks) as i64));
        }
        ticks.insert("drive_tick".into(), int(self.drive.tick_count() as i64));
        ticks.insert("global_tick".into(), int(self.drive.global_tick()));
        // How far the frame was between the last two ticks (setting `interpolate`).
        let alpha = self.accumulator.as_secs_f32() / TICK.as_secs_f32();
        ticks.insert("frame_alpha".into(), float(alpha));

        let r = self.drive.rules();
        let (next, _) = self.drive.next_station();
        let name = |i: usize| text(self.drive.station_name(i).unwrap_or("?"));
        let mut run = Table::new();
        run.insert("stage".into(), int(self.opts.stage as i64));
        run.insert("difficulty".into(), int(self.opts.difficulty as i64));
        run.insert("variant".into(), Value::Boolean(self.opts.variant));
        run.insert("route".into(), text(self.drive.com_name()));
        run.insert("start_station".into(), int(self.opts.station as i64));
        run.insert("station".into(), int(r.station as i64));
        run.insert("station_name".into(), name(r.station));
        run.insert("next_station".into(), int(next as i64));
        run.insert("next_station_name".into(), name(next));
        run.insert("phase".into(), text(format!("{:?}", self.drive.phase())));
        run.insert("paused".into(), Value::Boolean(self.flow.paused()));
        run.insert("world".into(), text(format!("{:?}", self.world)));
        run.insert("remaining_s".into(), int(r.remaining_s));
        run.insert("continues".into(), int(r.continues));

        let pos = self.drive.pos();
        let u = units(pos);
        let mut train = Table::new();
        train.insert("engine_index".into(), int(pos.engine_index as i64));
        train.insert("t".into(), float(pos.t));
        train.insert("route_units".into(), float(u));
        let metres = u * shuppatsu_formats::route::METRES_PER_UNIT;
        train.insert("route_m".into(), float(metres));
        train.insert("speed_kmh".into(), float(self.drive.speed_kmh()));
        train.insert("power".into(), int(self.latched.power));
        train.insert("brake".into(), int(self.latched.brake));
        train.insert("dist_to_stop_m".into(), float(self.drive.dist_to_stop_m()));
        train.insert("gradient".into(), float(self.drive.gradient()));

        let c = self.scene.cab_camera(pos);
        let mut camera = Table::new();
        camera.insert("view".into(), text(format!("{:?}", self.world)));
        camera.insert("eye".into(), vec3(c.eye));
        camera.insert("target".into(), vec3(c.target));

        let mut input = Table::new();
        input.insert("control_mode".into(), int(self.opts.control_mode as i64));
        let hint = match self.controls.hint_device() {
            Some(crate::hybrid::HintDevice::Pad) => "pad",
            Some(crate::hybrid::HintDevice::Keyboard) => "keyboard",
            None => "none (hybrid_input off)",
        };
        input.insert("hint_device".into(), text(hint));
        input.insert(
            "pad_connected".into(),
            Value::Boolean(self.controls.pad_connected()),
        );

        let mut t = Table::new();
        for (k, v) in [
            ("ticks", ticks),
            ("run", run),
            ("train", train),
            ("camera", camera),
            ("input", input),
        ] {
            t.insert(k.into(), Value::Table(v));
        }
        t
    }

    /// The run up to now as `shuppatsu play` replays it alone, from the save, global tick and
    /// hint device it started with. `None` for the attract drives, the stage intro and the
    /// check aids (`--ending`, `--intro`), which `shuppatsu play` does not start that way.
    pub fn run_replay(&self) -> Option<Replay> {
        if self.demo.is_some() || self.opening.is_some() || self.intro.is_some() {
            return None;
        }
        if self.opts.ending.is_some() || self.opts.intro {
            return None;
        }
        let o = &self.opts;
        let mut command: Vec<String> = vec![
            "play".into(),
            o.stage.to_string(),
            "--difficulty".into(),
            o.difficulty.to_string(),
            "--keys".into(),
            o.control_mode.to_string(),
        ];
        if o.station != 0 {
            command.extend(["--station".into(), o.station.to_string()]);
        }
        if let Some(s) = o.speedometer {
            command.extend(["--speedometer".into(), s.to_string()]);
        }
        if self.start.global_tick != 0 {
            command.extend(["--global-tick".into(), self.start.global_tick.to_string()]);
        }
        let base = self.tick_base.unwrap_or(0);
        // Under the front end the hints may follow the pad already.
        let hint = self.tick_base.and(self.start.hint);
        Some(Replay {
            command,
            locale: self.start.locale.clone(),
            settings: self.start.settings,
            save: self.start.eeprom.clone(),
            inputs: self.log.script(base, base + self.ticks, hint),
            tick: self.ticks,
        })
    }

    /// The window and the HUD canvas, for `mark.toml`.
    pub fn mark_window(&self) -> Window {
        Window::of(
            self.gpu.as_ref(),
            self.opts.size,
            self.layer.scale,
            self.layer.canvas_size(),
        )
    }

    /// The toast's text for mark `n`.
    pub fn mark_toast(&self, n: u32) -> String {
        self.mark_text.replace("{n}", &n.to_string())
    }

    /// `shuppatsu play`: writes the waiting mark once its frame was presented (its state is
    /// the one that frame shows).
    pub(super) fn finish_mark(&mut self) {
        if self.mark.is_none() || self.gpu.as_ref().is_some_and(|g| g.capture.is_some()) {
            return;
        }
        let Some(p) = self.mark.take() else {
            return;
        };
        let report = Report {
            screen: self.mark_screen().to_owned(),
            state: self.mark_state(),
            locale: self.opts.locale.clone(),
            settings: self.opts.settings,
            window: self.mark_window(),
            data_dir: crate::marks::install_dir(self.data.data_dir()),
            replay: self.run_replay(),
            session: None,
        };
        let toast = self.mark_toast(p.number);
        if let Err(e) = crate::marks::finish(&p, &report, self.gpu.as_mut(), &toast) {
            eprintln!("mark {}: {e:#}", p.number);
        }
    }
}

impl Play {
    /// The game data's directory (for `reproduce.sh`).
    pub fn data_dir(&self) -> &std::path::Path {
        self.data.data_dir()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::{PlayOptions, Shared, load_bank};
    use super::*;
    use crate::gamepad::{Controls, GenericInputs, PadOptions, PadSample};
    use crate::save::{Eeprom, Save};
    use crate::scene::GameData;
    use crate::script::{InputLog, Script, parse_script};
    use shuppatsu_formats::exe::Exe;
    use shuppatsu_render::sprites::SpriteLayer;

    /// Everything a frame of the run shows and the next tick depends on; the HUD's pixels
    /// every 50th tick (hashing them every tick is slow).
    pub(crate) fn run_state(p: &Play) -> String {
        let r = p.drive.rules();
        let hash = if p.ticks.is_multiple_of(50) {
            p.canvas
                .as_chunks::<8>()
                .0
                .iter()
                .fold(0xcbf2_9ce4_8422_2325u64, |h, c| {
                    (h ^ u64::from_le_bytes(*c)).wrapping_mul(0x100_0000_01b3)
                })
        } else {
            0
        };
        format!(
            "play tick {} drive tick {} global {} pos {:?} speed {} input {:?} phase {:?} \
             paused {} world {:?} station {} remaining {} continues {} hint {:?} canvas {hash:x}",
            p.ticks,
            p.drive.tick_count(),
            p.drive.global_tick(),
            p.drive.pos(),
            p.drive.speed_kmh(),
            p.latched,
            p.drive.phase(),
            p.flow.paused(),
            p.world,
            r.station,
            r.remaining_s,
            r.continues,
            p.controls.hint_device(),
        )
    }

    /// A headless run of `stage` from `save` (the exe's default when `None`) with `script`;
    /// its pad is fed by the caller (`scripted_pad`) when `pad` says it starts connected or
    /// not. Needs `SHUPPATSU_DATA_DIR`.
    pub(crate) fn headless_run(
        stage: usize,
        save: Option<&[u8]>,
        script: Script,
        pad: Option<bool>,
    ) -> Option<Play> {
        if std::env::var_os("SHUPPATSU_DATA_DIR").is_none() {
            eprintln!("skipped: SHUPPATSU_DATA_DIR not set");
            return None;
        }
        let data = GameData::from_env().unwrap();
        let exe_bytes = data.exe().unwrap();
        let exe = Exe::parse(&exe_bytes).unwrap();
        let pad = PadOptions {
            no_pad: true,
            scripted_pad: pad,
            ..Default::default()
        };
        let locale = crate::locale::LocaleConfig {
            lang: "en".into(),
            hud_layout: crate::presentation::HudLayout::Enhanced,
            ..Default::default()
        };
        let opts = PlayOptions {
            stage,
            difficulty: 1,
            difficulty_from_save: false,
            station: 0,
            control_mode: 0,
            pad: pad.clone(),
            size: (1280, 800),
            settings: crate::settings::Settings::default(),
            mute: true,
            inputs: None,
            save_file: None,
            global_tick: None,
            shot: None,
            speedometer: None,
            variant: false,
            ending: None,
            intro: false,
            locale,
        };
        let save = match save {
            Some(b) => Save::new(Eeprom::load(b, &Eeprom::defaults(&exe).unwrap()), None),
            None => Save::open(&exe, None).unwrap(),
        };
        let controls = Controls::new(&exe, 0, &pad).unwrap();
        let mut log = InputLog::default();
        log.start(controls.pad_connected());
        let shared = Shared {
            bank: load_bank(&data, &exe, &opts.locale).unwrap().0,
            layer: SpriteLayer::default(),
            save,
            controls,
            sound: None,
            gpu: None,
            data,
            log,
        };
        let mut play = Play::new(shared, opts).unwrap();
        if let Some(h) = script.hint {
            play.controls.set_hint_device(h);
        }
        play.script = script;
        Some(play)
    }

    /// The pad of a live session: the left stick pulled for power, then the brake on the
    /// right stick, A, a touch of LT and the triggers released.
    fn live_pad(t: u64) -> Option<PadSample> {
        let mut i = GenericInputs::default();
        match t {
            0..=99 => return None,
            200..=212 => i.left_y = -0.83,
            300..=305 => i.lt = 0.71,
            400..=409 => i.right_y = 0.9,
            500 => i.south = true,
            600..=640 => i.left_x = 0.01 * (t - 600) as f32,
            _ => {}
        }
        Some((crate::gamepad::generic_state(&i, &[0xff; 14]), i))
    }

    /// Keys of a live session, as window events leave them before each poll.
    fn live_keys(t: u64) -> [bool; 256] {
        let mut d = [false; 256];
        d[0xcb] = (20..=70).contains(&t) || (450..=480).contains(&t); // brake off
        d[0xd0] = (150..=152).contains(&t) || (700..=760).contains(&t); // power
        d[0x39] = (250..=290).contains(&t); // horn
        d[0x01] = t == 900 || t == 960; // pause, resume
        d[0xc8] = (1000..=1003).contains(&t); // power down
        d
    }

    /// A run driven live (keys as the window leaves them, a pad), marked at a tick: its
    /// recorded inputs replayed the way `reproduce.sh` replays them (`shuppatsu play` from
    /// the run's save, global tick and hint device) reach the same state and the same HUD
    /// pixels at the mark, under the front end's app ticks too.
    #[test]
    fn a_recorded_run_replays_to_the_marked_state() {
        const MARK: u64 = 1300;
        let Some(mut live) = headless_run(0, None, Script::default(), Some(false)) else {
            return;
        };
        // Under the front end: logged in app ticks from 4321.
        live.set_tick_base(4321);
        live.set_global_tick(4321);
        let mut states = Vec::new();
        while live.ticks < MARK {
            let t = live.ticks + 1;
            live.set_down(live_keys(t));
            live.feed_pad(live_pad(t));
            live.tick().unwrap();
            states.push(run_state(&live));
        }
        assert!(live.drive.speed_kmh() > 1.0, "{}", run_state(&live));
        let replay = live.run_replay().unwrap();
        assert_eq!(replay.tick, MARK);
        assert!(replay.command.contains(&"--global-tick".to_owned()));
        assert!(replay.inputs.contains("pad "), "{}", replay.inputs);

        let script = parse_script(&replay.inputs).unwrap();
        let pad = script.has_pad().then(|| script.pad_at(0).is_some());
        let mut again = headless_run(0, Some(&replay.save), script, pad).unwrap();
        again.set_global_tick(4321);
        let mut replayed = Vec::new();
        while again.ticks < MARK {
            again.tick().unwrap();
            replayed.push(run_state(&again));
        }
        for (i, (a, b)) in states.iter().zip(&replayed).enumerate() {
            assert_eq!(a, b, "play tick {}", i + 1);
        }
    }
}

//! `ddg-tools play-sound`: plays a sound code (or sequence) through `ddg-audio`, live on the
//! default device or rendered to a WAV in `out/` for inspection.

use crate::export::GameDirs;
use anyhow::{Context, Result, bail};
use ddg_audio::mixer::{Command, Mixer, MixerConfig};
use ddg_audio::{AudioOutput, SoundBank, SoundSystem, SoundTables};
use ddg_formats::exe::Exe;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The original's tick (33 ms timer).
const TICK: Duration = Duration::from_millis(33);
const WAV_RATE: u32 = 44_100;

/// Parses `0x31`, `31h` or decimal.
fn parse_code(s: &str) -> Result<u16> {
    let lower = s.to_ascii_lowercase();
    let v = if let Some(h) = lower.strip_prefix("0x") {
        u16::from_str_radix(h, 16)
    } else if let Some(h) = lower.strip_suffix('h') {
        u16::from_str_radix(h, 16)
    } else {
        lower.parse()
    };
    v.with_context(|| format!("bad sound code {s}"))
}

pub fn play_sound(args: &[String]) -> Result<()> {
    let code = parse_code(args.first().context("play-sound: missing <code>")?)?;
    let mut seconds = 600.0f64;
    let mut pitch = None;
    let mut volume = None;
    let mut release_after = None;
    let mut wav: Option<PathBuf> = None;
    let mut rest = args[1..].iter();
    while let Some(a) = rest.next() {
        let mut value = || rest.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--seconds" => seconds = value()?.parse()?,
            "--pitch" => pitch = Some(value()?.parse::<i16>()?),
            "--volume" => volume = Some(value()?.parse::<i16>()?),
            "--release-after" => release_after = Some(value()?.parse::<f64>()?),
            "--wav" => wav = Some(PathBuf::from(value()?)),
            other => bail!("unknown option {other}"),
        }
    }

    let dirs = GameDirs::from_env()?;
    let bank = Arc::new(SoundBank::load(&dirs.data)?);
    let exe = dirs.read_exe()?;
    let tables = Arc::new(SoundTables::from_exe(&Exe::parse(&exe)?)?);
    let is_sequence = code & ddg_audio::code::SEQUENCE_FLAG != 0;
    if !is_sequence && !bank.has(code) {
        bail!("no sound data for code {code:#x}");
    }
    // Looping sounds are released after 3 s unless told otherwise.
    let loops = bank.effect(code).is_some_and(|e| e.loop_end != 0)
        || bank.bgm(code).is_some_and(|t| t.looping);
    let release_tick = release_after
        .or(loops.then_some(3.0))
        .map(|s| (s / TICK.as_secs_f64()) as u64);

    let output = if wav.is_none() {
        Some(AudioOutput::open(
            bank.clone(),
            tables.code_volume.clone(),
            MixerConfig::default(),
        )?)
    } else {
        None
    };
    let rate = output.as_ref().map_or(WAV_RATE, |o| o.sample_rate());
    // Renders the WAV, or shadows the device mixer to know when playback has ended.
    let mut mixer = Mixer::new(
        bank,
        tables.code_volume.clone(),
        rate,
        MixerConfig::default(),
    );
    let mut snd = SoundSystem::new(tables);
    snd.request(code);
    if let Some(p) = pitch {
        snd.set_pitch(code, p);
    }
    if let Some(v) = volume {
        snd.set_volume(code, v);
    }
    println!("playing {code:#x} at {rate} Hz");

    let mut pcm: Vec<u8> = Vec::new();
    let mut buf = Vec::new();
    let max_ticks = (seconds / TICK.as_secs_f64()) as u64;
    let start = Instant::now();
    let mut rendered = 0u64;
    for tick in 0..max_ticks {
        if tick > 0 {
            snd.frame();
        }
        if Some(tick) == release_tick {
            snd.release(code);
        }
        let cmds = snd.take_commands();
        for &c in &cmds {
            mixer.apply(c);
        }
        if let Some(out) = &output {
            out.send_all(cmds);
        }
        // Frames up to the end of this tick, without drift.
        let target = (tick + 1) * rate as u64 * TICK.as_millis() as u64 / 1000;
        buf.resize(2 * (target - rendered) as usize, 0.0);
        rendered = target;
        mixer.render(&mut buf);
        if wav.is_some() {
            for s in &buf {
                pcm.extend_from_slice(&((s * 32767.0) as i16).to_le_bytes());
            }
        } else {
            let due = start + TICK * (tick as u32 + 1);
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
        }
        if !snd.sequence_active() && mixer.voices().next().is_none() {
            break;
        }
    }
    if let Some(out) = &output {
        // Let the device buffer drain.
        std::thread::sleep(Duration::from_millis(200));
        for e in out.take_errors() {
            eprintln!("stream error: {e}");
        }
        out.send(Command::StopAll);
    }
    if let Some(path) = wav {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, ddg_formats::audio::wav(&pcm, rate, 2, 16))?;
        println!(
            "wrote {} ({:.1} s)",
            path.display(),
            pcm.len() as f64 / 4.0 / rate as f64
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_syntax() {
        assert_eq!(parse_code("0x31").unwrap(), 0x31);
        assert_eq!(parse_code("19Eh").unwrap(), 0x19e);
        assert_eq!(parse_code("4115").unwrap(), 0x1013);
        assert!(parse_code("zz").is_err());
    }
}

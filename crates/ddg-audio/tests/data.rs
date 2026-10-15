//! Tests against the user's own game data. They skip unless `DDG_DATA_DIR` points at the
//! extracted disc (`install/Program_Executable_Files/{DATA,BIN}`).

use ddg_audio::code::{FIRST_BGM_CODE, LAST_BGM_CODE, LAST_EFFECT_CODE};
use ddg_audio::mixer::{Command, Mixer, MixerConfig, pitch_hz};
use ddg_audio::tables::{JointPattern, SoundTables};
use ddg_audio::{SoundBank, SoundSystem};
use ddg_formats::exe::Exe;
use std::path::PathBuf;
use std::sync::Arc;

fn install_dir() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("DDG_DATA_DIR")?);
    let dir = root.join("install/Program_Executable_Files");
    dir.is_dir().then_some(dir)
}

macro_rules! need_data {
    () => {
        match install_dir() {
            Some(d) => d,
            None => {
                eprintln!("skipped: DDG_DATA_DIR not set");
                return;
            }
        }
    };
}

fn tables(dir: &std::path::Path) -> SoundTables {
    let exe = std::fs::read(dir.join("BIN/dengo3k.exe")).unwrap();
    SoundTables::from_exe(&Exe::parse(&exe).unwrap()).unwrap()
}

#[test]
fn every_effect_and_bgm_entry_plays() {
    let dir = need_data!();
    let bank = Arc::new(SoundBank::load(&dir.join("DATA")).unwrap());
    assert_eq!(bank.effects().len(), 407);
    assert_eq!(bank.bgm_tracks().len(), 18);
    assert_eq!(
        bank.effects().iter().filter(|e| e.loop_end != 0).count(),
        22
    );
    let tables = tables(&dir);

    // Mix at 22050 Hz: every effect plays to its end (looping ones are released after 1 s).
    let rate = 22_050;
    let mut mixer = Mixer::new(
        bank.clone(),
        tables.code_volume.clone(),
        rate,
        MixerConfig::default(),
    );
    let mut buf = vec![0.0f32; 2 * 1024];
    for code in 1..=LAST_EFFECT_CODE {
        let fx = *bank.effect(code).unwrap();
        let hz = pitch_hz(code, 0, fx.sample_rate);
        assert!((100..=100_000).contains(&hz), "code {code:#x}: {hz} Hz");
        mixer.apply(Command::Play(code));
        assert!(mixer.is_playing(code), "code {code:#x} did not start");
        let mut chunks = 0;
        let mut peak = 0.0f32;
        while mixer.is_playing(code) {
            mixer.render(&mut buf);
            peak = buf.iter().fold(peak, |p, s| p.max(s.abs()));
            chunks += 1;
            if chunks == rate as usize / 1024 {
                mixer.apply(Command::Release(code));
            }
            assert!(chunks < 2000, "code {code:#x} never ends");
        }
        assert!(peak.is_finite() && peak <= 1.0);
    }

    // BGM: every track starts on a BGM voice and produces sound within its first seconds.
    let mut mixer = Mixer::new(
        bank.clone(),
        tables.code_volume.clone(),
        44_100,
        MixerConfig::default(),
    );
    for code in FIRST_BGM_CODE..=LAST_BGM_CODE {
        let t = *bank.bgm(code).unwrap();
        assert!(t.frames() > 44_100, "code {code:#x} shorter than a second");
        mixer.apply(Command::Play(code));
        let v = mixer.voices().find(|v| v.code == code).unwrap();
        assert!(v.slot >= 8 && v.hz == 44_100);
        let mut peak = 0.0f32;
        for _ in 0..(5 * 44_100 / 1024) {
            mixer.render(&mut buf);
            peak = buf.iter().fold(peak, |p, s| p.max(s.abs()));
        }
        assert!(peak > 0.01, "code {code:#x} silent");
        mixer.apply(Command::Release(code));
        assert!(!mixer.is_playing(code));
    }
}

#[test]
fn exe_tables() {
    let dir = need_data!();
    let t = tables(&dir);

    // Code volumes: 0 for code 0, 0x63..=0x7f elsewhere (audio-text.md §4).
    assert_eq!(t.code_volume.get(0), 0);
    for code in 1..=LAST_BGM_CODE {
        let v = t.code_volume.get(code);
        assert!((0x40..=0x7f).contains(&v), "code {code:#x}: volume {v:#x}");
    }

    // 47 sequences of valid codes; the in-train announcements 0x19..=0x23 open with the chime.
    assert_eq!(t.sequences.len(), 47);
    for (id, seq) in t.sequences.iter().enumerate() {
        assert!(!seq.is_empty(), "sequence {id:#x} empty");
        for s in seq {
            let code = s.code as u16 & !0x1000;
            assert!(
                (1..=LAST_BGM_CODE).contains(&code),
                "sequence {id:#x}: {s:?}"
            );
            assert!(s.delay >= -1, "sequence {id:#x}: {s:?}");
        }
    }
    for id in 0x19..=0x23 {
        assert_eq!(t.sequences[id][0].code, 0x1a7, "sequence {id:#x}");
    }
    // Example from the notes: sequence 0x13 starts 0x149/100, 0x14a/130.
    assert_eq!(t.sequences[0x13][0], ddg_audio::Step::new(0x149, 100));
    assert_eq!(t.sequences[0x13][1], ddg_audio::Step::new(0x14a, 130));

    // Pair table: five suffixes first, then station names.
    assert_eq!(t.call_pairs[0], ddg_audio::Step::new(0xc1, 0));
    assert_eq!(t.call_pairs[5], ddg_audio::Step::new(0x6e, 0x24));
    eprintln!("call pairs: {}", t.call_pairs.len());

    // Per-stage codes.
    for (s, st) in t.stages.iter().enumerate() {
        assert_eq!(st.motor, 0x31, "stage {s}");
        assert_eq!(st.joint, 0x32, "stage {s}");
        assert!(
            [4, 6, 8, 0x1a3].contains(&st.horn),
            "stage {s}: horn {:#x}",
            st.horn
        );
        assert_eq!(st.motor_base_pitch, 1, "stage {s}");
    }

    // Rail-joint rows: increasing click phases inside the period.
    assert_eq!(t.joints.len(), 21);
    for (r, j) in t.joints.iter().enumerate() {
        assert!(j.clicks.windows(2).all(|w| w[0] <= w[1]), "row {r}: {j:?}");
        assert!(j.clicks[3] < j.period, "row {r}: {j:?}");
        assert!(
            j.flags & JointPattern::JOINTS != 0 || j.flags == 0,
            "row {r}: {j:?}"
        );
    }
}

#[test]
fn table_sequences_play_through() {
    let dir = need_data!();
    let tables = Arc::new(tables(&dir));
    let bank = Arc::new(SoundBank::load(&dir.join("DATA")).unwrap());
    let mut mixer = Mixer::new(
        bank,
        tables.code_volume.clone(),
        22_050,
        MixerConfig::default(),
    );
    let mut snd = SoundSystem::new(tables.clone());
    let mut buf = vec![0.0f32; 2 * 735]; // one 33 ms tick at 22050 Hz
    for id in 0..tables.sequences.len() {
        snd.request(0x1000 | id as u16);
        let mut ticks = 0;
        let mut played = 0;
        while snd.sequence_active() {
            snd.frame();
            for c in snd.drain_commands() {
                if matches!(c, Command::Play(_)) {
                    played += 1;
                }
                mixer.apply(c);
            }
            mixer.render(&mut buf);
            ticks += 1;
            assert!(ticks < 30 * 120, "sequence {id:#x} never ends");
        }
        assert!(
            played + 1 >= tables.sequences[id].len().min(12),
            "sequence {id:#x}"
        );
        mixer.apply(Command::StopAll);
    }
}

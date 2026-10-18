use super::*;
use ddg_audio::SoundSystem;
use std::sync::Arc;

/// Pair table: suffixes 0..=4 (0xc1, 0xc0, 0xbf, 0xbd, 0xbc), driver names 5..=89 at
/// 0x40 + id, the suffixes again, announcer names 90 + id at 0x100 + id.
fn tables() -> SoundTables {
    let suffixes = [0xc1, 0xc0, 0xbf, 0xbd, 0xbc];
    let mut pairs = Vec::new();
    for block in 0..2 {
        for &c in &suffixes {
            pairs.push(Step::new(c, 0));
        }
        for id in 5..90 {
            let code = if block == 0 { 0x40 + id } else { 0x100 + id };
            pairs.push(Step::new(code, 40));
        }
    }
    let mut sequences = vec![Vec::new(); 0x2e];
    sequences[5] = vec![Step::new(0x9b, 72), Step::new(0xa7, 0)];
    sequences[0x11] = vec![Step::new(0xe7, 10), Step::new(0xe7, 10), Step::new(0xe7, 0)];
    // An in-train announcement ending "next is <name 39> <closing>".
    sequences[0x13] = vec![
        Step::new(0x149, 100),
        Step::new(0x111, 40),
        Step::new(0x100 + 39, 40),
        Step::new(0x112, 0),
    ];
    sequences[0xc] = vec![Step::new(0xa2, 72), Step::new(0xa7, 0)];
    sequences[0x12] = vec![Step::new(0xe4, 10), Step::new(0xe6, 10), Step::new(0xe8, 0)];
    // The Komachi platform call: a run of three phrases, then one of its own.
    sequences[0x18] = vec![
        Step::new(0x16a, 40),
        Step::new(0x16b, 40),
        Step::new(0x16c, 40),
        Step::new(0x16d, 0),
    ];
    // Chime (no line), "soon" + "Morioka" run, then a phrase without a key.
    sequences[0x21] = vec![
        Step::new(0x1a7, 20),
        Step::new(0x173, 30),
        Step::new(0x189, 30),
        Step::new(0x112, 0),
    ];
    SoundTables {
        call_pairs: pairs,
        sequences,
        ..SoundTables::default()
    }
}

fn templates() -> Vec<(String, String)> {
    crate::locale::Catalogue::parse(include_str!("../../../../locales/en.toml"))
        .unwrap()
        .subtitles
}

/// Every sound 1 s long (10 kHz, 10 000 samples).
fn book() -> SubtitleBook {
    let names = [(13, "Ōmagari"), (39, "Tōkamachi"), (76, "Amagasaki")]
        .into_iter()
        .map(|(id, n)| (id, n.to_owned()))
        .collect();
    SubtitleBook::new(
        templates(),
        names,
        &tables(),
        VoiceLengths(vec![1.0; 0x196]),
    )
}

/// The subtitles of whatever `f` makes the sound system start.
fn texts(f: impl FnOnce(&mut SoundSystem)) -> Vec<String> {
    let mut s = SoundSystem::new(Arc::new(tables()));
    s.set_voice_log(true);
    f(&mut s);
    let b = book();
    s.take_voice_events()
        .iter()
        .filter_map(|e| match e {
            VoiceEvent::Started(st) => Some(b.cues(st)),
            VoiceEvent::Silenced => None,
        })
        .flatten()
        .map(|c| c.text)
        .collect()
}

#[test]
fn catalogue_keys_are_known() {
    let t = templates();
    assert!(!t.is_empty());
    for (k, _) in &t {
        assert!(known_key(k), "unknown [subtitle] key {k}");
    }
    for k in TEMPLATE_KEYS {
        assert!(t.iter().any(|(t, _)| t == k), "en.toml lacks {k}");
    }
}

/// The coupling minigame's radio (docs/localisation.md, "Voice codes and their subtitles"):
/// a perfect game through `ddg_engine::coupling`, every sound it requests through the
/// sound system and the book. The squelch 0x3f and the effects have no line.
#[test]
fn coupling_radio_has_a_line_per_call() {
    use ddg_engine::coupling::{Coupling, CouplingEvent, CouplingInput, INTRO_TICKS};
    let mut c = Coupling::new();
    // (km/h, metres to the coupler, horn); the stop mark is 40 m before the coupler.
    let mut drive = vec![(0.0, 40.0, false); INTRO_TICKS as usize];
    drive.extend([
        (0.0, 40.0, true),
        (3.0, 20.0, false),
        (1.0, 8.0, false),
        (1.0, 7.0, false),
        (1.0, 6.4, false),
        (0.0, 5.5, false),
        (0.0, 5.5, true),
        (1.0, 2.9, false),
        (1.0, 1.9, false),
        (1.0, 0.9, false),
        (0.5, 0.0, false),
    ]);
    drive.extend([(0.0, 0.0, false); 210]);
    let mut codes = Vec::new();
    for (kmh, d, horn) in drive {
        let t = c.step(&CouplingInput {
            speed_mps: kmh / 3.6,
            dist_to_stop_m: d - 40.0,
            horn_pressed: horn,
        });
        codes.extend(t.events.iter().filter_map(|e| match e {
            CouplingEvent::Sound(code) => Some(*code),
            _ => None,
        }));
    }
    let lines: Vec<String> = codes
        .iter()
        .flat_map(|&code| texts(|s| s.request(code)))
        .collect();
    assert_eq!(
        lines,
        [
            "Begin coupling preparations. Over.",
            "Coupling preparations complete. Over.",
            "Stop once at 3 metres, then proceed.",
            // The reminder (every 90 ticks while 9 m or more away), then the 8 m call.
            "Come ahead.",
            "All right.",
            "Stop, stop!",
            "Now for the coupling.",
            "OK, 3 metres.",
            "OK, 2 metres.",
            "OK, 1 metre.",
            "Oh, good.",
        ]
    );
    for squelch in [0x3f, 0x11, 0x2a] {
        assert!(texts(|s| s.request(squelch)).is_empty());
    }
}

#[test]
fn station_calls_name_the_station() {
    // Approach call 0x41ffe0(stops ? 3 : 2, name).
    assert_eq!(texts(|s| s.call_pair(3, 13)), ["Ōmagari: stopping."]);
    assert_eq!(texts(|s| s.call_pair(2, 13)), ["Ōmagari: passing through."]);
    // A name the catalogue has no name for, or another suffix: nothing.
    assert!(texts(|s| s.call_pair(3, 14)).is_empty());
    assert!(texts(|s| s.call_pair(4, 13)).is_empty());
    // Course event 2 (0x420040): modes 0 / 2 "next", 1 the name twice.
    assert_eq!(
        texts(|s| s.call_station(0, 76)),
        ["The next station is Amagasaki."]
    );
    assert_eq!(
        texts(|s| s.call_station(2, 39)),
        ["The next station is Tōkamachi."]
    );
    assert_eq!(texts(|s| s.call_station(1, 76)), ["Amagasaki. Amagasaki."]);
}

#[test]
fn signal_and_limit_calls() {
    assert_eq!(texts(|s| s.call_signal(0, 1, 2)), ["Home signal: Caution."]);
    assert_eq!(
        texts(|s| s.call_signal(0, 2, 0)),
        ["Starting signal: Clear."]
    );
    // Prefix 3 (0xd4 本線): the main-line signal.
    assert_eq!(
        texts(|s| s.call_signal(3, 1, 4)),
        ["Main-line home signal: Stop."]
    );
    assert_eq!(
        texts(|s| s.call_signal(3, 2, 0)),
        ["Main-line starting signal: Clear."]
    );
    // Repeater calls: kind 0, with the repeated signal as the prefix.
    assert_eq!(texts(|s| s.call_signal(1, 0, 6)), ["Home repeater: Limit."]);
    assert_eq!(
        texts(|s| s.call_signal(0, 0, 5)),
        ["Repeater: High-speed clear."]
    );
    assert_eq!(
        texts(|s| s.call_prefix_speed(0, 45)),
        ["Speed limit 45 km/h."]
    );
    assert_eq!(
        texts(|s| s.call_prefix_speed(0, 47)),
        ["Speed limit 45 km/h."]
    );
    assert_eq!(
        texts(|s| s.call_prefix_speed(0, 0)),
        ["Speed limit lifted."]
    );
    assert_eq!(texts(|s| s.call_prefix_speed(0, 7)), ["Speed limit."]);
    // Numbered calls: sequence 5 = block 1, sequence 0xc = home signal 1.
    assert_eq!(texts(|s| s.request(0x1005)), ["Block signal 1: Clear."]);
    assert_eq!(texts(|s| s.request(0x100c)), ["Home signal 1: Clear."]);
}

#[test]
fn single_clips_and_sequences() {
    assert_eq!(texts(|s| s.request(0x97)), ["Starting signal: Clear."]);
    assert_eq!(texts(|s| s.request(0xbc)), ["On-time pass."]);
    assert_eq!(
        texts(|s| s.request(0xd5)),
        ["The doors are closing. Please be careful."]
    );
    // A cue per phrase (on screen a repeated line is only kept up longer).
    assert_eq!(
        texts(|s| s.request(0x1011)),
        ["Hey, what do you think you're doing?"; 3]
    );
    assert_eq!(
        texts(|s| s.request(0x1012)),
        ["Huh?", "No way!", "You're really bad at this."]
    );
    // The opening phrase, then "next is" + station + closing phrase.
    assert_eq!(
        texts(|s| s.request(0x1013)),
        [
            "Thank you for riding with us.",
            "The next station is Tōkamachi."
        ]
    );
    // Runs of phrases make one sentence.
    assert_eq!(
        texts(|s| s.request(0x1018)),
        [
            "The Akita Shinkansen Komachi for Tōkyō will soon depart from track 12.",
            "Please mind the closing doors."
        ]
    );
    assert_eq!(
        texts(|s| s.request(0x1021)),
        ["We will soon be arriving at Morioka."]
    );
    // Sound effects have no line.
    assert!(texts(|s| s.request(0x11)).is_empty());
}

#[test]
fn cue_timing_follows_the_phrases() {
    let b = book();
    let mut s = SoundSystem::new(Arc::new(tables()));
    s.set_voice_log(true);
    // pair[13] (delay 40) then the suffix: the suffix starts 41 sequencer ticks later and
    // lasts 1 s (60.6 sequencer ticks).
    s.call_pair(3, 13);
    let Some(VoiceEvent::Started(st)) = s.take_voice_events().pop() else {
        panic!("logged");
    };
    let c = &b.cues(&st)[0];
    assert_eq!(c.at, 0);
    let audio = ((41.0 + 2.0 * TICKS_PER_S) / 2.0f32).ceil() as u32;
    let read = READ_BASE_TICKS + READ_TICKS_PER_CHAR * c.text.chars().count() as u32;
    assert_eq!(c.ticks, (audio + LINGER_TICKS).max(read));
    // The announcement's "next" part comes up when its intro phrase plays: after
    // 101 sequencer ticks, i.e. in game tick 50 after the start.
    s.request(0x1013);
    let Some(VoiceEvent::Started(st)) = s.take_voice_events().pop() else {
        panic!("logged");
    };
    let c = &b.cues(&st)[1];
    assert_eq!(c.at, (101 - 1) / 2);
    // A short line is kept up for its reading time.
    let short = b.cue(&[Step::new(0x10, 0)], 0, 0, "Hi.".into()).unwrap();
    assert_eq!(
        short.ticks,
        (2.0 * TICKS_PER_S / 2.0).ceil() as u32 + LINGER_TICKS
    );
    let mut quiet = book();
    quiet.lengths = VoiceLengths(vec![0.1; 0x196]);
    let short = quiet
        .cue(&[Step::new(0x10, 0)], 0, 0, "Hi.".into())
        .unwrap();
    assert_eq!(short.ticks, READ_BASE_TICKS + 3 * READ_TICKS_PER_CHAR);
}

#[test]
fn lines_queue_replace_and_clear() {
    let mut subs = Subtitles::new(book());
    let push = |subs: &mut Subtitles, at: u32, ticks: u32, text: &str| {
        subs.pending.push((
            at,
            Cue {
                at,
                ticks,
                text: text.to_owned(),
            },
        ));
    };
    push(&mut subs, 0, 3, "One.");
    push(&mut subs, 1, 10, "Two.");
    subs.tick();
    assert_eq!(subs.lines().collect::<Vec<_>>(), ["One."]);
    subs.tick();
    assert_eq!(subs.lines().collect::<Vec<_>>(), ["One.", "Two."]);
    // A third line pushes the oldest out.
    push(&mut subs, 0, 10, "Three.");
    subs.tick();
    assert_eq!(subs.lines().collect::<Vec<_>>(), ["Two.", "Three."]);
    // The same line again is not repeated, only kept longer.
    push(&mut subs, 0, 50, "Two.");
    subs.tick();
    assert_eq!(subs.lines().collect::<Vec<_>>(), ["Two.", "Three."]);
    for _ in 0..12 {
        subs.tick();
    }
    assert_eq!(subs.lines().collect::<Vec<_>>(), ["Two."]);
    // A stop-all clears the screen and what was still to come.
    push(&mut subs, 5, 10, "Later.");
    subs.feed([VoiceEvent::Silenced]);
    for _ in 0..10 {
        subs.tick();
    }
    assert_eq!(subs.lines().count(), 0);
}

#[test]
fn events_become_lines_in_time() {
    let mut s = SoundSystem::new(Arc::new(tables()));
    s.set_voice_log(true);
    let mut subs = Subtitles::new(book());
    s.call_signal(0, 1, 0);
    subs.feed(s.take_voice_events());
    subs.tick();
    assert_eq!(subs.lines().collect::<Vec<_>>(), ["Home signal: Clear."]);
    let mut shown = 1;
    subs.tick();
    while subs.lines().count() > 0 {
        shown += 1;
        subs.tick();
    }
    // Two phrases: the aspect word 31 sequencer ticks after the kind, 1 s long, then the
    // linger; the reading time of 19 characters is longer.
    let audio = ((31.0 + 2.0 * TICKS_PER_S) / 2.0f32).ceil() as u32;
    assert!(audio + LINGER_TICKS < READ_BASE_TICKS + 19 * READ_TICKS_PER_CHAR);
    assert_eq!(shown, READ_BASE_TICKS + 19 * READ_TICKS_PER_CHAR);
}

#[test]
fn drawn_in_the_centre_group() {
    let mut subs = Subtitles::new(book());
    let mut layer = SpriteLayer::default();
    subs.draw(&mut layer);
    assert!(layer.widgets.is_empty());
    subs.show(Cue {
        at: 0,
        ticks: 5,
        text: "A.".into(),
    });
    subs.show(Cue {
        at: 0,
        ticks: 5,
        text: "B.".into(),
    });
    layer.pen = Anchor::Left;
    subs.draw(&mut layer);
    assert_eq!(layer.widgets.len(), 2);
    assert!(layer.widgets.iter().all(|w| w.anchor == Anchor::Centre));
    // The newest line at the bottom, ending at BOTTOM.
    let r = layer.widgets[0].rect;
    assert_eq!(r[1] + r[3], BOTTOM);
    assert!(layer.widgets[1].rect[1] < r[1]);
    assert!(layer.palettes[usize::from(PAL_SUBTITLE)].is_some());
}

/// With the coupling minigame's readout panel up, both lines sit above it.
#[test]
fn above_the_coupling_panel() {
    let mut subs = Subtitles::new(book());
    for text in ["A.", "B."] {
        subs.show(Cue {
            at: 0,
            ticks: 5,
            text: text.into(),
        });
    }
    let mut layer = SpriteLayer::default();
    // Slot 0x45 (the meter frame) at y 300 makes the panel's top.
    layer.slots[0x45] = ddg_render::sprites::Slot {
        x: 80,
        y: 300,
        tilemap: 0x17c,
        ..ddg_render::sprites::Slot::CLEAR
    };
    let top = crate::coupling_hud::panel_top(&layer);
    assert_eq!(top, Some(300.0));
    subs.draw_below(&mut layer, None, top);
    assert_eq!(layer.widgets.len(), 2);
    let r = layer.widgets[0].rect;
    assert_eq!(r[1] + r[3], 300.0 - PANEL_GAP);
    assert!(layer.widgets[1].rect[1] < r[1]);
    // The minigame's caption (slot 0x33) above the panel: above the caption.
    layer.slots[0x33] = ddg_render::sprites::Slot {
        y: 195,
        tilemap: 0x2d9,
        ..ddg_render::sprites::Slot::CLEAR
    };
    assert_eq!(crate::coupling_hud::panel_top(&layer), Some(195.0));
    // No panel: the usual place, even with a caption up.
    layer.slots[0x45] = ddg_render::sprites::Slot::CLEAR;
    assert_eq!(crate::coupling_hud::panel_top(&layer), None);
}

/// Against the user's data: every station of every stage has a subtitle name, the in-train
/// announcements that name a station resolve, and the clips with lines exist.
#[test]
fn book_from_the_data() {
    if std::env::var_os("DDG_DATA_DIR").is_none() {
        eprintln!("skipped: DDG_DATA_DIR not set");
        return;
    }
    let data = crate::scene::GameData::from_env().unwrap();
    let exe_bytes = data.exe().unwrap();
    let exe = ddg_formats::exe::Exe::parse(&exe_bytes).unwrap();
    let tables = SoundTables::from_exe(&exe).unwrap();
    let cat = crate::locale::Catalogue::parse(include_str!("../../../../locales/en.toml")).unwrap();
    let b = SubtitleBook::for_game(&cat, &exe, &data, &tables).unwrap();
    let hud = ddg_formats::hud::HudTables::read(&exe).unwrap();
    for (stage, ids) in hud.stage_station_ids.iter().enumerate() {
        for &id in ids {
            assert!(
                b.station(usize::from(id)).is_some(),
                "stage {stage}: no name for station-name id {id}"
            );
        }
    }
    let seq = |id: usize| Started {
        origin: Origin::Sequence(id),
        steps: tables.sequences[id].clone(),
    };
    // Hokuhoku and Kōbe line announcements name their next stop (audio-text.md §10.4).
    for (id, name) in [(0x13, 39), (0x16, 43), (0x2a, 76), (0x2b, 76), (0x2d, 88)] {
        let cues = b.cues(&seq(id));
        let want = format!("The next station is {}.", b.station(name).unwrap());
        let next = cues.iter().find(|c| c.text == want);
        assert!(next.is_some_and(|c| c.at > 0), "sequence {id:#x}: {cues:?}");
    }
    for id in 5..=0xb {
        let cues = b.cues(&seq(id));
        assert_eq!(cues[0].text, format!("Block signal {}: Clear.", id - 4));
    }
    for id in 0xc..=0x10 {
        let cues = b.cues(&seq(id));
        assert_eq!(cues[0].text, format!("Home signal {}: Clear.", id - 0xb));
    }
    // Every phrase of the announcements, the navigator and the result sequences is in a
    // sentence with a line; only the chime 0x1a7 is not.
    for id in (0..5)
        .chain(0x11..=0x23)
        .chain(0x25..=0x27)
        .chain(0x2a..=0x2e)
    {
        let steps = &tables.sequences[id];
        let mut i = 0;
        while i < steps.len() {
            let (end, text) = b.sentence_at(steps, i);
            let chime = steps[i].code == 0x1a7;
            assert!(
                chime || text.is_some(),
                "sequence {id:#x} step {i} ({:#x})",
                steps[i].code
            );
            i = end + 1;
        }
    }
    for (k, _) in &cat.subtitles {
        let codes = match (code_key(k), run_key(k)) {
            (Some(c), _) => vec![c],
            (_, Some((a, z))) => vec![a, z],
            _ => continue,
        };
        for c in codes {
            assert!(b.lengths.seconds(c as i16) > 0.15, "{k}: no voice");
        }
    }
}

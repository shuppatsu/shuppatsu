//! komachi.dat through our MPEG-1 decoders (docs/re/movie.md). Skips unless `DDG_DATA_DIR`
//! is set.

use ddg_formats::mpeg::{Movie, PictureType, mp2, ps};
use std::path::PathBuf;

fn movie_bytes() -> Option<Vec<u8>> {
    let root = PathBuf::from(std::env::var_os("DDG_DATA_DIR")?);
    std::fs::read(root.join("install/Program_Executable_Files/DATA/komachi.dat")).ok()
}

macro_rules! require_data {
    () => {
        match movie_bytes() {
            Some(d) => d,
            None => {
                eprintln!("skipped: DDG_DATA_DIR not set");
                return;
            }
        }
    };
}

#[test]
fn the_whole_movie_decodes_with_the_expected_frames_and_duration() {
    let data = require_data!();
    assert_eq!(ps::stream_ids(&data).unwrap(), vec![0xe0, 0xc0]);
    let mut movie = Movie::open(&data).unwrap();

    let seq = movie.video.read_sequence_header().unwrap().clone();
    assert_eq!((seq.width, seq.height), (320, 144));
    assert_eq!(seq.frame_rate(), (30_000, 1001));
    assert_eq!(seq.aspect_code, 12); // SAR 200:219
    assert_eq!(seq.bit_rate * 400, 2_048_000);
    assert_ne!(
        seq.non_intra_matrix, [16; 64],
        "the stream carries its own non-intra matrix"
    );

    let mut counts = [0usize; 3];
    let mut first_pts = None;
    let mut n = 0usize;
    loop {
        match movie.video.next_frame() {
            Ok(Some(f)) => {
                counts[match f.kind {
                    PictureType::I => 0,
                    PictureType::P => 1,
                    _ => 2,
                }] += 1;
                if n == 0 {
                    first_pts = f.pts;
                }
                n += 1;
                movie.video.recycle(f);
            }
            Ok(None) => break,
            Err(e) => panic!("video error at frame {n}: {e}"),
        }
    }
    assert_eq!(n, 4075);
    assert_eq!(counts, [278, 1082, 2715]);
    // 0.675 s: the first picture's PTS, also the audio's.
    assert_eq!(first_pts, Some(60_752));
    let video_seconds = n as f64 * 1001.0 / 30_000.0;
    assert!((video_seconds - 135.97).abs() < 0.01, "{video_seconds}");

    let audio = movie.audio.as_mut().unwrap();
    let h = audio.first_header().unwrap();
    assert_eq!(
        (h.sample_rate, h.bitrate_kbps, h.mode),
        (44_100, 192, mp2::Mode::Stereo)
    );
    let (mut frames, mut samples, mut audio_pts) = (0usize, 0usize, None);
    let mut peak = 0f32;
    loop {
        match audio.next_frame() {
            Ok(Some(f)) => {
                if frames == 0 {
                    audio_pts = f.pts;
                }
                frames += 1;
                samples += f.samples.len() / 2;
                peak = f.samples.iter().fold(peak, |m, v| m.max(v.abs()));
            }
            Ok(None) => break,
            Err(e) => panic!("audio error at frame {frames}: {e}"),
        }
    }
    assert_eq!(frames, 5205);
    assert_eq!(samples, 5205 * 1152);
    assert_eq!(audio_pts, Some(60_752));
    let audio_seconds = samples as f64 / 44_100.0;
    assert!(
        (audio_seconds - video_seconds).abs() < 0.01,
        "{audio_seconds} vs {video_seconds}"
    );
    assert!(peak > 0.5, "the soundtrack is not silent");
}

/// Damaged copies of the movie's first 300 KB decode without panicking.
#[test]
fn corrupted_movie_data_never_panics() {
    let data = require_data!();
    let head = &data[..300_000];
    let mut seed = 0x2545_f491u32;
    for round in 0..12 {
        let mut bad = head.to_vec();
        for _ in 0..(4 << (round % 4)) {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let i = seed as usize % bad.len();
            bad[i] ^= (seed >> 24) as u8 | 1;
        }
        let Ok(mut movie) = Movie::open(&bad) else {
            continue;
        };
        let mut calls = 0;
        while !matches!(movie.video.next_frame(), Ok(None)) {
            calls += 1;
            assert!(calls < 100_000);
        }
        if let Some(a) = movie.audio.as_mut() {
            while !matches!(a.next_frame(), Ok(None)) {}
        }
    }
}

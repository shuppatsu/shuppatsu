//! `shuppatsu-tools movie`: decode `komachi.dat` (or any MPEG-1 program stream) with our decoders,
//! time it, dump raw planes / PCM, and compare against reference dumps from another decoder
//! (scripts/movie-compare.sh makes them with ffmpeg; ffmpeg is never used at runtime).

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use shuppatsu_formats::mpeg::{Frame, Movie, PictureType};

pub const USAGE: &str = "\
usage: shuppatsu-tools movie <command> [file] [options]

  file defaults to $SHUPPATSU_DATA_DIR/install/Program_Executable_Files/DATA/komachi.dat

  info [file]                     stream facts, picture-type counts, duration, decode speed
  decode [file] [--yuv F] [--audio F] [--frames N]
                                  raw yuv420p planes (cropped) and f32le interleaved PCM
  compare [file] --ref-yuv F [--ref-audio F] [--frames N] [--csv F]
                                  per-frame PSNR against a reference yuv420p dump, audio SNR
                                  against a reference f32le dump
  png [file] --out DIR --at N[,N...] [--ref-yuv F] [--scale S]
                                  frames as PNG; with --ref-yuv, ours | reference | |diff|×8";

fn default_file() -> Result<PathBuf> {
    let root = std::env::var_os("SHUPPATSU_DATA_DIR")
        .map(PathBuf::from)
        .context("SHUPPATSU_DATA_DIR is not set; pass the file")?;
    Ok(root.join("install/Program_Executable_Files/DATA/komachi.dat"))
}

struct Opts {
    file: PathBuf,
    yuv: Option<PathBuf>,
    audio: Option<PathBuf>,
    ref_yuv: Option<PathBuf>,
    ref_audio: Option<PathBuf>,
    csv: Option<PathBuf>,
    out: Option<PathBuf>,
    frames: Option<usize>,
    at: Vec<usize>,
    scale: u32,
}

fn parse(args: &[String]) -> Result<Opts> {
    let mut o = Opts {
        file: PathBuf::new(),
        yuv: None,
        audio: None,
        ref_yuv: None,
        ref_audio: None,
        csv: None,
        out: None,
        frames: None,
        at: Vec::new(),
        scale: 2,
    };
    let mut file = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--yuv" => o.yuv = Some(val()?.into()),
            "--audio" => o.audio = Some(val()?.into()),
            "--ref-yuv" => o.ref_yuv = Some(val()?.into()),
            "--ref-audio" => o.ref_audio = Some(val()?.into()),
            "--csv" => o.csv = Some(val()?.into()),
            "--out" => o.out = Some(val()?.into()),
            "--frames" => o.frames = Some(val()?.parse()?),
            "--scale" => o.scale = val()?.parse()?,
            "--at" => {
                o.at = val()?
                    .split(',')
                    .map(|s| s.parse())
                    .collect::<std::result::Result<_, _>>()?
            }
            s if s.starts_with("--") => bail!("unknown option {s}\n\n{USAGE}"),
            s => file = Some(PathBuf::from(s)),
        }
    }
    o.file = match file {
        Some(f) => f,
        None => default_file()?,
    };
    Ok(o)
}

pub fn run(args: &[String]) -> Result<()> {
    let Some(cmd) = args.first() else {
        println!("{USAGE}");
        return Ok(());
    };
    let o = parse(&args[1..])?;
    match cmd.as_str() {
        "info" => info(&o),
        "decode" => decode(&o),
        "compare" => compare(&o),
        "png" => png(&o),
        _ => bail!("unknown movie command {cmd}\n\n{USAGE}"),
    }
}

fn open(o: &Opts) -> Result<Movie> {
    let data = std::fs::read(&o.file).with_context(|| o.file.display().to_string())?;
    Ok(Movie::open(&data)?)
}

fn info(o: &Opts) -> Result<()> {
    let t0 = Instant::now();
    let data = std::fs::read(&o.file).with_context(|| o.file.display().to_string())?;
    let read = t0.elapsed();
    let t1 = Instant::now();
    let mut movie = Movie::open(&data)?;
    let demux = t1.elapsed();
    let seq = movie.video.read_sequence_header()?.clone();
    let (num, den) = seq.frame_rate();
    println!(
        "video: {}×{}, {:.3} fps ({num}/{den}), aspect code {} (SAR {:.4}, DAR {:.4}), {} kbit/s, \
         custom intra matrix {}, custom non-intra matrix {}",
        seq.width,
        seq.height,
        f64::from(num) / f64::from(den),
        seq.aspect_code,
        seq.sample_aspect(),
        seq.display_aspect(),
        seq.bit_rate * 400 / 1000,
        seq.intra_matrix != default_intra(),
        seq.non_intra_matrix != [16; 64],
    );
    let t2 = Instant::now();
    let (mut n, mut counts, mut errors) = (0usize, [0usize; 4], 0usize);
    let (mut first_pts, mut last_pts) = (None, None);
    loop {
        match movie.video.next_frame() {
            Ok(Some(f)) => {
                n += 1;
                counts[match f.kind {
                    PictureType::I => 0,
                    PictureType::P => 1,
                    PictureType::B => 2,
                    PictureType::D => 3,
                }] += 1;
                if let Some(p) = f.pts {
                    first_pts.get_or_insert(p);
                    last_pts = Some(p);
                }
                movie.video.recycle(f);
            }
            Ok(None) => break,
            Err(e) => {
                errors += 1;
                eprintln!("video error: {e}");
            }
        }
    }
    let video_time = t2.elapsed();
    let duration = n as f64 * f64::from(den) / f64::from(num);
    println!(
        "  {n} frames (I {} P {} B {} D {}), {:.3} s at the frame rate, {errors} errors; first PTS {:?}, last {:?}",
        counts[0], counts[1], counts[2], counts[3], duration, first_pts, last_pts
    );
    println!(
        "  video decode {:.3} s = {:.2} ms/frame = {:.1}× real time",
        video_time.as_secs_f64(),
        video_time.as_secs_f64() * 1000.0 / n.max(1) as f64,
        duration / video_time.as_secs_f64()
    );
    if let Some(audio) = movie.audio.as_mut() {
        let h = audio.first_header()?;
        println!(
            "audio: Layer II {} Hz, {:?}, {} kbit/s, {} channels",
            h.sample_rate,
            h.mode,
            h.bitrate_kbps,
            h.channels()
        );
        let t3 = Instant::now();
        let (mut frames, mut samples, mut errors, mut first) = (0usize, 0usize, 0usize, None);
        loop {
            match audio.next_frame() {
                Ok(Some(f)) => {
                    frames += 1;
                    samples += f.samples.len() / f.channels;
                    if first.is_none() {
                        first = f.pts;
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    errors += 1;
                    eprintln!("audio error: {e}");
                }
            }
        }
        let t = t3.elapsed().as_secs_f64();
        let secs = samples as f64 / f64::from(h.sample_rate);
        println!(
            "  {frames} frames, {samples} samples per channel = {secs:.3} s, {errors} errors; first PTS {first:?}"
        );
        println!("  audio decode {t:.3} s = {:.1}× real time", secs / t);
    }
    println!(
        "file read {:.3} s, demux {:.3} s",
        read.as_secs_f64(),
        demux.as_secs_f64()
    );
    Ok(())
}

fn default_intra() -> [u8; 64] {
    [
        8, 16, 19, 22, 26, 27, 29, 34, 16, 16, 22, 24, 27, 29, 34, 37, 19, 22, 26, 27, 29, 34, 34,
        38, 22, 22, 26, 27, 29, 34, 37, 40, 22, 26, 27, 29, 32, 35, 40, 48, 26, 27, 29, 32, 35, 40,
        48, 58, 26, 27, 29, 34, 38, 46, 56, 69, 27, 29, 35, 38, 46, 56, 69, 83,
    ]
}

fn frame_bytes(f: &Frame) -> Vec<u8> {
    let mut v = f.y_plane();
    let (cb, cr) = f.chroma_planes();
    v.extend(cb);
    v.extend(cr);
    v
}

fn decode(o: &Opts) -> Result<()> {
    let mut movie = open(o)?;
    if let Some(path) = &o.yuv {
        let mut w = BufWriter::new(File::create(path)?);
        let mut n = 0;
        while o.frames.is_none_or(|max| n < max) {
            match movie.video.next_frame() {
                Ok(Some(f)) => {
                    w.write_all(&frame_bytes(&f))?;
                    n += 1;
                }
                Ok(None) => break,
                Err(e) => eprintln!("video error: {e}"),
            }
        }
        println!("{n} frames → {}", path.display());
    }
    if let (Some(path), Some(audio)) = (&o.audio, movie.audio.as_mut()) {
        let mut w = BufWriter::new(File::create(path)?);
        let mut n = 0;
        loop {
            match audio.next_frame() {
                Ok(Some(f)) => {
                    for s in &f.samples {
                        w.write_all(&s.to_le_bytes())?;
                    }
                    n += f.samples.len();
                }
                Ok(None) => break,
                Err(e) => eprintln!("audio error: {e}"),
            }
        }
        println!("{n} samples → {}", path.display());
    }
    Ok(())
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let se: f64 = a
        .iter()
        .zip(b)
        .map(|(&x, &y)| {
            let d = f64::from(x) - f64::from(y);
            d * d
        })
        .sum();
    if se == 0.0 {
        return f64::INFINITY;
    }
    let mse = se / a.len() as f64;
    10.0 * (255.0 * 255.0 / mse).log10()
}

fn compare(o: &Opts) -> Result<()> {
    let ref_path = o.ref_yuv.as_ref().context("compare needs --ref-yuv")?;
    let mut movie = open(o)?;
    let seq = movie.video.read_sequence_header()?.clone();
    let (w, h) = (usize::from(seq.width), usize::from(seq.height));
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let frame_len = w * h + 2 * cw * ch;
    let mut reference = BufReader::new(File::open(ref_path)?);
    let mut csv = match &o.csv {
        Some(p) => Some(BufWriter::new(File::create(p)?)),
        None => None,
    };
    if let Some(c) = csv.as_mut() {
        writeln!(
            c,
            "frame,type,psnr_y,psnr_cb,psnr_cr,psnr_all,max_abs_y,identical_y"
        )?;
    }
    let mut buf = vec![0u8; frame_len];
    let (mut n, mut min_all, mut sum_all, mut exact, mut finite) =
        (0usize, f64::INFINITY, 0.0, 0usize, 0usize);
    let mut per_type: [(usize, f64, f64); 4] = [(0, 0.0, f64::INFINITY); 4];
    let mut worst_frame = 0;
    while o.frames.is_none_or(|max| n < max) {
        let f = match movie.video.next_frame() {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => {
                eprintln!("video error: {e}");
                continue;
            }
        };
        if reference.read_exact(&mut buf).is_err() {
            println!("reference ended after {n} frames");
            break;
        }
        let ours = frame_bytes(&f);
        let (y0, y1) = (&ours[..w * h], &buf[..w * h]);
        let py = psnr(y0, y1);
        let pcb = psnr(&ours[w * h..w * h + cw * ch], &buf[w * h..w * h + cw * ch]);
        let pcr = psnr(&ours[w * h + cw * ch..], &buf[w * h + cw * ch..]);
        let pall = psnr(&ours, &buf);
        let max_abs = y0
            .iter()
            .zip(y1)
            .map(|(&a, &b)| (i32::from(a) - i32::from(b)).abs())
            .max()
            .unwrap_or(0);
        let identical = y0.iter().zip(y1).filter(|(a, b)| a == b).count() as f64 / y0.len() as f64;
        if pall.is_infinite() {
            exact += 1;
        } else {
            finite += 1;
            sum_all += pall;
        }
        if pall < min_all {
            min_all = pall;
            worst_frame = n;
        }
        let t = match f.kind {
            PictureType::I => 0,
            PictureType::P => 1,
            PictureType::B => 2,
            PictureType::D => 3,
        };
        let e = &mut per_type[t];
        e.0 += 1;
        e.1 += pall.min(99.0);
        e.2 = e.2.min(pall);
        if let Some(c) = csv.as_mut() {
            writeln!(
                c,
                "{n},{:?},{py:.3},{pcb:.3},{pcr:.3},{pall:.3},{max_abs},{identical:.5}",
                f.kind
            )?;
        }
        n += 1;
    }
    println!(
        "video: {n} frames compared; {exact} bit-exact; PSNR (all planes) mean of the inexact {:.2} dB, min {:.2} dB (frame {worst_frame})",
        if finite > 0 {
            sum_all / finite as f64
        } else {
            f64::INFINITY
        },
        min_all
    );
    for (name, e) in ["I", "P", "B", "D"].iter().zip(per_type) {
        if e.0 > 0 {
            println!(
                "  {name}: {} frames, mean {:.2} dB (exact frames counted as 99), min {:.2} dB",
                e.0,
                e.1 / e.0 as f64,
                e.2
            );
        }
    }
    if let (Some(path), Some(audio)) = (&o.ref_audio, movie.audio.as_mut()) {
        let mut bytes = Vec::new();
        File::open(path)?.read_to_end(&mut bytes)?;
        let reference: Vec<f32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect();
        let mut ours = Vec::with_capacity(reference.len());
        loop {
            match audio.next_frame() {
                // Through 16-bit PCM, as the reference (ffmpeg's fixed-point mp2 decoder) is.
                Ok(Some(f)) => ours.extend(
                    f.samples
                        .iter()
                        .map(|&v| f32::from(shuppatsu_formats::mpeg::mp2::to_i16(v)) / 32768.0),
                ),
                Ok(None) => break,
                Err(e) => eprintln!("audio error: {e}"),
            }
        }
        let n = ours.len().min(reference.len());
        let (mut sig, mut noise, mut peak) = (0f64, 0f64, 0f64);
        let same = ours[..n]
            .iter()
            .zip(&reference[..n])
            .filter(|(a, b)| a == b)
            .count();
        for (&a, &b) in ours[..n].iter().zip(&reference[..n]) {
            sig += f64::from(b) * f64::from(b);
            let d = f64::from(a) - f64::from(b);
            noise += d * d;
            peak = peak.max(d.abs());
        }
        // Per frame (1152 stereo samples): the worst SNR, ignoring near-silent frames.
        let (mut worst, mut worst_at, mut below_60) = (f64::INFINITY, 0, 0);
        for (i, (a, b)) in ours[..n]
            .chunks(2304)
            .zip(reference[..n].chunks(2304))
            .enumerate()
        {
            let s: f64 = b.iter().map(|&v| f64::from(v) * f64::from(v)).sum();
            let e: f64 = a
                .iter()
                .zip(b)
                .map(|(&x, &y)| (f64::from(x) - f64::from(y)).powi(2))
                .sum();
            if s < 1e-6 * b.len() as f64 {
                continue;
            }
            let snr = 10.0 * (s / e.max(1e-30)).log10();
            below_60 += usize::from(snr < 60.0);
            if snr < worst {
                worst = snr;
                worst_at = i;
            }
        }
        println!(
            "audio: worst frame SNR {worst:.2} dB (frame {worst_at}), {below_60} frames under 60 dB"
        );
        println!(
            "audio: {:.3} % of the 16-bit samples identical",
            100.0 * same as f64 / n.max(1) as f64
        );
        println!(
            "audio: {} vs {} samples; SNR {:.2} dB, peak difference {:.2e} ({:.2} LSB of 16 bits)",
            ours.len(),
            reference.len(),
            10.0 * (sig / noise.max(1e-30)).log10(),
            peak,
            peak * 32768.0
        );
    }
    Ok(())
}

fn png(o: &Opts) -> Result<()> {
    let out = o.out.clone().context("png needs --out")?;
    std::fs::create_dir_all(&out)?;
    let mut movie = open(o)?;
    let seq = movie.video.read_sequence_header()?.clone();
    let (w, h) = (usize::from(seq.width), usize::from(seq.height));
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let frame_len = w * h + 2 * cw * ch;
    let mut reference = match &o.ref_yuv {
        Some(p) => Some(BufReader::new(File::open(p)?)),
        None => None,
    };
    let last = o.at.iter().copied().max().context("png needs --at")?;
    let mut buf = vec![0u8; frame_len];
    let mut rgba = Vec::new();
    for n in 0..=last {
        let f = loop {
            match movie.video.next_frame() {
                Ok(Some(f)) => break f,
                Ok(None) => bail!("the stream has only {n} frames"),
                Err(e) => eprintln!("video error: {e}"),
            }
        };
        if let Some(r) = reference.as_mut() {
            r.read_exact(&mut buf)?;
        }
        if !o.at.contains(&n) {
            continue;
        }
        f.to_rgba(&mut rgba);
        let s = o.scale.max(1) as usize;
        let panels = if reference.is_some() { 3 } else { 1 };
        let gap = if panels > 1 { 4 * s } else { 0 };
        let (pw, ph) = (w * s, h * s);
        let width = pw * panels + gap * (panels - 1);
        let mut img = image::RgbImage::new(width as u32, ph as u32);
        let mut blit = |panel: usize, px: &dyn Fn(usize, usize) -> [u8; 3]| {
            for y in 0..ph {
                for x in 0..pw {
                    let c = px(x / s, y / s);
                    img.put_pixel((panel * (pw + gap) + x) as u32, y as u32, image::Rgb(c));
                }
            }
        };
        blit(0, &|x, y| {
            let o = (y * w + x) * 4;
            [rgba[o], rgba[o + 1], rgba[o + 2]]
        });
        if reference.is_some() {
            let mut rf = f.clone();
            for row in 0..h {
                rf.y[row * rf.y_stride..row * rf.y_stride + w]
                    .copy_from_slice(&buf[row * w..row * w + w]);
            }
            for row in 0..ch {
                let cb = &buf[w * h + row * cw..w * h + row * cw + cw];
                let cr = &buf[w * h + cw * ch + row * cw..w * h + cw * ch + row * cw + cw];
                rf.cb[row * rf.c_stride..row * rf.c_stride + cw].copy_from_slice(cb);
                rf.cr[row * rf.c_stride..row * rf.c_stride + cw].copy_from_slice(cr);
            }
            let mut rgba_ref = Vec::new();
            rf.to_rgba(&mut rgba_ref);
            blit(1, &|x, y| {
                let o = (y * w + x) * 4;
                [rgba_ref[o], rgba_ref[o + 1], rgba_ref[o + 2]]
            });
            let ours = frame_bytes(&f);
            blit(2, &|x, y| {
                let d = |a: usize| {
                    ((i32::from(ours[a]) - i32::from(buf[a])).unsigned_abs() * 8).min(255) as u8
                };
                let c = w * h + (y / 2) * cw + x / 2;
                [d(y * w + x), d(c), d(c + cw * ch)]
            });
        }
        let path = out.join(format!("frame_{n:04}.png"));
        img.save(&path)?;
        println!("{}", path.display());
    }
    Ok(())
}

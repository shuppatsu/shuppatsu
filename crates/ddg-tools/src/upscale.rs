//! `ddg-tools upscale-textures`: builds a texture pack (docs/texture-packs.md) from the user's
//! `tex*.rom` files. The pack is derived from game data: it goes to the gitignored `out/` or
//! the user's own directory, never into the repository.
//!
//! Per page:
//! 1. Regions: the whole page (padded by wrapping, as tiled textures repeat across it), then
//!    every distinct texture rect (padded with its own edge pixels, so atlas neighbours do not
//!    bleed in). Rects are pasted after the page, the largest first.
//! 2. Each region is cut out with its padding; its colour-keyed texels (RGB565 `0x0000`, the
//!    only transparent value) get the colours of their opaque neighbours by dilation, so the
//!    upscaler never sees black next to an edge.
//! 3. The colour is upscaled by the method; the key mask separately, by xBRZ on a black/white
//!    image thresholded at one half (or nearest neighbour), so transparency stays crisp.
//! 4. The padding is cut off and the region pasted at its scaled place. Transparent texels
//!    keep their dilated colour with alpha 0 (smooth filtering blends towards it, not black).

use crate::export::GameDirs;
use anyhow::{Context, Result, bail};
use ddg_formats::tex::{TexFile, TexRect};
use ddg_formats::texpack::{RgbaImage, encode_qoi, page_path};
use ddg_render::sprites::xbrz;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

pub const USAGE: &str = "\
upscale-textures --method <xbrz|esrgan|waifu2x|nearest> [--scale N] [--out DIR]
                 [--model NAME] [--mask <smooth|nearest>] [--pad N] [--only texr1,textr2]
                 [--tool PATH] [--noise 0..3] [--keep-work]
  builds a texture pack from $DDG_DATA_DIR's tex*.rom (default --scale 4, --out
  out/texpacks/<method>[-<model>]). esrgan runs realesrgan-ncnn-vulkan (Vulkan; --model
  realesrgan-x4plus [default], realesrgan-x4plus-anime, realesr-animevideov3), waifu2x runs
  waifu2x-converter-cpp (OpenCL, else CPU); both from PATH or --tool, e.g.
  `nix shell nixpkgs#realesrgan-ncnn-vulkan nixpkgs#waifu2x-converter-cpp`";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Method {
    Nearest,
    Xbrz,
    Esrgan { model: String },
    Waifu2x { noise: u8 },
}

impl Method {
    /// The pack's default directory name: `xbrz`, `esrgan-<model>`, `waifu2x`, `nearest`.
    pub fn name(&self) -> String {
        match self {
            Self::Nearest => "nearest".to_owned(),
            Self::Xbrz => "xbrz".to_owned(),
            Self::Esrgan { model } => format!("esrgan-{model}"),
            Self::Waifu2x { .. } => "waifu2x".to_owned(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    pub method: Method,
    pub scale: u32,
    pub out: PathBuf,
    pub smooth_mask: bool,
    pub pad: u32,
    pub only: Option<Vec<String>>,
    pub tool: Option<PathBuf>,
    pub keep_work: bool,
}

pub fn parse_args(args: &[String]) -> Result<Options> {
    let mut method = None;
    let mut model = "realesrgan-x4plus".to_owned();
    let mut noise = 1u8;
    let mut scale = 4;
    let mut out = None;
    let mut smooth_mask = true;
    let mut pad = None;
    let mut only = None;
    let mut tool = None;
    let mut keep_work = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = || {
            it.next()
                .with_context(|| format!("{a} needs a value\n\n{USAGE}"))
        };
        match a.as_str() {
            "--method" => method = Some(val()?.clone()),
            "--model" => model = val()?.clone(),
            "--noise" => noise = val()?.parse().context("--noise needs 0..3")?,
            "--scale" => scale = val()?.parse().context("--scale needs a whole number")?,
            "--out" => out = Some(PathBuf::from(val()?)),
            "--mask" => {
                smooth_mask = match val()?.as_str() {
                    "smooth" => true,
                    "nearest" => false,
                    v => bail!("--mask {v}: expected smooth or nearest"),
                }
            }
            "--pad" => pad = Some(val()?.parse().context("--pad needs a number")?),
            "--only" => {
                only = Some(
                    val()?
                        .split(',')
                        .map(|s| s.trim().to_ascii_lowercase())
                        .collect(),
                )
            }
            "--tool" => tool = Some(PathBuf::from(val()?)),
            "--keep-work" => keep_work = true,
            other => bail!("unknown option {other}\n\n{USAGE}"),
        }
    }
    let method = match method.as_deref() {
        Some("nearest") => Method::Nearest,
        Some("xbrz") => Method::Xbrz,
        Some("esrgan") => Method::Esrgan {
            model: model.clone(),
        },
        Some("waifu2x") => Method::Waifu2x {
            noise: noise.min(3),
        },
        Some(m) => bail!("unknown method {m}\n\n{USAGE}"),
        None => bail!("--method is required\n\n{USAGE}"),
    };
    if !(1..=8).contains(&scale) {
        bail!("--scale must be 1..=8");
    }
    let name = method.name();
    let ai = matches!(method, Method::Esrgan { .. } | Method::Waifu2x { .. });
    Ok(Options {
        out: out.unwrap_or_else(|| PathBuf::from("out/texpacks").join(name)),
        pad: pad.unwrap_or(if ai { 8 } else { 2 }),
        method,
        scale,
        smooth_mask,
        only,
        tool,
        keep_work,
    })
}

/// A pixel region of a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// Padding repeats the page (tiled textures) instead of the region's edge pixels.
    pub wrap: bool,
}

/// The pixels a rect covers on a `pw`×`ph` page (1/256-page units, rounded outwards; the
/// stored size is size − 1).
pub fn rect_pixels(r: &TexRect, pw: u32, ph: u32) -> (u32, u32, u32, u32) {
    let span = |start: u8, size_m1: u8, len: u32| {
        let a = u32::from(start) * len / 256;
        let b = ((u32::from(start) + u32::from(size_m1) + 1) * len).div_ceil(256);
        (a.min(len), b.min(len))
    };
    let (x0, x1) = span(r.u, r.w, pw);
    let (y0, y1) = span(r.v, r.h, ph);
    (x0, y0, x1, y1)
}

/// The regions of a page: the whole page (wrapping), then each distinct rect that is not the
/// whole page, largest first.
pub fn regions<'a>(pw: u32, ph: u32, rects: impl IntoIterator<Item = &'a TexRect>) -> Vec<Region> {
    let mut out = vec![Region {
        x: 0,
        y: 0,
        w: pw,
        h: ph,
        wrap: true,
    }];
    let mut sub: Vec<Region> = Vec::new();
    for r in rects {
        let (x0, y0, x1, y1) = rect_pixels(r, pw, ph);
        let reg = Region {
            x: x0,
            y: y0,
            w: x1 - x0,
            h: y1 - y0,
            wrap: false,
        };
        if reg.w == 0 || reg.h == 0 || (reg.w == pw && reg.h == ph) || sub.contains(&reg) {
            continue;
        }
        sub.push(reg);
    }
    sub.sort_by_key(|r| std::cmp::Reverse(r.w * r.h));
    out.extend(sub);
    out
}

/// A padded region: RGB plus the opaque mask, `w`×`h`.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    pub w: u32,
    pub h: u32,
    pub rgb: Vec<[u8; 3]>,
    pub opaque: Vec<bool>,
}

/// Cuts `reg` with `pad` pixels around it out of a page (`rgba`, alpha 0 = keyed).
pub fn extract(rgba: &[u8], pw: u32, ph: u32, reg: Region, pad: u32) -> Tile {
    let (w, h) = (reg.w + 2 * pad, reg.h + 2 * pad);
    let mut tile = Tile {
        w,
        h,
        rgb: Vec::with_capacity((w * h) as usize),
        opaque: Vec::with_capacity((w * h) as usize),
    };
    let coord = |c: i64, start: u32, len: u32, page: u32| -> u32 {
        if reg.wrap {
            c.rem_euclid(i64::from(page)) as u32
        } else {
            c.clamp(i64::from(start), i64::from(start + len) - 1) as u32
        }
    };
    for ty in 0..h {
        let y = coord(
            i64::from(reg.y) + i64::from(ty) - i64::from(pad),
            reg.y,
            reg.h,
            ph,
        );
        for tx in 0..w {
            let x = coord(
                i64::from(reg.x) + i64::from(tx) - i64::from(pad),
                reg.x,
                reg.w,
                pw,
            );
            let i = ((y * pw + x) * 4) as usize;
            tile.rgb.push([rgba[i], rgba[i + 1], rgba[i + 2]]);
            tile.opaque.push(rgba[i + 3] != 0);
        }
    }
    tile
}

/// Gives every keyed texel a colour: repeatedly the mean of its already coloured 8-neighbours
/// (a dilation outwards from the opaque texels). A tile without any opaque texel stays as is.
pub fn fill_keyed(tile: &mut Tile) {
    let (w, h) = (tile.w as i32, tile.h as i32);
    let mut known = tile.opaque.clone();
    if !known.iter().any(|&k| k) {
        return;
    }
    loop {
        let mut next = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) as usize;
                if known[i] {
                    continue;
                }
                let (mut sum, mut n) = ([0u32; 3], 0);
                for (dx, dy) in [
                    (-1, -1),
                    (0, -1),
                    (1, -1),
                    (-1, 0),
                    (1, 0),
                    (-1, 1),
                    (0, 1),
                    (1, 1),
                ] {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= w || ny >= h {
                        continue;
                    }
                    let j = (ny * w + nx) as usize;
                    if known[j] {
                        for (s, v) in sum.iter_mut().zip(tile.rgb[j]) {
                            *s += u32::from(v);
                        }
                        n += 1;
                    }
                }
                if n > 0 {
                    next.push((i, sum.map(|s| ((s + n / 2) / n) as u8)));
                }
            }
        }
        if next.is_empty() {
            break;
        }
        for (i, c) in next {
            tile.rgb[i] = c;
            known[i] = true;
        }
    }
}

/// Nearest-neighbour enlargement of a `w`×`h` buffer.
pub(crate) fn nearest<T: Copy>(src: &[T], w: u32, h: u32, s: u32) -> Vec<T> {
    let (ow, oh) = (w * s, h * s);
    (0..ow * oh)
        .map(|i| src[((i / ow / s) * w + (i % ow) / s) as usize])
        .collect()
}

/// The opaque mask enlarged by `s`: xBRZ on black/white, thresholded at one half (`smooth`),
/// or nearest neighbour.
pub fn scale_mask(opaque: &[bool], w: u32, h: u32, s: u32, smooth: bool) -> Vec<bool> {
    if !smooth || s == 1 {
        return nearest(opaque, w, h, s);
    }
    let bw: Vec<[u8; 3]> = opaque
        .iter()
        .map(|&o| if o { [255; 3] } else { [0; 3] })
        .collect();
    xbrz::scale(&bw, w as usize, h as usize, s as usize)
        .iter()
        .map(|p| p[0] >= 128)
        .collect()
}

/// Box-filters a `w`×`h` RGB buffer down by a whole factor `f`.
fn box_down(src: &[[u8; 3]], w: u32, h: u32, f: u32) -> Vec<[u8; 3]> {
    let (ow, oh) = (w / f, h / f);
    let mut out = Vec::with_capacity((ow * oh) as usize);
    for y in 0..oh {
        for x in 0..ow {
            let mut sum = [0u32; 3];
            for k in 0..f * f {
                let p = src[((y * f + k / f) * w + x * f + k % f) as usize];
                for (s, v) in sum.iter_mut().zip(p) {
                    *s += u32::from(v);
                }
            }
            out.push(sum.map(|s| ((s + f * f / 2) / (f * f)) as u8));
        }
    }
    out
}

/// Pastes an upscaled tile (colour `rgb`, mask `opaque`, both (w·s)×(h·s) with the padding)
/// into the output page at the region's place, without the padding.
pub fn paste(
    page: &mut RgbaImage,
    reg: Region,
    pad: u32,
    s: u32,
    tile: &Tile,
    rgb: &[[u8; 3]],
    opaque: &[bool],
) {
    let tw = (tile.w * s) as usize;
    for y in 0..reg.h * s {
        for x in 0..reg.w * s {
            let src = ((y + pad * s) as usize) * tw + (x + pad * s) as usize;
            let dst = (((reg.y * s + y) * page.width + reg.x * s + x) * 4) as usize;
            let c = rgb[src];
            page.rgba[dst..dst + 4].copy_from_slice(&[
                c[0],
                c[1],
                c[2],
                if opaque[src] { 255 } else { 0 },
            ]);
        }
    }
}

/// One tile of the job, with where it goes.
struct Job {
    file: usize,
    page: usize,
    region: Region,
    tile: Tile,
}

pub fn run(args: &[String]) -> Result<()> {
    let opts = parse_args(args)?;
    let dirs = GameDirs::from_env()?;
    let t0 = Instant::now();
    let names: Vec<String> = dirs
        .data_names("tex")?
        .into_iter()
        .filter(|n| {
            opts.only.as_ref().is_none_or(|o| {
                let stem = n.rsplit_once('.').map_or(n.as_str(), |(s, _)| s);
                o.iter().any(|x| x.eq_ignore_ascii_case(stem))
            })
        })
        .collect();
    if names.is_empty() {
        bail!("no tex*.rom matched");
    }
    let files: Vec<TexFile> = names
        .iter()
        .map(|n| TexFile::parse(&dirs.read_data(n)?).with_context(|| n.clone()))
        .collect::<Result<_>>()?;

    // Tiles of every page.
    let mut jobs = Vec::new();
    let mut pages: Vec<Vec<RgbaImage>> = Vec::new();
    for (fi, tex) in files.iter().enumerate() {
        let mut out_pages = Vec::new();
        for (pi, page) in tex.pages.iter().enumerate() {
            let (pw, ph) = (u32::from(page.width), u32::from(page.height));
            let rgba = page.to_rgba8();
            let rects = tex.rects.iter().filter(|r| usize::from(r.page) == pi);
            for region in regions(pw, ph, rects) {
                let mut tile = extract(&rgba, pw, ph, region, opts.pad);
                fill_keyed(&mut tile);
                jobs.push(Job {
                    file: fi,
                    page: pi,
                    region,
                    tile,
                });
            }
            out_pages.push(RgbaImage {
                width: pw * opts.scale,
                height: ph * opts.scale,
                rgba: vec![0; (pw * ph * opts.scale * opts.scale * 4) as usize],
            });
        }
        pages.push(out_pages);
    }
    let n_pages: usize = pages.iter().map(Vec::len).sum();
    eprintln!(
        "{} files, {n_pages} pages, {} tiles; upscaling ×{} with {}",
        files.len(),
        jobs.len(),
        opts.scale,
        opts.method.name()
    );

    let t_up = Instant::now();
    let tiles: Vec<&Tile> = jobs.iter().map(|j| &j.tile).collect();
    let scaled = upscale_tiles(&opts, &tiles)?;
    let up_time = t_up.elapsed();

    let s = opts.scale;
    let masks = parallel_map(&jobs, |j| {
        scale_mask(&j.tile.opaque, j.tile.w, j.tile.h, s, opts.smooth_mask)
    });
    for ((job, rgb), mask) in jobs.iter().zip(&scaled).zip(&masks) {
        paste(
            &mut pages[job.file][job.page],
            job.region,
            opts.pad,
            s,
            &job.tile,
            rgb,
            mask,
        );
    }

    let mut bytes = 0u64;
    for (name, file_pages) in names.iter().zip(&pages) {
        for (i, img) in file_pages.iter().enumerate() {
            let path = opts.out.join(page_path(name, i));
            std::fs::create_dir_all(path.parent().expect("a page path has a dir"))?;
            let q = encode_qoi(img);
            bytes += q.len() as u64;
            std::fs::write(&path, q).with_context(|| path.display().to_string())?;
        }
    }
    let total = t0.elapsed();
    std::fs::write(
        opts.out.join("pack.toml"),
        format!(
            "# Texture pack made by ddg-tools upscale-textures (docs/texture-packs.md).\n\
             # Derived from the game data: never commit it.\nmethod = \"{}\"\nscale = {}\n\
             mask = \"{}\"\npad = {}\npages = {n_pages}\ntiles = {}\nupscale_seconds = {:.1}\n\
             total_seconds = {:.1}\n",
            opts.method.name(),
            opts.scale,
            if opts.smooth_mask {
                "smooth"
            } else {
                "nearest"
            },
            opts.pad,
            jobs.len(),
            up_time.as_secs_f64(),
            total.as_secs_f64()
        ),
    )?;
    println!(
        "{n_pages} pages written to {} ({:.1} MiB) in {:.1?} (upscaling {:.1?})",
        opts.out.display(),
        bytes as f64 / 1048576.0,
        total,
        up_time
    );
    Ok(())
}

/// `f` over `items` on all cores, results in order.
fn parallel_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let n = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = items.len().div_ceil(n).max(1);
    std::thread::scope(|sc| {
        let handles: Vec<_> = items
            .chunks(chunk)
            .map(|c| sc.spawn(|| c.iter().map(&f).collect::<Vec<R>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("worker panicked"))
            .collect()
    })
}

/// The upscaled colour of every tile, (w·s)×(h·s), by `opts.method` at `opts.scale` (external
/// tools work in `opts.out/.work`).
pub(crate) fn upscale_tiles(opts: &Options, tiles: &[&Tile]) -> Result<Vec<Vec<[u8; 3]>>> {
    let s = opts.scale;
    match &opts.method {
        Method::Nearest => Ok(parallel_map(tiles, |t| nearest(&t.rgb, t.w, t.h, s))),
        Method::Xbrz => Ok(parallel_map(tiles, |t| {
            xbrz::scale(&t.rgb, t.w as usize, t.h as usize, s as usize)
        })),
        Method::Esrgan { model } => {
            // The x4plus models only scale ×4; animevideov3 has ×2, ×3 and ×4.
            let native = if model.starts_with("realesr-animevideov3") && (2..=4).contains(&s) {
                s
            } else {
                4
            };
            let tool = opts
                .tool
                .clone()
                .unwrap_or_else(|| PathBuf::from("realesrgan-ncnn-vulkan"));
            external(opts, tiles, native, |inp, out| {
                let mut c = Command::new(&tool);
                c.arg("-i").arg(inp).arg("-o").arg(out).args([
                    "-n",
                    model,
                    "-s",
                    &native.to_string(),
                    "-f",
                    "png",
                ]);
                c
            })
        }
        Method::Waifu2x { noise } => {
            let tool = opts
                .tool
                .clone()
                .unwrap_or_else(|| PathBuf::from("waifu2x-converter-cpp"));
            external(opts, tiles, s, |inp, out| {
                let mut c = Command::new(&tool);
                c.arg("-i")
                    .arg(inp)
                    .arg("-o")
                    .arg(out)
                    .args(["--scale-ratio", &s.to_string()])
                    .args(["--noise-level", &noise.to_string()])
                    .args(["-m", if *noise > 0 { "noise-scale" } else { "scale" }])
                    .args(["-f", "png"]);
                c
            })
        }
    }
}

/// Runs an external upscaler once over a directory of tile PNGs and reads the results back
/// (box-filtered down when the tool's factor `native` is larger than the pack's).
fn external(
    opts: &Options,
    tiles: &[&Tile],
    native: u32,
    command: impl Fn(&Path, &Path) -> Command,
) -> Result<Vec<Vec<[u8; 3]>>> {
    let s = opts.scale;
    if !native.is_multiple_of(s) && native != s {
        bail!("the tool scales ×{native}, which ×{s} does not divide");
    }
    let work = opts.out.join(".work");
    let (inp, outp) = (work.join("in"), work.join("out"));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&inp)?;
    std::fs::create_dir_all(&outp)?;
    let indices: Vec<usize> = (0..tiles.len()).collect();
    parallel_map(&indices, |&idx| -> Result<()> {
        let t = tiles[idx];
        let buf: Vec<u8> = t.rgb.iter().flatten().copied().collect();
        image::RgbImage::from_raw(t.w, t.h, buf)
            .context("tile size")?
            .save(inp.join(format!("t{idx:06}.png")))?;
        Ok(())
    })
    .into_iter()
    .collect::<Result<()>>()?;
    let mut cmd = command(&inp, &outp);
    eprintln!("running {cmd:?}");
    let status = cmd.status().with_context(|| {
        format!(
            "starting {:?} (is it on PATH? see --tool)",
            cmd.get_program()
        )
    })?;
    if !status.success() {
        bail!("{:?} failed: {status}", cmd.get_program());
    }
    let produced: Vec<PathBuf> = std::fs::read_dir(&outp)?
        .filter_map(|e| Some(e.ok()?.path()))
        .collect();
    let find = |idx: usize| {
        let stem = format!("t{idx:06}");
        produced
            .iter()
            .find(|p| {
                p.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                    n == format!("{stem}.png") || n.starts_with(&format!("{stem}_"))
                })
            })
            .cloned()
    };
    let result = parallel_map(&indices, |&idx| -> Result<Vec<[u8; 3]>> {
        let t = tiles[idx];
        let path = find(idx).with_context(|| format!("no output for tile {idx}"))?;
        let img = image::open(&path)
            .with_context(|| path.display().to_string())?
            .to_rgb8();
        let (w, h) = (t.w * native, t.h * native);
        if img.dimensions() != (w, h) {
            bail!(
                "{}: {:?}, expected {w}×{h}",
                path.display(),
                img.dimensions()
            );
        }
        let rgb: Vec<[u8; 3]> = img.as_raw().as_chunks::<3>().0.to_vec();
        Ok(if native == s {
            rgb
        } else {
            box_down(&rgb, w, h, native / s)
        })
    })
    .into_iter()
    .collect::<Result<Vec<_>>>()?;
    if !opts.keep_work {
        let _ = std::fs::remove_dir_all(&work);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(u: u8, v: u8, w: u8, h: u8) -> TexRect {
        TexRect {
            id: 0,
            page: 0,
            u,
            v,
            w,
            h,
        }
    }

    #[test]
    fn rect_pixels_round_outwards() {
        assert_eq!(rect_pixels(&rect(0, 0, 255, 255), 64, 32), (0, 0, 64, 32));
        assert_eq!(
            rect_pixels(&rect(128, 64, 127, 63), 256, 256),
            (128, 64, 256, 128)
        );
        // 16-px page: 16 units per pixel; 8..24 covers pixels 0 and 1.
        assert_eq!(rect_pixels(&rect(8, 0, 15, 15), 16, 16), (0, 0, 2, 1));
    }

    #[test]
    fn regions_whole_page_first_then_distinct_rects_largest_first() {
        let rs = [
            rect(0, 0, 255, 255),
            rect(0, 0, 63, 63),
            rect(64, 0, 191, 127),
            rect(0, 0, 63, 63),
        ];
        let r = regions(256, 256, &rs);
        assert_eq!(r.len(), 3);
        assert!(r[0].wrap && r[0].w == 256);
        assert_eq!((r[1].x, r[1].w, r[1].h, r[1].wrap), (64, 192, 128, false));
        assert_eq!((r[2].w, r[2].h), (64, 64));
    }

    /// A 4×2 page: an opaque 2×2 red rect on the left, a transparent texel and a blue one on
    /// the right.
    fn page() -> (Vec<u8>, u32, u32) {
        let red = [255, 0, 0, 255];
        let blue = [0, 0, 255, 255];
        let key = [0, 0, 0, 0];
        let px = [red, red, key, blue, red, red, blue, blue];
        (px.concat(), 4, 2)
    }

    #[test]
    fn padding_repeats_the_rects_own_edge_not_the_neighbour() {
        let (rgba, w, h) = page();
        let reg = Region {
            x: 0,
            y: 0,
            w: 2,
            h: 2,
            wrap: false,
        };
        let t = extract(&rgba, w, h, reg, 2);
        assert_eq!((t.w, t.h), (6, 6));
        // Everything is red: the padding never reaches the blue texels at x = 2..4.
        assert!(t.rgb.iter().all(|&c| c == [255, 0, 0]));
        assert!(t.opaque.iter().all(|&o| o));
        // Wrapping (the whole page) brings the right edge in on the left.
        let whole = Region {
            x: 0,
            y: 0,
            w: 4,
            h: 2,
            wrap: true,
        };
        let t = extract(&rgba, w, h, whole, 1);
        assert_eq!(
            t.rgb[t.w as usize],
            [0, 0, 255],
            "x = -1 is the page's last column"
        );
    }

    #[test]
    fn keyed_texels_take_neighbour_colours_and_stay_transparent() {
        let (rgba, w, h) = page();
        let whole = Region {
            x: 0,
            y: 0,
            w: 4,
            h: 2,
            wrap: false,
        };
        let mut t = extract(&rgba, w, h, whole, 0);
        assert_eq!(t.rgb[2], [0, 0, 0]);
        fill_keyed(&mut t);
        // The keyed texel is the mean of its 5 opaque neighbours (red, red, blue, blue, blue).
        assert_eq!(t.rgb[2], [102, 0, 153]);
        assert!(!t.opaque[2]);

        // Through the whole pipeline at ×2 (nearest colour, nearest mask): the keyed texel is
        // a transparent 2×2 block with its filled colour.
        let mut out = RgbaImage {
            width: 8,
            height: 4,
            rgba: vec![9; 8 * 4 * 4],
        };
        let rgb = nearest(&t.rgb, t.w, t.h, 2);
        let mask = scale_mask(&t.opaque, t.w, t.h, 2, false);
        paste(&mut out, whole, 0, 2, &t, &rgb, &mask);
        let at = |x: u32, y: u32| {
            let i = ((y * 8 + x) * 4) as usize;
            out.rgba[i..i + 4].to_vec()
        };
        assert_eq!(at(4, 0), [102, 0, 153, 0]);
        assert_eq!(at(5, 1), [102, 0, 153, 0]);
        assert_eq!(at(0, 3), [255, 0, 0, 255]);
        assert_eq!(at(7, 0), [0, 0, 255, 255]);
    }

    #[test]
    fn smooth_mask_keeps_solid_areas_and_rounds_steps() {
        // An L-shaped opaque area: the smooth mask is opaque inside, transparent outside.
        let opaque = [
            true, false, false, //
            true, true, false, //
            true, true, true,
        ];
        let m = scale_mask(&opaque, 3, 3, 4, true);
        assert_eq!(m.len(), 144);
        assert!(m[(11 * 12) + 1], "deep inside stays opaque");
        assert!(!m[11], "far outside stays transparent");
        let near = scale_mask(&opaque, 3, 3, 4, false);
        assert_ne!(m, near, "the staircase is smoothed");
    }

    #[test]
    fn box_down_averages() {
        let src = [[0, 0, 0], [255, 255, 255], [255, 255, 255], [0, 0, 0]];
        assert_eq!(box_down(&src, 2, 2, 2), [[128, 128, 128]]);
    }
}

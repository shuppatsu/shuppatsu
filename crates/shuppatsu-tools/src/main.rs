//! Developer CLI for reconnaissance and asset inspection.

mod check;
mod export;
mod frame_report;
mod gltf;
mod hud_upscale;
mod inventory;
mod movie;
mod pe;
mod sound;
mod trace;
mod upscale;

use anyhow::{Context, Result, bail};
use std::path::PathBuf;

const USAGE: &str = "\
usage: shuppatsu-tools <command> [args]

commands:
  inventory <dir>                 list files with size, magic, hash; extension histogram
  manifest <dir> <variant>        print a data-manifest TOML (hashes only) for <dir>
  pe <file>                       PE sections/entropy, imports, Rich header, resources
  strings <file> [--min N] [--jp] ASCII and Shift-JIS strings with offsets
  pack-rgba <in.png> <out>        run-length pack an image of the port's own art (the logo,
                                  scripts/build-logo.sh) for embedding in the game
  export <what> [--out DIR] [--palette N]
                                  write PNG/WAV from $SHUPPATSU_DATA_DIR to out/export (default)
                                  what: textures | tilemaps | audio | models | routes | all
  frame-report [--hz N] <trace.tsv>...
                                  per-screen frame times of SHUPPATSU_FRAME_TRACE files
                                  (docs/performance.md)
  check [-v]                      Phase 2 exit check: every file in $SHUPPATSU_DATA_DIR parsed or documented
  trace-replay <trace.csv> [-v] [--rules | --drive]
                                  replay an oracle trace through the engine physics (or, with
                                  --rules, the game rules; with --drive, the whole engine from
                                  the logged notches and buttons only) and compare
  play-sound <code> [--wav FILE] [--rate HZ] [--seconds S] [--pitch P | --sweep FROM:TO]
                    [--volume V] [--release-after S] [--resampler linear|nearest|sinc]
                                  play a sound code (hex 0x31 / 31h or decimal; 0x1000|n =
                                  sequence n) live, or render it to a WAV (gitignored out/)
  movie <info|decode|compare|png> [file] [...]
                                  decode komachi.dat with our MPEG-1 decoders: facts and speed,
                                  raw dumps, PSNR/SNR against a reference (movie for the options)
  upscale-textures --method <xbrz|esrgan|waifu2x|nearest> [--scale N] [--out DIR] [...]
                                  build a texture pack from $SHUPPATSU_DATA_DIR (upscale-textures
                                  --help for the options; docs/texture-packs.md)
  upscale-sprites [--frame <shot.layer.txt>]... [--out DIR]
                                  contact sheets of the HUD sprites upscaled by every method
                                  (upscale-sprites --help; docs/texture-packs.md)";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else {
        println!("{USAGE}");
        return Ok(());
    };
    let path = |i: usize| -> Result<PathBuf> {
        args.get(i)
            .map(PathBuf::from)
            .with_context(|| format!("missing argument\n\n{USAGE}"))
    };

    match cmd.as_str() {
        "inventory" => inventory::print_inventory(&path(1)?),
        "manifest" => {
            let variant = args.get(2).context("missing variant name")?;
            inventory::print_manifest(&path(1)?, variant)
        }
        "pe" => pe::print_report(&path(1)?),
        "pack-rgba" => {
            let img = image::open(path(1)?)?.into_rgba8();
            let packed = shuppatsu_render::rgba::RgbaImage {
                width: img.width(),
                height: img.height(),
                rgba: img.into_raw(),
            }
            .pack();
            let out = path(2)?;
            std::fs::write(&out, &packed)?;
            println!("wrote {} ({} bytes)", out.display(), packed.len());
            Ok(())
        }
        "play-sound" => sound::play_sound(&args[1..]),
        "movie" => movie::run(&args[1..]),
        "export" => run_export(&args[1..]),
        "upscale-textures" => {
            if args.iter().any(|a| a == "--help" || a == "-h") {
                println!("{}", upscale::USAGE);
                return Ok(());
            }
            upscale::run(&args[1..])
        }
        "upscale-sprites" => {
            if args.iter().any(|a| a == "--help" || a == "-h") {
                println!("{}", hud_upscale::USAGE);
                return Ok(());
            }
            hud_upscale::run(&args[1..])
        }
        "trace-replay" => {
            let dirs = export::GameDirs::from_env()?;
            let flag = |f: &str| args.iter().skip(2).any(|a| a == f);
            let exe = dirs.bin.join("dengo3k.exe");
            let ok = if flag("--drive") {
                trace::drive::replay_drive(&path(1)?, &exe, &dirs.data, flag("-v"))?
            } else if flag("--rules") {
                trace::rules::replay_rules(&path(1)?, &exe, flag("-v"))?
            } else {
                trace::replay_physics(&path(1)?, &exe, flag("-v"))?
            };
            if ok { Ok(()) } else { bail!("trace mismatch") }
        }
        "frame-report" => frame_report::run(&args[1..]),
        "check" => {
            let verbose = args.get(1).is_some_and(|a| a == "-v");
            let root = std::env::var_os("SHUPPATSU_DATA_DIR")
                .map(PathBuf::from)
                .context("SHUPPATSU_DATA_DIR is not set (run inside `nix develop`)")?;
            if check::run(&root, verbose)? {
                Ok(())
            } else {
                bail!("some files failed to parse or are unclassified")
            }
        }
        "strings" => {
            let mut min = 6;
            let mut jp_only = false;
            let mut rest = args[2..].iter();
            while let Some(a) = rest.next() {
                match a.as_str() {
                    "--min" => min = rest.next().context("--min needs a value")?.parse()?,
                    "--jp" => jp_only = true,
                    other => bail!("unknown option {other}"),
                }
            }
            let data = std::fs::read(path(1)?)?;
            for s in shuppatsu_formats::strings::scan(&data, min) {
                if !jp_only || s.japanese {
                    println!("{:08x}  {}", s.offset, s.text.replace('\n', "\\n"));
                }
            }
            Ok(())
        }
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            Ok(())
        }
        other => bail!("unknown command {other}\n\n{USAGE}"),
    }
}

fn run_export(args: &[String]) -> Result<()> {
    let what = args.first().context("export: missing <what>")?.as_str();
    let mut out = PathBuf::from("out/export");
    let mut palette = 4u8;
    let mut rest = args[1..].iter();
    while let Some(a) = rest.next() {
        match a.as_str() {
            "--out" => out = PathBuf::from(rest.next().context("--out needs a path")?),
            "--palette" => palette = rest.next().context("--palette needs N")?.parse()?,
            other => bail!("unknown option {other}"),
        }
    }
    let dirs = export::GameDirs::from_env()?;
    let all = what == "all";
    let mut any = false;
    if all || what == "textures" {
        println!("textures: {} pages", export::textures(&dirs, &out)?);
        any = true;
    }
    if all || what == "tilemaps" {
        println!(
            "tilemaps: {} images",
            export::tilemaps(&dirs, &out, palette)?
        );
        any = true;
    }
    if all || what == "audio" {
        println!("audio: {} files", export::audio(&dirs, &out)?);
        any = true;
    }
    if all || what == "models" {
        println!("models: {} glTF files", export::models(&dirs, &out)?);
        any = true;
    }
    if all || what == "routes" {
        println!("routes: {} glTF files", export::routes(&dirs, &out)?);
        any = true;
    }
    if !any {
        bail!("unknown export target {what}");
    }
    println!("written to {}", out.display());
    Ok(())
}

//! `ddg-tools check`: the Phase 2 exit criterion. Every extracted file must either parse with a
//! ddg-formats parser or be documented here as content the reimplementation does not need.

use anyhow::Result;
use ddg_formats::{audio, cg, exe::Exe, lz, rom2::Rom2, tex::TexFile};
use std::path::Path;

pub enum Status {
    Parsed(String),
    Documented(&'static str),
    Failed(String),
    Unknown,
}

type Parser = fn(&[u8]) -> std::result::Result<String, String>;

fn ok<T, E: std::fmt::Display>(
    r: std::result::Result<T, E>,
    f: impl FnOnce(T) -> String,
) -> std::result::Result<String, String> {
    r.map(f).map_err(|e| e.to_string())
}

fn parse_parts(d: &[u8]) -> std::result::Result<String, String> {
    let rom = Rom2::parse(d).map_err(|e| e.to_string())?;
    for (i, entry) in rom.iter().enumerate() {
        let block = entry.get(2..).ok_or(format!("entry {i}: too short"))?;
        let unpacked = lz::unpack_block(block).map_err(|e| format!("entry {i}: {e}"))?;
        ddg_formats::part::Part::parse(&unpacked).map_err(|e| format!("entry {i}: {e}"))?;
    }
    Ok(format!("{} model parts", rom.len()))
}

fn parse_route(d: &[u8]) -> std::result::Result<String, String> {
    ok(ddg_formats::route::Route::parse(d), |r| {
        format!("route, {} main-line points", r.main_line().len())
    })
}

fn parse_mpeg_ps(d: &[u8]) -> std::result::Result<String, String> {
    // MPEG-1 system stream pack header 00 00 01 BA with the '0010' marker (decision D2).
    if d.len() > 4 && d[..4] == [0, 0, 1, 0xba] && d[4] >> 4 == 2 {
        Ok("MPEG-1 program stream (playback postponed, D2)".into())
    } else {
        Err("not an MPEG-1 program stream".into())
    }
}

/// Classification rules for game files, by lowercase file name.
fn game_parser(name: &str) -> Option<Parser> {
    let n = name.to_ascii_lowercase();
    Some(match n.as_str() {
        "cg.rom" => |d| ok(cg::CgFile::parse(d), |c| format!("{} tiles", c.len())),
        "tbl.rom" => |d| ok(cg::parse_tbl(d), |m| format!("{} tilemaps", m.len())),
        "effect.rom" => |d| ok(audio::parse_effects(d), |e| format!("{} effects", e.len())),
        "bgm.rom" => |d| ok(audio::parse_bgm(d), |t| format!("{} BGM tracks", t.len())),
        "komachi.dat" => parse_mpeg_ps,
        "dengo3k.exe" | "dengosetting.exe" => |d| {
            ok(Exe::parse(d), |e| {
                format!("PE32, {} sections", e.section_names().count())
            })
        },
        _ if n.starts_with("tex") && n.ends_with(".rom") => |d| {
            ok(TexFile::parse(d), |t| {
                format!("{} pages, {} rects", t.pages.len(), t.rects.len())
            })
        },
        _ if n.starts_with("parts") && n.ends_with(".rom") => parse_parts,
        _ if n.ends_with(".com") => parse_route,
        _ => return None,
    })
}

/// Content the reimplementation does not need, by path (relative to DDG_DATA_DIR, lowercase).
fn documented(rel: &str) -> Option<&'static str> {
    let r = rel.to_ascii_lowercase();
    let file = r.rsplit('/').next().unwrap_or(&r);
    Some(if r.starts_with("disc/directx6/") {
        "Microsoft DirectX 6.1 / DirectX Media redistributable"
    } else if r.starts_with("disc/driver/") || r.starts_with("driver/") {
        "official controller driver (Win9x game-port VxD) and its installer; see docs/recon.md"
    } else if r.starts_with("setup-support/") {
        "InstallShield runtime support files"
    } else if r.starts_with("install/") && file.starts_with("cd2") && file.ends_with(".dll") {
        "Cd2 CD-ROM scan library used by the disc check (copy protection, not analysed)"
    } else if r.starts_with("disc/") && file == "data1.cab" {
        "InstallShield 5 cabinet holding the game (extracted to install/)"
    } else if r.starts_with("disc/") {
        match file {
            "autorun.exe" | "autorun.inf" | "autorun.ini" | "autorun.bmp" | "dengo3k.ico" => {
                "CD autorun launcher"
            }
            "setup.exe" | "setup.ini" | "setup.ins" | "setup.lid" | "_inst32i.ex_"
            | "_isdel.exe" | "_setup.dll" | "_sys1.cab" | "_user1.cab" | "lang.dat"
            | "layout.bin" | "os.dat" | "data.tag" => "InstallShield 5 installer",
            "readme.txt" => "Japanese README (install notes, driver tips)",
            _ => return None,
        }
    } else {
        return None;
    })
}

pub fn classify(root: &Path, path: &Path) -> Status {
    let rel = path
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if rel.starts_with("install/")
        && let Some(parser) = game_parser(&name)
    {
        return match std::fs::read(path) {
            Ok(data) => match parser(&data) {
                Ok(s) => Status::Parsed(s),
                Err(e) => Status::Failed(e),
            },
            Err(e) => Status::Failed(e.to_string()),
        };
    }
    documented(&rel).map_or(Status::Unknown, Status::Documented)
}

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<std::result::Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if e.file_type()?.is_dir() {
            walk(&e.path(), out)?;
        } else {
            out.push(e.path());
        }
    }
    Ok(())
}

/// Prints one line per file plus a summary. Returns false if anything failed or is unknown.
pub fn run(root: &Path, verbose: bool) -> Result<bool> {
    let mut files = Vec::new();
    walk(root, &mut files)?;
    let (mut parsed, mut doc, mut failed, mut unknown) = (0, 0, 0, 0);
    for path in &files {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path)
            .display()
            .to_string();
        match classify(root, path) {
            Status::Parsed(s) => {
                parsed += 1;
                if verbose {
                    println!("ok       {rel}: {s}");
                }
            }
            Status::Documented(s) => {
                doc += 1;
                if verbose {
                    println!("doc      {rel}: {s}");
                }
            }
            Status::Failed(e) => {
                failed += 1;
                println!("FAILED   {rel}: {e}");
            }
            Status::Unknown => {
                unknown += 1;
                println!("UNKNOWN  {rel}");
            }
        }
    }
    println!(
        "{} files: {parsed} parsed, {doc} documented, {failed} failed, {unknown} unknown",
        files.len()
    );
    Ok(failed == 0 && unknown == 0)
}

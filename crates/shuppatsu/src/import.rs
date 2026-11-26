//! Importing the game data from what the user has: a disc image, a copy of the disc's files
//! (or the disc itself, mounted), or an installed copy of the game (docs/re/disc-import.md).
//!
//! An image or a disc copy is unpacked once into the port's own data folder ([`import_dir`]):
//! the 57 files of the disc's InstallShield cabinet, each checked against the committed hash
//! manifest (`data-manifest/3000-bandai.toml`) before anything is kept. The game then finds
//! them there by itself ([`crate::data_dir::locate`]), and the image is no longer needed. An
//! installed game is used where it is, so nothing is copied.
//!
//! Archives (`.zip`, `.rar`, self-extracting `.exe`, …) are recognised only to say that they
//! must be extracted first (decision 2026-11-25: no archive dependencies).

use anyhow::{Context, Result, bail};
use shuppatsu_formats::disc::{Iso, ReadAt, Track, parse_cue};
use shuppatsu_formats::installshield::Cabinet;
use shuppatsu_formats::sha256::sha256;
use std::fs::File;
use std::path::{Path, PathBuf};

/// The variant the manifest describes; the only one the engine plays (docs/recon.md §7).
pub const VARIANT: &str = "3000-bandai";
/// The committed per-file hashes (hashes only, no game data).
const MANIFEST: &str = include_str!("../../../data-manifest/3000-bandai.toml");
/// Where the cabinet's tree sits in the manifest's paths.
const MANIFEST_PREFIX: &str = "install/Program_Executable_Files/";
/// Written into the import folder last; its presence marks a finished import.
pub const MARKER: &str = "shuppatsu-import.toml";

/// What a chosen file or folder is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// An installed game or an already extracted disc: the folder holding `DATA` and `BIN`.
    Installed(PathBuf),
    /// A copy of the disc's files, or a mounted disc: its `data1.cab`.
    DiscFolder(PathBuf),
    /// A disc image: the file holding the data track (the `.bin` of a `.cue`, the `.mdf` of
    /// an `.mds`).
    Image(PathBuf),
    /// An archive that has to be extracted first; the extension, for the message.
    Archive(String),
    /// Nothing the game can use.
    Unknown,
}

impl Source {
    /// Whether [`import`] can take it.
    pub fn usable(&self) -> bool {
        matches!(
            self,
            Self::Installed(_) | Self::DiscFolder(_) | Self::Image(_)
        )
    }
}

/// Extensions of disc images (the browser marks them).
pub const IMAGE_EXTENSIONS: [&str; 6] = ["cue", "mds", "mdf", "bin", "iso", "img"];
const ARCHIVE_EXTENSIONS: [&str; 9] = ["zip", "rar", "7z", "exe", "lzh", "tar", "gz", "xz", "cab"];

/// Looks at `path` (cheaply: names, a cue sheet, never a whole image).
pub fn classify(path: &Path) -> Source {
    if path.is_dir() {
        if let Some(d) = crate::data_dir::try_install_dir(path) {
            return Source::Installed(d);
        }
        if let Some(cab) = find_ignoring_case(path, "data1.cab") {
            return Source::DiscFolder(cab);
        }
        return Source::Unknown;
    }
    let ext = extension(path);
    match ext.as_str() {
        "cue" => std::fs::read(path)
            .ok()
            .and_then(|t| parse_cue(&t).ok())
            .map(|cue| {
                let dir = path.parent().unwrap_or(Path::new("."));
                let file = Path::new(&cue.file)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or(cue.file);
                find_ignoring_case(dir, &file).map_or(Source::Unknown, Source::Image)
            })
            .unwrap_or(Source::Unknown),
        "mds" => {
            let dir = path.parent().unwrap_or(Path::new("."));
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());
            stem.and_then(|s| find_ignoring_case(dir, &format!("{s}.mdf")))
                .map_or(Source::Unknown, Source::Image)
        }
        "mdf" | "bin" | "iso" | "img" => Source::Image(path.to_owned()),
        e if ARCHIVE_EXTENSIONS.contains(&e) => Source::Archive(e.to_owned()),
        _ => Source::Unknown,
    }
}

fn extension(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// `dir/name`, matching the name without regard to case (disc copies keep the disc's case).
fn find_ignoring_case(dir: &Path, name: &str) -> Option<PathBuf> {
    let exact = dir.join(name);
    if exact.is_file() {
        return Some(exact);
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
        .map(|e| e.path())
        .filter(|p| p.is_file())
}

/// The port's folder for imported game data: `%LOCALAPPDATA%\shuppatsu\game` on Windows,
/// `~/Library/Application Support/shuppatsu/game` on macOS,
/// else `$XDG_DATA_HOME/shuppatsu/game` or `~/.local/share/shuppatsu/game`; `$SHUPPATSU_HOME/game`
/// when that is set (crate::save::HOME_VAR).
pub fn import_dir() -> Option<PathBuf> {
    import_dir_in(|k| std::env::var_os(k))
}

fn import_dir_in(env: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    let absolute = |v: std::ffi::OsString| Some(PathBuf::from(v)).filter(|p| p.is_absolute());
    if let Some(home) = env(crate::save::HOME_VAR).and_then(absolute) {
        return Some(home.join("game"));
    }
    let base = if cfg!(windows) {
        env("LOCALAPPDATA").and_then(absolute)?
    } else if cfg!(target_os = "macos") {
        crate::save::mac_support_dir(&env)?
    } else {
        env("XDG_DATA_HOME").and_then(absolute).or_else(|| {
            env("HOME")
                .and_then(absolute)
                .map(|h| h.join(".local/share"))
        })?
    };
    Some(base.join("shuppatsu").join("game"))
}

/// The finished import, if there is one.
pub fn imported() -> Option<PathBuf> {
    let dir = import_dir()?;
    dir.join(MARKER).is_file().then_some(dir)
}

/// How far an import is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    /// Bytes of game files written, of `total`.
    pub done: u64,
    pub total: u64,
    /// The file being unpacked (`DATA/texR1.rom`).
    pub file: String,
}

/// Imports `source`: unpacks and verifies an image or a disc copy into [`import_dir`], or
/// checks an installed game. Returns the folder holding `DATA` and `BIN`. `progress` is called
/// before each file and at the end.
pub fn import(source: &Source, progress: &mut dyn FnMut(&Progress)) -> Result<PathBuf> {
    import_any(source, progress).map_err(|e| {
        // Reading and writing files fails for its own reasons; the rest is the copy itself.
        if e.chain()
            .any(|c| c.downcast_ref::<std::io::Error>().is_some())
        {
            e
        } else {
            e.context(crate::exit::Exit::WrongDisc)
        }
    })
}

fn import_any(source: &Source, progress: &mut dyn FnMut(&Progress)) -> Result<PathBuf> {
    let want = manifest();
    match source {
        Source::Installed(dir) => {
            check_installed(dir, &want)?;
            Ok(dir.clone())
        }
        Source::DiscFolder(cab) => {
            let src = DiskFile::open(cab)?;
            unpack(&src, &want, progress)
        }
        Source::Image(path) => {
            let src = DiskFile::open(path)?;
            let track = Track::detect(&src, 0).with_context(|| {
                format!(
                    "{}: no CD data track found (not a disc image, or an unsupported layout)",
                    path.display()
                )
            })?;
            let iso = Iso::new(track)?;
            let cab = iso.find("DATA1.CAB")?.with_context(|| {
                format!(
                    "{}: this disc has no DATA1.CAB; it is not the game's disc (volume {:?})",
                    path.display(),
                    iso.volume
                )
            })?;
            unpack(&iso.open(&cab), &want, progress)
        }
        Source::Archive(ext) => bail!(
            "this is a .{ext} archive: extract it first with your archive tool, then choose the \
             disc image (.cue, .mds/.mdf, .bin or .iso) that was inside"
        ),
        Source::Unknown => bail!("not a disc image, a copy of the disc or an installed game"),
    }
}

/// An installed game: its exe must be the 3000-bandai one (other variants aren't supported).
fn check_installed(dir: &Path, want: &[(String, [u8; 32])]) -> Result<()> {
    let (_, hash) = want
        .iter()
        .find(|(p, _)| p == "BIN/dengo3k.exe")
        .context("manifest has the exe")?;
    let exe = dir.join("BIN").join("dengo3k.exe");
    let data = std::fs::read(&exe).with_context(|| format!("reading {}", exe.display()))?;
    if &sha256(&data) != hash {
        bail!(
            "{}: this is not the {VARIANT} release of the game (its dengo3k.exe differs); only \
             that release is supported so far",
            dir.display()
        );
    }
    Ok(())
}

/// Unpacks every file of the cabinet at `src` into a fresh folder next to [`import_dir`],
/// checks it against the manifest, and moves the folder into place.
fn unpack(
    src: &(impl ReadAt + ?Sized),
    want: &[(String, [u8; 32])],
    progress: &mut dyn FnMut(&Progress),
) -> Result<PathBuf> {
    let cab = Cabinet::read(src).context("reading the disc's InstallShield cabinet")?;
    let dest = import_dir().context("no folder for the game data (HOME / LOCALAPPDATA unset)")?;
    let parent = dest.parent().context("import folder has a parent")?;
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let staging = parent.join("game.partial");
    if staging.exists() {
        std::fs::remove_dir_all(&staging)
            .with_context(|| format!("removing {}", staging.display()))?;
    }

    let mut p = Progress {
        total: cab.files.iter().map(|f| u64::from(f.expanded_size)).sum(),
        ..Progress::default()
    };
    let mut seen = 0;
    for f in &cab.files {
        p.file = f.path.clone();
        progress(&p);
        let Some((_, hash)) = want.iter().find(|(path, _)| *path == f.path) else {
            bail!(
                "{} is not a file of the {VARIANT} disc: this is a different release of the game, \
                 which is not supported yet",
                f.path
            );
        };
        let data = cab
            .extract(src, f)
            .with_context(|| format!("unpacking {}", f.path))?;
        if &sha256(&data) != hash {
            bail!(
                "{} differs from the {VARIANT} disc's: the image is damaged or a different \
                 release",
                f.path
            );
        }
        let out = staging.join(&f.path);
        if let Some(d) = out.parent() {
            std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
        }
        std::fs::write(&out, &data).with_context(|| format!("writing {}", out.display()))?;
        p.done += data.len() as u64;
        seen += 1;
    }
    if seen != want.len() {
        bail!(
            "the disc's cabinet has {seen} of the {} game files; the image is incomplete",
            want.len()
        );
    }
    std::fs::write(
        staging.join(MARKER),
        format!("# Written by shuppatsu after a verified import.\nvariant = \"{VARIANT}\"\n"),
    )?;
    if dest.exists() {
        std::fs::remove_dir_all(&dest).with_context(|| format!("removing {}", dest.display()))?;
    }
    std::fs::rename(&staging, &dest)
        .with_context(|| format!("moving {} to {}", staging.display(), dest.display()))?;
    p.file.clear();
    progress(&p);
    Ok(dest)
}

/// The manifest's installed files: `(path below the install folder, sha256)`.
fn manifest() -> Vec<(String, [u8; 32])> {
    let mut out = Vec::new();
    let mut path = None;
    for line in MANIFEST.lines() {
        if let Some(p) = line.strip_prefix("path = \"") {
            path = p
                .strip_suffix('"')
                .and_then(|p| p.strip_prefix(MANIFEST_PREFIX))
                .map(str::to_owned);
        } else if let Some(h) = line.strip_prefix("sha256 = \"")
            && let Some(p) = path.take()
            && let Some(hash) = parse_hex(h.trim_end_matches('"'))
        {
            out.push((p, hash));
        }
    }
    out
}

fn parse_hex(s: &str) -> Option<[u8; 32]> {
    let mut out = [0u8; 32];
    if s.len() != 64 {
        return None;
    }
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// A file read at offsets.
struct DiskFile {
    file: File,
    len: u64,
}

impl DiskFile {
    fn open(path: &Path) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let len = file.metadata()?.len();
        Ok(Self { file, len })
    }
}

impl ReadAt for DiskFile {
    fn size(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> shuppatsu_formats::Result<()> {
        read_exact_at(&self.file, offset, buf).map_err(|e| {
            shuppatsu_formats::FormatError::Invalid {
                what: "disc image",
                detail: format!("read at {offset:#x}: {e}"),
            }
        })
    }
}

#[cfg(unix)]
fn read_exact_at(file: &File, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::read_exact_at(file, buf, offset)
}

#[cfg(windows)]
fn read_exact_at(file: &File, offset: u64, buf: &mut [u8]) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    let mut done = 0;
    while done < buf.len() {
        match file.seek_read(&mut buf[done..], offset + done as u64)? {
            0 => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            n => done += n,
        }
    }
    Ok(())
}

/// `shuppatsu import <image or folder>`: imports from the command line, with progress.
///
/// `shuppatsu import --progress [--game=<path>]` is the launcher's windowless setup step
/// (RecompLauncher's `prepare`): it prints `progress <0..1> <text>` lines on stdout, finishes
/// at once with `progress 1 …` when a finished import is there (an update or a second copy
/// changes nothing: every import is the same verified files), and without `--game` only
/// checks for one.
pub fn run(args: &[String]) -> Result<()> {
    if args.iter().any(|a| a == "--progress") {
        let game = args
            .iter()
            .find_map(|a| a.strip_prefix("--game="))
            .map(PathBuf::from);
        return prepare(game.as_deref());
    }
    let [path] = args else {
        bail!(
            "usage: shuppatsu import <disc image (.cue, .mds/.mdf, .bin, .iso) or folder>\n       \
             shuppatsu import --progress [--game=<path>]"
        );
    };
    let source = classify(Path::new(path));
    let dir = import_printing(&source)?;
    match source {
        Source::Installed(_) => println!("{} is a usable game folder", dir.display()),
        _ => println!("imported and verified into {}", dir.display()),
    }
    Ok(())
}

/// `--disc <path>` (a launcher passes the same image on every start): an installed game is
/// used where it is; an image or disc copy is imported unless a finished import exists.
pub fn ensure(path: &Path) -> Result<()> {
    let source = classify(path);
    if let Source::Installed(dir) = &source {
        check_installed(dir, &manifest())?;
        crate::data_dir::choose(dir.clone());
        return Ok(());
    }
    if imported().is_some() {
        return Ok(());
    }
    let dir = import_printing(&source).with_context(|| format!("--disc {}", path.display()))?;
    eprintln!("imported and verified into {}", dir.display());
    Ok(())
}

/// The launcher's setup step ([`run`] with `--progress`). The reason for a failure is the last
/// thing printed (by `main`), in words a player can act on.
fn prepare(game: Option<&Path>) -> Result<()> {
    use std::io::Write;
    let mut out = std::io::stdout();
    if imported().is_some() {
        writeln!(out, "progress 1 Game data already imported")?;
        return Ok(());
    }
    let Some(game) = game else {
        bail!("no game data imported yet: choose your disc image again");
    };
    let source = classify(game);
    if matches!(source, Source::Installed(_)) {
        bail!(
            "{} is an installed game, not a disc image: choose the disc image (.cue, .mds/.mdf, \
             .bin or .iso)",
            game.display()
        );
    }
    writeln!(out, "progress 0 Reading your disc image")?;
    let mut last = String::new();
    import(&source, &mut |p| {
        if p.file != last {
            let frac = p.done as f64 / p.total.max(1) as f64;
            let text = if p.file.is_empty() {
                "Checked all game files".to_owned()
            } else {
                format!("Unpacking {}", p.file)
            };
            // Progress is best effort: a closed pipe must not fail the import.
            let _ = writeln!(out, "progress {frac:.3} {text}");
            let _ = out.flush();
            last.clone_from(&p.file);
        }
    })?;
    writeln!(out, "progress 1 Game data imported and verified")?;
    Ok(())
}

/// [`import`] with a line on stderr per file.
fn import_printing(source: &Source) -> Result<PathBuf> {
    let mut last = String::new();
    import(source, &mut |p| {
        if !p.file.is_empty() && p.file != last {
            eprintln!("{:>3}%  {}", p.done * 100 / p.total.max(1), p.file);
            last.clone_from(&p.file);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_lists_the_installed_files() {
        let m = manifest();
        assert_eq!(m.len(), 57);
        assert!(m.iter().any(|(p, _)| p == "BIN/dengo3k.exe"));
        assert!(
            m.iter()
                .all(|(p, _)| p.starts_with("BIN/") || p.starts_with("DATA/"))
        );
    }

    #[test]
    fn import_dir_follows_the_platform() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| std::ffi::OsString::from(v))
            }
        };
        // One folder for everything when a launcher gives one.
        let home: &'static str = if cfg!(windows) { "C:\\r" } else { "/r" };
        let with_home =
            move |k: &str| (k == crate::save::HOME_VAR).then(|| std::ffi::OsString::from(home));
        assert_eq!(
            import_dir_in(with_home),
            Some(PathBuf::from(home).join("game"))
        );
        assert_eq!(
            crate::save::config_dir_in(with_home),
            Some(PathBuf::from(home))
        );
        assert_eq!(
            crate::marks::dir_in(with_home),
            Some(PathBuf::from(home).join("marks"))
        );
        if cfg!(windows) {
            assert_eq!(
                import_dir_in(env(&[("LOCALAPPDATA", "C:\\Users\\a\\AppData\\Local")])),
                Some(PathBuf::from(
                    "C:\\Users\\a\\AppData\\Local\\shuppatsu\\game"
                ))
            );
        } else {
            assert_eq!(
                import_dir_in(env(&[("HOME", "/home/a")])),
                Some(PathBuf::from("/home/a/.local/share/shuppatsu/game"))
            );
            assert_eq!(
                import_dir_in(env(&[("XDG_DATA_HOME", "/x"), ("HOME", "/home/a")])),
                Some(PathBuf::from("/x/shuppatsu/game"))
            );
            assert_eq!(import_dir_in(env(&[("XDG_DATA_HOME", "rel")])), None);
        }
    }

    #[test]
    fn classifies_by_name_and_content() {
        let tmp = std::env::temp_dir().join(format!("shuppatsu-import-{}", std::process::id()));
        let disc = tmp.join("disc");
        std::fs::create_dir_all(&disc).unwrap();
        std::fs::write(disc.join("DATA1.CAB"), b"").unwrap();
        std::fs::write(tmp.join("game.mdf"), b"").unwrap();
        std::fs::write(tmp.join("game.mds"), b"").unwrap();
        std::fs::write(tmp.join("Game.BIN"), b"").unwrap();
        std::fs::write(
            tmp.join("game.cue"),
            "FILE \"game.bin\" BINARY\n TRACK 01 MODE1/2352\n",
        )
        .unwrap();
        std::fs::write(tmp.join("game.zip"), b"").unwrap();
        assert_eq!(classify(&disc), Source::DiscFolder(disc.join("DATA1.CAB")));
        assert_eq!(
            classify(&tmp.join("game.mds")),
            Source::Image(tmp.join("game.mdf"))
        );
        assert_eq!(
            classify(&tmp.join("game.cue")),
            Source::Image(tmp.join("Game.BIN"))
        );
        assert_eq!(
            classify(&tmp.join("game.zip")),
            Source::Archive("zip".into())
        );
        assert_eq!(classify(&tmp.join("missing.cue")), Source::Unknown);
        assert_eq!(classify(&tmp), Source::Unknown);
        assert!(!Source::Archive("rar".into()).usable());
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}

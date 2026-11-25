//! Where the user's extracted disc data is (docs/building.md). The first that is set wins:
//!
//! 1. the `SHUPPATSU_DATA_DIR` environment variable;
//! 2. the `--data <dir>` flag (any position, any subcommand);
//! 3. `data/extracted` or `data` next to the executable;
//! 4. `data/extracted` or `data` in the current directory (a checkout, `nix run .`);
//! 5. `data_dir` in config.toml (a relative path is relative to the config directory);
//! 6. the game data imported from a disc image (crate::import, `shuppatsu import`, the setup
//!    screen or `--disc`).
//!
//! A directory may be the extracted disc (`install/Program_Executable_Files/{DATA,BIN}` inside,
//! as scripts/extract-disc.sh writes it) or an installed copy of the game (`DATA` and `BIN`
//! directly inside), or a folder holding either of those in an `extracted` subfolder (so
//! `--data data` finds `data/extracted`).

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The `--data` value, recorded by [`args`].
static FLAG: OnceLock<Option<PathBuf>> = OnceLock::new();
/// The `--disc` value (an image or folder to import from, crate::import), recorded by [`args`].
static DISC: OnceLock<Option<PathBuf>> = OnceLock::new();
/// A game folder chosen at run time (an installed game given to `--disc` or picked on the
/// setup screen), used right after `--data`.
static CHOSEN: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Makes [`locate`] return `dir` (a folder holding `DATA` and `BIN`) unless the environment
/// or `--data` names one.
pub fn choose(dir: PathBuf) {
    *CHOSEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(dir);
}

/// The installed layout inside an extracted disc.
const DISC_INSTALL: &str = "install/Program_Executable_Files";

/// The command line without `--data <dir>` and `--disc <path>` (or `=` forms), which it
/// records for [`locate`] and [`disc`].
pub fn args() -> Vec<String> {
    let (rest, flag) = split_flag(std::env::args().collect(), "--data");
    let (rest, disc) = split_flag(rest, "--disc");
    let _ = FLAG.set(flag);
    let _ = DISC.set(disc);
    rest
}

/// The `--disc` path, once [`args`] has run.
pub fn disc() -> Option<PathBuf> {
    DISC.get().cloned().flatten()
}

fn split_flag(args: Vec<String>, name: &str) -> (Vec<String>, Option<PathBuf>) {
    let eq = format!("{name}=");
    let mut rest = Vec::with_capacity(args.len());
    let mut flag = None;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == name {
            flag = it.next().map(PathBuf::from);
        } else if let Some(v) = a.strip_prefix(eq.as_str()) {
            flag = Some(PathBuf::from(v));
        } else {
            rest.push(a);
        }
    }
    (rest, flag)
}

/// The game's installed directory (holding `DATA` and `BIN`) from the lookup order above.
pub fn locate() -> Result<PathBuf> {
    locate_any().context(crate::exit::Exit::NoData)
}

fn locate_any() -> Result<PathBuf> {
    if let Some(d) = std::env::var_os("SHUPPATSU_DATA_DIR").filter(|d| !d.is_empty()) {
        return install_dir(Path::new(&d), "SHUPPATSU_DATA_DIR");
    }
    if let Some(Some(d)) = FLAG.get() {
        return install_dir(d, "--data");
    }
    if let Some(d) = CHOSEN.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return Ok(d);
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| Some(p.parent()?.to_owned()));
    if let Some(dir) = &exe_dir {
        for d in [dir.join("data").join("extracted"), dir.join("data")] {
            if let Some(i) = try_install_dir(&d) {
                return Ok(i);
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        for d in [cwd.join("data").join("extracted"), cwd.join("data")] {
            if let Some(i) = try_install_dir(&d) {
                return Ok(i);
            }
        }
    }
    if let Some(d) = config_data_dir() {
        return install_dir(&d, "config.toml data_dir");
    }
    if let Some(d) = crate::import::imported() {
        return Ok(d);
    }
    bail!(
        "the game data was not found. Import your disc image with `shuppatsu import <image>` (or \
         start the game in a window and choose it), or put an extracted disc or installed game in \
         a folder named `data` next to the executable{} or in the current directory, pass --data \
         <dir>, set SHUPPATSU_DATA_DIR, or set data_dir in {} (docs/building.md)",
        exe_dir.map_or(String::new(), |d| format!(
            " ({})",
            d.join("data").display()
        )),
        crate::config::path().map_or("config.toml".into(), |p| p.display().to_string()),
    )
}

/// `root` as an extracted disc or an installed game directory.
pub fn try_install_dir(root: &Path) -> Option<PathBuf> {
    let extracted = root.join("extracted");
    [
        root.join(DISC_INSTALL),
        root.to_owned(),
        extracted.join(DISC_INSTALL),
        extracted,
    ]
    .into_iter()
    .find(|d| d.join("DATA").is_dir() && d.join("BIN").is_dir())
}

fn install_dir(root: &Path, source: &str) -> Result<PathBuf> {
    match try_install_dir(root) {
        Some(d) => Ok(d),
        None => bail!(
            "{source}: {} holds neither {DISC_INSTALL}/DATA (an extracted disc) nor DATA and BIN \
             (an installed game)",
            root.display()
        ),
    }
}

/// `data_dir` from config.toml, if set.
fn config_data_dir() -> Option<PathBuf> {
    let path = crate::config::path()?;
    let text = std::fs::read_to_string(&path).ok()?;
    let table: toml::Table = text.parse().ok()?;
    let d = PathBuf::from(table.get("data_dir")?.as_str()?);
    Some(if d.is_relative() {
        path.parent()?.join(d)
    } else {
        d
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn data_flag_is_taken_out_anywhere() {
        let args = strings(&["shuppatsu", "play", "0", "--data", "/d", "--mute"]);
        let (rest, flag) = split_flag(args, "--data");
        assert_eq!(rest, strings(&["shuppatsu", "play", "0", "--mute"]));
        assert_eq!(flag, Some(PathBuf::from("/d")));
        let (rest, flag) = split_flag(strings(&["shuppatsu", "--data=C:\\x"]), "--data");
        assert_eq!(rest, strings(&["shuppatsu"]));
        assert_eq!(flag, Some(PathBuf::from("C:\\x")));
        assert_eq!(split_flag(strings(&["shuppatsu"]), "--data").1, None);
        let (rest, disc) = split_flag(strings(&["shuppatsu", "--disc", "/i.cue"]), "--disc");
        assert_eq!(
            (rest, disc),
            (strings(&["shuppatsu"]), Some(PathBuf::from("/i.cue")))
        );
    }

    #[test]
    fn both_layouts_are_accepted() {
        let tmp = std::env::temp_dir().join(format!("shuppatsu-data-dir-{}", std::process::id()));
        let disc = tmp.join("disc");
        let installed = tmp.join("installed");
        for d in [disc.join(DISC_INSTALL), installed.clone()] {
            std::fs::create_dir_all(d.join("DATA")).unwrap();
            std::fs::create_dir_all(d.join("BIN")).unwrap();
        }
        assert_eq!(try_install_dir(&disc), Some(disc.join(DISC_INSTALL)));
        assert_eq!(try_install_dir(&installed), Some(installed.clone()));
        assert_eq!(try_install_dir(&tmp), None);
        assert!(install_dir(&tmp, "test").is_err());
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}

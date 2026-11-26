//! Save data: the original's `eeprom.dat` (docs/re/menus.md §13.1), byte for byte, so a file
//! from the original game can be dropped in and ours can be taken back.
//!
//! The file is the 0x35a-byte mirror at 0x856020: the arcade settings block, 13 ranking tables
//! of 10 rows, the options and the per-route clear marks. It has no header; the first two bytes
//! of the settings block are a magic the boot code compares with the exe's default image
//! (0x418ef0): on a mismatch (no file, or a short read that left zeros) the whole default image
//! is copied in. The defaults are read from the user's exe, never stored here.
//!
//! Rankings live twice, as in the original: unpacked in memory (`0x84fca0` / `0x84fc00`, what the
//! screens show and edit) and packed in the mirror (what is written). The original's name entry
//! only packs its table when a name is completed (0x41e670); the port packs a timed-out entry
//! too (decided port behaviour, menus.md §14).
//!
//! The original writes the file only at exit (0x434f50). The port also writes it right after
//! every change (a ranking entry, a clear mark, an options change, a calibration; decided port
//! behaviour, menus.md §14), atomically and in the same format: see [`Save::persist`].

use anyhow::{Context, Result};
use shuppatsu_formats::exe::Exe;
use std::path::{Path, PathBuf};

/// `eeprom.dat` size (0x434ed0 / 0x434f50 read and write exactly this many bytes).
pub const EEPROM_LEN: usize = 0x35a;
pub const TABLE_COUNT: usize = 13;
pub const ROWS: usize = 10;
pub const NAME_LEN: usize = 4;
/// Routes 0..=19 index `0x46bbe8` and the clear marks; index 20 is the clear mark of select
/// stage 20 (the route-10 variant, `0x48ddf0 = 1`), which `Stage_Run` writes at
/// `0x85635a + 20` = eeprom +0x34e.
pub const ROUTE_COUNT: usize = 20;
pub const CLEAR_MARK_COUNT: usize = ROUTE_COUNT + 1;
/// Highest ranking value (`0x270f`, 0x41e8d0).
pub const MAX_SCORE: u16 = 9999;
/// Blank name symbol (0x41e8d0 fills new rows with it).
pub const BLANK: u8 = 0xb;

/// File offsets (menus.md §13.1).
const MAGIC_LEN: usize = 2;
/// The settings word holding the difficulty in bits 14–15 (`0x856026`).
const DIFFICULTY: usize = 6;
const SETTINGS_SUM: usize = 0x20;
const NAMES: usize = 0x22;
const SCORES: usize = 0x22a;
const RANKING_SUM: usize = 0x330;
const OPTIONS: usize = 0x332;
const CLEAR_MARKS: usize = 0x33a;
const PRO_BIT: u16 = 0x8000;

/// The default image, in three pieces (0x418ef0): settings +0x00..+0x22 from 0x467768,
/// rankings +0x22..+0x332 from 0x467790, options and marks +0x332..+0x35a from 0x467aa0.
const DEFAULT_PIECES: [(u32, usize, usize); 3] = [
    (0x467768, 0, NAMES),
    (0x467790, NAMES, OPTIONS),
    (0x467aa0, OPTIONS, EEPROM_LEN),
];

/// One ranking row as the screens see it (unpacked).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    /// Symbol indices into the 59-symbol alphabet (`0x46bd30`).
    pub name: [u8; NAME_LEN],
    pub score: u16,
    /// The "PRO" badge (bit 15 of the packed score).
    pub pro: bool,
}

impl Row {
    pub const EMPTY: Self = Self {
        name: [BLANK; NAME_LEN],
        score: 0,
        pro: false,
    };

    /// The four score digits drawn by the ranking page (unpacked like 0x41e520: `/1000`,
    /// `%1000/100`, `%100/10`, `%10`).
    pub fn digits(&self) -> [u8; 4] {
        let s = self.score;
        [
            (s / 1000) as u8,
            (s % 1000 / 100) as u8,
            (s % 100 / 10) as u8,
            (s % 10) as u8,
        ]
    }
}

pub type Table = [Row; ROWS];

/// The six option bytes at +0x332 (menus.md §4 / §13.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub control_mode: u8,
    pub vibration: u8,
    /// 0 hidden, 1 normal, 2 big digits.
    pub speed_meter: u8,
    /// 0 hidden, 1 centimetres, 2 metres.
    pub distance_meter: u8,
    pub sound: u8,
}

/// The `eeprom.dat` image.
#[derive(Clone, PartialEq, Eq)]
pub struct Eeprom {
    bytes: [u8; EEPROM_LEN],
}

impl std::fmt::Debug for Eeprom {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Eeprom")
            .field("options", &self.options())
            .field(
                "clear_marks",
                &&self.bytes[CLEAR_MARKS..][..CLEAR_MARK_COUNT],
            )
            .finish_non_exhaustive()
    }
}

impl Eeprom {
    /// The first-boot image from the exe (0x418ef0 copies it when the magic is wrong).
    pub fn defaults(exe: &Exe<'_>) -> Result<Self> {
        let mut bytes = [0u8; EEPROM_LEN];
        for (va, from, to) in DEFAULT_PIECES {
            bytes[from..to].copy_from_slice(
                exe.bytes_at(va, to - from)
                    .with_context(|| format!("default eeprom image at {va:#x}"))?,
            );
        }
        Ok(Self { bytes })
    }

    /// What the boot sequence leaves in the mirror after reading `file` (0x434ed0, then the
    /// magic check 0x418ef0): the file's bytes over a zeroed mirror, or `defaults` when the
    /// first two bytes do not match theirs.
    pub fn load(file: &[u8], defaults: &Self) -> Self {
        let mut bytes = [0u8; EEPROM_LEN];
        let n = file.len().min(EEPROM_LEN);
        bytes[..n].copy_from_slice(&file[..n]);
        if bytes[..MAGIC_LEN] == defaults.bytes[..MAGIC_LEN] {
            Self { bytes }
        } else {
            defaults.clone()
        }
    }

    /// An image from raw bytes (tests).
    #[cfg(test)]
    pub fn from_bytes(bytes: [u8; EEPROM_LEN]) -> Self {
        Self { bytes }
    }

    pub fn bytes(&self) -> &[u8; EEPROM_LEN] {
        &self.bytes
    }

    /// Unpacks one ranking table (0x41e520).
    pub fn table(&self, table: usize) -> Table {
        let mut rows = [Row::EMPTY; ROWS];
        for (i, row) in rows.iter_mut().enumerate() {
            let n = NAMES + (table * ROWS + i) * NAME_LEN;
            row.name.copy_from_slice(&self.bytes[n..n + NAME_LEN]);
            let s = SCORES + (table * ROWS + i) * 2;
            let v = u16::from_le_bytes([self.bytes[s], self.bytes[s + 1]]);
            row.score = v & !PRO_BIT;
            row.pro = v & PRO_BIT != 0;
        }
        rows
    }

    /// Packs one ranking table back (0x41e670) and refreshes the checksums (0x418930, whose
    /// sum routine 0x425dc0 returns 0 on PC).
    pub fn store_table(&mut self, table: usize, rows: &Table) {
        for (i, row) in rows.iter().enumerate() {
            let n = NAMES + (table * ROWS + i) * NAME_LEN;
            self.bytes[n..n + NAME_LEN].copy_from_slice(&row.name);
            let s = SCORES + (table * ROWS + i) * 2;
            let v = row.score | if row.pro { PRO_BIT } else { 0 };
            self.bytes[s..s + 2].copy_from_slice(&v.to_le_bytes());
        }
        self.bytes[SETTINGS_SUM..SETTINGS_SUM + 2].fill(0);
        self.bytes[RANKING_SUM..RANKING_SUM + 2].fill(0);
    }

    pub fn options(&self) -> Options {
        let o = &self.bytes[OPTIONS..];
        Options {
            control_mode: o[0],
            vibration: o[1],
            speed_meter: o[2],
            distance_meter: o[3],
            sound: o[4],
        }
    }

    /// Option byte `item` 0..=5 of the options menu, minus the difficulty (menus.md §4):
    /// control mode, vibration, speed meter, distance meter, sound (item 3 lives at +0x336).
    pub fn set_option(&mut self, item: usize, value: u8) {
        let off = match item {
            0 => OPTIONS,
            2 => OPTIONS + 1,
            3 => OPTIONS + 4,
            4 => OPTIONS + 2,
            5 => OPTIONS + 3,
            _ => return,
        };
        self.bytes[off] = value;
    }

    /// The difficulty 0..=3 (Easy, Normal, Hard, Very hard): bits 14–15 of the u16 at +6,
    /// mirrored to `0x829d10` (0x434ed0, `Options_Set` case 1).
    pub fn difficulty(&self) -> u8 {
        (u16::from_le_bytes([self.bytes[DIFFICULTY], self.bytes[DIFFICULTY + 1]]) >> 14) as u8
    }

    pub fn set_difficulty(&mut self, value: u8) {
        let old = u16::from_le_bytes([self.bytes[DIFFICULTY], self.bytes[DIFFICULTY + 1]]);
        let v = (u16::from(value & 3) << 14) | (old & 0x3fff);
        self.bytes[DIFFICULTY..DIFFICULTY + 2].copy_from_slice(&v.to_le_bytes());
    }

    /// Clear mark of a route: 0 none, 1..=3 (menus.md §6.3); index 20 is select stage 20.
    pub fn clear_mark(&self, route: usize) -> u8 {
        if route < CLEAR_MARK_COUNT {
            self.bytes[CLEAR_MARKS + route]
        } else {
            0
        }
    }

    /// `Stage_Run` after a clear (0x4080d0): 3 without a continue, 2 with 1–3, 1 with 4 or more;
    /// a mark is only ever raised.
    pub fn raise_clear_mark(&mut self, route: usize, continues: i32) {
        if route >= CLEAR_MARK_COUNT {
            return;
        }
        let mark = match continues {
            ..=0 => 3,
            1..=3 => 2,
            _ => 1,
        };
        let b = &mut self.bytes[CLEAR_MARKS + route];
        *b = (*b).max(mark);
    }
}

/// The save as the game holds it: the eeprom mirror plus the unpacked rankings.
#[derive(Debug, Clone)]
pub struct Save {
    pub eeprom: Eeprom,
    pub rankings: [Table; TABLE_COUNT],
    /// Where [`Save::write`] puts it; `None` keeps it in memory (headless shots).
    path: Option<PathBuf>,
    /// Setting `save_on_change` (default on): [`Save::persist`] writes the file at once; off,
    /// only the exit write does, as the original.
    pub save_on_change: bool,
}

impl Save {
    pub fn new(eeprom: Eeprom, path: Option<PathBuf>) -> Self {
        let rankings = std::array::from_fn(|t| eeprom.table(t));
        Self {
            eeprom,
            rankings,
            path,
            save_on_change: true,
        }
    }

    /// Loads `path` (missing file = first boot), falling back to the exe's defaults.
    pub fn open(exe: &Exe<'_>, path: Option<PathBuf>) -> Result<Self> {
        let defaults = Eeprom::defaults(exe)?;
        let file = match &path {
            Some(p) => match std::fs::read(p) {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(e) => return Err(e).with_context(|| p.display().to_string()),
            },
            None => Vec::new(),
        };
        Ok(Self::new(Eeprom::load(&file, &defaults), path))
    }

    /// Packs one unpacked table into the mirror (0x41e670).
    pub fn commit_table(&mut self, table: usize) {
        if let Some(rows) = self.rankings.get(table) {
            let rows = *rows;
            self.eeprom.store_table(table, &rows);
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Writes the mirror (0x434f50, at exit). Written to a temporary file and renamed.
    pub fn write(&self) -> Result<()> {
        match &self.path {
            Some(path) => write_atomic(path, self.eeprom.bytes()),
            None => Ok(()),
        }
    }

    /// The port's write right after a change (decided port behaviour; the original only
    /// writes at exit). A failure is reported and play goes on; the exit write tries again.
    /// Without a path (headless shots), or with `save_on_change` off, nothing is written.
    pub fn persist(&self, what: &str) {
        if !self.save_on_change {
            return;
        }
        if let Err(e) = self.write() {
            eprintln!("warning: saving after {what}: {e:#}");
        }
    }

    /// The name entry ended (`Action::RankingDone`): `rows` replace the in-memory `table`;
    /// with `commit` the table is packed (0x41e670) and the file written.
    pub fn finish_ranking(&mut self, table: usize, rows: Table, commit: bool) {
        let Some(t) = self.rankings.get_mut(table) else {
            return;
        };
        *t = rows;
        if commit {
            self.commit_table(table);
            self.persist("a ranking entry");
        }
    }

    /// A cleared run's mark ([`Eeprom::raise_clear_mark`]), then the file written.
    pub fn mark_clear(&mut self, route: usize, continues: i32) {
        self.eeprom.raise_clear_mark(route, continues);
        self.persist("a clear mark");
    }
}

/// Writes `bytes` to a temporary file next to `path` and renames it over `path`, so an
/// interrupted write never leaves a truncated file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes).with_context(|| tmp.display().to_string())?;
    std::fs::rename(&tmp, path).with_context(|| path.display().to_string())?;
    Ok(())
}

/// `eeprom.dat` in the user's config dir ([`config_dir`]).
pub fn default_path() -> Option<PathBuf> {
    Some(config_dir()?.join("eeprom.dat"))
}

/// One folder for everything the game writes (settings, saves, logs, issue marks and the
/// imported game data), like a portable emulator's; a launcher sets it to its own data folder
/// (RecompLauncher passes `{data}`, recomplauncher.json). Unset: the system's usual places.
pub const HOME_VAR: &str = "SHUPPATSU_HOME";

/// The config folder's name.
const DIR_NAME: &str = "shuppatsu";
/// Its name before the project was renamed; [`migrate`] copies it over once.
const OLD_DIR_NAME: &str = "ddg";

/// The port's directory for `eeprom.dat`, `joystick.dat` and `config.toml`:
/// `%APPDATA%\shuppatsu` on Windows, else `$XDG_CONFIG_HOME/shuppatsu` or
/// `~/.config/shuppatsu` (relative values are ignored). The first call copies the old `ddg`
/// folder there if only that one exists ([`migrate`]).
pub fn config_dir() -> Option<PathBuf> {
    static DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let dir = config_dir_in(|k| std::env::var_os(k))?;
        // A folder given by the launcher is its own; nothing to carry over into it.
        if std::env::var_os(HOME_VAR).is_some_and(|v| !v.is_empty()) {
            return Some(dir);
        }
        match migrate(&dir) {
            Ok(None) => Some(dir),
            Ok(Some(note)) => {
                eprintln!("{note}");
                let _ = MIGRATION_NOTE.set(note);
                Some(dir)
            }
            Err(e) => {
                let old = dir.with_file_name(OLD_DIR_NAME);
                eprintln!(
                    "warning: could not copy {} to {} ({e:#}); using the old folder",
                    old.display(),
                    dir.display()
                );
                Some(old)
            }
        }
    })
    .clone()
}

static MIGRATION_NOTE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// What [`config_dir`] reported when it copied the old folder (the Windows log repeats it,
/// since that first report may have gone to a missing stderr).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn migration_note() -> Option<&'static str> {
    MIGRATION_NOTE.get().map(String::as_str)
}

/// [`config_dir`] with the environment looked up by `env` (tests point `XDG_CONFIG_HOME` at a
/// temporary directory without touching the process environment or the real home). Pure: no
/// migration.
pub fn config_dir_in(env: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    let absolute = |v: std::ffi::OsString| Some(PathBuf::from(v)).filter(|p| p.is_absolute());
    if let Some(home) = env(HOME_VAR).and_then(absolute) {
        return Some(home);
    }
    let dir = if cfg!(windows) {
        env("APPDATA").and_then(absolute)?
    } else if cfg!(target_os = "macos") {
        mac_support_dir(&env)?
    } else {
        env("XDG_CONFIG_HOME")
            .and_then(absolute)
            .or_else(|| env("HOME").and_then(absolute).map(|h| h.join(".config")))?
    };
    Some(dir.join(DIR_NAME))
}

/// `~/Library/Application Support`: where a Mac keeps an app's settings and data. Mac apps
/// don't follow the XDG variables, so on macOS settings, saves, logs, marks and the imported
/// game all live in `~/Library/Application Support/shuppatsu`.
pub fn mac_support_dir(env: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    let home = PathBuf::from(env("HOME")?);
    home.is_absolute()
        .then(|| home.join("Library").join("Application Support"))
}

/// When `dir` does not exist but its old-named sibling (`ddg`, from before the rename) does,
/// copies the old folder's contents to `dir` and returns a note saying so. The old folder is
/// never changed or deleted. The copy goes to a temporary sibling first and is renamed into
/// place, so an interrupted copy is redone on the next start.
pub fn migrate(dir: &Path) -> Result<Option<String>> {
    let old = dir.with_file_name(OLD_DIR_NAME);
    if dir.exists() || !old.is_dir() {
        return Ok(None);
    }
    let tmp = dir.with_file_name(format!("{DIR_NAME}.migrating"));
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp).with_context(|| tmp.display().to_string())?;
    }
    copy_tree(&old, &tmp)?;
    std::fs::rename(&tmp, dir).with_context(|| dir.display().to_string())?;
    Ok(Some(format!(
        "copied the settings and saves from {} to {} (the old folder is kept)",
        old.display(),
        dir.display()
    )))
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).with_context(|| to.display().to_string())?;
    for entry in std::fs::read_dir(from).with_context(|| from.display().to_string())? {
        let entry = entry?;
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if entry.file_type()?.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst).with_context(|| src.display().to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mac_folder_is_application_support() {
        fn env(home: &'static str) -> impl Fn(&str) -> Option<std::ffi::OsString> {
            move |k| (k == "HOME").then(|| home.into())
        }
        assert_eq!(
            mac_support_dir(env("/Users/a")),
            Some(PathBuf::from("/Users/a/Library/Application Support"))
        );
        assert_eq!(mac_support_dir(env("rel")), None);
        assert_eq!(mac_support_dir(|_: &str| None), None);
    }

    /// A synthetic default image (tests must not depend on game data).
    fn defaults() -> Eeprom {
        let mut bytes = [0u8; EEPROM_LEN];
        bytes[0] = 0x11;
        bytes[1] = 0x22;
        for (i, b) in bytes[NAMES..SCORES].iter_mut().enumerate() {
            *b = (i % 59) as u8;
        }
        for i in 0..TABLE_COUNT * ROWS {
            let v = (1000 - i as u16) | if i % 3 == 0 { PRO_BIT } else { 0 };
            bytes[SCORES + 2 * i..][..2].copy_from_slice(&v.to_le_bytes());
        }
        bytes[OPTIONS + 3] = 2;
        Eeprom { bytes }
    }

    #[test]
    fn round_trip_is_byte_exact() {
        let d = defaults();
        let mut save = Save::new(d.clone(), None);
        for t in 0..TABLE_COUNT {
            save.commit_table(t);
        }
        assert_eq!(save.eeprom, d);
        // Edit, pack, reload: the same rows come back.
        save.rankings[4][2] = Row {
            name: [12, 13, 57, BLANK],
            score: 9999,
            pro: true,
        };
        save.commit_table(4);
        let again = Save::new(Eeprom::load(save.eeprom.bytes(), &d), None);
        assert_eq!(again.rankings, save.rankings);
        assert_eq!(again.eeprom.bytes(), save.eeprom.bytes());
        let s = SCORES + (4 * ROWS + 2) * 2;
        assert_eq!(&again.eeprom.bytes()[s..s + 2], &[0x0f, 0xa7]);
    }

    #[test]
    fn wrong_magic_or_missing_file_gives_defaults() {
        let d = defaults();
        assert_eq!(Eeprom::load(&[], &d), d);
        let mut other = *d.bytes();
        other[0] = 0;
        other[OPTIONS + 3] = 0;
        assert_eq!(Eeprom::load(&other, &d), d);
        // A short file with the right magic keeps its bytes over zeros (0x434ed0 + 0x418ef0).
        let short = Eeprom::load(&d.bytes()[..0x40], &d);
        assert_eq!(&short.bytes()[..0x40], &d.bytes()[..0x40]);
        assert!(short.bytes()[0x40..].iter().all(|&b| b == 0));
    }

    #[test]
    fn writes_and_reopens_the_file() {
        let dir = std::env::temp_dir().join(format!("shuppatsu-save-test-{}", std::process::id()));
        let path = dir.join("shuppatsu").join("eeprom.dat");
        let d = defaults();
        let mut save = Save::new(d.clone(), Some(path.clone()));
        save.eeprom.raise_clear_mark(7, 0);
        save.rankings[0][0].score = 4321;
        save.commit_table(0);
        save.write().unwrap();
        let file = std::fs::read(&path).unwrap();
        assert_eq!(file.len(), EEPROM_LEN);
        let again = Save::new(Eeprom::load(&file, &d), None);
        assert_eq!(again.rankings[0][0].score, 4321);
        assert_eq!(again.eeprom.clear_mark(7), 3);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn migrates_the_old_folder_once_and_keeps_it() {
        let base = std::env::temp_dir().join(format!("shuppatsu-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (old, new) = (base.join(OLD_DIR_NAME), base.join(DIR_NAME));
        // Nothing to migrate.
        std::fs::create_dir_all(&base).unwrap();
        assert_eq!(migrate(&new).unwrap(), None);
        assert!(!new.exists());
        // The old folder is copied, files and subfolders, and left as it was.
        std::fs::create_dir_all(old.join("sub")).unwrap();
        std::fs::write(old.join("eeprom.dat"), [1, 2, 3]).unwrap();
        std::fs::write(old.join("config.toml"), "lang = \"en\"\n").unwrap();
        std::fs::write(old.join("sub").join("combo.txt"), "x").unwrap();
        let note = migrate(&new).unwrap().expect("a note");
        assert!(note.contains("kept"), "{note}");
        assert_eq!(std::fs::read(new.join("eeprom.dat")).unwrap(), [1, 2, 3]);
        assert_eq!(
            std::fs::read_to_string(new.join("config.toml")).unwrap(),
            "lang = \"en\"\n"
        );
        assert_eq!(
            std::fs::read(new.join("sub").join("combo.txt")).unwrap(),
            b"x"
        );
        assert_eq!(std::fs::read(old.join("eeprom.dat")).unwrap(), [1, 2, 3]);
        assert!(!base.join(format!("{DIR_NAME}.migrating")).exists());
        // Once the new folder exists it is never overwritten again.
        std::fs::write(new.join("eeprom.dat"), [9]).unwrap();
        assert_eq!(migrate(&new).unwrap(), None);
        assert_eq!(std::fs::read(new.join("eeprom.dat")).unwrap(), [9]);
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// A fresh settings folder under the system temp dir (never the real home), given as
    /// `$SHUPPATSU_HOME` so it is the same on every system; the environment is passed to
    /// [`config_dir_in`], not set on the process.
    fn temp_xdg(name: &str) -> (PathBuf, PathBuf) {
        let xdg = std::env::temp_dir().join(format!("shuppatsu-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&xdg);
        let env = |k: &str| (k == HOME_VAR).then(|| xdg.clone().into_os_string());
        let path = config_dir_in(env).unwrap().join("eeprom.dat");
        assert!(path.starts_with(&xdg), "{}", path.display());
        (xdg, path)
    }

    fn on_disk(path: &Path, d: &Eeprom) -> Save {
        Save::new(Eeprom::load(&std::fs::read(path).unwrap(), d), None)
    }

    #[test]
    fn saves_after_every_change() {
        use crate::front::options::{self, item};
        if cfg!(windows) {
            return;
        }
        let (xdg, path) = temp_xdg("persist");
        let d = defaults();
        let mut save = Save::new(d.clone(), Some(path.clone()));

        // A name entry that ends without packing (no such path is left in the port, but the
        // flag is honoured) writes nothing.
        let mut rows = save.rankings[2];
        rows[0].score = 1234;
        save.finish_ranking(2, rows, false);
        assert!(!path.exists());

        // A ranking entry (completed or timed out: both commit) is on disk at once.
        save.finish_ranking(2, rows, true);
        assert_eq!(on_disk(&path, &d).rankings[2][0].score, 1234);

        // A clear mark.
        save.mark_clear(5, 2);
        assert_eq!(on_disk(&path, &d).eeprom.clear_mark(5), 2);

        // An options change (App::apply → options::set, then persist).
        options::set(&mut save.eeprom, item::DIFFICULTY, 3);
        save.persist("an options change");
        assert_eq!(on_disk(&path, &d).eeprom.difficulty(), 3);

        // A calibration (App::save_calibration writes joystick.dat, then persists).
        save.eeprom.set_option(item::CONTROL, 6);
        save.persist("a calibration");
        assert_eq!(on_disk(&path, &d).eeprom.options().control_mode, 6);

        // Same format, no temporary file left behind.
        assert_eq!(std::fs::read(&path).unwrap(), save.eeprom.bytes());
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["eeprom.dat"]);
        std::fs::remove_dir_all(&xdg).unwrap();
    }

    /// Setting `save_on_change` off: the original's single write at exit.
    #[test]
    fn without_save_on_change_only_the_exit_writes() {
        use crate::front::options::{self, item};
        if cfg!(windows) {
            return;
        }
        let (xdg, path) = temp_xdg("exit-only");
        let d = defaults();
        let mut save = Save::new(d.clone(), Some(path.clone()));
        save.save_on_change = false;
        let mut rows = save.rankings[2];
        rows[0].score = 4321;
        save.finish_ranking(2, rows, true);
        save.mark_clear(5, 2);
        options::set(&mut save.eeprom, item::DIFFICULTY, 3);
        save.persist("an options change");
        save.persist("a calibration");
        assert!(!path.exists(), "nothing before the exit");
        // The exit write (0x434f50) has everything.
        save.write().unwrap();
        let back = on_disk(&path, &d);
        assert_eq!(back.rankings[2][0].score, 4321);
        assert_eq!(back.eeprom.clear_mark(5), 2);
        assert_eq!(back.eeprom.difficulty(), 3);
        std::fs::remove_dir_all(&xdg).unwrap();
    }

    #[test]
    fn headless_saves_never_write() {
        let mut save = Save::new(defaults(), None);
        save.mark_clear(1, 0);
        save.finish_ranking(0, save.rankings[0], true);
        save.persist("a test");
        assert!(save.path().is_none());
    }

    #[test]
    fn clear_marks_only_rise() {
        let mut e = defaults();
        e.raise_clear_mark(3, 5);
        assert_eq!(e.clear_mark(3), 1);
        e.raise_clear_mark(3, 2);
        assert_eq!(e.clear_mark(3), 2);
        e.raise_clear_mark(3, 0);
        assert_eq!(e.clear_mark(3), 3);
        e.raise_clear_mark(3, 4);
        assert_eq!(e.clear_mark(3), 3);
        e.raise_clear_mark(20, 0);
        assert_eq!(e.bytes()[0x34e], 3);
        e.raise_clear_mark(21, 0);
    }

    #[test]
    fn reads_defaults_from_exe() {
        let Ok(data) = crate::scene::GameData::from_env() else {
            eprintln!("SHUPPATSU_DATA_DIR unset; skipping");
            return;
        };
        let bytes = data.exe().unwrap();
        let exe = Exe::parse(&bytes).unwrap();
        let d = Eeprom::defaults(&exe).unwrap();
        let save = Save::new(d.clone(), None);
        // Every default table is sorted by score, names are in the alphabet, digits are 0..=9.
        for t in &save.rankings {
            assert!(t.windows(2).all(|w| w[0].score >= w[1].score), "{t:?}");
            assert!(t.iter().all(|r| r.name.iter().all(|&c| c < 59)));
            assert!(t.iter().all(|r| r.score <= MAX_SCORE));
        }
        let o = d.options();
        assert!(o.control_mode <= 6 && o.distance_meter <= 2, "{o:?}");
        eprintln!("{d:?}");
    }
}

//! Reads the user's disc image (`SHUPPATSU_CUE`: the `.mdf`, `.bin` or `.iso`) end to end:
//! detects the sector layout, finds `data1.cab` in the ISO 9660 filesystem and extracts every
//! file of the InstallShield cabinet, comparing each with `data-manifest/3000-bandai.toml`.
//! Skips when `SHUPPATSU_CUE` is unset.

use shuppatsu_formats::disc::{Iso, ReadAt, Track, parse_cue};
use shuppatsu_formats::installshield::Cabinet;
use shuppatsu_formats::sha256::sha256;
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};

struct DiskFile(File, u64);

impl ReadAt for DiskFile {
    fn size(&self) -> u64 {
        self.1
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> shuppatsu_formats::Result<()> {
        #[cfg(unix)]
        use std::os::unix::fs::FileExt;
        #[cfg(windows)]
        use std::os::windows::fs::FileExt;
        #[cfg(unix)]
        let r = self.0.read_exact_at(buf, offset);
        #[cfg(windows)]
        let r = {
            let mut done = 0;
            let mut r = Ok(());
            while done < buf.len() {
                match self.0.seek_read(&mut buf[done..], offset + done as u64) {
                    Ok(0) => {
                        r = Err(std::io::ErrorKind::UnexpectedEof.into());
                        break;
                    }
                    Ok(n) => done += n,
                    Err(e) => {
                        r = Err(e);
                        break;
                    }
                }
            }
            r
        };
        r.map_err(|e| shuppatsu_formats::FormatError::Invalid {
            what: "image file",
            detail: e.to_string(),
        })
    }
}

/// `install/Program_Executable_Files/<path>` → sha256 hex, from the committed manifest.
fn manifest() -> HashMap<String, String> {
    let text = include_str!("../../../data-manifest/3000-bandai.toml");
    let mut out = HashMap::new();
    let mut path = None;
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("path = \"") {
            path = p
                .strip_suffix('"')
                .and_then(|p| p.strip_prefix("install/Program_Executable_Files/"))
                .map(str::to_owned);
        } else if let Some(h) = line.strip_prefix("sha256 = \"")
            && let Some(p) = path.take()
        {
            out.insert(p, h.trim_end_matches('"').to_owned());
        }
    }
    out
}

fn image() -> Option<(PathBuf, u64)> {
    let path = PathBuf::from(std::env::var_os("SHUPPATSU_CUE")?);
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("cue"))
    {
        let cue = parse_cue(&std::fs::read(&path).ok()?).ok()?;
        return Some((path.parent().unwrap_or(Path::new(".")).join(cue.file), 0));
    }
    Some((path, 0))
}

#[test]
fn extracts_the_installed_files_from_the_image() {
    let Some((path, start)) = image() else {
        eprintln!("skipped: SHUPPATSU_CUE not set");
        return;
    };
    let file = File::open(&path).unwrap();
    let len = file.metadata().unwrap().len();
    let src = DiskFile(file, len);
    let track = Track::detect(&src, start).expect("an ISO 9660 data track");
    let iso = Iso::new(track).unwrap();
    assert_eq!(iso.volume, "DENGO3K");
    // recomplauncher.json identifies the disc by this file in its top folder (`check.iso9660`).
    assert!(iso.find("DENGO3K.ICO").unwrap().is_some_and(|e| !e.dir));
    let cab_entry = iso
        .find("DATA1.CAB")
        .unwrap()
        .expect("data1.cab on the disc");
    let cab_src = iso.open(&cab_entry);
    let cab = Cabinet::read(&cab_src).unwrap();

    let want = manifest();
    assert_eq!(
        cab.files.len(),
        want.len(),
        "cabinet files vs manifest entries"
    );
    for f in &cab.files {
        let data = cab.extract(&cab_src, f).unwrap();
        let got: String = sha256(&data).iter().map(|b| format!("{b:02x}")).collect();
        let expected = want
            .get(&f.path)
            .unwrap_or_else(|| panic!("{} not in the manifest", f.path));
        assert_eq!(&got, expected, "{}", f.path);
    }
}

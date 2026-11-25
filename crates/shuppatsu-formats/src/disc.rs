//! CD images and the ISO 9660 filesystem on them, for importing the game data straight from
//! the user's disc image (docs/re/disc-import.md).
//!
//! Images are read through [`ReadAt`] so a 240 MB image need not be in memory; the crate stays
//! free of I/O (the game implements [`ReadAt`] for files). Supported layouts of the data track:
//!
//! * cooked 2048-byte sectors (`.iso`, or a `MODE1/2048` cue track);
//! * raw 2352-byte sectors, Mode 1 or Mode 2 Form 1 (`.bin`, Alcohol `.mdf`);
//! * raw 2352 + 96-byte subchannel sectors (2448, Alcohol `.mdf` with subchannel data).
//!
//! The layout is detected from the image itself: the ISO 9660 volume descriptor at sector 16
//! (the `CD001` identifier), so no `.cue` or `.mds` is needed for a single-file image whose
//! data track starts at byte 0.

use crate::error::{FormatError, Result};
use encoding_rs::SHIFT_JIS;

const WHAT: &str = "disc image";

/// User data bytes per sector.
pub const SECTOR: usize = 2048;

/// Random access to a byte source.
pub trait ReadAt {
    /// Total length in bytes.
    fn size(&self) -> u64;
    /// Fills `buf` from `offset`; an error if the source is shorter.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()>;
}

impl ReadAt for [u8] {
    fn size(&self) -> u64 {
        self.len() as u64
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let start = usize::try_from(offset).unwrap_or(usize::MAX);
        let src = start
            .checked_add(buf.len())
            .and_then(|end| self.get(start..end))
            .ok_or(FormatError::Truncated {
                what: WHAT,
                offset: start,
                need: buf.len(),
                len: self.len(),
            })?;
        buf.copy_from_slice(src);
        Ok(())
    }
}

impl ReadAt for Vec<u8> {
    fn size(&self) -> u64 {
        self.as_slice().size()
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        self.as_slice().read_at(offset, buf)
    }
}

/// How the data track's sectors are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// Bytes per stored sector (2048, 2352 or 2448).
    pub stride: u64,
    /// Offset of the 2048 user-data bytes inside a stored sector.
    pub data: u64,
}

impl Layout {
    pub const COOKED: Self = Self {
        stride: 2048,
        data: 0,
    };
    pub const RAW_MODE1: Self = Self {
        stride: 2352,
        data: 16,
    };
    pub const RAW_MODE2_FORM1: Self = Self {
        stride: 2352,
        data: 24,
    };

    /// The same layout with 96 bytes of subchannel after each raw sector.
    fn with_subchannel(self) -> Self {
        Self {
            stride: self.stride + 96,
            ..self
        }
    }
}

/// The 12-byte sync pattern that starts every raw sector.
const SYNC: [u8; 12] = [
    0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0,
];

/// A data track: a byte source, where the track's sector 0 starts, and its layout.
pub struct Track<'a, R: ReadAt + ?Sized> {
    src: &'a R,
    start: u64,
    layout: Layout,
}

impl<'a, R: ReadAt + ?Sized> Track<'a, R> {
    pub fn new(src: &'a R, start: u64, layout: Layout) -> Self {
        Self { src, start, layout }
    }

    /// Detects the layout of a track starting at `start` from its ISO 9660 primary volume
    /// descriptor (sector 16). `None` when no layout shows one.
    pub fn detect(src: &'a R, start: u64) -> Option<Self> {
        let raw = [Layout::RAW_MODE1, Layout::RAW_MODE2_FORM1];
        let candidates = [
            Layout::COOKED,
            raw[0],
            raw[1],
            raw[0].with_subchannel(),
            raw[1].with_subchannel(),
        ];
        for layout in candidates {
            let track = Self::new(src, start, layout);
            if layout.stride != 2048 {
                let mut sync = [0u8; 16];
                let at = start + 16 * layout.stride;
                if src.read_at(at, &mut sync).is_err() || sync[..12] != SYNC {
                    continue;
                }
                // Byte 15 is the mode: 1 for Mode 1, 2 for Mode 2 (Form 1 has data at 24).
                let mode = sync[15];
                if (mode == 1) != (layout.data == 16) {
                    continue;
                }
            }
            let mut pvd = [0u8; SECTOR];
            if track.sector(16, &mut pvd).is_ok() && pvd[0] == 1 && &pvd[1..6] == b"CD001" {
                return Some(track);
            }
        }
        None
    }

    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// Reads sector `lba`'s 2048 user-data bytes.
    pub fn sector(&self, lba: u32, buf: &mut [u8; SECTOR]) -> Result<()> {
        let at = self.start + u64::from(lba) * self.layout.stride + self.layout.data;
        self.src.read_at(at, buf)
    }

    /// Reads `len` bytes of user data starting `offset` bytes into sector `lba`.
    pub fn read(&self, lba: u32, offset: u64, buf: &mut [u8]) -> Result<()> {
        if self.layout.stride == 2048 {
            let at = self.start + u64::from(lba) * 2048 + offset;
            return self.src.read_at(at, buf);
        }
        let mut pos = 0usize;
        let mut sector = u64::from(lba) + offset / SECTOR as u64;
        let mut within = (offset % SECTOR as u64) as usize;
        let mut tmp = [0u8; SECTOR];
        while pos < buf.len() {
            let n = (SECTOR - within).min(buf.len() - pos);
            let at = self.start + sector * self.layout.stride + self.layout.data;
            if within == 0 && n == SECTOR {
                self.src.read_at(at, &mut buf[pos..pos + n])?;
            } else {
                self.src.read_at(at, &mut tmp)?;
                buf[pos..pos + n].copy_from_slice(&tmp[within..within + n]);
            }
            pos += n;
            sector += 1;
            within = 0;
        }
        Ok(())
    }
}

/// The first data track a `.cue` sheet names: its file (as written in the sheet, relative to
/// the sheet's folder) and its sector layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CueTrack {
    pub file: String,
    pub layout: Layout,
}

/// Parses a cue sheet and returns its first data track. Text is decoded as Shift-JIS when it
/// isn't UTF-8 (Japanese file names).
pub fn parse_cue(text: &[u8]) -> Result<CueTrack> {
    let text = match std::str::from_utf8(text) {
        Ok(t) => t.to_owned(),
        Err(_) => SHIFT_JIS.decode(text).0.into_owned(),
    };
    let mut file = None;
    for line in text.lines() {
        let line = line.trim();
        let (keyword, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        match keyword.to_ascii_uppercase().as_str() {
            "FILE" => {
                let rest = rest.trim();
                let name = if let Some(quoted) = rest.strip_prefix('"') {
                    quoted.split('"').next().unwrap_or("")
                } else {
                    rest.split_whitespace().next().unwrap_or("")
                };
                file = Some(name.to_owned());
            }
            "TRACK" => {
                let mode = rest
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .to_ascii_uppercase();
                let layout = match mode.as_str() {
                    "MODE1/2048" => Layout::COOKED,
                    "MODE1/2352" => Layout::RAW_MODE1,
                    "MODE2/2352" => Layout::RAW_MODE2_FORM1,
                    _ => continue, // audio or unsupported: look for a data track
                };
                let file = file
                    .clone()
                    .ok_or_else(|| FormatError::invalid("cue sheet", "TRACK before FILE"))?;
                return Ok(CueTrack { file, layout });
            }
            _ => {}
        }
    }
    Err(FormatError::invalid("cue sheet", "no data track"))
}

/// One directory entry of an ISO 9660 filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The name without its `;1` version suffix.
    pub name: String,
    pub lba: u32,
    pub size: u32,
    pub dir: bool,
}

/// The ISO 9660 filesystem of a data track (primary volume descriptor only; the game's disc
/// uses 8.3 names that need no Joliet).
pub struct Iso<'a, R: ReadAt + ?Sized> {
    track: Track<'a, R>,
    root: Entry,
    /// The volume label.
    pub volume: String,
}

impl<'a, R: ReadAt + ?Sized> Iso<'a, R> {
    pub fn new(track: Track<'a, R>) -> Result<Self> {
        let mut pvd = [0u8; SECTOR];
        track.sector(16, &mut pvd)?;
        if pvd[0] != 1 || &pvd[1..6] != b"CD001" {
            return Err(FormatError::BadMagic {
                what: "ISO 9660 volume descriptor",
                found: pvd[..6].to_vec(),
            });
        }
        let volume = String::from_utf8_lossy(&pvd[40..72]).trim_end().to_owned();
        let root = record(&pvd[156..190], true)?
            .ok_or_else(|| FormatError::invalid("ISO 9660", "no root directory record"))?;
        Ok(Self {
            track,
            root,
            volume,
        })
    }

    pub fn root(&self) -> &Entry {
        &self.root
    }

    /// The entries of directory `dir` (without `.` and `..`).
    pub fn list(&self, dir: &Entry) -> Result<Vec<Entry>> {
        if !dir.dir {
            return Err(FormatError::invalid(
                "ISO 9660",
                format!("{} is not a directory", dir.name),
            ));
        }
        let mut data = vec![0u8; dir.size as usize];
        self.track.read(dir.lba, 0, &mut data)?;
        let mut out = Vec::new();
        let mut pos = 0usize;
        while pos < data.len() {
            let len = usize::from(data[pos]);
            if len == 0 {
                // Records don't cross sectors: skip to the next one.
                pos = (pos / SECTOR + 1) * SECTOR;
                continue;
            }
            let end = (pos + len).min(data.len());
            if let Some(e) = record(&data[pos..end], false)? {
                out.push(e);
            }
            pos += len;
        }
        Ok(out)
    }

    /// Looks a `/`-separated path up from the root, ignoring case.
    pub fn find(&self, path: &str) -> Result<Option<Entry>> {
        let mut cur = self.root.clone();
        for part in path.split('/').filter(|p| !p.is_empty()) {
            let Some(next) = self
                .list(&cur)?
                .into_iter()
                .find(|e| e.name.eq_ignore_ascii_case(part))
            else {
                return Ok(None);
            };
            cur = next;
        }
        Ok(Some(cur))
    }

    /// A file's bytes as a [`ReadAt`] view, without reading them.
    pub fn open(&self, file: &Entry) -> IsoFile<'_, 'a, R> {
        IsoFile {
            iso: self,
            entry: file.clone(),
        }
    }

    /// Reads a whole file.
    pub fn read(&self, file: &Entry) -> Result<Vec<u8>> {
        let mut data = vec![0u8; file.size as usize];
        self.track.read(file.lba, 0, &mut data)?;
        Ok(data)
    }
}

/// One file of an [`Iso`], readable at any offset.
pub struct IsoFile<'i, 'a, R: ReadAt + ?Sized> {
    iso: &'i Iso<'a, R>,
    entry: Entry,
}

impl<R: ReadAt + ?Sized> ReadAt for IsoFile<'_, '_, R> {
    fn size(&self) -> u64 {
        u64::from(self.entry.size)
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        if offset + buf.len() as u64 > self.size() {
            return Err(FormatError::Truncated {
                what: "ISO 9660 file",
                offset: offset as usize,
                need: buf.len(),
                len: self.entry.size as usize,
            });
        }
        self.iso.track.read(self.entry.lba, offset, buf)
    }
}

/// Parses one directory record; `None` for `.` and `..` unless `root` (the volume
/// descriptor's root record is named like `.`).
fn record(r: &[u8], root: bool) -> Result<Option<Entry>> {
    if r.len() < 34 {
        return Err(FormatError::Truncated {
            what: "ISO 9660 directory record",
            offset: 0,
            need: 34,
            len: r.len(),
        });
    }
    let lba = u32::from_le_bytes([r[2], r[3], r[4], r[5]]);
    let size = u32::from_le_bytes([r[10], r[11], r[12], r[13]]);
    let dir = r[25] & 2 != 0;
    let name_len = usize::from(r[32]);
    let raw = r
        .get(33..33 + name_len)
        .ok_or_else(|| FormatError::invalid("ISO 9660 directory record", "name past the record"))?;
    if !root && (raw == [0] || raw == [1]) {
        return Ok(None);
    }
    let name = SHIFT_JIS.decode(raw).0;
    let name = name.split(';').next().unwrap_or("");
    let name = name.strip_suffix('.').unwrap_or(name).to_owned();
    Ok(Some(Entry {
        name,
        lba,
        size,
        dir,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal ISO 9660 image: PVD at 16, root directory at 18 with `A.TXT` and the
    /// directory `SUB` (at 19) holding `B.BIN`; file data at 20 and 21.
    fn cooked_image() -> Vec<u8> {
        let mut img = vec![0u8; 23 * SECTOR];
        fn rec(name: &[u8], lba: u32, size: u32, dir: bool) -> Vec<u8> {
            let mut r = vec![0u8; 33 + name.len() + (name.len() + 1) % 2];
            r[0] = r.len() as u8;
            r[2..6].copy_from_slice(&lba.to_le_bytes());
            r[10..14].copy_from_slice(&size.to_le_bytes());
            r[25] = if dir { 2 } else { 0 };
            r[32] = name.len() as u8;
            r[33..33 + name.len()].copy_from_slice(name);
            r
        }
        let pvd = &mut img[16 * SECTOR..17 * SECTOR];
        pvd[0] = 1;
        pvd[1..6].copy_from_slice(b"CD001");
        pvd[40..72].copy_from_slice(b"DENGO3K                         ");
        let root = rec(&[0], 18, SECTOR as u32, true);
        pvd[156..156 + root.len()].copy_from_slice(&root);
        let mut dir = Vec::new();
        dir.extend(rec(&[0], 18, SECTOR as u32, true));
        dir.extend(rec(&[1], 18, SECTOR as u32, true));
        dir.extend(rec(b"A.TXT;1", 20, 5, false));
        dir.extend(rec(b"SUB", 19, SECTOR as u32, true));
        img[18 * SECTOR..18 * SECTOR + dir.len()].copy_from_slice(&dir);
        let sub = rec(b"B.BIN;1", 21, 3000, false);
        img[19 * SECTOR..19 * SECTOR + sub.len()].copy_from_slice(&sub);
        img[20 * SECTOR..20 * SECTOR + 5].copy_from_slice(b"hello");
        for (i, b) in img[21 * SECTOR..21 * SECTOR + 3000].iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        img
    }

    /// The same image as raw Mode 1 sectors.
    fn raw_image(cooked: &[u8]) -> Vec<u8> {
        let mut raw = Vec::new();
        for s in cooked.chunks(SECTOR) {
            raw.extend_from_slice(&SYNC);
            raw.extend_from_slice(&[0, 2, 0, 1]); // MSF + mode 1
            raw.extend_from_slice(s);
            raw.extend_from_slice(&[0u8; 288]); // EDC/ECC
        }
        raw
    }

    fn check(img: &[u8], layout: Layout) {
        let track = Track::detect(img, 0).expect("detected");
        assert_eq!(track.layout(), layout);
        let iso = Iso::new(track).unwrap();
        assert_eq!(iso.volume, "DENGO3K");
        let names: Vec<_> = iso
            .list(iso.root())
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["A.TXT", "SUB"]);
        let a = iso.find("a.txt").unwrap().unwrap();
        assert_eq!(iso.read(&a).unwrap(), b"hello");
        let b = iso.find("/sub/b.bin").unwrap().unwrap();
        let data = iso.read(&b).unwrap();
        assert_eq!(data.len(), 3000);
        assert!(data.iter().enumerate().all(|(i, &v)| v == (i % 251) as u8));
        // Reads across the sector boundary through the ReadAt view.
        let f = iso.open(&b);
        let mut buf = [0u8; 10];
        f.read_at(2044, &mut buf).unwrap();
        assert_eq!(buf[0], (2044 % 251) as u8);
        assert_eq!(buf[9], (2053 % 251) as u8);
        assert!(f.read_at(2995, &mut buf).is_err());
        assert!(iso.find("missing").unwrap().is_none());
    }

    #[test]
    fn reads_cooked_and_raw_images() {
        let cooked = cooked_image();
        check(&cooked, Layout::COOKED);
        check(&raw_image(&cooked), Layout::RAW_MODE1);
    }

    #[test]
    fn rejects_non_iso() {
        assert!(Track::detect(&vec![0u8; 40 * SECTOR], 0).is_none());
    }

    #[test]
    fn cue_sheets() {
        let cue =
            b"FILE \"Dengo 3K.bin\" BINARY\r\n  TRACK 01 MODE1/2352\r\n    INDEX 01 00:00:00\r\n";
        assert_eq!(
            parse_cue(cue).unwrap(),
            CueTrack {
                file: "Dengo 3K.bin".into(),
                layout: Layout::RAW_MODE1
            }
        );
        let audio_first =
            b"FILE a.bin BINARY\nTRACK 01 AUDIO\nFILE b.iso BINARY\nTRACK 02 MODE1/2048\n";
        assert_eq!(parse_cue(audio_first).unwrap().file, "b.iso");
        assert!(parse_cue(b"TRACK 01 MODE1/2352\n").is_err());
        assert!(parse_cue(b"FILE a.wav WAVE\nTRACK 01 AUDIO\n").is_err());
    }
}

//! `ddg-game install-desktop`: an opt-in desktop entry plus hicolor icons (from the user's exe)
//! under `$XDG_DATA_HOME`, so launchers and Wayland compositors (which ignore window icons) can
//! show the game with its icon. Runs only when asked; `--uninstall` removes the files again.

use crate::identity::{APP_ID, TITLE};
use anyhow::{Context, Result, bail};
use ddg_formats::icon::IconImage;
use std::path::{Path, PathBuf};

pub const USAGE: &str = "\
usage: ddg-game install-desktop [--exec <path>] [--exe <dengo3k.exe>] [--uninstall]

Writes $XDG_DATA_HOME/applications/ddg-game.desktop and the exe's icon as
$XDG_DATA_HOME/icons/hicolor/<n>x<n>/apps/ddg-game.png (native sizes, plus 128 and 256
nearest-upscaled). XDG_DATA_HOME defaults to ~/.local/share.
  --exec <path>   program the entry starts (default: this executable)
  --exe <file>    exe to take the icon from (default: $DDG_DATA_DIR's dengo3k.exe)
  --uninstall     remove the files instead";

/// Upscaled sizes for HiDPI launchers.
const UPSCALED: [u32; 2] = [128, 256];

pub fn run(args: &[String]) -> Result<()> {
    let mut exec: Option<PathBuf> = None;
    let mut exe: Option<PathBuf> = None;
    let mut uninstall = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--exec" => exec = Some(it.next().context("--exec needs a path")?.into()),
            "--exe" => exe = Some(it.next().context("--exe needs a file")?.into()),
            "--uninstall" => uninstall = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            other => bail!("unknown option {other}\n\n{USAGE}"),
        }
    }
    let data_home = data_home()?;
    if uninstall {
        return remove(&data_home);
    }
    let exe_bytes = match exe {
        Some(p) => std::fs::read(&p).with_context(|| format!("reading {}", p.display()))?,
        None => crate::scene::GameData::from_env()?.exe()?,
    };
    let (images, best) = crate::identity::exe_icon(&exe_bytes)?;
    let exec = match exec {
        Some(p) => p,
        None => std::env::current_exe().context("locating this executable")?,
    };
    let data_dir = std::env::var_os("DDG_DATA_DIR").map(PathBuf::from);

    let mut icons: Vec<IconImage> = images;
    for size in UPSCALED {
        if !icons.iter().any(|i| i.width == size) {
            icons.push(upscale_nearest(&best, size));
        }
    }
    for img in &icons {
        let dir = data_home.join(format!(
            "icons/hicolor/{0}x{0}/apps",
            img.width.max(img.height)
        ));
        let path = dir.join(format!("{APP_ID}.png"));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&path, encode_png(img))
            .with_context(|| format!("writing {}", path.display()))?;
        println!("wrote {} ({}x{})", path.display(), img.width, img.height);
    }
    let apps = data_home.join("applications");
    std::fs::create_dir_all(&apps)?;
    let entry = apps.join(format!("{APP_ID}.desktop"));
    std::fs::write(&entry, desktop_entry(&exec, data_dir.as_deref()))
        .with_context(|| format!("writing {}", entry.display()))?;
    println!("wrote {}", entry.display());
    Ok(())
}

/// `$XDG_DATA_HOME`, or `~/.local/share`.
fn data_home() -> Result<PathBuf> {
    if let Some(d) = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        return Ok(d.into());
    }
    let home = std::env::var_os("HOME").context("neither XDG_DATA_HOME nor HOME is set")?;
    Ok(PathBuf::from(home).join(".local/share"))
}

/// Removes the entry and every `hicolor/*/apps/ddg-game.png`.
fn remove(data_home: &Path) -> Result<()> {
    let mut files = vec![
        data_home
            .join("applications")
            .join(format!("{APP_ID}.desktop")),
    ];
    if let Ok(sizes) = std::fs::read_dir(data_home.join("icons/hicolor")) {
        for s in sizes.flatten() {
            files.push(s.path().join("apps").join(format!("{APP_ID}.png")));
        }
    }
    let mut removed = 0;
    for f in files {
        if f.is_file() {
            std::fs::remove_file(&f).with_context(|| format!("removing {}", f.display()))?;
            println!("removed {}", f.display());
            removed += 1;
        }
    }
    if removed == 0 {
        println!("nothing to remove under {}", data_home.display());
    }
    Ok(())
}

/// The `.desktop` file. `DDG_DATA_DIR` is baked in when set, since launchers don't run the dev
/// shell that normally provides it.
fn desktop_entry(exec: &Path, data_dir: Option<&Path>) -> String {
    let mut cmd = String::new();
    if let Some(d) = data_dir {
        cmd.push_str("env ");
        cmd.push_str(&exec_arg(&format!("DDG_DATA_DIR={}", d.display())));
        cmd.push(' ');
    }
    cmd.push_str(&exec_arg(&exec.display().to_string()));
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.5\n\
         Name={TITLE}\n\
         Comment=Native reimplementation; plays from your own disc\n\
         Exec={cmd}\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         Categories=Game;Simulation;\n\
         StartupWMClass={APP_ID}\n"
    )
}

/// One `Exec` argument, quoted per the Desktop Entry spec when needed. The spec's escapes are
/// then escaped once more for the string-value level (backslashes doubled).
fn exec_arg(s: &str) -> String {
    let s = s.replace('%', "%%");
    let reserved = |c: char| " \t\n\"'\\><~|&;$*?#()`".contains(c);
    if !s.chars().any(reserved) {
        return s;
    }
    let mut out = String::from("\"");
    for c in s.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            out.push_str("\\\\");
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// Nearest-neighbour scale to `size`×`size` (integer factors keep pixels crisp).
fn upscale_nearest(img: &IconImage, size: u32) -> IconImage {
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        let sy = y * img.height / size;
        for x in 0..size {
            let sx = x * img.width / size;
            let i = ((sy * img.width + sx) * 4) as usize;
            rgba.extend_from_slice(&img.rgba[i..i + 4]);
        }
    }
    IconImage {
        width: size,
        height: size,
        bit_count: img.bit_count,
        rgba,
    }
}

/// A minimal RGBA8 PNG: filter 0 rows in stored (uncompressed) deflate blocks.
pub fn encode_png(img: &IconImage) -> Vec<u8> {
    let mut raw = Vec::with_capacity((img.width * 4 + 1) as usize * img.height as usize);
    for row in img.rgba.chunks(img.width as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65_535).peekable();
    if blocks.peek().is_none() {
        z.extend([1, 0, 0, 0xff, 0xff]);
    }
    while let Some(b) = blocks.next() {
        z.push(u8::from(blocks.peek().is_none()));
        let len = b.len() as u16;
        z.extend(len.to_le_bytes());
        z.extend((!len).to_le_bytes());
        z.extend_from_slice(b);
    }
    z.extend(adler32(&raw).to_be_bytes());

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend(img.width.to_be_bytes());
    ihdr.extend(img.height.to_be_bytes());
    ihdr.extend([8, 6, 0, 0, 0]); // 8-bit RGBA, deflate, adaptive filters, no interlace
    let mut png = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    for (kind, data) in [(b"IHDR", &ihdr[..]), (b"IDAT", &z[..]), (b"IEND", &[][..])] {
        png.extend((data.len() as u32).to_be_bytes());
        let start = png.len();
        png.extend_from_slice(kind);
        png.extend_from_slice(data);
        png.extend(crc32(&png[start..]).to_be_bytes());
    }
    png
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + u32::from(x)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums() {
        assert_eq!(crc32(b"IEND"), 0xae42_6082);
        assert_eq!(adler32(b"Wikipedia"), 0x11e6_0398);
    }

    #[test]
    fn png_layout() {
        let img = IconImage {
            width: 2,
            height: 1,
            bit_count: 32,
            rgba: vec![1, 2, 3, 4, 5, 6, 7, 8],
        };
        let png = encode_png(&img);
        assert!(png.ends_with(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xae, 0x42, 0x60, 0x82]));
        // IDAT: zlib header, one final stored block of 1 + 8 bytes, then the raw row.
        let idat = png.windows(4).position(|w| w == b"IDAT").unwrap() + 4;
        assert_eq!(&png[idat..idat + 7], &[0x78, 0x01, 1, 9, 0, 0xf6, 0xff]);
        assert_eq!(&png[idat + 7..idat + 16], &[0, 1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn upscales_and_quotes() {
        let img = IconImage {
            width: 1,
            height: 1,
            bit_count: 4,
            rgba: vec![9, 8, 7, 255],
        };
        let big = upscale_nearest(&img, 4);
        assert_eq!(big.rgba.len(), 64);
        assert!(big.rgba.chunks(4).all(|p| p == [9, 8, 7, 255]));
        assert_eq!(
            exec_arg("/nix/store/x/bin/ddg-game"),
            "/nix/store/x/bin/ddg-game"
        );
        assert_eq!(exec_arg("/a b/100%"), "\"/a b/100%%\"");
        assert_eq!(exec_arg("/a$b"), "\"/a\\\\$b\"");
        let e = desktop_entry(Path::new("/x/ddg-game"), Some(Path::new("/d")));
        assert!(e.contains("Exec=env DDG_DATA_DIR=/d /x/ddg-game\n"));
        assert!(e.contains("StartupWMClass=ddg-game\n"));
    }
}

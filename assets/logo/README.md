# Shuppatsu logo

Original artwork made for this project: the app icon (a white bullet-nose train under a green
proceed signal) and the station-board header (出発 | しゅっぱつ / Shuppatsu). It contains no game
data and nothing taken from Densha de GO! or its publishers.

**License:** GPL-3.0-or-later, the same as the project ([LICENSE](../../LICENSE)). The lettering
is set in M PLUS 2 Medium ([`assets/fonts/`](../fonts/), SIL Open Font License 1.1).

## Sources

| file | what |
|---|---|
| `icon.svg` | the app icon master, 512 px (used for 48 px and up) |
| `icon-32.svg`, `icon-16.svg` | the same icon redrawn by hand on the 32 and 16 px grids |
| `src/header-light.svg`, `src/header-dark.svg` | the README header board, 1280×320, with live text |

## Derived (regenerate with `nix develop -c scripts/build-logo.sh`)

| file | what |
|---|---|
| `header-light.svg`, `header-dark.svg` | the headers with all text converted to paths, so they render the same without the font (the README uses them) |
| `gen/icon-{16,32,48,64,128,256,512}.png` | hicolor icons (`install-desktop`, the Nix package) |
| `gen/shuppatsu.ico` | the Windows exe icon (16, 32, 48, 256), linked in by `crates/shuppatsu/build.rs` |
| `gen/icon-256.shpk`, `gen/icon-32.shpk` | window icons, run-length packed RGBA embedded in the game |
| `gen/board.shpk` | the dark board without backdrop or posts, for the first-run language screen |

`.shpk` is the packed RGBA layout of `shuppatsu_render::rgba` (written by
`shuppatsu-tools pack-rgba`), so the game needs no image decoder.

#!/usr/bin/env bash
# Builds the 64-bit Windows release of shuppatsu and shuppatsu-tools (mingw-w64 cross build from the
# dev shell) and packs out/dist/shuppatsu-windows.zip. No game data goes into the package: the player
# brings their own disc (README-windows.txt below).
#
# Usage: nix develop -c scripts/package-windows.sh
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
target=x86_64-pc-windows-gnu
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"

cargo build --release --locked --target "$target" -p shuppatsu -p shuppatsu-tools

stage="out/dist/shuppatsu-windows"
rm -rf "$stage" out/dist/shuppatsu-windows.zip
mkdir -p "$stage"
for exe in shuppatsu shuppatsu-tools; do
  x86_64-w64-mingw32-strip -o "$stage/$exe.exe" "target/$target/release/$exe.exe"
done
cp LICENSE "$stage/LICENSE.txt"
cp assets/fonts/OFL-Mplus.txt "$stage/OFL-Mplus.txt"
cp docs/settings.md "$stage/settings.md"

# CRLF so Notepad on older Windows shows the lines.
sed 's/$/\r/' > "$stage/README-windows.txt" <<EOF
Shuppatsu $version for Windows (64-bit)
=====================================

Shuppatsu ("departure") is an open-source engine for playing Densha de GO! 2
Kosoku-hen 3000-bandai (Unbalance, 1999), an unofficial fan project not affiliated with or
endorsed by TAITO Corporation or Unbalance. Densha de GO! is a trademark of TAITO Corporation.
It contains no game data: it plays from the files of your own disc.

Contents
  shuppatsu.exe         the game
  shuppatsu-tools.exe   command-line tools (inventory, export to PNG/glTF, ...)
  settings.md           every setting and its config.toml key
  LICENSE.txt           GPL-3.0-or-later (this program)
  OFL-Mplus.txt         SIL Open Font License of the bundled M+ font

Requirements
  Windows 10 or 11, 64-bit, with a Direct3D 12 or Vulkan graphics driver.

1. Start shuppatsu.exe and choose your disc
  On the first start the game shows its game data screen. Choose your disc image (.cue,
  .mds/.mdf, .bin or .iso) or drop it onto the window; the disc in a drive, a copy of its
  files, or an installed copy of the original game (by default
  C:\\Program Files\\TAITO\\DENGO3K) work too: open the folder and choose "Use this folder".
  An image packed in an archive (.zip, .rar, self-extracting .exe) must be extracted first.

  The game files are unpacked once, checked against the 3000-bandai release, and kept in
  %LOCALAPPDATA%\\shuppatsu\\game; the image is not needed afterwards. From a command prompt:
    shuppatsu.exe --console import <image>   import and verify, then exit
    shuppatsu.exe --disc <image>             import on the first start, then play

2. Other places the game looks for its data (the first that is set wins)
  a. the environment variable SHUPPATSU_DATA_DIR
  b. shuppatsu.exe --data <folder>
  c. a folder named "data" next to shuppatsu.exe (or "data\\extracted")
  d. data_dir = 'C:\\path\\to\\data' in %APPDATA%\\shuppatsu\\config.toml
  e. the imported data

Saves, settings and logs
  %APPDATA%\\shuppatsu\\ holds eeprom.dat (scores, names), config.toml (settings) and
  shuppatsu.log (the log of the last start). The game shows a message box if it cannot start;
  the log has the details. Run "shuppatsu.exe --console" from a command prompt to see the log
  there instead. A %APPDATA%\\ddg\\ folder from an older version is copied to
  %APPDATA%\\shuppatsu\\ on the first start (the old folder is kept).

Graphics
  Direct3D 12 is used first and Vulkan if D3D12 is missing or cannot compile the shaders.
  To force one: set WGPU_BACKEND=vulkan (or dx12) before starting.

Controls
  Keyboard: Down/Up power, Right/Left brake, Space horn, Enter confirm, Esc pause.
  Game controllers through Windows.Gaming.Input (Xbox and other HID gamepads); the
  Densha de GO! controller modes and calibration: "shuppatsu.exe --help" (pad subcommand).

Source code and the Linux version: see the project's README.
EOF

(cd out/dist && 7z a -tzip -mx=9 shuppatsu-windows.zip ./shuppatsu-windows >/dev/null)
echo "wrote out/dist/shuppatsu-windows.zip"
ls -l out/dist/shuppatsu-windows.zip "$stage"

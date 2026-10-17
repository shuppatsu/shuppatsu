#!/usr/bin/env bash
# Builds the 64-bit Windows release of ddg-game and ddg-tools (mingw-w64 cross build from the
# dev shell) and packs out/dist/ddg-windows.zip. No game data goes into the package: the player
# brings their own disc (README-windows.txt below).
#
# Usage: nix develop -c scripts/package-windows.sh
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
target=x86_64-pc-windows-gnu
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"

cargo build --release --locked --target "$target" -p ddg-game -p ddg-tools

stage="out/dist/ddg-windows"
rm -rf "$stage" out/dist/ddg-windows.zip
mkdir -p "$stage"
for exe in ddg-game ddg-tools; do
  x86_64-w64-mingw32-strip -o "$stage/$exe.exe" "target/$target/release/$exe.exe"
done
cp LICENSE "$stage/LICENSE.txt"
cp assets/fonts/OFL-Mplus.txt "$stage/OFL-Mplus.txt"
cp docs/settings.md "$stage/settings.md"

# CRLF so Notepad on older Windows shows the lines.
sed 's/$/\r/' > "$stage/README-windows.txt" <<EOF
ddg-game $version for Windows (64-bit)
======================================

A native reimplementation of Densha de GO! 2 Kosoku-hen 3000-bandai (Unbalance, 1999).
It contains no game data: it plays from the files of your own disc.

Contents
  ddg-game.exe    the game
  ddg-tools.exe   command-line tools (inventory, export to PNG/glTF, ...)
  settings.md     every setting and its config.toml key
  LICENSE.txt     GPL-3.0-or-later (this program)
  OFL-Mplus.txt   SIL Open Font License of the bundled M+ font

Requirements
  Windows 10 or 11, 64-bit, with a Direct3D 12 or Vulkan graphics driver.

1. Get the game data from your disc
  You need a folder holding the game's DATA and BIN folders. Either layout works:
    data\\DATA, data\\BIN                                (an installed copy of the game)
    data\\install\\Program_Executable_Files\\DATA, ...BIN  (the disc extracted on Linux)
  If the original game is installed, its folder already is one (by default
  C:\\Program Files\\TAITO\\DENGO3K): point --data or DDG_DATA_DIR at it, or copy its DATA
  and BIN folders into a "data" folder next to ddg-game.exe.

2. Tell the game where it is (the first that is set wins)
  a. the environment variable DDG_DATA_DIR
  b. ddg-game.exe --data <folder>
  c. a folder named "data" next to ddg-game.exe (or "data\\extracted")
  d. data_dir = 'C:\\path\\to\\data' in %APPDATA%\\ddg\\config.toml
  Simplest: put the "data" folder next to ddg-game.exe and double-click the exe.

Saves, settings and logs
  %APPDATA%\\ddg\\ holds eeprom.dat (scores, names), config.toml (settings) and
  ddg-game.log (the log of the last start). The game shows a message box if it cannot start;
  the log has the details. Run "ddg-game.exe --console" from a command prompt to see the log
  there instead.

Graphics
  Direct3D 12 is used first and Vulkan if D3D12 is missing or cannot compile the shaders.
  To force one: set WGPU_BACKEND=vulkan (or dx12) before starting.

Controls
  Keyboard: Down/Up power, Right/Left brake, Space horn, Enter confirm, Esc pause.
  Game controllers through Windows.Gaming.Input (Xbox and other HID gamepads); the
  Densha de GO! controller modes and calibration: "ddg-game.exe --help" (pad subcommand).

Source code and the Linux version: see the project's README.
EOF

(cd out/dist && 7z a -tzip -mx=9 ddg-windows.zip ./ddg-windows >/dev/null)
echo "wrote out/dist/ddg-windows.zip"
ls -l out/dist/ddg-windows.zip "$stage"

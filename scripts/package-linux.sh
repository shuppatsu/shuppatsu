#!/usr/bin/env bash
# Builds the portable x86-64 Linux release of shuppatsu and shuppatsu-tools and packs
#   out/dist/Shuppatsu-v<version>-Linux-x64.tar.xz   the folder below, for unpacking anywhere
#   out/dist/Shuppatsu-v<version>-Linux-x64.AppImage the same folder as an AppImage
# No game data goes into the package: the player brings their own disc (README-linux.txt).
#
# Portability comes from the build machine, not from this script: run it on an old base
# (CI uses an ubuntu:22.04 container, glibc 2.35) with a rustup toolchain, pkg-config and the
# ALSA and libudev headers. A build in the Nix dev shell links against /nix/store and only runs
# on Nix. The libraries winit/wgpu load at runtime (Vulkan, Wayland, xkbcommon, X11, EGL) come
# from the player's system and are not bundled.
#
# The AppImage step needs appimagetool: $APPIMAGETOOL, or `appimagetool` on PATH. Without it,
# only the archive is written.
#
# Usage: scripts/package-linux.sh
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"
name="Shuppatsu-v$version-Linux-x64"

cargo build --release --locked -p shuppatsu -p shuppatsu-tools

dist=out/dist
stage="$dist/$name"
rm -rf "$stage" "$dist/$name.tar.xz" "$dist/$name.AppImage"
mkdir -p "$stage"
for exe in shuppatsu shuppatsu-tools; do
  strip -o "$stage/$exe" "target/release/$exe"
done
cp LICENSE "$stage/LICENSE.txt"
cp assets/fonts/OFL-Mplus.txt "$stage/OFL-Mplus.txt"
cp docs/settings.md "$stage/settings.md"

# AppImage entry points; also in the archive, so both are the same folder (a launcher that
# unpacks the AppImage starts AppRun).
cat > "$stage/AppRun" <<'EOF'
#!/bin/sh
here="$(dirname "$(readlink -f "$0")")"
exec "$here/shuppatsu" "$@"
EOF
chmod 755 "$stage/AppRun"
cat > "$stage/shuppatsu.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=Shuppatsu
Comment=Train driving engine for Densha de GO! 2 data from your own disc
Exec=shuppatsu
Icon=shuppatsu
Categories=Game;Simulation;
StartupWMClass=shuppatsu
Terminal=false
EOF
cp assets/logo/gen/icon-256.png "$stage/shuppatsu.png"

cat > "$stage/README-linux.txt" <<EOF
Shuppatsu $version for Linux (x86-64)
=====================================

Shuppatsu ("departure") is an open-source engine for playing Densha de GO! 2
Kosoku-hen 3000-bandai (Unbalance, 1999), an unofficial fan project not affiliated with or
endorsed by TAITO Corporation or Unbalance. Densha de GO! is a trademark of TAITO Corporation.
It contains no game data: it plays from the files of your own disc.

Contents
  shuppatsu             the game
  shuppatsu-tools       command-line tools (check, inventory, export to PNG/glTF, ...)
  AppRun, shuppatsu.desktop, shuppatsu.png   AppImage entry point, desktop entry and icon
  settings.md           every setting and its config.toml key
  LICENSE.txt           GPL-3.0-or-later (this program)
  OFL-Mplus.txt         SIL Open Font License of the bundled M+ font

Requirements
  x86-64 Linux with glibc 2.35 or newer (Ubuntu 22.04, Debian 12, Fedora 36 or later), a
  Vulkan driver (Mesa RADV/ANV/NVK or the NVIDIA driver; OpenGL 3.3 is the fallback),
  Wayland or X11, and ALSA (PipeWire and PulseAudio provide it).

1. Get the game data from your disc
  Extract your disc image with scripts/extract-disc.sh from the source repository, or copy
  an installed copy of the game (the folder holding DATA and BIN).

2. Tell the game where it is (the first that is set wins)
  a. the environment variable SHUPPATSU_DATA_DIR
  b. shuppatsu --data <folder>
  c. a folder named "data" (or "data/extracted") next to the shuppatsu binary
  d. a folder named "data" (or "data/extracted") in the current directory
  e. data_dir = '/path/to/data' in ~/.config/shuppatsu/config.toml
  With the AppImage, use a, b, d or e.

3. Start it
  ./shuppatsu (or ./$name.AppImage). "shuppatsu install-desktop" adds a menu entry.
  "shuppatsu --help" lists the options.

Saves (eeprom.dat) and settings (config.toml) live in ~/.config/shuppatsu; issue marks in
~/.local/share/shuppatsu/marks.

Source code, controls and troubleshooting: see the project's README.
EOF

tar -C "$dist" -cJf "$dist/$name.tar.xz" "$name"
echo "wrote $dist/$name.tar.xz"

tool="${APPIMAGETOOL:-$(command -v appimagetool || true)}"
if [ -n "$tool" ]; then
  # APPIMAGE_EXTRACT_AND_RUN: appimagetool is itself an AppImage; containers have no FUSE.
  ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$tool" --no-appstream "$stage" "$dist/$name.AppImage"
  echo "wrote $dist/$name.AppImage"
else
  echo "appimagetool not found (set APPIMAGETOOL): AppImage skipped" >&2
fi

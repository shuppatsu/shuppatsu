#!/usr/bin/env bash
# Builds the experimental macOS release of shuppatsu: one universal Shuppatsu.app (Apple
# Silicon and Intel) and packs
#   out/dist/Shuppatsu-v<version>-macOS.zip   Shuppatsu.app, recomplauncher.json and the texts
# No game data goes into the package: the player brings their own disc (README-macos.txt).
#
# Runs on a Mac (CI: a GitHub macOS runner) with Xcode's command-line tools (lipo, iconutil,
# codesign, ditto) and rustup; the two Apple targets are added if missing. The app is signed
# ad hoc only, not with an Apple Developer ID, so macOS asks once before the first start
# (README-macos.txt says how).
#
# Usage: scripts/package-macos.sh
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)"
name="Shuppatsu-v$version-macOS"
# macOS 11 (Big Sur) is the first with Apple Silicon; Metal, which wgpu draws with, is there.
export MACOSX_DEPLOYMENT_TARGET=11.0

targets=(aarch64-apple-darwin x86_64-apple-darwin)
rustup target add "${targets[@]}"
for t in "${targets[@]}"; do
  cargo build --release --locked --target "$t" -p shuppatsu -p shuppatsu-tools
done

dist=out/dist
stage="$dist/$name"
app="$stage/Shuppatsu.app"
rm -rf "$stage" "$dist/$name.zip"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
for exe in shuppatsu shuppatsu-tools; do
  lipo -create -output "$app/Contents/MacOS/$exe" \
    "target/aarch64-apple-darwin/release/$exe" "target/x86_64-apple-darwin/release/$exe"
  strip -x "$app/Contents/MacOS/$exe"
done

# The icon, from the project's own logo PNGs.
iconset="$dist/shuppatsu.iconset"
rm -rf "$iconset"
mkdir -p "$iconset"
for n in 16 32 128 256 512; do
  cp "assets/logo/gen/icon-$n.png" "$iconset/icon_${n}x${n}.png"
done
cp assets/logo/gen/icon-32.png "$iconset/icon_16x16@2x.png"
cp assets/logo/gen/icon-64.png "$iconset/icon_32x32@2x.png"
cp assets/logo/gen/icon-256.png "$iconset/icon_128x128@2x.png"
cp assets/logo/gen/icon-512.png "$iconset/icon_256x256@2x.png"
iconutil -c icns -o "$app/Contents/Resources/shuppatsu.icns" "$iconset"
rm -rf "$iconset"

cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Shuppatsu</string>
  <key>CFBundleDisplayName</key><string>Shuppatsu</string>
  <key>CFBundleIdentifier</key><string>io.github.shuppatsu</string>
  <key>CFBundleExecutable</key><string>shuppatsu</string>
  <key>CFBundleIconFile</key><string>shuppatsu</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>LSMinimumSystemVersion</key><string>$MACOSX_DEPLOYMENT_TARGET</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.simulation-games</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
  <key>NSHumanReadableCopyright</key><string>GPL-3.0-or-later. Contains no game data.</string>
</dict>
</plist>
EOF
# Ad hoc: Apple Silicon runs nothing unsigned. Not a Developer ID signature (see the top).
codesign --force --deep --sign - "$app"
codesign --verify --deep --strict "$app"

cp LICENSE "$stage/LICENSE.txt"
cp assets/fonts/OFL-Mplus.txt "$stage/OFL-Mplus.txt"
cp docs/settings.md "$stage/settings.md"
cp recomplauncher.json "$stage/recomplauncher.json"

cat > "$stage/README-macos.txt" <<EOF
Shuppatsu $version for macOS (experimental)
==========================================

Shuppatsu ("departure") is an open-source engine for playing Densha de GO! 2
Kosoku-hen 3000-bandai (Unbalance, 1999), an unofficial fan project not affiliated with or
endorsed by TAITO Corporation or Unbalance. Densha de GO! is a trademark of TAITO Corporation.
It contains no game data: it plays from the files of your own disc.

EXPERIMENTAL: this build is made automatically and has not been tested on a Mac by the
project yet. Reports of what works and what doesn't are very welcome (the project's issues).

Contents
  Shuppatsu.app         the game (Apple Silicon and Intel); shuppatsu-tools is inside it,
                        in Shuppatsu.app/Contents/MacOS
  settings.md           every setting and its config.toml key
  recomplauncher.json   setup and settings for RecompLauncher (recomplauncher.com)
  LICENSE.txt           GPL-3.0-or-later (this program)
  OFL-Mplus.txt         SIL Open Font License of the bundled M+ font

Requirements
  macOS 11 (Big Sur) or newer, on Apple Silicon or Intel.

1. The first start
  The app is not signed with an Apple Developer ID, so macOS stops the first start and says
  it can't check it. Either:
    - open it once, then System Settings > Privacy & Security > "Open Anyway"; or
    - in Terminal:  xattr -dr com.apple.quarantine /path/to/Shuppatsu.app
  After that it opens like any app. Drag it to Applications first if you like.

2. Choose your disc
  On the first start the game shows its game data screen: choose your disc image (.cue,
  .mds/.mdf, .bin or .iso) or drop it onto the window. A mounted disc or a copy of its files
  work too (discs are under /Volumes): open the folder and choose "Use this folder". An image
  packed in an archive (.zip, .rar, self-extracting .exe) must be extracted first.

  The game files are unpacked once, checked against the 3000-bandai release, and kept in
  ~/Library/Application Support/shuppatsu/game; the image is not needed afterwards.
  From Terminal (Shuppatsu.app/Contents/MacOS/shuppatsu):
    shuppatsu import <image>      import and verify, then exit
    shuppatsu --disc <image>      import on the first start, then play
    shuppatsu --help              every option

Saves (eeprom.dat), settings (config.toml), a log of each start (logs/, the newest five) and
issue marks (marks/) live in ~/Library/Application Support/shuppatsu.

Source code, controls and troubleshooting: see the project's README.
EOF

# ditto keeps the app's signature and attributes, which a plain zip may not.
ditto -c -k --keepParent "$stage" "$dist/$name.zip"
echo "wrote $dist/$name.zip"

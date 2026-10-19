#!/usr/bin/env bash
# Regenerates every derived logo file under assets/logo/ from its sources (assets/logo/README.md):
#   header-{light,dark}.svg     the README headers with all text as paths (from src/)
#   gen/icon-<n>.png            hicolor app icons, n = 16 32 48 64 128 256 512 (16 and 32 from the
#                               hand-hinted icon-16.svg / icon-32.svg, the rest from icon.svg)
#   gen/shuppatsu.ico           the Windows exe icon (16, 32, 48, 256)
#   gen/icon-256.shpk, gen/icon-32.shpk
#                               packed RGBA window icons embedded in the game
#   gen/board.shpk              the header board (dark, no backdrop or posts) at 1232×264 for
#                               the language screen; 4× its 308×66 size in the 640×480 design
# Text uses the bundled M PLUS 2 (assets/fonts); system fonts are never consulted.
#
# Usage: nix develop -c scripts/build-logo.sh
# resvg/usvg, svgo and oxipng come from nixpkgs (fetched with `nix shell` when
# missing); cargo from the dev shell.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
for tool in resvg usvg svgo oxipng; do
  if ! command -v "$tool" >/dev/null; then
    exec nix shell nixpkgs#resvg nixpkgs#svgo nixpkgs#oxipng \
      -c "${BASH_SOURCE[0]}" "$@"
  fi
done
cd "$root/assets/logo"
fonts=(--skip-system-fonts --use-fonts-dir "$root/assets/fonts")
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
mkdir -p gen

# README headers: text to paths (usvg), then compacted (svgo, 1 decimal at 1280 px wide).
for mode in light dark; do
  usvg "${fonts[@]}" "src/header-$mode.svg" "$tmp/header-$mode.svg"
  svgo --quiet --multipass -p 1 "$tmp/header-$mode.svg" -o "header-$mode.svg"
  # usvg drops the viewBox; without it the README's scaled-down header would be cropped.
  sed -i '1s/<svg xmlns="http:\/\/www.w3.org\/2000\/svg" width="1280" height="320">/<svg xmlns="http:\/\/www.w3.org\/2000\/svg" width="1280" height="320" viewBox="0 0 1280 320">/' \
    "header-$mode.svg"
  grep -q 'viewBox="0 0 1280 320"' "header-$mode.svg"
  if grep -q '<text' "header-$mode.svg"; then
    echo "header-$mode.svg still has text" >&2
    exit 1
  fi
done

# App icons.
for n in 16 32 48 64 128 256 512; do
  src=icon.svg
  [ -f "icon-$n.svg" ] && src="icon-$n.svg"
  resvg "${fonts[@]}" -w "$n" -h "$n" "$src" "gen/icon-$n.png"
done
oxipng --quiet --opt 4 --strip all gen/icon-*.png

# The .ico, written directly: a directory of PNG-compressed images (Windows Vista and later;
# ImageMagick would store them as uncompressed bitmaps, 280 KB).
le() { # le <bytes> <value>: little-endian as printf escapes
  local i v=$2
  for ((i = 0; i < $1; i++)); do printf '\\x%02x' $((v & 255)); v=$((v >> 8)); done
}
ico_sizes=(16 32 48 256)
{
  printf "$(le 2 0)$(le 2 1)$(le 2 ${#ico_sizes[@]})"
  offset=$((6 + 16 * ${#ico_sizes[@]}))
  for n in "${ico_sizes[@]}"; do
    len=$(stat -c %s "gen/icon-$n.png")
    # Width and height (0 = 256), no palette, reserved, 1 plane, 32 bpp, size, offset.
    printf "$(le 1 $((n & 255)))$(le 1 $((n & 255)))$(le 2 0)$(le 2 1)$(le 2 32)$(le 4 "$len")$(le 4 $offset)"
    offset=$((offset + len))
  done
  for n in "${ico_sizes[@]}"; do cat "gen/icon-$n.png"; done
} >gen/shuppatsu.ico

# The language screen's board: the dark header without its backdrop and posts, cropped to the
# board and its frame (x 24..1256, y 16..280).
sed -e '/<rect width="1280" height="320"/d' -e '/<rect x="250" y="272"/d' \
  -e 's/viewBox="0 0 1280 320" width="1280" height="320"/viewBox="24 16 1232 264" width="1232" height="264"/' \
  src/header-dark.svg >"$tmp/board.svg"
resvg "${fonts[@]}" "$tmp/board.svg" "$tmp/board.png"

pack() { cargo run --quiet --release --locked -p shuppatsu-tools -- pack-rgba "$@"; }
pack "$tmp/board.png" gen/board.shpk
pack gen/icon-256.png gen/icon-256.shpk
pack gen/icon-32.png gen/icon-32.shpk
ls -l header-*.svg gen/

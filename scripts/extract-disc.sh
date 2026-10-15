#!/usr/bin/env bash
# Extract the user's disc image into $DDG_DATA_DIR (default ./data/extracted).
# Usage: scripts/extract-disc.sh <image.cue | image.mdf | image.iso>
# Run inside `nix develop`. No root, no loop-mounting.
set -euo pipefail

img=$(realpath "$1")
out=${DDG_DATA_DIR:-$PWD/data/extracted}
work=$PWD/data/work
mkdir -p "$out" "$work"

case "$img" in
  *.cue) cue=$img ;;
  *.mdf | *.bin)
    # Single raw-sector (2352 B) data track without a usable cue: synthesise one.
    cue=$work/$(basename "${img%.*}").cue
    printf 'FILE "%s" BINARY\n  TRACK 01 MODE1/2352\n    INDEX 01 00:00:00\n' "$img" > "$cue"
    ;;
  *.iso) iso=$img ;;
  *) echo "unsupported image: $img" >&2; exit 1 ;;
esac

if [[ -z ${iso:-} ]]; then
  bin=$(sed -n 's/^FILE "\(.*\)" BINARY.*/\1/p' "$cue" | head -1)
  [[ $bin = /* ]] || bin=$(dirname "$cue")/$bin
  (cd "$work" && bchunk -w "$bin" "$cue" track)   # -w: audio tracks as WAV
  iso=$(ls "$work"/track*.iso | head -1)
fi

rm -rf "$out/disc" "$out/install"
7z x -y -o"$out/disc" "$iso" >/dev/null || echo "warning: 7z reported errors (damaged image?)" >&2
mv "$work"/track*.wav "$out/disc/" 2>/dev/null || true

# Game files live in an InstallShield 5 cabinet; unpack the "installed" tree.
cab=$(find "$out/disc" -maxdepth 1 -iname data1.cab | head -1)
if [[ -n $cab ]]; then
  unshield -d "$out/install" x "$cab" || echo "warning: unshield reported errors" >&2
fi
echo "extracted to $out"

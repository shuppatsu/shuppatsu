#!/usr/bin/env bash
# Dev check of our MPEG-1 decoders against ffmpeg (used only here, never at runtime).
# Decodes komachi.dat with ffmpeg to raw yuv420p and f32le PCM in out/movie/ (gitignored),
# then runs `shuppatsu-tools movie compare` for per-frame PSNR and audio SNR.
#
# usage: scripts/movie-compare.sh [idct]   idct: ffmpeg's -idct choice (default: simple)
set -euo pipefail
cd "$(dirname "$0")/.."
: "${SHUPPATSU_DATA_DIR:=$PWD/data/extracted}"
src="$SHUPPATSU_DATA_DIR/install/Program_Executable_Files/DATA/komachi.dat"
idct="${1:-simple}"
out=out/movie
mkdir -p "$out"
env -u LD_LIBRARY_PATH nix shell nixpkgs#ffmpeg -c ffmpeg -v error -y -idct "$idct" -i "$src" \
  -map 0:v -f rawvideo -pix_fmt yuv420p "$out/ffmpeg-$idct.yuv" \
  -map 0:a -c:a pcm_f32le -f f32le "$out/ffmpeg.f32"
cargo run -q --release -p shuppatsu-tools -- movie compare "$src" \
  --ref-yuv "$out/ffmpeg-$idct.yuv" --ref-audio "$out/ffmpeg.f32" --csv "$out/psnr-$idct.csv"

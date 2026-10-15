#!/usr/bin/env bash
# Run the original game under Wine (measurement oracle only). Run inside `nix develop`.
# Expects the game installed at "$WINEPREFIX/drive_c/Program Files (x86)/TAITO/DENGO3K".
# Usage: scripts/run-original.sh [dengo3k.exe|DengoSetting.exe] [args...]
set -euo pipefail
: "${WINEPREFIX:?run inside nix develop}"
export LC_ALL=ja_JP.UTF-8 # ANSI code page 932; LANG alone can be overridden by the host
exe=${1:-dengo3k.exe}
shift || true
cd "$WINEPREFIX/drive_c/Program Files (x86)/TAITO/DENGO3K/BIN" # game uses ..\data paths
exec wine "$exe" "$@"

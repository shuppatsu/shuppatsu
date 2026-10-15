#!/usr/bin/env bash
# Run the original game under Wine (measurement oracle only). Run inside `nix develop`.
# Expects the game installed at "$WINEPREFIX/drive_c/Program Files (x86)/TAITO/DENGO3K"
# and the disc presented by scripts/mount-disc.sh.
#
# Usage: scripts/run-original.sh [dengo3k.exe|DengoSetting.exe] [args...]
# Env:   DDG_WINE_DESKTOP=WxH   virtual desktop size (default 640x480; "off" to disable)
#
# Why a virtual desktop: the game calls SetDisplayMode(640x480x16) and creates a 640x480
# Z-buffer. On Wayland/XWayland Wine can't change the real mode, so the back buffer stays at
# desktop size; wined3d then disables the mismatched depth buffer and ignores the per-frame
# clears (wrong z-order, smeared frames). A virtual desktop of the requested size avoids that.
set -euo pipefail
: "${WINEPREFIX:?run inside nix develop}"
export LC_ALL=ja_JP.UTF-8 # ANSI code page 932; LANG alone can be overridden by the host
exe=${1:-dengo3k.exe}
shift || true
desktop=${DDG_WINE_DESKTOP:-640x480}
cd "$WINEPREFIX/drive_c/Program Files (x86)/TAITO/DENGO3K/BIN" # game uses ..\data paths
if [[ $desktop == off ]]; then
  exec wine "$exe" "$@"
fi
exec wine explorer "/desktop=dengo3k,$desktop" "$exe" "$@"

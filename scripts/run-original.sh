#!/usr/bin/env bash
# Run the original game under Wine (measurement oracle only). Run inside `nix develop`.
# Expects the game installed at "$WINEPREFIX/drive_c/Program Files (x86)/TAITO/DENGO3K"
# and the disc presented by scripts/mount-disc.sh.
#
# Usage: scripts/run-original.sh [dengo3k.exe|DengoSetting.exe] [args...]
# Env:   DDG_WINE_DISPLAY=gamescope|wayland|desktop (default: wayland if available, else desktop)
#        DDG_WINE_WINDOW_W/H                gamescope window size (default 1280x960)
#        DDG_WINE_DESKTOP=WxH               virtual desktop size for "desktop" (default 640x480)
#
# Why: the game calls SetDisplayMode(640x480x16) and creates a 640x480 Z-buffer. Under
# winex11 on XWayland the real mode never changes, the back buffer stays desktop-sized, and
# wined3d disables the mismatched depth buffer and skips frame clears (wrong z-order, smeared
# frames). winewayland emulates the mode change with compositor scaling (full screen, correct
# sizes); a virtual desktop of the requested size is the X11 fallback (unscaled).
set -euo pipefail
: "${WINEPREFIX:?run inside nix develop}"
export LC_ALL=ja_JP.UTF-8 # ANSI code page 932; LANG alone can be overridden by the host
exe=${1:-dengo3k.exe}
shift || true
mode=${DDG_WINE_DISPLAY:-$([[ -n ${WAYLAND_DISPLAY:-} ]] && echo wayland || echo desktop)}
bin='C:\Program Files (x86)\TAITO\DENGO3K\BIN'
cd "$WINEPREFIX/drive_c/Program Files (x86)/TAITO/DENGO3K/BIN" # game uses ..\data paths

case $mode in
  wayland)
    wine reg add 'HKCU\Software\Wine\Drivers' /v Graphics /d wayland /f >/dev/null
    exec wine "$exe" "$@"
    ;;
  gamescope)
    # Nested compositor: a normal desktop window whose inner "screen" is 640x480, so the
    # game's mode change matches and the output is scaled into the window.
    wine reg add 'HKCU\Software\Wine\Drivers' /v Graphics /d x11 /f >/dev/null
    # The dev shell's LD_LIBRARY_PATH breaks gamescope (SDL3 load fails); restore it for Wine only.
    exec env -u LD_LIBRARY_PATH nix shell nixpkgs#gamescope -c gamescope -w 640 -h 480 \
      -W "${DDG_WINE_WINDOW_W:-1280}" -H "${DDG_WINE_WINDOW_H:-960}" -- \
      env LD_LIBRARY_PATH="${LD_LIBRARY_PATH:-}" wine "$exe" "$@"
    ;;
  desktop)
    wine reg add 'HKCU\Software\Wine\Drivers' /v Graphics /d x11 /f >/dev/null
    # explorer resolves the program on the Windows search path, not the cwd, so pass the
    # game directory explicitly via start.exe (/d also sets the working directory).
    exec wine explorer "/desktop=dengo3k,${DDG_WINE_DESKTOP:-640x480}" start /wait /d "$bin" "$exe" "$@"
    ;;
  *) echo "DDG_WINE_DISPLAY must be gamescope, wayland or desktop" >&2; exit 1 ;;
esac

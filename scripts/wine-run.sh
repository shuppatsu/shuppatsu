#!/usr/bin/env bash
# Cargo runner for x86_64-pc-windows-gnu: runs a Windows exe under Wine in a test prefix that is
# separate from the original game's (./wine/prefix). Set as CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER
# by the dev shell, so `cargo test --target x86_64-pc-windows-gnu` works (docs/building.md).
#
# Environment: SHUPPATSU_WINE_PREFIX overrides the prefix (default ./wine/test-prefix). SHUPPATSU_DATA_DIR,
# SHUPPATSU_TRACE_DIR and SHUPPATSU_FONT given as Unix paths are mapped to Wine's Z: drive.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export WINEPREFIX="${SHUPPATSU_WINE_PREFIX:-$root/wine/test-prefix}"
export WINEDEBUG="${WINEDEBUG:--all}"
# No Mono/Gecko install prompts when the prefix is created.
export WINEDLLOVERRIDES="${WINEDLLOVERRIDES:-mscoree,mshtml=}"

if [ ! -d "$WINEPREFIX" ]; then
  mkdir -p "$WINEPREFIX"
  wineboot -u >/dev/null 2>&1 || true
fi

# Unix absolute path -> Z:\ path.
to_wine() { printf 'Z:%s' "${1//\//\\}"; }
for v in SHUPPATSU_DATA_DIR SHUPPATSU_TRACE_DIR SHUPPATSU_FONT SHUPPATSU_DISPLAY_FONT; do
  val="${!v:-}"
  if [ -n "$val" ] && [ "${val:0:1}" = / ]; then
    # Wine passes the Unix environment through to the Windows process.
    export "$v=$(to_wine "$val")"
  fi
done

exec wine "$@"

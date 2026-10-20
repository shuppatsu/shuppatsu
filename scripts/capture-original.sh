#!/usr/bin/env bash
# Capture frames of the ORIGINAL game at chosen driving ticks, headless, for side-by-side
# comparisons with the port. Run inside `nix develop`, with the disc presented by
# scripts/mount-disc.sh. Measurement only: the game runs unmodified (oracle hook, CLAUDE.md).
#
# Usage: scripts/capture-original.sh <name> <group> <row> <inputs> <tick>[,<tick>...]
#   name    output prefix: out/captures/<name>_<tick>.png, trace out/captures/<name>.csv
#           (not out/traces: the replay tests expect complete driving recordings there)
#   group   route-select line group 0..4 (docs/re/menus.md §6.1; 0 = 北陸 = stages 5, 6, 8, 9, 7)
#   row     train row within the group (e.g. group 0 row 3 = stage 9, 485 series from 直江津)
#   inputs  oracle input script (`tick notch brake`, hook driving ticks; crates/shuppatsu-hook)
#   tick    driving ticks at which to grab the screen (comma-separated; "-" for none)
#           (ticks count from 0 again on each leg; `leg` lines in the inputs drive later legs;
#           with a `from wait` line, legs and ticks start at the pre-departure wait and the
#           levers are set during it, for false-start probes)
# Env:  SHUPPATSU_CAP_DISPLAY  X display for Xvfb (default :77)
#       SHUPPATSU_CAP_SCHEMA   trace schema (default oracle/full.schema)
#       SHUPPATSU_CAP_STAGE    expected stage (0x8570a4); the run aborts if the trace shows another
#       SHUPPATSU_CAP_LEG      leg (1 = first) the capture ticks belong to (default 1)
#       SHUPPATSU_CAP_BOOT     seconds to wait before the first key (default 25)
#       SHUPPATSU_CAP_RECORD   seconds to record the screen right after the train is chosen (ffmpeg
#                        x11grab at 60 fps, out/captures/<name>.mkv plus PNG frames in
#                        out/captures/<name>_rec/), for what comes before driving (stage intro,
#                        departure board). Frames carry no tick; align them on content.
#
# How it works: Xvfb at 640x480x16 (the game's own mode, so wined3d keeps its Z-buffer and
# clears; see run-original.sh), the game in a Wine virtual desktop of the same size, started
# through scripts/run-oracle.sh so the hook drives the train from <inputs> and logs a trace.
# The menus are walked with xdotool key presses (Enter skips attract to the title picture, Enter
# opens the main menu, Enter = アーケードモード, Down×group + Enter, Down×row + Enter). The
# trace's hook_tick column (flushed every 30 rows) says when each capture tick is reached; the
# screen is then grabbed with ImageMagick `import`. Captures are therefore up to ~1 s late, so
# pick ticks where the train stands still. Rendering is wined3d on llvmpipe; the game still
# runs at its 30 Hz tick.
#
# Difficulty: the original plays at the difficulty of the prefix's save, BIN\eeprom.dat (+6 bits
# 14-15, docs/re/menus.md §13.1). With no eeprom.dat (the prefix as installed) it boots with the
# exe's default save image (0x467768), whose difficulty is 1 (Normal): 0x40e910 sets 0x829d10
# from it whether or not a file was read. `shuppatsu play` defaults to difficulty 0 (Easy), so
# pass the same `--difficulty N` to the port for comparisons; the trace's `difficulty` column
# says which one the original used. Example: stage 5 starts with 30 s at difficulty 1 and 90 s
# at 0 (start-time table 0x4599b0), so the same inputs left 015 s in the original and 080 s in
# the port at difficulty 0, and 015 s at `--difficulty 1`.
#
# Example (stage 9, stopped ~860 m before 十日町 passing):
#   scripts/capture-original.sh horizon9 0 3 out/explain/horizon_verify/hook_inputs.txt 5700
set -euo pipefail
: "${WINEPREFIX:?run inside nix develop}"
name=${1:?usage: see header}
group=${2:?group}
row=${3:?row}
inputs=${4:?inputs}
record=${SHUPPATSU_CAP_RECORD:-}
ticks=${5:?ticks}
disp=${SHUPPATSU_CAP_DISPLAY:-:77}
schema=${SHUPPATSU_CAP_SCHEMA:-oracle/full.schema}
boot=${SHUPPATSU_CAP_BOOT:-25}

mkdir -p out/captures
trace=out/captures/$name.csv
log=out/captures/$name.wine.log
rm -f "$trace"

nix shell nixpkgs#xorg.xorgserver -c Xvfb "$disp" -screen 0 640x480x16 +extension GLX \
  >"out/captures/$name.xvfb.log" 2>&1 &
xvfb=$!
cleanup() {
  wineserver -k 2>/dev/null || true
  kill "$xvfb" 2>/dev/null || true
  wait "$xvfb" 2>/dev/null || true
}
trap cleanup EXIT
sleep 3

export DISPLAY=$disp
unset WAYLAND_DISPLAY
SHUPPATSU_ORACLE_DIR=out/captures SHUPPATSU_WINE_DISPLAY=desktop SHUPPATSU_WINE_DESKTOP=640x480 \
  scripts/run-oracle.sh "$name" "$schema" "$inputs" >"$log" 2>&1 &

key() { nix shell nixpkgs#xdotool -c xdotool key --delay 200 "$@"; }
grab() { nix shell nixpkgs#imagemagick -c import -display "$disp" -window root "$1"; }

# The hook build in run-oracle.sh happens first; wait until the trace exists, then for attract.
for _ in $(seq 300); do [[ -f $trace ]] && break; sleep 1; done
[[ -f $trace ]] || { echo "no trace after 300 s; see $log" >&2; exit 1; }
sleep "$boot"
nix shell nixpkgs#xdotool -c xdotool mousemove 320 240
key Return; sleep 3 # attract -> title picture
key Return; sleep 3 # title picture -> main menu
key Return; sleep 3 # アーケードモード -> line select
for _ in $(seq "$group"); do key Down; done
key Return; sleep 3 # -> train select
for _ in $(seq "$row"); do key Down; done
sleep 1
grab "out/captures/${name}_select.png"
if [[ -n $record ]]; then
  # Started before the key, so the first frames after it are not lost to ffmpeg's start-up.
  # The dev shell's LD_LIBRARY_PATH (alsa-lib) breaks ffmpeg; it runs without it.
  rm -rf "out/captures/${name}_rec" "out/captures/$name.mkv"
  env -u LD_LIBRARY_PATH nix shell nixpkgs#ffmpeg-full -c ffmpeg -loglevel error -f x11grab -framerate 60 \
    -video_size 640x480 -i "$disp" -t "$record" -c:v ffv1 "out/captures/$name.mkv" &
  rec=$!
  sleep 3
fi
key Return

if [[ -n $record ]]; then
  wait "$rec"
  mkdir -p "out/captures/${name}_rec"
  env -u LD_LIBRARY_PATH nix shell nixpkgs#ffmpeg-full -c ffmpeg -loglevel error -i "out/captures/$name.mkv" \
    "out/captures/${name}_rec/%04d.png"
  echo "recorded $record s: out/captures/$name.mkv, frames in out/captures/${name}_rec/"
fi
[[ $ticks == - ]] && exit 0

# hook_tick (column 1) is -1 outside driving; stage is a column of the full/base schemas.
col=$(head -1 "$trace" | tr , '\n' | grep -nx stage | cut -d: -f1 || true)
IFS=, read -ra want <<<"$ticks"
for t in "${want[@]}"; do
  while :; do
    last=$(tail -n 1 "$trace")
    now=${last%%,*}
    if [[ -n ${SHUPPATSU_CAP_STAGE:-} && -n $col && $now != -1 && $now =~ ^[0-9]+$ ]]; then
      st=$(cut -d, -f"$col" <<<"$last")
      [[ $st == "$SHUPPATSU_CAP_STAGE" ]] || { echo "stage $st, expected $SHUPPATSU_CAP_STAGE" >&2; exit 1; }
    fi
    # Legs started so far: one row with hook_tick 0 each.
    legs=$(grep -c "^0," "$trace" || true)
    ((legs >= ${SHUPPATSU_CAP_LEG:-1})) || { sleep 0.5; continue; }
    [[ $now =~ ^[0-9]+$ ]] && ((now >= t)) && break
    pgrep -f dengo3k.exe >/dev/null || { echo "game exited; see $log" >&2; exit 1; }
    sleep 0.5
  done
  grab "out/captures/${name}_$t.png"
  echo "captured driving tick $t (trace at $now): out/captures/${name}_$t.png"
done

#!/usr/bin/env bash
# Run the original game under Wine with the ddg-hook oracle DLL. Run inside `nix develop`.
#
# Usage: scripts/run-oracle.sh <name> [schema] [inputs]
#   name    trace name; writes <dir>/<name>.csv and <dir>/<name>.log
#           (dir = $DDG_ORACLE_DIR, default out/traces; the replay tests read every csv there)
#   schema  trace schema (default oracle/base.schema)
#   inputs  optional input script (`tick notch brake` lines, driving ticks)
# Other env (e.g. DDG_WINE_DISPLAY) is passed to scripts/run-original.sh.
set -euo pipefail
: "${WINEPREFIX:?run inside nix develop}"
name=${1:?usage: scripts/run-oracle.sh <name> [schema] [inputs]}
schema=$(realpath "${2:-oracle/base.schema}")
inputs=${3:+$(realpath "$3")}
dir=${DDG_ORACLE_DIR:-out/traces}
mkdir -p "$dir"
out=$(realpath "$dir")

cargo build -q --release -p ddg-hook --target i686-pc-windows-gnu
bin="$WINEPREFIX/drive_c/Program Files (x86)/TAITO/DENGO3K/BIN"
cp target/i686-pc-windows-gnu/release/ddg_hook.dll "$bin/dinput.dll"

# Unix paths are reachable through Wine's Z: drive.
win() { printf 'Z:%s' "${1//\//\\}"; }
export WINEDLLOVERRIDES="dinput=n,b${WINEDLLOVERRIDES:+;$WINEDLLOVERRIDES}"
export DDG_HOOK_SCHEMA=$(win "$schema")
export DDG_HOOK_TRACE=$(win "$out/$name.csv")
export DDG_HOOK_LOG=$(win "$out/$name.log")
if [[ -n $inputs ]]; then export DDG_HOOK_INPUTS=$(win "$inputs"); fi
echo "trace: $out/$name.csv"
exec scripts/run-original.sh dengo3k.exe

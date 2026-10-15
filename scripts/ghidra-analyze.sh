#!/usr/bin/env bash
# Headless Ghidra import + analysis of the original exe, then per-function decompilation
# export to re/decomp/<exe>/ (gitignored). Run inside `nix develop`.
# Usage: scripts/ghidra-analyze.sh <path/to/file.exe>
set -euo pipefail
exe=$(realpath "$1"); name=$(basename "${exe%.*}")
mkdir -p re/ghidra re/decomp/"$name"
"$GHIDRA_HOME/support/analyzeHeadless" re/ghidra densha \
  -import "$exe" -overwrite \
  -scriptPath "$PWD/scripts/ghidra" \
  -postScript ExportDecomp.java "$PWD/re/decomp/$name" \
  -log "$PWD/re/ghidra/$name.log"

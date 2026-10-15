# Densha de GO! 2 Kōsoku-hen 3000-bandai — native Rust reimplementation

## Goal

A native, open-source reimplementation of the Windows version of **Densha de GO! 2 Kōsoku-hen**
(Unbalance, 1999), targeting the **3000-bandai** update and also supporting the original
Kōsoku-hen release. The program reads the original game data from the user's own disc
at runtime. It is a new engine, not a wrapper around the original exe.

Must-haves:
- Arcade-faithful gameplay: physics, signals, speed limits, stopping-point scoring and the penalty
  system must match the original. Measure the original's behaviour instead of guessing it.
- Rendering with `wgpu` (Vulkan backend on Linux; DX12/Vulkan on Windows).
- Any aspect ratio (4:3, 16:10, 16:9, 21:9) with Hor+ FOV, plus an optional 4:3 pillarbox mode.
- High refresh rates: fixed simulation tick at the original rate; rendering interpolates and is uncapped or vsynced.
- Translations: English first, with the structure ready for more languages.
- Linux first (native, no Wine needed to play), Windows second.

## Hard rules

1. **No game data in the repo, ever.** This covers assets, extracted files, disc images, executables,
   Ghidra projects and decompiler output. They live in gitignored dirs (`data/`, `re/`, `out/`, `wine/`).
   Committing file hashes (for variant detection) is fine.
2. **Never download game files from the internet.** Use only what's at `$DDG_CUE` / `$DDG_DATA_DIR`.
3. **Write fresh Rust; don't transliterate decompiler output.** Ghidra output is reference material
   in `re/`. Behaviour and constants get documented in `docs/re/`, and the implementation is new,
   idiomatic code.
4. **Prefer reading tables from the user's data or exe at runtime** (physics curves, scoring values,
   route data) over hardcoding them. Small, verified constants may be hardcoded, each with a comment
   giving the source address.
5. **Copy protection:** if the disc or exe has any, stop and report it. Do not bypass it.

## Inputs

- `DDG_CUE`: path to the user's bin/cue disc image.
- `DDG_DATA_DIR`: extracted disc contents (default `./data/extracted`, gitignored).
- Disc variant (original vs 3000-bandai) is detected from file hashes and layout and recorded in
  `data-manifest/<variant>.toml` (hashes only).

## Environment

- Primary machine: NixOS unstable, an AMD RDNA 3.5 integrated GPU (RADV Vulkan), 2560×1600 16:10 laptop panel and a
  4K 120 Hz external display. Test 16:10 and 120 Hz explicitly.
- Everything comes from a `flake.nix` dev shell. Nothing is installed imperatively. The shell provides:
  - a stable Rust toolchain (rust-overlay or fenix) with the targets `x86_64-unknown-linux-gnu` and `i686-pc-windows-gnu`;
  - vulkan-loader, wayland, libxkbcommon and X11 libs for winit/wgpu;
  - bchunk, p7zip/libarchive, ghidra (headless), wine (32-bit capable) and a mingw32 cross toolchain;
  - hexyl and binwalk.
- Extract disc images without root: `bchunk` to get an ISO plus WAVs, then `7z x` or `bsdtar`. Don't loop-mount.
- Use a dedicated `WINEPREFIX=./wine/prefix` with `LANG=ja_JP.UTF-8` for running the original game.

## Workspace layout

```
crates/
  ddg-formats/   parsers for every original file format; pure, no I/O beyond &[u8]
  ddg-tools/     CLI: recon, inventory, dump, export (PNG/glTF to out/ for inspection)
  ddg-engine/    deterministic simulation: train physics, signals, timetable, scoring
                 (no graphics/audio deps; fixed tick; serialisable state for traces)
  ddg-render/    wgpu renderer: route, scenery, cab, HUD, text
  ddg-audio/     mixer, SFX, CD-audio tracks read from the bin
  ddg-game/      the binary: winit loop, input, menus, config, localisation
  ddg-hook/      i686 Windows DLL used only as a measurement oracle (see below)
docs/
  recon.md       phase 0 report
  re/            one note per subsystem/format
locales/         en.toml, … (string IDs → translated text; Japanese source read from data at runtime)
data-manifest/   per-variant file hashes
```

Allowed dependencies: wgpu, winit, glam, bytemuck, gilrs, cpal, encoding_rs, pelite, thiserror,
anyhow, serde, toml, image (tools only) and fontdue. Ask before adding anything else, because minimal
dependencies are a goal.

## Approach

### Reverse engineering
- Run Ghidra headless (`analyzeHeadless` from the nix store) and keep the project in `re/ghidra/`.
  Export per-function decompilation to `re/decomp/` with a script, so it's greppable.
- Find the important code by following xrefs from file I/O (CreateFileA/ReadFile), timing
  (timeGetTime/QueryPerformanceCounter), DirectX setup, input and text drawing.
- Every claim in `docs/re/*.md` carries a confidence level (`confirmed` / `likely` / `guess`) and its
  evidence: a function address, a file offset or an experiment.
- Treat Shift-JIS everywhere: filenames, strings and text tables. Decode with encoding_rs.

### Oracle and differential testing
- `ddg-hook` is a 32-bit proxy DLL (for whichever DLL recon shows the exe imports, e.g. ddraw/dinput/winmm),
  loaded into the original exe under Wine with `WINEDLLOVERRIDES`. It hooks game functions and logs
  per-tick state (position, speed, notch, brake, signal aspect, score/penalty, timers) to a trace file in `out/`.
- Scripted inputs are replayed in both the original and `ddg-engine`, and the traces are compared.
  Engine changes must keep the recorded traces matching within documented tolerances.
- Trace files are derived from game data, so they stay in gitignored `out/traces/`. Committed tests
  load them via `DDG_TRACE_DIR` and skip when it's absent.

### Fidelity vs enhancements
- "Arcade" defaults reproduce the original exactly. Every enhancement (widescreen, interpolation,
  extended draw distance, translations, QoL) is a config option with its own name.
- Widescreen: keep the original vertical FOV and widen horizontally. The original culls to a 4:3
  frustum, so scenery loading and culling must cover the wider view without pop-in at the edges.
  Anchor HUD elements to screen edges.
- High framerate: render interpolates between the last two sim states, and camera position along the
  track is interpolated on the track curve, not linearly.

## Phases

Each phase ends with a short report in the session and updated docs. Don't start the next phase
without the user's go-ahead.

0. **Recon.** Set up the flake, workspace skeleton and `.gitignore`. Extract the disc, including audio
   tracks. Inventory the files with sizes, magic bytes and an extension histogram. Analyse each PE:
   sections and entropy, imports (which DirectX/DInput/DSound/winmm), Rich header and compiler, resources,
   ASCII and Shift-JIS strings. Determine the install model (what the installer copies, registry keys,
   run-from-CD files) and the variant. Write the hash manifest and `docs/recon.md`.
   **Exit:** recon.md answers every point above, or lists it as an open question.
1. **Code map.** Ghidra analysis; locate the main loop and tick rate, the loaders, the renderer setup
   (resolution, FOV, projection), input, audio and text drawing. **Exit:** `docs/re/architecture.md`.
2. **Formats.** Parsers in `ddg-formats` for containers, textures, models, route/track geometry,
   scenery placement, signals, timetables and strings. `ddg-tools export` writes PNG/glTF to `out/`.
   **Exit:** every file on the disc is either parsed or documented as unknown.
3. **Viewer.** A wgpu fly-through of a full route (track, scenery, stations) at arbitrary aspect ratio
   and framerate. **Exit:** one route renders end-to-end at 16:10, 120 Hz, without edge pop-in.
4. **Simulation.** `ddg-engine` with the oracle hook and differential traces.
   **Exit:** one route's traces match the original within tolerance.
5. **Game.** Menus, cab view, HUD, scoring screens, audio, input (keyboard, gamepad, real DDG controllers),
   localisation. **Exit:** every route is playable start to finish in English.
6. **Polish.** Config UI, Windows build, Nix package, README with the bring-your-own-disc instructions.

## Conventions

- Rust 2024 edition. `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo test` must pass.
- Data-dependent tests read `DDG_DATA_DIR` / `DDG_TRACE_DIR` and skip cleanly when unset.
- Parse binary data with explicit offsets and little-endian reads. Never transmute file bytes.
- `unsafe` only in `ddg-hook` and FFI, each block with a `// SAFETY:` comment.
- Libraries use thiserror; binaries use anyhow.
- Make small commits with conventional commit messages.

## Stop and ask when

- copy protection or anything that would need bypassing turns up;
- something derived from game data seems to need committing;
- you want a dependency outside the allowed list;
- a choice trades fidelity against an enhancement;
- a phase's scope is unclear, or a finding changes the plan above.

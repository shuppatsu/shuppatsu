# densha: Densha de GO! 2 Kōsoku-hen 3000-bandai, native

A native, open-source reimplementation of the Windows version of **Densha de GO! 2
Kōsoku-hen 3000-bandai** (電車でGO！２ 高速編 ３０００番台), written from scratch in Rust. It
plays the original game from **your own disc**: routes, models, textures, sounds, music and
the original's own tables are read from the disc's files at runtime. It is a new engine, not
a wrapper around the original exe, and it needs neither Windows nor Wine.

What you get on top of the original, each one a setting you can turn off:

* any window size and aspect ratio (4:3, 16:10, 16:9, 21:9) with a wider view (Hor+), no
  scenery pop-in at the edges, and an optional HUD laid out to the window's edges;
* smooth motion at high refresh rates: the simulation keeps the original's 30 Hz tick, and
  the picture is interpolated between ticks;
* an English mode: menus, HUD, captions and station names in English over the original art,
  plus English subtitles for the Japanese voices;
* modern gamepads, and the Densha de GO! controllers.

Driving, signals, speed limits, stop scoring and penalties follow the original. They were
measured from the original game, not guessed (see [docs/re/](docs/re/)).

## Legal stance

* This is an unofficial fan project, not affiliated with or endorsed by TAITO, Unbalance or
  JR East. *Densha de GO!* is a trademark of TAITO.
* **No game files are included** in this repository, its releases or its Nix package, and the
  program never downloads any. You need your own original disc.
* The program only reads files you extracted from that disc. It contains no code or data
  copied from the original; the original was studied to make the new engine behave the same.
* The disc's CD check is never touched: the port reads the extracted files and does not need
  the disc in a drive.

## Requirements

* **Linux** (x86-64; the main platform) or **Windows** (x86-64, 10 or later).
* A GPU with **Vulkan** (Linux) or **DirectX 12 / Vulkan** (Windows). Any GPU from the last
  ten years should do.
* The **3000-bandai** disc (2000, volume label `DENGO3K`). The port is developed and tested
  against it. The earlier *Kōsoku-hen* release (1999) is a goal, but not yet supported.
* About 450 MB of disk space for the extracted files.

## Getting the game data

You need the files from the disc, extracted into a folder. Do this once.

### 1. Make an image of your disc

Rip your disc to an image with any CD ripping tool, as **BIN/CUE**, **MDF/MDS** or **ISO**.
A raw image (BIN/CUE or MDF) is best: it is a 1:1 copy, and the same image can also run the
original game. On Linux, for example (with `cdrdao`; `/dev/sr0` is your drive):

```sh
cdrdao read-cd --read-raw --datafile dengo3k.bin --device /dev/sr0 dengo3k.toc
toc2cue dengo3k.toc dengo3k.cue
```

### 2. Extract it

`scripts/extract-disc.sh` unpacks an image without root or mounting. It needs `bchunk`, `7z`
and `unshield`; the dev shell has all three (`nix develop`, see
[Development](#development)).

```sh
nix develop                                  # or install bchunk, p7zip and unshield yourself
scripts/extract-disc.sh /path/to/dengo3k.cue # also .mdf, .bin or .iso; for MDF/MDS pass the .mdf
```

It writes to `$DDG_DATA_DIR`, by default `data/extracted` in the current directory, and uses
`data/work` for temporary files. It unpacks the disc's file system into `disc/`, then the
game's InstallShield cabinet into `install/`, so no Windows installer has to run.

Without the script: copy the installed game folder of a Windows install
(`C:\Program Files\TAITO\DENGO3K`, with `BIN` and `DATA`) into
`install/Program_Executable_Files/`. Either way, the game needs this layout:

```
data/extracted/                          ← DDG_DATA_DIR points here
  disc/                                  the disc's files (only needed for ddg-tools)
  install/Program_Executable_Files/
    BIN/dengo3k.exe                      the original exe: tables, the icon
    DATA/*.COM, cg.rom, tbl.rom, ...     routes, models, textures, sounds, music
```

### 3. Check it (optional)

[`data-manifest/3000-bandai.toml`](data-manifest/3000-bandai.toml) lists the SHA-256 of every
file on a good 3000-bandai disc (hashes only, no data). The quickest check is the exe:

```sh
sha256sum data/extracted/install/Program_Executable_Files/BIN/dengo3k.exe
# a5a555efe3ee8089d2d60268f58e4c72fc673a0b3adf5a1e8cfa1bf039c9353c
```

For the whole tree, `ddg-tools manifest` prints the same list for your files; no output from
the `diff` means every file matches. `ddg-tools check` then confirms that every file is parsed
or known (`0 failed, 0 unknown`):

```sh
ddg-tools manifest data/extracted 3000-bandai > mine.toml
diff <(grep -E '^(path|sha256)' mine.toml) <(grep -E '^(path|sha256)' data-manifest/3000-bandai.toml)
DDG_DATA_DIR=data/extracted ddg-tools check
```

(`ddg-tools` is `nix run .#ddg-tools --` or `cargo run --release -p ddg-tools --`.)

## Installing and running

The game looks for your extracted data in this order (the first that exists wins):

1. the `DDG_DATA_DIR` environment variable;
2. `--data <dir>` on the command line;
3. a folder `data/extracted` or `data` next to the `ddg-game` executable;
4. a folder `data/extracted` or `data` in the current directory (e.g. `nix run .` in a checkout);
5. `data_dir = "…"` in `config.toml` (see [Settings](#settings)).

Each can point at an extracted disc (the folder holding `install/`) or at an installed copy of
the original game (the folder holding `DATA` and `BIN`, e.g. `C:\Program Files\TAITO\DENGO3K`).
For example:

```sh
export DDG_DATA_DIR=/path/to/data/extracted
```

### With Nix (Linux)

```sh
nix run .                                # in a checkout of this repository
nix profile install .                    # installs ddg-game, with a desktop entry
nix run .#ddg-tools -- check             # the tools
```

The package contains the game and its libraries only, no game data. On NixOS it works as is.
On other distributions Nix's Vulkan loader may not find your system's GPU driver; run it
through [nixGL](https://github.com/nix-community/nixGL) (`nixVulkanIntel` for Mesa drivers,
`nixVulkanNvidia` for NVIDIA) if it reports no adapter.

### From source

You need a recent stable Rust toolchain and, on Linux, the development files of ALSA
and libudev plus pkg-config. At run time, the Vulkan loader, Wayland or X11 and libxkbcommon.

```sh
cargo build --release -p ddg-game
target/release/ddg-game
```

In `nix develop` everything is provided.

### Windows

Build the zip with `scripts/package-windows.sh` (cross-compiled from the Nix dev shell, see
[docs/building.md](docs/building.md)); it writes `out/dist/ddg-windows.zip`. Unzip it and put your
extracted data in a `data` folder next to `ddg-game.exe` (or use `--data` / `data_dir`). The game
uses DirectX 12, falling back to Vulkan. There is no console window; the log goes to
`%APPDATA%\ddg\ddg-game.log` (or run with `--console`). `install-desktop` is Linux-only; on
Windows the taskbar icon comes from your `dengo3k.exe` automatically.

### Command line

`ddg-game` with no arguments starts the game at the title screen. `ddg-game --help` lists every
option. Some useful ones:

```sh
ddg-game --size 2560x1600         # window size (default 1280x800)
ddg-game --lang en                # language for this run (en, or ja = the original art)
ddg-game play 0                   # straight into a stage (0..19), skipping the menus
ddg-game install-desktop          # desktop entry with the game's own icon (from your exe)
```

`install-desktop` writes a desktop entry and the icon from your `dengo3k.exe` to
`~/.local/share`, so launchers show the game with its real icon (`--uninstall` removes them).
It bakes the current `DDG_DATA_DIR` into the entry. After `nix profile install`, pass
`--exec ddg-game` so the entry keeps working after updates.

## First start

On the first start the game asks for the language: **English** or **日本語** (the original,
untouched). Pick with up/down and Enter. The choice is saved as `lang` in `config.toml`; to
change it, use `--lang` for one run, or edit or remove the `lang` line (removing it brings the
question back).

## Controls

Choose the control mode in the game's options menu, as in the original. Modes 0 to 3 are
keyboard modes, 4 to 6 controller modes:

| Mode | Power | Brake |
|---|---|---|
| 0 (default) | ↓ more, ↑ less | → more, ← less (one press past B8 = emergency) |
| 1 | Z more, A less | → more, ← less |
| 2 | hold 1–5 for P1–P5 (nothing = neutral) | numpad 1–8 = B1–B8, 9 = emergency, 0 = release |
| 3 | one handle: ↓ toward power, ↑ toward brake | |
| 4 | gamepad, digital: stick or d-pad like mode 0 | |
| 5 | gamepad, analog one-handle lever (stick Y) | |
| 6 | two-handle controller (Densha de GO! controller), calibrated | |

In every mode: **Space** horn, **Enter** ATS confirm and menu confirm, **Esc** pause and
cancel, arrows in the menus. Holding **Shift** lets the keyboard drive the menus in a
controller mode. Unlike the original, the keyboard's horn and ATS confirm also work in
controller modes (setting `keyboard_with_controller`).

**Gamepads** (anything SDL/gilrs knows): South (A / ✕) is ATS and menu confirm, East (B / ○)
horn and cancel, Start pause. The left stick or d-pad drives the levers in mode 4 and 5.

**Densha de GO! controllers**:

* Controllers that report their handles as two axes: mode 6. Calibrate them in the game's
  options (ADJUSTMENT), as in the original, or from a terminal with
  `ddg-game pad calibrate --joystick-dat ~/.config/ddg/joystick.dat`. The game keeps
  `joystick.dat` next to its save.
* The PS1/PS2 two-handle controllers on a USB adapter report the notches as button
  combinations. Teach the game the combinations once, then pass the file when playing:

  ```sh
  ddg-game pad learn --combo ~/.config/ddg/combo.txt   # follow the prompts
  ddg-game --combo ~/.config/ddg/combo.txt
  ```

* `ddg-game pad monitor` shows what the game reads from a controller.

The original's game-port controller driver is not needed.

## Settings

Settings live in `config.toml`:

* Linux: `$XDG_CONFIG_HOME/ddg/config.toml` (usually `~/.config/ddg/config.toml`)
* Windows: `%APPDATA%\ddg\config.toml`

The same folder holds the save (`eeprom.dat`, the original's format) and `joystick.dat`.
Every key is optional, and each can also be given as an environment variable (`DDG_<KEY>`)
or a flag (`--<key>` / `--no-<key>`). Flags win over the environment, which wins over the
file. [docs/settings.md](docs/settings.md) lists every setting with what the original did.

```toml
lang = "en"
hud_layout = "enhanced"   # HUD at the window's edges
ui_scaling = "integer"    # crisp whole-pixel 2D
texture_filtering = true  # no shimmering textures
```

Every enhancement is a named setting, so you can go back to the original's behaviour one
thing at a time.

### English mode

With `lang = "en"` the game draws English text over the original Japanese art: menus, the
HUD, signal and station captions and messages. Some extras, all on by default:

* `subtitles`: English subtitles for the spoken station, signal and speed-limit calls,
  announcements and the navigator.
* `one_line_captions`, `landmark_names`, `board_names`: station captions as one line, English
  names under the landmark plates, romaji under the route board.
* `staff_kanji`: the staff roll's names followed by their kanji, as in the original credits.

The text uses the bundled M+ 2 font; `--font` / `font` takes your own TTF/OTF list.
Translations live in [`locales/`](locales/) (see [docs/localisation.md](docs/localisation.md)
to add a language).

### Widescreen and scaling

* The 3D view keeps the original's vertical field of view and widens to the window
  (`wide_visibility` keeps scenery from popping in at the sides). `square_pixels` drops the
  original's 7.9 % horizontal stretch.
* The 2D layer (HUD, menus) is the original's 640×480 design, scaled up. `ui_scaling`:
  `smooth` (default, sharp bilinear), `integer` (whole multiples, centred, crispest) or
  `nearest`. `ui_scale` fixes the multiple (1–4).
* `hud_layout = "enhanced"` moves the driving HUD to the window's edges on wide screens;
  `original` keeps the centred 4:3 layout. `wide_menus` extends the menu backdrops to the
  edges.
* `interpolate` and `vsync` control smoothness: by default the picture is interpolated between
  the 30 Hz ticks and presented with vsync at your display's rate; `--uncapped` turns vsync
  off.

## Not supported

* The **Komachi movie** (`komachi.dat`, an MPEG-1 video) does not play.
* The original *Kōsoku-hen* release (not 3000-bandai), see Requirements.

## Troubleshooting

* **`the game data was not found`**: put the extracted data where the game looks for it (see
  [Installing and running](#installing-and-running)), e.g. `--data /path/to/data/extracted`.
* **`… not found in …/DATA`**: the extraction is incomplete or the layout differs. Check
  that `install/Program_Executable_Files/DATA` and `BIN/dengo3k.exe` exist, and run the
  checks in [Getting the game data](#3-check-it-optional).
* **No window, or "no adapter" / Vulkan errors**: check `vulkaninfo --summary`. Outside
  NixOS, run the Nix package through nixGL (above).
* **No sound**: the game plays through ALSA (PipeWire and PulseAudio provide it). `--mute`
  starts silent.
* **The controller does nothing**: choose a controller mode (4–6) in the options; without a
  connected pad the game falls back to keyboard mode 0. Check what arrives with
  `ddg-game pad monitor --mode 6`.
* **A setting has no effect**: a bad value in `config.toml` is reported on the terminal and
  only that setting is ignored. A file that does not parse is reported and ignored as a whole.
* **Start over with the settings**: delete `config.toml` (your save is `eeprom.dat`, keep it).

## Development

This repository holds code and documentation only. **Never commit game data**: no disc
images, extracted files, exes, decompiler output or traces. They live in the gitignored
`data/`, `re/`, `out/` and `wine/`. Read [CLAUDE.md](CLAUDE.md) for the project's rules.

```sh
nix develop                     # Rust, Vulkan, Wayland/X11, bchunk, 7z, Ghidra, Wine, mingw
cargo test                      # data tests skip unless DDG_DATA_DIR is set
cargo clippy --all-targets -- -D warnings
cargo fmt
nix build                       # the package; runs the tests without data
```

The dev shell sets `DDG_DATA_DIR` to `data/extracted` when it exists, so the data tests run.

Crates: `ddg-formats` (file formats), `ddg-engine` (the deterministic simulation),
`ddg-render` (wgpu), `ddg-audio`, `ddg-game` (the program), `ddg-tools` (inspection and
export: `ddg-tools` lists the commands) and `ddg-hook`.

Fidelity is checked against the original: `ddg-hook` is a 32-bit proxy DLL that runs inside
the original game under Wine and logs its state every tick (`scripts/run-oracle.sh`, schemas in
`oracle/`). `ddg-tools trace-replay` and the tests replay those traces through the engine and
compare (traces in `DDG_TRACE_DIR`, default `out/traces`; they are derived from game data and
stay out of the repo). The reverse-engineering notes, each claim with its evidence, are in
[docs/re/](docs/re/), and the disc analysis is [docs/recon.md](docs/recon.md).

## Credits

* **Densha de GO! 2 Kōsoku-hen 3000-bandai**: © TAITO 1996, 2000, Windows version sold by
  Unbalance, with JR East Planning. All game content (art, sound, routes) is theirs and comes
  from your disc.
* **M+ FONTS** (M+ 2 Medium), bundled for the translated text, under the SIL Open Font
  License 1.1 ([assets/fonts/OFL-Mplus.txt](assets/fonts/OFL-Mplus.txt)).
* Built with Rust and [wgpu](https://wgpu.rs), winit, glam, cpal, gilrs, fontdue,
  encoding_rs, pelite and serde; reverse engineering with Ghidra, Wine, bchunk, 7-Zip and
  unshield.

## License

GPL-3.0-or-later, see [LICENSE](LICENSE). The bundled font is under the SIL OFL 1.1.

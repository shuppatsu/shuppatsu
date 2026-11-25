# Building

Everything comes from the flake's dev shell (`nix develop`); nothing is installed imperatively.
The shell has a stable Rust with the `x86_64-unknown-linux-gnu`, `x86_64-pc-windows-gnu` and
`i686-pc-windows-gnu` targets, the mingw-w64 cross compilers for both Windows targets, and
Wine.

## Linux

```sh
nix develop
cargo build --release -p shuppatsu -p shuppatsu-tools   # target/release/shuppatsu, shuppatsu-tools
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace                            # data tests need SHUPPATSU_DATA_DIR (see below)
```

`nix build` (the flake's `packages`) builds the same binaries as a Nix package (see README.md).
Running from the dev shell works because it puts the Vulkan loader, Wayland and X11 libraries on
`LD_LIBRARY_PATH` (and libglvnd's libEGL for the OpenGL fallback). A bare `target/release/shuppatsu`
outside the shell can't find them on NixOS.

### Portable Linux build

`scripts/package-linux.sh` builds the release binaries, strips them and writes
`out/dist/Shuppatsu-v<version>-Linux-x64.tar.xz` and the same folder as
`Shuppatsu-v<version>-Linux-x64.AppImage` (with `AppRun`, `shuppatsu.desktop`, the 256 px icon,
`LICENSE.txt`, `OFL-Mplus.txt`, `settings.md` and a `README-linux.txt`; no game data).

The binaries only run on other distributions when built on an old base, so the release builds
them in an `ubuntu:22.04` container: glibc 2.35 is the newest symbol version they need, and they
link only libc, libm, libgcc_s, libasound and libudev. Vulkan, Wayland, xkbcommon, X11 and EGL
are loaded at runtime from the player's system. A build in the dev shell links against
`/nix/store` and runs only on Nix. To build the portable one locally:

```sh
docker run --rm -v "$PWD":/src -w /src ubuntu:22.04 bash -c '
  apt-get update && apt-get install -y curl build-essential pkg-config libasound2-dev libudev-dev xz-utils file git &&
  curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal && . ~/.cargo/env &&
  APPIMAGETOOL=/path/to/appimagetool scripts/package-linux.sh'
```

Without `appimagetool` (`$APPIMAGETOOL` or on `PATH`) only the archive is written. NixOS hosts
with `programs.appimage.binfmt` route every AppImage, appimagetool included, to `appimage-run`,
which doesn't exist inside a container. Extract appimagetool there first (`unsquashfs -o <ELF
size>`) and point `APPIMAGETOOL` at its `AppRun`.

Verified 2026-11-25: the archive's binaries start on Debian 12 and Fedora 41, and the AppImage
plays the attract sequence from the disc data on NixOS (`appimage-run`, RADV).

## Continuous integration and releases

`.github/workflows/ci.yml` runs on every push to `main` and every pull request: `cargo fmt
--check`, clippy with `-D warnings` and `cargo test --workspace` on Ubuntu (data tests skip), and
the Windows cross build in the Nix dev shell.

`.github/workflows/release.yml` runs when a `v*` tag is pushed. It checks that the tag matches
the workspace version in `Cargo.toml`, builds the Linux packages in `ubuntu:22.04` and the
Windows zip with `scripts/package-windows.sh` in the Nix dev shell, and publishes a GitHub
release made by `github-actions` with:

| Asset | Contents |
|---|---|
| `Shuppatsu-vX.Y.Z-Linux-x64.tar.xz` | portable Linux build |
| `Shuppatsu-vX.Y.Z-Linux-x64.AppImage` | the same folder as an AppImage |
| `Shuppatsu-vX.Y.Z-Windows-x64.zip` | the Windows build |
| `SHA256SUMS.txt` | their hashes |

The names carry the system and processor, as RecompLauncher expects. A tag with a suffix
(`v0.2.0-beta.1`) becomes a pre-release. To release:

```sh
# bump [workspace.package] version in Cargo.toml, commit, then
git tag v0.2.0 && git push origin v0.2.0
```

## Windows (cross-compiled from Linux)

The game and tools build for **x86_64-pc-windows-gnu** with nixpkgs' mingw-w64 GCC as the
linker (`pkgsCross.mingwW64`). This was chosen over `x86_64-pc-windows-msvc` with cargo-xwin for
three reasons. The GNU target is already in rust-overlay, and nixpkgs has the toolchain, so the
flake needs no new inputs. cargo-xwin would download the MSVC CRT and Windows SDK at build time,
and that is neither in nixpkgs nor reproducible. The hook DLL (`i686-pc-windows-gnu`) already
uses the same setup.

```sh
nix develop
cargo build --release --target x86_64-pc-windows-gnu -p shuppatsu -p shuppatsu-tools
# target/x86_64-pc-windows-gnu/release/{shuppatsu,shuppatsu-tools}.exe
scripts/package-windows.sh                        # out/dist/shuppatsu-windows.zip
```

Linker settings live in the flake as `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_*`. mcfgthread is linked
statically, so the exes import only system DLLs. Check with
`x86_64-w64-mingw32-objdump -p shuppatsu.exe | grep 'DLL Name'`; no MinGW runtime DLL has to ship.

`scripts/package-windows.sh` builds the release exes, strips them and zips them with
`LICENSE.txt` (GPL-3.0-or-later), `OFL-Mplus.txt` (the licence of the font compiled into the
exe), `settings.md` and a `README-windows.txt`. It includes no game data.

**Exe icon.** `crates/shuppatsu/build.rs` compiles `crates/shuppatsu/shuppatsu.rc` with the dev
shell's `x86_64-w64-mingw32-windres` (or `$WINDRES`) when the target is `windows-gnu`, and links
the resource object into the exe: `assets/logo/gen/shuppatsu.ico` (16, 32, 48 and 256 px, PNG
entries) as icon group 1, which Explorer and shortcuts show. Without `windres` the build only
warns and the exe has no icon; other targets skip the step. Check with
`x86_64-w64-mingw32-objdump -p shuppatsu.exe | grep -A2 '\.rsrc'`; the Windows test
`logo::tests::exe_has_the_icon_resource` reads it back with `shuppatsu_formats::icon`. The window
uses the same logo: the hinted 32 px icon in the title bar, 256 px for the taskbar and Alt-Tab
(`identity.rs`, embedded by `logo.rs`).

### Platform behaviour

| Area | Windows | Linux |
|---|---|---|
| Graphics | wgpu, **DX12 first**, then Vulkan, then GL (`gpu_backend.rs`). A backend is skipped when a probe pipeline does not compile. `WGPU_BACKEND=vulkan`/`dx12` forces one | wgpu defaults: Vulkan, else OpenGL (below) |
| Audio | cpal, WASAPI | cpal, ALSA (PipeWire/Pulse through their ALSA plugins) |
| Gamepads | gilrs, Windows.Gaming.Input | gilrs, evdev |
| Config, saves, log | `%APPDATA%\shuppatsu\` (`config.toml`, `eeprom.dat`, `joystick.dat`, `shuppatsu.log`); copied once from `%APPDATA%\ddg\` (the folder's name before the rename) when only that exists | `$XDG_CONFIG_HOME/shuppatsu/` (`~/.config/shuppatsu`); likewise copied once from `~/.config/ddg` |
| Console | release: GUI subsystem, no console window (see below); debug: console | terminal |
| File names | case-insensitive file system | data files are matched case-insensitively (`GameData::read`) |
| Font lists (`--font a;b`) | `;`-separated (`std::env::split_paths`) | `:`-separated |
| `install-desktop` | not available (make a shortcut) | desktop entry + icon |

**Logs on Windows** (`win_console.rs`): a release `shuppatsu.exe` started without a usable stderr
(from Explorer or a shortcut) writes stdout and stderr to `%APPDATA%\shuppatsu\shuppatsu.log`, which is
overwritten on each start. If it fails to start, it also shows the error in a message box.
`--console` attaches to the console of the command prompt that started it, or opens a new one.
A redirected stderr (`shuppatsu.exe 2> log.txt`) is used as it is. `shuppatsu-tools.exe` is a normal
console program.

### OpenGL fallback

Without Vulkan (or DX12 on Windows) wgpu opens OpenGL: desktop GL or GLES 3 through EGL on
Linux, WGL on Windows. `WGPU_BACKEND=gl` forces it, for testing. The game then runs with these
fallbacks, listed in one log line when the device opens (`gpu_backend::adapt`), e.g.

```
graphics: Gl (AMD Radeon Graphics (radeonsi, …)): fallbacks: 2D layer composed on the CPU (no GPU compositor); world pages through one sampler (GL)
```

| Feature | On OpenGL |
|---|---|
| 2D layer | composed on the CPU (the reference compose) into an `Rgba8UnormSrgb` canvas. The GPU compositor needs a storage texture sampled through an sRGB view of it, which GL cannot reinterpret |
| World pass sampling | GL pairs a texture with one sampler per shader, so the world shader samples its page through one sampler slot, bound to the nearest sampler in the original's sampling mode (`gpu.rs`, `world_shader_source`); `msaa`'s alpha-to-coverage lookup is then filtered by hand from four texel loads |
| `anisotropic` | off when the driver lacks it (wgpu clamps it to 1); Mesa has it |
| `msaa` | the highest count the device has (the renderer probes 8, 4, 2); Mesa has 8 |
| Issue marks | the GL surface cannot be read back, so the marked frame is drawn into a texture, read back and then put on the surface (`Gpu::target`) |

Shots on GL differ from Vulkan's in a few pixels: the 2D screens by at most 1/255 in a handful
of pixels (blending rounding); the 3D screens in 0.01–0.4 % of the pixels, at texel boundaries of
magnified nearest-sampled textures and polygon edges (GL rasterises with Y flipped, so a texel
or edge on the boundary can land on the other side); the movie's bilinear picture by up to 15 %
in 0.06 % of the pixels. Vulkan and DX12 are unchanged by all of this.

## Where the game data is found

The program needs the files of the user's own disc: a directory holding the game's `DATA` and
`BIN`. Two layouts are accepted (`data_dir.rs`):

- an extracted disc as `scripts/extract-disc.sh` writes it: `<dir>/install/Program_Executable_Files/{DATA,BIN}`;
- an installed copy of the original game: `<dir>/{DATA,BIN}`, by default
  `C:\Program Files\TAITO\DENGO3K` (docs/recon.md §6).

The first source that is set wins:

1. the environment variable `SHUPPATSU_DATA_DIR`;
2. `--data <dir>` (or `--data=<dir>`) on the `shuppatsu` command line, before or after a
   subcommand;
3. `data/extracted` or `data` next to the executable;
4. `data_dir = '<dir>'` in `config.toml`; a relative path is relative to the config directory;
5. the data imported from a disc image (`shuppatsu import`, `--disc` or the game data screen;
   `src/import.rs`, docs/re/disc-import.md): `$XDG_DATA_HOME/shuppatsu/game` (default
   `~/.local/share/shuppatsu/game`) or `%LOCALAPPDATA%\shuppatsu\game`.

Without any of them a windowed start shows the game data screen (`src/setup.rs`) instead of
failing; headless runs fail with the error.

If a source from 1, 2 or 4 is set but its directory doesn't fit either layout, that is an error;
the game does not fall through to the next source. If nothing is found, the error names every
option. `shuppatsu-tools` reads only `SHUPPATSU_DATA_DIR` because it is a developer tool.

The dev shell sets `SHUPPATSU_DATA_DIR=./data/extracted` when that directory exists.

## Testing the Windows build under Wine

The dev shell sets `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER` to `scripts/wine-run.sh`. The script
runs the exe under Wine in a **separate prefix**, `./wine/test-prefix`, and never touches the
original game's `./wine/prefix`; `SHUPPATSU_WINE_PREFIX` overrides it. It also maps Unix paths in
`SHUPPATSU_DATA_DIR`, `SHUPPATSU_TRACE_DIR`, `SHUPPATSU_FONT` and `SHUPPATSU_DISPLAY_FONT` to `Z:\…`.

```sh
nix develop
cargo test --target x86_64-pc-windows-gnu --workspace --exclude shuppatsu-hook
scripts/wine-run.sh target/x86_64-pc-windows-gnu/release/shuppatsu.exe \
    play 0 --shot "300:Z:$PWD/out/play0.ppm"
scripts/wine-run.sh target/x86_64-pc-windows-gnu/release/shuppatsu.exe --shot "300:Z:$PWD/out/app.ppm"
```

To exercise the log file and message box the way Explorer would start the game (no inherited
stdio), use `wine start /wait /unix "$PWD/shuppatsu.exe" …` with `WINEPREFIX=$PWD/wine/test-prefix`.

### Verified under Wine 11.19 (wineWow64Packages.unstable), 2026-10

- The full test suite passes for `x86_64-pc-windows-gnu` without shuppatsu-hook: 476 passed, 2 ignored, as on
  Linux; this includes the data tests.
- The headless `play 0 --shot 300:…` and the app's `--shot 300:…` produce PPMs **byte-identical**
  to the Linux build. Data was found through `SHUPPATSU_DATA_DIR` (Z: path), `--data`, and a `data`
  folder next to the unzipped release exe.
- The windowed game opens and presents on the **Vulkan** backend (winevulkan → RADV, FIFO,
  `Bgra8UnormSrgb`). WASAPI audio
  reaches PipeWire (`application.name = shuppatsu.exe`). gilrs starts without errors.
- **DX12 under Wine does not work, and the fallback handles it.** wgpu's DX12 backend
  compiles HLSL with FXC, and naga's sampler heap is a shader-model 5.1 resource array.
  Microsoft's FXC supports that, but Wine's vkd3d-shader replacement for `d3dcompiler_47`
  returns `E5017: not yet implemented`. The probe in `gpu_backend.rs` catches the error and
  the game continues on Vulkan.
- The release exe is a GUI-subsystem exe. Started via `wine start`, it logged to
  `%APPDATA%\shuppatsu\shuppatsu.log`. A bad `--data` path wrote the error there and blocked in the
  message box.
- `shuppatsu-tools.exe check` reports the same result as on Linux (399 files, 0 failed, 0 unknown).

### Needs a real Windows machine

- The **DX12** path itself: rendering, presentation and the HLSL shaders compiled by
  Microsoft's FXC (or DXC, if `dxcompiler.dll` is present).
- The Vulkan path on Windows drivers, and the GL fallback.
- Audio on real WASAPI devices: device changes and exclusive-mode drivers.
- Gamepads and Densha de GO! controllers through Windows.Gaming.Input. Note that WGI only
  delivers input while the window has focus.
- `--console` (AttachConsole/AllocConsole), which Wine's terminal handling doesn't represent
  faithfully.
- High-DPI scaling, 120 Hz presentation and fullscreen/borderless behaviour on Windows.

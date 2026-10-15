# Phase 0 — Recon report

Variant examined: **Densha de GO! 2 Kōsoku-hen 3000-bandai** (Windows, TAITO/Unbalance, 2000).
Confidence tags: `confirmed` / `likely` / `guess`, each with evidence.

Generated artefacts (gitignored, regenerate with the commands below):
`out/recon/inventory-{disc,install}.md`, `out/recon/pe-*.txt`, `out/recon/strings-*.txt`.

```sh
nix develop
scripts/extract-disc.sh data/disc/DENGO3K.mdf # -> data/extracted/{disc,install}
cargo run -p ddg-tools -- inventory data/extracted/install
cargo run -p ddg-tools -- pe data/extracted/install/Program_Executable_Files/BIN/dengo3k.exe
cargo run -p ddg-tools -- strings <file> [--min N] [--jp]
cargo run -p ddg-tools -- manifest data/extracted 3000-bandai
```

## 1. Source image

| fact | value | confidence / evidence |
|---|---|---|
| Format | Alcohol 120% `DENGO3K.mdf` + `DENGO3K.mds` (MDS v1.3), raw 2352-byte sectors | confirmed: 241,032,960 B = 102,480 × 2352; `MEDIA DESCRIPTOR` header |
| Session/tracks | 1 session, **3 Mode 1 data tracks**, no CD-DA | confirmed: MDS track blocks + sector headers |
| Track 1 | LBA 0–101725 (101,726 sectors): the ISO 9660 filesystem, all files | confirmed |
| Track 2 | LBA 101726–102177 (452 sectors): all zero | confirmed |
| Track 3 | LBA 102178–102479 (302 sectors): read only by the disc check (see §5) | confirmed |
| Volume | ISO 9660 + Joliet, label `DENGO3K`, 102,179 sectors (just reaches LBA 102178) | confirmed: PVD at sector 16 |
| Integrity | 0 EDC errors over all 102,480 sectors; no DPM block, no bad-sector list in the MDS | confirmed |

History: the first image supplied (`./DENGO3K.mdf`, no `.mds`) was a truncated copy of this same
file. It is byte-identical up to offset 208,938,752 and zero after that. It is superseded by
`data/disc/DENGO3K.{mdf,mds}`, which came from the user's own backup archive.

`scripts/extract-disc.sh` treats the MDF as one MODE1/2352 track. That's correct for extraction
because all three tracks are Mode 1 and contiguous. It does not preserve the TOC; anything that
needs the TOC (e.g. presenting the disc to Wine) must use the `.mds`.

## 2. Disc layout and inventory

Disc (330 files, of which 295 are the DirectX 6 redistributable):

| path | what | confidence |
|---|---|---|
| `AUTORUN.INF`, `Autorun.exe/.ini/.bmp` | launcher menu: install, readme, dxdiag, DirectX 6 setup | confirmed (file contents) |
| `SETUP.EXE`, `_INST32I.EX_`, `_SETUP.DLL`, `_ISDEL.EXE`, `_sys1.cab`, `_user1.cab`, `setup.ins` … | InstallShield 5 installer | confirmed (cab magic `ISc(`, file set) |
| `DATA1.CAB` | 166 MB InstallShield 5 cab with the whole game (57 files) | confirmed |
| `DRIVER/` | separate InstallShield setup: driver for the official DDG controller | confirmed |
| `DIRECTX6/` | DirectX 6.0/6.1 + DirectX Media redistributable | confirmed |

Installed game (`DATA1.CAB`), 57 files, 223,675,314 B, all extracted (MD5-verified by unshield):

| file(s) | count | bytes | magic | entropy | notes |
|---|---:|---:|---|---|---|
| `BIN/dengo3k.exe` | 1 | 585,728 | `MZ` PE32 | 6.0 | the game |
| `BIN/DengoSetting.exe` | 1 | 126,976 | `MZ` PE32 | 6.0 | settings tool (fog, display) |
| `BIN/Cd2*.dll` | 5 | 96,224 | `MZ` PE32 / NE | — | CD-ROM scan library (see §5) |
| `DATA/*.COM` | 15 | 18,944,008 | `u32 a, u32 a` then `RAIL DATA VER7.0` at 0x20 | 6.1–6.6 | route/track data; one per route segment (AKITA×5, HOKUHO×2, KEIHIN×3, TOUKAI×3, YAMATE×2) |
| `DATA/bgm.rom` | 1 | 110,746,800 | `ROM2`, 18 entries | 7.6 | music (no CD audio; likely compressed/ADPCM — guess) |
| `DATA/effect.rom` | 1 | 6,789,624 | `ROM2`, 407 entries | 6.3 | sound effects (guess from name) |
| `DATA/tbl.rom` | 1 | 219,864 | `ROM2`, 1365 entries | 6.8 | tables (guess from name; candidates for physics/scoring) |
| `DATA/parts{R1..R5,RC,TR1..TR9}.rom` | 15 | 4,086,612 | `ROM2` | 5.5–6.5 | models/parts |
| `DATA/tex{R1..R5,RC,TR1..TR9}.rom` | 15 | 38,026,552 | no magic; leading u16 counts, 8-byte records | 5.2–7.0 | textures |
| `DATA/cg.rom` | 1 | 5,051,558 | no magic; u32 offset table | 6.4 | 2D graphics (guess) |
| `DATA/komachi.dat` | 1 | 39,001,368 | `00 00 01 BA 21…` | — | **MPEG-1 program stream** (pack header, MPEG-1 marker), played via DirectShow `amstream` (confirmed format; usage likely, §4) |

`ROM2` container (likely, checked on tbl/bgm/effect/parts): `"ROM2"`, `u32 count`, then
`count × (u32 offset, u32 size)`; the first offset is always `8 + 8·count` and entries are contiguous.
Full per-file listings with hashes: `out/recon/inventory-*.md`; hashes in
`data-manifest/3000-bandai.toml`.

The `R*` / `TR*` split (5 + 9 + `C`) likely matches route sets (R) and train/cab sets (TR) — guess.

## 3. Executables

| binary | type | linked | toolchain | notes |
|---|---|---|---|---|
| `dengo3k.exe` | PE32 GUI, base 0x400000 | 2000-03-03 | MSVC 6.0 (linker 6.0; Rich: Utc12 C/C++ builds 8168/8447, MASM 6.13, cvtres 5.00), MFC statically linked | not packed: `.text` entropy 6.6, no overlay, standard section names. `.data` vsize 4 MB (large BSS) |
| `DengoSetting.exe` | PE32 GUI | 2000-03-02 | MSVC 6.0 + MFC (static) | dialog app writing registry settings |
| `Cd2ctl32.dll` | PE32 DLL | 1999-04-15 | MSVC (linker 4.20) | exports `_CDRomScanW@4`; picks the 95 or NT implementation |
| `Cd2win95.dll` | PE32 DLL | 1999-04-15 | linker 3.0 | flat thunk to 16-bit `Cd2qt16`/`Cd2win16` |
| `Cd2winnt.dll` | PE32 DLL | 1999-04-15 | linker 4.20 | `CreateFileA` + `DeviceIoControl` (raw device access) |
| `Cd2qt16.dll`, `Cd2win16.dll` | NE 16-bit | — | MS linker 5.6x | `CDROMSCAN16`; error strings "Read Secter", "DISK INFO" |
| `Autorun.exe` | PE32 | 2000-02-15 | Borland C++Builder (VCL) | launcher only |
| `DRIVER/SETUP.EXE` | NE 16-bit | — | InstallShield 5 bootstrap | |

Imports of `dengo3k.exe` (confirmed, `out/recon/pe-dengo3k.txt`):

| DLL | imports | meaning |
|---|---|---|
| `DDRAW.dll` | `DirectDrawCreate`, `DirectDrawEnumerateExA` | DirectDraw; Direct3D obtained via `QueryInterface` (likely; DX6-era Immediate Mode) |
| `DSOUND.dll` | ordinal 1 (`DirectSoundCreate`) | sound |
| `DINPUT.dll` | `DirectInputCreateA` | keyboard/joystick (the DDG controller appears as a DirectInput joystick) |
| `WINMM.dll` | `timeSetEvent`, `timeKillEvent`, `timeGetTime` | **multimedia timer**: likely drives the fixed tick; rate to be found in Phase 1 |
| `ole32.dll` | `CoCreateInstance` … | DirectShow `amstream` for `Komachi.DAT` playback |
| `ADVAPI32.dll` | `RegCreateKeyExA`, `RegQueryValueExA` | reads settings from the registry |
| `KERNEL32.dll` | incl. `GetLogicalDriveStringsA`, `GetDriveTypeA`, `GetVolumeInformationA`, `CreateFileA`, `ReadFile`, `MapViewOfFile` | CD check (§5) and file I/O |
| `CD2CTL32.dll` | `_CDRomScanW@4` | CD check (§5) |
| `IMM32`, `GDI32`, `USER32`, `COMCTL32`, `comdlg32`, `WINSPOOL` | | MFC/window plumbing |

No `QueryPerformanceCounter` import (confirmed). No D3D DLL import, which fits D3D-through-DDraw.

Resources in `dengo3k.exe`: two icons, an icon group, and one MFC string-table block. No dialogs,
menus or bitmaps (confirmed). All UI art is in the data files.

## 4. Notable strings (`dengo3k.exe`, Shift-JIS decoded)

| offset | string | interpretation |
|---|---|---|
| 0x86ed4 | `CD-ROM 『%s』 をドライブにセットしてください。` | "Insert CD-ROM «%s» in the drive"; `%s` is likely `DENGO3K` at 0x86f18 (the volume label) |
| 0x86f04 | `DENGO2_3000_WIN1` | not found on the readable disc; likely a mutex/window-class name (guess) |
| 0x875d0 | `Software\TAITO\電車でGO！２ 高速編 ３０００番台` | registry key (confirmed string; hive to verify in Phase 1) |
| 0x875c8–0x87590 | `Option`, `FOGENABLE`, `FOGTABLESTART`, `FOGTABLEEND`, `FOGTABLEDENSITY` | registry values, also written by DengoSetting |
| 0x8bb9c | `..\bin\dengo3k.ini` | ini file next to the exe (not shipped, so probably created at runtime) |
| 0x87557 | `..\BIN\REPLAY.DAT` | replay save file |
| 0x8bc80… | `CReplayMgr/Rank/Round/Station/Event::m_nNoch, m_nBrake, m_nTime …` | **replay system that records notch/brake input with timestamps**, a ready-made input-script format for the Phase 4 oracle (likely) |
| 0x8baa0 | `vga_640x480`, `vga_800x600` | supported display modes |
| 0x87060 | `デバッグモードでは…High Color (16bit)…VRAM 16MB…Voodoo2では動作しません` | a debug mode exists (16-bit colour, ≥16 MB VRAM, not on Voodoo2) |
| 0x86308… | `SOUND CODE : %03XH %s`, `LAMP..... %s`, `VER %d.%d%s %c` | debug/test-menu output, arcade test-mode heritage |
| 0x85b80… | romanised station names (`OHSAKA`, `SHINJYUKU`, `KOUBE`, …) | station tables in the exe |
| 0x84db8 | `EASY/NORMAL/HARD/HARDEST`, `CONFIGURATION`, `ADJUSTMENT` | option/test menus |
| 0x87134… | `CLSID_AMMultiMediaStream … regsvr32 amstream.dll`, `..\data\Komachi.DAT` | `Komachi.DAT` (an MPEG-1 program stream, confirmed) is played through DirectShow (likely) |

The exe opens data with paths relative to `BIN\` (`..\data\…`), so the working directory must be
`BIN` (confirmed by the strings; to verify at runtime).

## 5. Copy protection assessment

**Finding: the exe checks for its disc at startup, using the bundled `Cd2*` CD-ROM library and the
volume label `DENGO3K`. It is not a commercial scheme: no SafeDisc/SecuROM/LaserLock files or
section names, and the exe is not packed or encrypted. Per CLAUDE.md rule 5 the check was not
analysed and is not bypassed.**

- The **new engine** reads only the extracted data files and never runs or touches the check.
- For the **Wine oracle**, the user's own raw image (with its full TOC) is presented to Wine as a
  CD drive, so the original check runs unmodified. Patching it is off the table.

## 6. Install model

| item | value | confidence / evidence |
|---|---|---|
| Installer | InstallShield 5 (16-bit `SETUP.EXE` bootstrap, `_INST32I.EX_`) | confirmed |
| Installed tree | `<target>\BIN\` (exe, settings tool, Cd2 DLLs), `<target>\DATA\` (all data) | confirmed: cab component paths `Program Executable Files\BIN|DATA\…` |
| Default target | `C:\Program Files\TAITO\DENGO3K` | confirmed: `Readme.txt` (標準では、C:¥Program Files¥TAITO¥DENGO3K); `setup.ins` uses `PROGRAMFILES`, `TARGET_NAME` |
| Shortcuts | game + "環境設定アプリケーション" (DengoSetting) | likely: `SETTING_NAME`, `SETTING_KEY=DengoSetting.exe` |
| Uninstall key | `電車でGO！２ 高速編 ３０００番台` | likely: `UNINST_KEY` |
| Registry (game) | `Software\TAITO\電車でGO！２ 高速編 ３０００番台`, subkey `Option` (fog), display settings | likely: strings + `RegCreateKeyExA` |
| Runtime files | `BIN\dengo3k.ini`, `BIN\replay.dat` written by the game; replays are one file for all routes | `replay.dat`: confirmed (Readme); ini: likely |
| Run-from-CD | nothing; CD needed only for the presence check | likely |
| Requirements | DirectX 6+ (6.1 bundled and recommended; installer can run `DIRECTX6\DXSETUP.EXE`), about 120 MB virtual memory (Readme); DirectShow `amstream.dll` for the video | confirmed (disc contents, strings) |
| Controller | official DDG controller on a **15-pin game port**, Win9x VxD `DGOC44.VXD` (+ `DGOC44.dll`, INF registers it as a DirectInput OEM joystick with 2 axes + buttons); readme: game port only, USB not supported | confirmed: `DRIVER/DATA1.CAB` |
| Product version | 1.00 | confirmed: `DATA.TAG`, `_user1.cab` |
| Credits | published by TAITO, sold by Unbalance, with JR East Planning (ジェイアール東日本企画); © TAITO 1996, 2000 | confirmed: Readme |

The driver package also contains `dengo.ini` / `dengo2.ini`: per-GPU (PCI `vendor.device`)
viewport fixes for **Densha de GO!** and **Densha de GO! 2**. `dgcsave.cfg` lists `Dengo.exe`,
`Dengo2.exe`, `Dengo3k.exe`. These are the other Windows titles sharing the driver.

## 7. Variant

**3000-bandai: confirmed.** Volume label `DENGO3K`, exe `dengo3k.exe`, product name
`電車でGO！２ 高速編 ３０００番台` (`DATA.TAG`, `Autorun.ini`, registry key string), version 1.00.

For the original Kōsoku-hen the detection key would most likely be `dengo2.exe` (from
`dgcsave.cfg` / `dengo2.ini`). That's a guess; no original disc is available to confirm it.
Hash manifest: `data-manifest/3000-bandai.toml`: image SHA-256 + track layout, and 399 extracted
files (disc, installed game, driver, installer support), hashes and sizes only.

## 8. Open questions

1. The disc check: deliberately not analysed (§5).
2. Tick rate: `timeSetEvent` interval and the sim/render split (Phase 1).
3. Direct3D version/interfaces obtained through DirectDraw (Phase 1).
4. `bgm.rom` audio encoding; `tex*.rom` and `cg.rom` layouts; `.COM` header fields (Phase 2).
5. Registry hive (HKLM vs HKCU) and the full value list; `dengo3k.ini` contents (Phase 1, or by running under Wine).
6. Whether a newer patch or exe version exists for 3000-bandai: `VER %d.%d%s %c` is built at
   runtime, and this disc says 1.00.
7. Whether the 16-bit `Cd2` path runs under Wine on modern Linux (affects the oracle setup).
8. Purpose of the empty 452-sector track 2: plain spacer or part of the check (guess: spacer).

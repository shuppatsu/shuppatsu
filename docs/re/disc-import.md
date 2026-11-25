# Importing the game data from a disc image

How the engine reads the user's disc image or disc folder without external tools, replacing
`bchunk`, `7z` and `unshield` in `scripts/extract-disc.sh`. Code: `shuppatsu_formats::disc`,
`::installshield` and `::inflate`.

## What is needed from the disc

Only the installed tree: the 57 files the InstallShield cabinet `data1.cab` puts under
`Program Executable Files\{BIN,DATA}` (docs/recon.md §6). Nothing else on the disc is read by the
engine, and the CD check's track is never looked at. **confirmed**: `data-manifest/3000-bandai.toml`
lists exactly those 57 files under `install/Program_Executable_Files/`, and the engine runs from
that tree alone.

## Disc images

| Image | Data track | Detection |
|---|---|---|
| `.iso` | cooked 2048-byte sectors | `CD001` at byte 16 × 2048 + 1 |
| `.bin` + `.cue` | raw 2352 (Mode 1, data at +16; Mode 2 Form 1, data at +24) | first data `TRACK` of the cue, then the sync pattern and mode byte at sector 16 |
| Alcohol `.mdf` (+ `.mds`) | raw 2352, or 2448 with 96 subchannel bytes | as `.bin`; the `.mds` is not needed because the data track starts at byte 0 |

The user's image is an MDF/MDS pair with three Mode 1 tracks of raw 2352-byte sectors
(docs/recon.md §1). Track 1 holds the ISO 9660 filesystem (volume `DENGO3K`, 8.3 names, no
Joliet needed). **confirmed**: `crates/shuppatsu-formats/tests/data_disc.rs` with
`SHUPPATSU_CUE=data/disc/DENGO3K.mdf`.

## InstallShield 5 cabinet (`data1.cab`)

Signature `ISc(`, version word `0x01000004`, single volume, header inside the cabinet (no
`data1.hdr`). The layout is in the module docs of `installshield.rs`. **confirmed**: all 57 files
extract byte-identical to `unshield` 1.6.2 (the manifest's hashes were made from unshield's
output) in 1.4 s (release build).

- Descriptor 0 has flag 8 (invalid) and is skipped. The others have flag 4 (compressed).
- Compressed data is a sequence of chunks: a u16 length, then an independent raw DEFLATE stream
  of that length. No chunk is split across files.
- Directory table: `""`, `BIN`, `DATA`. The only file group is "Program Executable Files".

## Not supported, on purpose

- Archives (`.zip`, `.rar`, self-extracting `.exe`). The user extracts them first. RAR would need
  a dependency outside the allowlist (decision 2026-11-25).
- Multi-volume, split or obfuscated cabinets, and InstallShield 6 or later. The disc doesn't use
  them.

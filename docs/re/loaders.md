# File I/O and loaders (dengo3k.exe, 3000-bandai)

Scope: which function opens which file, when it does so, how each container is parsed, and the
decompressors. All addresses are VAs in `dengo3k.exe` (image base 0x400000; file offset = VA - 0x400000).
Each claim carries a confidence tag: **confirmed** means seen in code *and* checked against the data files,
**likely** means read from code only, and **guess** means an inference.
The data experiments were read-only Perl scripts over `data/extracted/.../DATA/`. Nothing derived from
the game data is committed here beyond structure descriptions.

The CD check (`Cd2*`, FUN_00425bc0, FUN_00425bf0) was not analysed. That was off-limits.

## 1. Loader function table

### Generic I/O helpers (CRT / MFC)

| Address | Suggested name | Role | Conf. |
|---|---|---|---|
| 0x438c1f | `crt_fopen` | fopen; mode strings `"rb"` @0x48c1b4, `"wb"` @0x48c494 | likely |
| 0x438ad7 / 0x438d92 / 0x438a5a | `crt_fread` / `crt_fwrite` / `crt_fclose` | | likely |
| 0x444da0 | `crt_stat` | `_stat`, used to get the file size (st_size read at local+0x14) | likely |
| 0x43f0e0, 0x442333 | `crt_fsopen`, `crt_sopen` | CRT open; 0x442333 is the CreateFileA call site | likely |
| 0x43ef07 | `crt_read` | `_read` (ReadFile with text-mode CR/LF handling) | likely |
| 0x4465ce / 0x4466eb | `CFile::Open` / `CFile::Read` | MFC, used only by the replay CArchive | likely |

### Game loaders

| Address | Suggested name | File(s) | When | Role | Conf. |
|---|---|---|---|---|---|
| 0x42fe40 | `data_load_alloc(name)` | `..\data\<name>` | — | Builds the path from prefix `..\data\` (0x48c1b8), stats the file, mallocs `st_size` bytes, reads the whole file, returns the buffer | confirmed |
| 0x42fd90 | `data_load_into(name, dst[, max])` | `..\data\<name>` | — | Same, but into a caller buffer, and returns the size. The caller passes a max size; the function never reads it (no bounds check) | likely |
| 0x42fef0 | `rom2_relocate(buf)` | any ROM2 | — | For i in 0..count: entry[i].offset += buf. Offsets turn into pointers in place | confirmed |
| 0x430ec0 | `ui_gfx_load` | `tbl.rom`, `cg.rom` | startup (from GAME_INIT 0x40e910) | Loads both. Relocates tbl (ROM2) and builds the sprite size table. Relocates cg's offset table (its own format) | confirmed |
| 0x431e70 | `ui_gfx_free` | | exit | Frees both buffers | likely |
| 0x430fb0 | `sprite_layers_draw` | (cg tiles) | per frame | Draws sprite slots from tbl+cg, **decoding cg tiles on the fly** (§4) | confirmed |
| 0x430090 | `sound_thread_start` | | startup (GAME_INIT) | Starts the thread 0x430230 with CreateThread | confirmed |
| 0x430230 | `sound_thread_main` | `effect.rom`, `..\data\bgm.rom` | startup (in the sound thread) | Loads effect.rom whole and relocates it. **Memory-maps** bgm.rom copy-on-write (CreateFileMapping PAGE_WRITECOPY, MapViewOfFile FILE_MAP_COPY) and relocates the mapping. Then DirectSoundCreate (DSOUND ordinal 1), 10 streaming buffers, and a message loop | confirmed |
| 0x430a40 | `sound_stream_fill(voice, half)` | | per notify | Copies the next half-buffer of sample data (§6) | confirmed |
| 0x430e50 | `sound_data_free` | | exit | | likely |
| 0x432140 | `area_assets_load(area)` | parts/tex RC + R*area* | per stage | No-op when area==0. Otherwise resets the pools (0x432170), then loads index 0 (RC, common) and then `area` | confirmed |
| 0x432170 | `model_pools_reset` | | per stage | Clears the part-id table (0x817c7c, 0x331f entries), the tex-rect table (0x825144, 3000×6 B) and the texture surfaces (0x824920, 512 entries); releases the surfaces; resets the pool cursors | confirmed |
| 0x432200 | `parts_tex_load(idx, isTrain)` | `tex*.rom`, `parts*.rom` | per stage / train change | Loads tex*.rom (malloc), creates DirectDraw texture surfaces, frees the file, then loads parts*.rom into the static pool and registers part IDs (§3, §5) | confirmed |
| 0x4327c0 / 0x4327d0 | `train_assets_load(tr)` | `partsTR*`, `texTR*` | per stage / train switch | `parts_tex_load(tr, 1)`, cached per TR index in 0x825120[tr] | confirmed |
| 0x40ed90 | `stage_assets_load(stage)` | | stage start | Area from 0x463c18[stage], then up to 2 TR sets from 0x463c74[0x463d14[stage]] | confirmed |
| 0x40ee50 | `stage_train_switch(stage, variant)` | | mid-game | Reloads the TR set only if it changed | likely |
| 0x432840 | `part_get_unpacked(id)` | (parts pool) | on demand | Looks up a part, **LZ-decompresses** it when flagged (0x425af0), expands it into the runtime model heap 0x796480..0x816480 and caches it | confirmed (decompression) / likely (rest) |
| 0x425af0 | `lz_unpack_multipass(src, dst)` | | on demand | Multi-pass byte LZ (§4) | confirmed |
| 0x4327e0 | `unpacked_parts_flush` | | when heap full / route load | Points 0x817c7c entries that reference the runtime heap back at the packed source (saved in the first word); resets both heap cursors | likely |
| 0x435320 | `route_com_load(com)` | `<route>.com` | segment start | Cached by index (0x48c4a4). Reads into the static buffer 0x492540 (max 0x1c3f40 B), then relocates | confirmed |
| 0x435380 | `com_relocate(buf)` | | | Pointer fix-ups for the .COM (§7) | confirmed |
| 0x41cb00 | `route_segment_start(com, area, station)` | | segment start | Calls route_com_load, then copies header pointers into globals | likely |
| 0x408010 / 0x408080 | `stage_select_segment(stage, station)` | | stage start / continue | Picks the .COM from the stage's segment list by station index (§8) | confirmed (table) |
| 0x429bf0 | `WinMain` | registry, ini, REPLAY.DAT | process start/exit | Reads the registry, calls the options parser, loads the replay, runs GAME_INIT and the main loop; at exit saves eeprom and replay | confirmed |
| 0x42b700 | `options_parse(optstr)` | `..\bin\dengo3k.ini` | process start | Tokenises the option string for `-flag` / `/flag`, then reads `[default]` ini keys (§9) | confirmed |
| 0x429bd0 | `xor55(buf, n)` | | | XORs each byte with 0x55 (de-obfuscates the registry `Option` value) | confirmed |
| 0x42e180 / 0x42e2c0 | `replay_load(path)` / `replay_save(path, force)` | `..\BIN\REPLAY.DAT` | start / exit | MFC CArchive (§10) | likely |
| 0x434ed0 / 0x434f50 | `eeprom_load` / `eeprom_save` | `eeprom.dat` (cwd = BIN) | end of GAME_INIT / exit | Raw 0x35a-byte dump of 0x856020 | confirmed |
| 0x433d60 / 0x434a00 | `input_init` / `joystick_calibrate` | `joystick.dat` (cwd = BIN) | startup (via 0x4332e0) / calibration screen | Reads/writes 0x18 B @0x48c434 + 0x2c B @0x48c44c | likely |
| 0x429110 | `movie_play_komachi` | `..\data\Komachi.DAT` | attract/demo (0x404e10), menu case 5 (0x42a1c0) | DirectShow IAMMultiMediaStream playback | likely |

Not shipped but created by the game at runtime: `BIN\eeprom.dat`, `BIN\joystick.dat`, `BIN\REPLAY.DAT`
(and `dengo3k.ini` only if the user writes it).

## 2. ROM2 container (tbl, effect, bgm, parts*)

`"ROM2"` (u32 0x324d4f52) · u32 `count` · `count` × { u32 `offset`, u32 `size` }. Offsets are from the
start of the file. **confirmed** (0x42fef0 relocates `entry[i].offset += base` for i < count; the data agrees).
Entries are reached by index: `ptr = *(base + 8 + i*8)` after relocation. No names and no compression at the
container level.

## 3. tex*.rom (no magic): texture pages + texture-rectangle records

Parsed by 0x432200. **confirmed**: the walk ends exactly at EOF for texRC, texR1, texTR1 and texTR9.

```
u16 nRect, u16 nPage
nRect × 8 B:  u16 rectId, u16 page, u8 u, u8 v, u8 w, u8 h
nPage × :     u16 width, u16 height, width*height × u16 pixel   (row-major, no padding)
```
- Pixels are uploaded as **RGB565** (DDPIXELFORMAT RGB, 16 bpp, masks F800/07E0/001F) by row memcpy.
  **likely**. No colour key or alpha is set at load time.
- `rectId` is a global id (table 0x825144, 3000 entries × 6 B). Stored there: page + running page base
  (0x8248f8), u, v, w, h. The table is written unconditionally, so a later file wins for the same rectId
  (the opposite of parts, where the first file wins). **likely**
- In `part_get_unpacked` the UVs are rebuilt as `(rect.w+1)*uv + rect.u` but `rect.h*uv + rect.v` (V uses `h`,
  not `h+1`), scaled by the float at 0x47c95c (0x3b800000 = 1/256). The rect fields are in **1/256-of-page units**,
  not texels; w and h are stored as size−1, so 255 spans a full page of any pixel size. **confirmed** (corrected
  in Phase 2, see models.md; the first draft of this note said `h+1` and texels)
- **Orientation:** pages are stored bottom-up. The loader copies row *i* to surface row *i* (no flip), and model UVs
  address that layout. A renderer keeps the stored order; only viewers should flip. **confirmed** (0x432200, visual
  check of exported pages)
- Surface size: the smallest power of two ≥ the image, starting from the device minimum (0x816758/0x816610).
  The device maximum (0x816760/0x81675c) caps it, as do the ini flags (`texmax128`: 128, but 256 for images
  ≥1024/512; `texmax256`: 256). Square-only devices get w=h. If the surface size differs from the image size, the
  image goes into an exact-size system surface and is Blt-scaled into the texture. **likely**
- Up to 512 surfaces in total (0x824920..0x825120). The page base advances by nPage for each file loaded.

## 4. Decompression routines

There is no general-purpose file decompressor. Every container is stored raw except for two codecs.
**confirmed** (all loaders above read files straight into memory; a grep for LZSS ring-buffer constants found
nothing).

### 4a. Multi-pass byte LZ: `lz_unpack_multipass` @0x425af0 (used for parts)

Packed block layout (pointer P = part entry + 2, see §5):
```
P+0  u32  unpacked size
P+4  u8   0x73 (constant in all 13087 parts)
P+5  u8   passes   (0 = stored raw at P+8; 1..3 = compressed)
P+6  u16  0
P+8  pass-1 input
```
Each pass reads `i32 outLen` followed by a token stream and writes exactly `outLen` bytes:
- token `b < 0x80`: back-reference. `len = b+1`, `dist = next_byte + 1`. Copy byte by byte from `out[pos-dist]`.
  Overlap is allowed, so this works as RLE.
- token `b >= 0x80`: literal run. Copy the next `b - 0x7f` (=(b&0x7f)+1) bytes.
- Repeat until `pos >= outLen`.

With `passes > 1`, the output of one pass is the input of the next. Each intermediate output starts with
its own `i32 outLen`. The routine ping-pongs between a stack buffer (_chkstk, about 16 KB) and the destination,
choosing by pass-count parity so that the last pass lands in the destination.
**confirmed**: a Perl reimplementation unpacked all 9877 compressed parts in the 15 parts*.rom files to
exactly the size in P+0. The 3210 raw parts have `P+0 == entry size − 10`. Largest unpacked part: 13689 B.

### 4b. cg.rom 16×16 tile codec (decoded inside `sprite_layers_draw` @0x430fb0)

Tile = 1 type byte + stream, giving 256 palette indices in row-major 16×16 order (index table 0x816d84 is built
in 0x430ec0 as `row*pitch + col`). Index 0 is transparent.
- **type 1** (RLE): `b < 0x80`: `v = next byte`, run of `b+1` pixels of index v (v=0 skips them).
  `b >= 0x80`: `(b&0x7f)+1` literal index bytes.
- **type 2** (LZ, same token grammar as 4a, but in pixels): `b < 0x80`: copy `b+1` pixels from
  `pos − (next_byte+1)`. `b >= 0x80`: `(b&0x7f)+1` literal index bytes.
**confirmed**: all 44306 tiles decode to exactly 256 pixels within their slot. Types: 8496 × 1, 35810 × 2.

## 5. parts*.rom (ROM2 of model parts)

- Entry = `u16 partId` + packed block (§4a). `parts_tex_load` stores `0x817c7c[partId] = entry+2` and the
  **first loaded wins**. RC (common) loads before R*n*, and TR sets load later. **confirmed** (code). The ID range is 0..13086,
  which matches the table size 0x331f. Per-file ranges were checked: R* 0..11761, TR* 11762..13086.
- The parts files are read into one static pool 0x656480 of 0x140000 B. The cursor 0x824900 is 4-aligned after
  each file and reset per area. **likely**
- `part_get_unpacked` converts a packed part into a runtime mesh. After unpacking, the stream is
  `u16 nGroups`; per group: `u16 rectId`, `u16 nVerts`, nVerts × 5 × i16 (x, y, z, u, v; scaled by 1/256),
  `u16 nIdx`, then nIdx indices (u8, or u16 when nVerts ≥ 255). **guess**: structure read from the code, not verified
  against data. Belongs to the formats phase.
- Special case: for COM 6, 8 or 9 (akita1a, akita1c, akita2a), part 0x1153 is replaced by a built-in part in the
  exe at 0x48c39e. **likely**

Area index (R*n*) to route (**confirmed** by part-id overlap between each .COM's placements and the parts files, and
by the stage tables in §8): R1 = Yamanote, R2 = Akita (Komachi), R3 = Tōkaidō, R4 = Hokuhoku, R5 = Keihin.
RC = common to all areas.

## 6. effect.rom and bgm.rom (audio)

**effect.rom** (ROM2, 407 entries). Each entry: 16-byte header + **unsigned 8-bit mono PCM**. **confirmed** (data + 0x430a40):
```
u32 sampleRate (9000..44100; 295 entries are 10000)   u32 dataLen (= entry size − 16)
u32 loopStart   u32 loopEnd (0 = one-shot)
```
- The sound id *n* (1..406) maps to effect entry *n−1* (`*(rom + n*8)`). **confirmed**
- Playback: the voice plays from 0 to loopEnd and wraps to loopStart while held. "Release" (msg 0x465 → 0x430870)
  jumps to loopEnd and plays on to dataLen. With loopEnd==0 the sound plays to the end. Silence fill is 0x80. **confirmed**
- Frequency = `rate * (256 − pitch) / 256` (0x430e00); id 0x31 inverts the pitch. Volume 0..127 maps to
  DirectSound hundredths of a dB, clamped to −10000..0 (0x430da0; the float maths was lost in the decompile). **likely**

**bgm.rom** (ROM2, 18 entries): **raw 16-bit stereo 44100 Hz PCM**, no per-entry header. **likely**
(buffer format at 0x430230; sizes are multiples of 4; entropy 7.6).
- Music id *n* ≥ 407 maps to bgm entry *n−407* (`*(map − 0xcb0 + n*8)`). **confirmed** (code)
- Only id 0x19e (414 → entry 7) loops (whole entry). Other tracks play once, then two silent half-buffers, then stop.
  **confirmed** (code)

Voices: 10 DirectSound buffers (array 0x8169b8, stride 32). Voices 0–7 are SFX: buffers of 0x2000 B, created as
22050 Hz/8-bit/mono with the frequency changed per sound. Voices 8–9 are BGM: 0x10000 B, 44100/16/stereo. Flags
0x1a8 = LOCSOFTWARE|CTRLFREQUENCY|CTRLVOLUME|CTRLPOSITIONNOTIFY. Each buffer has two notify events (half
buffers); the thread waits on 20 events and refills the opposite half. Thread messages: 0x464 play(id),
0x465 release(id), 0x466 stop all, 0x467 pitch(id,p), 0x468 volume(id,v), 0x469 pause/resume all; WM_QUIT ends
the thread. Senders: 0x430100 (play) and 0x4301b0 (volume) via PostThreadMessageA. **confirmed** (code)

## 7. Route `.COM` files ("RAIL DATA VER7.0")

Header: 8 × u32 file-relative offsets, then the 16-byte ASCII `RAIL DATA VER7.0` at 0x20. Data starts at 0x30.
`com_relocate` (0x435380) turns every offset into a pointer. **confirmed** (layout + code). Field roles come from
`route_segment_start` 0x41cb00 and from the data:

| # | Global | Content | Conf. |
|---|---|---|---|
| h0 | — | relocated only. Always equals h1 in all 15 files | confirmed (data) |
| h1 | 0x85010c | 8-byte records `{u16 type, u16 flag, u32 offset (0 = none)}`, terminated by type 0xFFFF | confirmed (layout) / guess (meaning) |
| h2 | 0x8500b4 | object placements, 20 B each: `u32 partId, f32 x, f32 y, f32 z, f32 yaw` (yaw values such as π/2 and π) | likely |
| h3 | 0x850210 | **track segment table** at 0x30, 24 B each: `{ptr start, ptr end, ptr linkA, ptr linkB, u32, u32}`, terminated by start == −1. start/end delimit a polyline of 12-byte points (f32 x, f32 z?, u32/f32 attr). Segment 0 is the main line (6899 points in KEIHIN1A) | likely |
| h4 | 0x8501ec | array of pointers, **one per 4 main-line points** (count = first non-zero entry's target − array start, which gives 1724 for KEIHIN1A). Each entry is a list of 20-byte records `{i16 flag (<0 = last), u16, u32, ptr, ptr, f32}` | confirmed (count) / guess (meaning) |
| h5 | 0x850218 | pairs `{ptr to main-line point, u32 point index}`, terminated by ptr 0. These are the station stop points (KEIHIN1A has 5) | likely |
| h6 | 0x8501f8 | u16 per main-line point, increasing: indices into h7 | guess |
| h7 | 0x850144 | u16 lists separated by 0, probably visible-part lists | guess |

The engine starts the train at 12 bytes (1 point) after segment 0's start; `end − 0xb4` (15 points before
the end) is kept as a limit. **likely** (0x41cb00). The buffer is static, so only one .COM is resident.
The cache key is the COM index (0x48c4a4). The .COM files are stored raw (no compression).

COM index → file (table 0x47ca70; index 0 = none; filenames are lowercase in the exe and uppercase on disc,
so the reimplementation must match case-insensitively): 1 keihin1a, 2 keihin1b, 3 keihin1c, 4 yamate1a,
5 yamate1b, 6 akita1a, 7 akita1b, 8 akita1c, 9 akita2a, 10 akita2b, 11 hokuho1a, 12 hokuho2a, 13 toukai2a,
14 toukai1a, 15 toukai1b. **confirmed**

## 8. Stage → area / train / route-segment tables (exe .rdata, read at runtime)

- `0x463c18[stage]` (u32): area R-index used by `area_assets_load`; `area−1` is passed to 0x41cb00 to select
  per-area palettes and the tables at 0x486170/0x486eb0. **confirmed**
- `0x463d14[stage]` → row of `0x463c74` (16 B, 4 × i32 TR indices, −1 = none). TR k loads `partsTR(k+1)`/`texTR(k+1)`.
  Stage 10 adds 0x1000 to the cached id (special case). **likely**
- `0x485610[stage]` → segment list `{i32 comIndex, i32 firstStation}…`, terminated by −1. `stage_select_segment`
  picks the last segment whose firstStation ≤ the requested station and passes `station − firstStation`.
  **confirmed** (decoded table):

| stage | area | segments (com@firstStation) |
|---|---|---|
| 0–2, 17–19 | R2 Akita | 0–2: akita1a@0, akita1b@8, akita1c@24; 17–19: akita1b@0, akita1c@16 |
| 3, 4 | R2 | akita2a / akita2b |
| 5–6, 7–9 | R4 Hokuhoku | hokuho1a / hokuho2a |
| 10, 11 | R5 Keihin | 10: keihin1a@0, 1b@4, 1c@8; 11: keihin1c@0 |
| 12–13, 14–15 | R3 Tōkaidō | toukai2a@0 + toukai1b@2 / toukai1a@0 + toukai1b@2 |
| 16 | R1 Yamanote | yamate1a@0, yamate1b@5 |

Entries 20–23 of 0x485610 point at 0x459750, which does not decode as a segment list. They are probably unused or
a different mode. Open question. The romanised station-name lists (pointer arrays around 0x4858f0..0x485b2c,
with strings at 0x485b80..0x485fe8) have a per-stage pointer table at 0x485b30. No direct code xref was found,
so the consumer is still open. **guess**

## 9. tbl.rom + cg.rom: 2D sprite graphics (not physics or scoring)

**tbl.rom** (ROM2, 1365 entries) holds **sprite tile maps** for the 2D UI/HUD: `u8 wTiles, u8 hTiles,
wTiles*hTiles × u16 cgTileId` (row-major, 0 = empty). **confirmed**: every entry has size == 2+2·w·h, and the
max tile id is 44305 < 44306. At load, 0x430ec0 fills a 16-byte-stride table at 0x850aa8 (1365 rows) with
`w*16, h*16` (pixel size; used for centring in 0x42b930). **confirmed**

**cg.rom** (own format): `u32 count (44306)`, `count × u32 offset` (relocated in place), then the tiles (§4b).
**confirmed**

Drawing (0x430fb0): there are 207 sprite slots (0x816f88, 16 B each): `i16 y, i16 x, i8 srcCol, i8 srcRow,
i8 wOverride, i8 hOverride, u8 palette, u8 layer(0..3), i16 tblIndex (−1 = free)`, set by 0x4316e0/0x431730/… Layers
are drawn 3 → 0. Positions are on a 640×480 canvas, offset by (screenW−640)/2 and (screenH−480)/2 (screen sizes at
0x48c1e0 / 0x48c1f0: 512/640/800/1024 × 384/480/600/768). Palettes: 128 × 256 u32 at 0x829d5c, filled from **exe
.data** blobs by 0x436720/0x436770 (e.g. 0x4636c0, 0x488290 … in GAME_INIT, and per area in 0x41cb00 / 0x40eed0).
The u32 colour layout as converted to 565/555 is byte0 = B, byte1 = R, byte2 = G. A computed colour of 0 becomes
1, so black is never transparent. **likely**

There is no evidence that tbl.rom holds physics or scoring tables. Those must come from the exe or the .COM files.

## 10. Configuration and save files

**Registry** (0x429bf0): `HKLM\Software\TAITO\電車でGO！２ 高速編 ３０００番台` (Shift-JIS at 0x4875d0). It is
opened with RegCreateKeyEx. Values: `Option` (REG_BINARY, 128 B, each byte XOR 0x55, which gives an option string
in command-line syntax), and `FOGENABLE` (atoi), `FOGTABLESTART`, `FOGTABLEEND`, `FOGTABLEDENSITY` (atof, as
strings) → 0x816830, 0x48754c, 0x487550, 0x487554. **confirmed** (code)

**Options** (0x42b700): the `Option` string is tokenised on `" \t\r\n"` (0x47c800). Each `-name` or `/name`
matching the table at 0x47c7a0 sets bit *i* of 0x816840. Then for every name, `GetPrivateProfileIntA("default",
name, 0, "..\bin\dengo3k.ini")`, and a non-zero value also sets the bit. So the ini is
`[default]` + `name=1` lines. **confirmed**

| bit | key | bit | key | bit | key |
|---|---|---|---|---|---|
| 0 | hakutaka | 8 | t | 16 | texmax256 (tex ≤256) |
| 1 | extratime | 9 | nosound | 17 | turbo |
| 2 | bonusgame | 10 | secondryv | 18 | bank |
| 3 | goodending | 11 | squareonly | 19 | alwayswarp |
| 4 | badending | 12 | modelviewer | 20 | texmax128 (tex ≤128) |
| 5 | shinkansen | 13 | nocdcheck (CD-check related; not analysed) | 21 | vga_640x480 → res mode 1 |
| 6 | alwaystop | 14 | debug (windowed: clears fullscreen 0x816d80) | 22 | vga_800x600 → mode 2 |
| 7 | inisection | 15 | f | 23 | vga_1024x768 → mode 3 |

Bit meanings beyond the texture, resolution and debug bits are only names. Their effects belong to other notes.

**REPLAY.DAT** (`..\BIN\REPLAY.DAT`, 0x487558): loaded in WinMain before GAME_INIT (0x42e180) and saved at exit
(0x42e2c0, only when dirty or forced). MFC CFile + CArchive (buffer 0x1000). **likely**
- Line 1: text `RPLY1.3\r\n` (the version CString at 0x816988 is set from 0x48bc60 by 0x42bcb0). The load
  aborts unless the line matches. On failure it shows the message boxes at 0x48c148 / 0x48c17c.
- Binary mode is chosen when the file extension is `DAT` (string 0x48be54). Any other extension writes a text dump
  with the `CReplay*::m_*` format strings.
- Nesting is CReplayMgr → Rank → Round → Station → Event. Each list is prefixed by an MFC `WriteCount`
  (u16, or 0xFFFF followed by u32).
  - Mgr: `u32 bReady`, count, ranks. Rank: `u32 index, u32 bReady`, count, rounds. Round: `u32 index, u32 bReady`,
    count, stations. Station: `u32 index, u32 bReady, u32 counter`, count, events.
  - Event (31 B): `u8 bReady, f32 dist, i32 time, i32 counter, f32 speed, i8 notch, i8 brake, i32 inPush,
    i32 inPull, i32 inBuf`. `time` is in 1/30 s ticks: the text dump formats h:m:s:f with 108000/1800/30. That is
    evidence for a **30 Hz** game tick. **likely**

**eeprom.dat** (cwd-relative, i.e. `BIN\`): a raw 858-byte (0x35a) image of 0x856020..0x85637a. Read at the end of
GAME_INIT. Afterwards `0x829d10 = u16@0x856026 >> 14` and 0x41e520 runs. Written at exit. It holds settings and
records ("arcade EEPROM"). The layout is not yet mapped. **confirmed** (I/O) / guess (content)

**joystick.dat**: 0x18 + 0x2c bytes of controller calibration. Read by input init, written by the calibration
screen. **likely**

**Komachi.DAT**: an MPEG-1 PS played through DirectShow (0x429110 → 0x428db0 open file). It is not parsed by the game
itself. **likely**

## 11. Load timeline

1. WinMain 0x429bf0: registry → options/ini → window → REPLAY.DAT load.
2. GAME_INIT 0x40e910: input init (joystick.dat) → palettes from exe → tbl.rom + cg.rom (0x430ec0) → sound thread
   (effect.rom read, bgm.rom mapped) → eeprom.dat.
3. Stage start (0x4080d0 → 0x40ed90): pools reset; partsRC/texRC + partsR*n*/texR*n*; partsTR/texTR for the train(s).
4. Segment start (0x408010/0x408080 → 0x41cb00 → 0x435320): the .COM for the station range. Parts are unpacked lazily
   through 0x432840 while rendering.
5. Exit: sound thread stop, eeprom.dat save, sound and UI buffers freed, REPLAY.DAT save.

## Open questions
- Exact meaning of the .COM h1/h4/h6/h7 records and the third point word (0x20000000 on the first point). This is
  formats-phase work.
- Stage entries 20–23 and the consumer of the station-name tables.
- eeprom.dat layout; the effect of ini bits 0–12, 15, 17–19.
- Volume curve constants in 0x430da0 (the float maths was lost in the decompile; read the disassembly).

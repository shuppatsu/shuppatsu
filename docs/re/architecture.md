# dengo3k.exe — architecture (Phase 1 code map)

Overview of how the original *Densha de GO! 2 Kōsoku-hen 3000-bandai* (`dengo3k.exe`, 2000-03-03,
MSVC 6 + static MFC, DirectX 6) is put together, and what that means for the reimplementation.
Each subsystem has its own note with function tables and evidence:

| Note | Covers |
|---|---|
| [main-loop.md](main-loop.md) | WinMain, task scheduler, timing, game flow, pause, replay |
| [loaders.md](loaders.md) | every file loader and container/compression format, config and save files |
| [renderer.md](renderer.md) | DirectDraw 4 / Direct3D 3 setup, projection, viewport, fog, draw calls, textures, 2D layers |
| [input.md](input.md) | DirectInput, the 7 control modes, notch decoding, calibration, button words |
| [audio-text.md](audio-text.md) | sound thread, sample formats, sequences/announcements, where text comes from |

Confidence tags: **confirmed** = read from code/bytes, **likely** = strong inference, **guess** = needs
an experiment. Addresses are VAs (image base 0x400000; file offset = VA − 0x400000 for this exe).
The CD check (`0x425ca0` → `0x425bf0`/`0x425bc0`, Cd2 `CDRomScan`) is copy protection and is
intentionally not analysed (CLAUDE.md rule 5).

Method: Ghidra 12.1 headless analysis (`scripts/ghidra-analyze.sh`, output in gitignored `re/`), plus
`objdump` for code Ghidra left as labels; key constants re-checked against the exe bytes; runtime
behaviour checked under Wine 11.19 where noted.

## 1. Big picture

```
WinMain (0x429bf0)                         plain Win32, not CWinApp::Run
 ├─ single instance, CD check, DX6 check, registry options/fog, window "DENGO3K"
 ├─ load ..\BIN\REPLAY.DAT
 ├─ Game_Init (0x40e910)                    load data, start sound thread, create render task
 └─ Game_MainFlow (0x419510), never returns
      attract → title menu → mode/route select → Stage_Run (0x4080d0) → …
      every wait is `while (flag) Task_Tick();`

Task_Tick (0x4253c0) = ONE 33 ms TICK
 ├─ update all tasks (9 intrusive lists @0x84c180; each task = tiny state machine via its update fn)
 │    list 6 holds the render task → FramePacer (0x436fd0) decides draw/skip → draw callbacks
 ├─ global tick counter ++ (0x84c494)
 ├─ pump ≤1 window message
 ├─ poll input (0x4333d0) + replay record/playback (0x407ac0)
 └─ soft reset → longjmp to title

Sound thread (0x430230): DirectSound streaming, commands via PostThreadMessage 0x464–0x469
```

- **Single-threaded simulation**, cooperative tasks, blocking flow code: the arcade design (confirmed).
- The per-tick **train simulation** is the driving loop in `Stage_Run` at 0x4084f5, which calls
  `0x407c10` (track/signal lookup), `0x40dad0` (distance to stop), then `0x409fc0`, `0x40acd0`,
  `0x40b4a0`, `0x40b710`, `0x40b780`, `0x40c010`, `0x40c4e0` (physics/scoring, not yet mapped), then
  `Task_Tick` (confirmed call site). That's where Phase 4 starts.

## 2. Timing (confirmed unless noted)

- A periodic multimedia timer, `timeSetEvent(33, 33, event, 0, PERIODIC|EVENT_SET)` (0x436f30),
  pulses an auto-reset event every **33 ms (30.30 Hz)**.
- **Fixed step:** exactly one sim update per tick; no delta time anywhere.
- Render pacing (0x436fd0): when on time, wait for the next pulse and draw. When late, skip drawing
  for up to 2 consecutive ticks; on the 3rd it draws anyway and resets the deadline, so the game
  **slows down** rather than catching up.
- The game counts **30 ticks as one second** (replay time format, stage timer `% 30`), so game time
  runs ~1 % fast against the wall clock (likely; measure with the oracle).
- Inputs are latched at the end of tick N−1 and used by the sim in tick N.

**Engine design:** sim at 1000/33 Hz with an accumulator; state is pure data so render can
interpolate (CLAUDE.md). "Arcade" timing reproduces the 33 ms step; the frame-skip/slowdown rule is
optional.

## 3. Data and formats (detail in loaders.md)

| File(s) | Format | Status |
|---|---|---|
| `ROM2` containers (`tbl`, `bgm`, `effect`, `parts*`) | `"ROM2"`, u32 count, count × (u32 offset, u32 size) | confirmed |
| `parts*.rom` | per entry u16 part id + packed block; **multi-pass LZ** (`0x425af0`), header u32 size, u8 0x73, u8 passes; decoded lazily at draw time | confirmed (all 9877 compressed parts unpack to exact size in a test reimplementation) |
| `tex*.rom` | u16 rect count, u16 page count, 8-byte rects, pages of **RGB565**; black = transparent | confirmed (all files walk exactly to EOF) |
| `cg.rom` | 44306 16×16 8-bit palettised tiles (RLE or LZ per tile); palettes live in the **exe** `.data` | confirmed |
| `tbl.rom` | 1365 tilemaps (u8 w, u8 h, u16 tile ids), used for all 2D/HUD/text | confirmed |
| `bgm.rom` | 18 entries of raw 44.1 kHz s16 stereo PCM; only code 0x19e loops | confirmed |
| `effect.rom` | 407 entries: 16-byte header (rate, length, loop start, loop end) + u8 mono PCM; code n → entry n−1 | confirmed |
| `*.COM` | route segments, 8 u32 section offsets + `RAIL DATA VER7.0`; polylines, placements, stops | layout confirmed, field meanings likely/guess |
| `komachi.dat` | MPEG-1 program stream (video + audio), DirectShow playback from the title menu | confirmed |

Route mapping: stage tables in the exe (0x463c18, 0x463c74/0x463d14, 0x485610) give each stage its
area (R1 Yamanote, R2 Akita, R3 Tōkaidō, R4 Hokuhoku, R5 Keihin, RC common), train sets and
`.COM` segments (index → file name table 0x47ca70; file names must be matched case-insensitively).

**Physics and scoring tables are not in `tbl.rom`**: they must be in the exe or the `.COM` files
(Phase 4).

## 4. Rendering (detail in renderer.md)

- DirectDraw 4 + Direct3D 3 (DX6), HAL device, **fullscreen 16-bit only**, 640×480 by default
  (512×384 … 1024×768 table, all 4:3), 16-bit Z-buffer, one back buffer (confirmed).
- **Projection:** vertical FOV in tenths of a degree (`0x84c150`), default **30.0°**, aspect **1.42**,
  near **0.05**, far **32.0** (verified in the exe bytes). Left-handed matrix, quantised through 4.12
  fixed point. View = Z flip; the camera is folded into WORLD (confirmed).
- **496×400 virtual screen** for all viewports and 2D quads, stretched non-uniformly to the mode.
  The 3D view is the band y 52..400 (496×348 virtual units ≈ 1.42), so on a 4:3 display the world is
  drawn **≈7.9 % wider than it is tall** (confirmed arithmetic; see decision D1).
- Pre-lit vertices (`D3DFVF_LVERTEX`), triangle lists, no D3D lighting, no alpha blending, no filtering
  or mipmap states set (nearest by default), colour-key transparency (confirmed).
- **Fog:** linear table fog 1.0 → 10.0, colour #BFBFBF; scenery culled beyond an XZ radius of **10
  units** (r² ≤ 100, verified), matching the fog end. Neither depends on aspect ratio, so Hor+
  widening needs no change there. The sky panorama draws only 3 tiles and the sky/ground fill quads
  span only the 496-unit frame; both need widening. Track/scenery section selection is still open.
- **2D:** a software sprite layer blitted into the back buffer (cg.rom tiles via tbl.rom maps, 207
  slots, 4 priority layers), plus untextured rect fills. A built-in mode centres the 640×480 2D at
  higher resolutions with black borders, a natural model for HUD anchoring.

## 5. Input (detail in input.md)

- DirectInput 5: system keyboard (non-exclusive) and the first joystick (exclusive), immediate-mode
  polling once per tick (confirmed).
- Power notch 0–5 (`0x829d00`), brake 0–10 (`0x829d04`; 9 = EB, 10 = unknown) (confirmed values).
- **Seven control modes** (byte `0x856352`, stored in `eeprom.dat`): four keyboard layouts, a joystick
  d-pad, a single analog axis with fixed thresholds, and two calibrated analog levers (the PC DDG
  controller via the game-port driver) (confirmed).
- Calibration: the ADJUSTMENT screen writes `joystick.dat` (6 + 11 i32 positions) (confirmed).
- Buttons are three active-low words (held / pressed / released) at `0x48c428..30` (confirmed).

## 6. Audio and text (detail in audio-text.md)

- One sound thread; 8 SFX voices (22050 Hz u8 mono buffers) + 2 BGM voices (44.1 kHz s16 stereo),
  half-buffer streaming with notify events; no panning (confirmed).
- Volume: linear in millibels from a per-code table × requested volume; pitch = rate·(256−p)/256
  (confirmed). Motor sound pitch follows a speed-like value (likely; the oracle should confirm X).
- **All speech is pre-recorded** (announcements, signal calls), started by sound sequences
  (`code | 0x1000`, table at 0x486190) (confirmed).
- **No font rendering at all:** every visible string, Japanese included, is tile art (confirmed for
  the render path). Translation therefore means mapping tbl.rom tilemap ids to strings in `locales/`
  and drawing them with our own text renderer over/in place of the original tiles; voice clips can
  get optional subtitles keyed by sequence id.

## 7. Configuration and save files

| Where | What | Conf. |
|---|---|---|
| `HKLM\Software\TAITO\電車でGO！２ 高速編 ３０００番台` `Option` | 128-byte XOR-0x55 string of `-flag` options (resolution, texture caps, `-debug`, …) | confirmed |
| same key, `FOGENABLE`/`FOGTABLE*` | fog on/off and parameters (strings) | confirmed |
| `BIN\dengo3k.ini` `[default]` | the same 24 option flags (name = 1) | confirmed |
| `BIN\eeprom.dat` | raw 0x35a-byte dump of the arcade-style settings block `0x856020` (incl. control mode) | confirmed I/O, layout open |
| `BIN\joystick.dat` | lever calibration | confirmed |
| `BIN\REPLAY.DAT` | MFC `CArchive`, header `RPLY1.3`; per difficulty/route/station lists of 31-byte input-change events | layout confirmed, hierarchy likely |

## 8. Running the original (oracle environment)

Verified on the primary machine (2026-10-15):
- `scripts/mount-disc.sh` serves the user's MDF/MDS through CDEmu (a vhba module loaded by the user),
  so the disc check passes unmodified.
- `scripts/run-original.sh` runs the game under Wine 11.19 with code page 932. On Wayland it uses
  winewayland, which emulates the 640×480 mode switch. Under XWayland's winex11 the back buffer stays
  desktop-sized: wined3d then disables the 640×480 Z-buffer and skips frame clears (wrong z-order,
  smearing). `DDG_WINE_DISPLAY=gamescope` gives a windowed run.

## 9. Decisions for the user

- **D1, aspect fidelity: DECIDED (user, 2026-10-15): keep the original aspect.** The original
  renders 3D ≈7.9 % horizontally stretched on 4:3 (virtual 496×348 band with projection aspect
  1.42), and the reimplementation reproduces that by default. Hor+ widening applies on top of it:
  the vertical FOV and the same pixel shape are kept, and the view extends sideways.
- **D2, intro movie: POSTPONED (user, 2026-10-15).** `komachi.dat` is MPEG-1 (video plus an MPEG
  audio stream, layer still to be confirmed). No maintained pure-Rust MPEG-1 video decoder exists.
  Idea to revisit later: transcode it once with ffmpeg at setup time into a format that's easy to
  play. The output is derived from game data, so it stays in the user's data dir and is never
  committed. Writing our own decoder (~1.5–2k lines) is the fallback.

## 10. Open questions → next phases

1. Physics, signals, speed limits, stop scoring and penalties: the driving-loop functions
   0x409fc0–0x40c4e0 (Phase 4; start with 0x40acd0 and 0x40c4e0).
2. `.COM` field semantics and stage tables: largely resolved in Phase 2 (routes.md); open items listed there.
3. Which camera preset is the in-cab view (30°/1.42 likely); world unit scale (hook `0x84c150`).
4. Track/scenery section selection for drawing (affects edge pop-in at wide aspect).
5. Brake code 10; `eeprom.dat` layout; the input-word semantics in full.
6. Real tick period and the 30-ticks-per-second assumption, measured under the oracle.

## 11. Phase 2 status: formats (2026-10-15)

Every file on the disc is either parsed by `ddg-formats` or documented as not needed:
`ddg-tools check` → **399 files: 52 parsed, 347 documented, 0 failed, 0 unknown**
(documented = DirectX 6 redistributable, InstallShield installer and runtime, controller driver,
autorun launcher, README, and the Cd2 CD-check DLLs).

| Module | Covers | Spec |
|---|---|---|
| `rom2`, `lz` | ROM2 containers; multi-pass LZ (all 13087 parts unpack exactly) | loaders.md |
| `part` | model parts: textured triangle lists, rect-relative UVs | models.md |
| `tex` | texture pages (RGB565, stored bottom-up) and rects; the game's UV mapping | loaders.md, models.md |
| `cg` | cg.rom tiles, tbl.rom tilemaps, palettes from the exe | loaders.md, audio-text.md |
| `audio` | effect.rom (u8 PCM, loop points), bgm.rom (s16 stereo) | audio-text.md |
| `route` | .COM track geometry (1.25 m per point), segment graph, stops, placements, visibility, traffic spawns | routes.md |
| `stages` | per-stage tables read from the exe: area, trains, segments, timetables, course/limit/traffic events, station names | routes.md |
| `exe` | VA → bytes for tables in the user's exe | (code) |

`ddg-tools export textures|tilemaps|audio|models|routes|all` writes PNG/WAV/glTF into `out/export`.
Visually checked: texture pages, HUD tilemaps (palette slot 4), and a route with its placed scenery.

Open (moved to later phases):
- Runtime save/config files (`eeprom.dat`, `joystick.dat`, `REPLAY.DAT`): not on the disc. Parse
  once the original has written real ones (Phase 4/5).
- Per-area track-model tables (`0x486170`, `0x486eb0`): needed to draw rails and ballast (Phase 3).
- Course-event kinds (signal aspects, kind 3), timetable fields 4–8 (Phase 4, with the oracle).
- Which palette each tilemap uses at runtime (Phase 5 menus/HUD).
- `komachi.dat` playback (decision D2, postponed).

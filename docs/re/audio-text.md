# Audio and text sources — dengo3k.exe (3000-bandai)

Sources: Ghidra decomp in `re/decomp/dengo3k/`, objdump (binutils from the nix store) of
regions that Ghidra did not turn into functions, and hexdumps of `DATA/bgm.rom`, `DATA/effect.rom`,
`DATA/tbl.rom`, `DATA/cg.rom` and `DATA/komachi.dat`. Addresses are VAs (file offset + 0x400000 for
.text/.rdata/.data). Confidence tags: **confirmed** (read from code and/or data bytes), **likely**,
**guess**.

## Function table

| Address | Suggested name | Role | Conf. |
|---|---|---|---|
| 0x42fe40 | `rom_load_whole` | builds `"..\data\" + name`, stats the file, mallocs, freads the whole file | confirmed |
| 0x42fef0 | `rom2_relocate` | turns each ROM2 entry's offset into an absolute pointer (offset + base) | confirmed |
| 0x42ffa0 | `voice_array_ctor` | constructs 10 voice objects (0x20 bytes each) at 0x8169a8 (MFC CEvent pair per voice) | likely |
| 0x430020 | `voice_dtor` | releases the voice's IDirectSoundBuffer | confirmed |
| 0x430090 | `audio_thread_start` | CreateThread(`audio_thread_main`), keeps handle at 0x81699c and thread id at 0x8169a4 | confirmed |
| 0x4300b0 | `audio_thread_stop` | posts WM_QUIT (0x12) to the thread, waits 1000 ms, closes the handle | confirmed |
| 0x430100 | `snd_post_play(code)` | PostThreadMessage 0x464; ignored while paused (0x816998) or audio is off (0x816ae8==0) | confirmed |
| 0x430130 | `snd_post_release(code)` | message 0x465 | confirmed |
| 0x430160 | `snd_post_stop_all()` | message 0x466 | confirmed |
| 0x430180 | `snd_post_pitch(code, p)` | message 0x467 | confirmed |
| 0x4301b0 | `snd_post_volume(code, v)` | message 0x468 | confirmed |
| 0x4301e0 | `snd_post_pause(on)` | sets the pause flag 0x816998, message 0x469 | confirmed |
| 0x430210 | `snd_seq_tick_frame` | if not paused, calls `snd_seq_tick` **twice** | confirmed |
| 0x430230 | `audio_thread_main` | loads ROMs, sets up DirectSound and buffers, runs the event/message loop | confirmed |
| 0x430720 | `voice_play` | handler for 0x464 | confirmed |
| 0x4307f0 | `voice_find(code)` | finds the slot that is already playing `code` | confirmed |
| 0x430820 | `voice_alloc(code)` | finds a free slot: SE uses slots 0–7, BGM uses 8–9 | confirmed |
| 0x430870 | `voice_release` | handler for 0x465 (jumps to the release tail or stops) | confirmed |
| 0x430920 | `voice_stop_all` | handler for 0x466 | confirmed |
| 0x430960 | `voice_set_pitch` | handler for 0x467 → SetFrequency | confirmed |
| 0x4309b0 | `voice_set_volume` | handler for 0x468 → SetVolume | confirmed |
| 0x430a00 | `voice_pause_all` | handler for 0x469 (Stop, or Play-looping, on every active voice) | confirmed |
| 0x430a40 | `voice_fill_half(slot, which)` | streaming refill of half a buffer (SE: 0x1000 bytes, BGM: 0x8000 bytes) | confirmed |
| 0x430da0 | `snd_volume_mb(code, v)` | volume parameter → DirectSound millibels (see below) | confirmed (from disassembly) |
| 0x430e00 | `snd_pitch_hz(code, p)` | pitch parameter → Hz | confirmed |
| 0x430e50 | `snd_free_roms` | frees the bgm/effect pointers | confirmed |
| 0x42ff20 / 0x42ff30 | `snd_play_w` / `snd_volume_w` | thin wrappers | confirmed |
| 0x41fd50 | `snd_seq_init` | clears the 4 sequence slots | confirmed |
| 0x41fda0 | `snd_request(code)` | if `code & 0x1000` → start sequence `code & 0xfff`, else `snd_post_play` | confirmed |
| 0x41fdd0 | `snd_seq_start_table(id)` | copies sequence `id` from the pointer table 0x486190 into a free slot | confirmed |
| 0x41fea0 | `snd_seq_start_list(list)` | same, but from a caller-built list | confirmed |
| 0x41ff60 | `snd_seq_tick` | advances the sequence slots (see below) | confirmed |
| 0x4206b0 | `snd_seq_clear` | marks every sequence slot free | confirmed |
| 0x420630 / 0x420650 / 0x420660 | release / release / stop-all wrappers | | confirmed |
| 0x41ffe0 | `voice_call_pair(a, b)` | 2-phrase voice call built from the table at 0x46c368 | confirmed |
| 0x420180 | `voice_call_signal_speed(aspect, kmh)` | "signal + speed" call: speed/5 picks code 0xae..0xcd | likely |
| 0x4229c0 | `train_task_create` | creates the player-train task; starts the motor sound at its base pitch | confirmed |
| 0x422c20 | `train_task_update` (not a Ghidra function) | per-tick train update; motor/running-noise pitch and volume (0x423bee–0x423c74) | likely |
| 0x424070 | `train_task_sound_reset` (label) | resets code 9 to vol 0x7f and the motor code to its base pitch | confirmed |
| 0x42b700 | `parse_options` | command-line `-opt` / `/opt` and `..\bin\dengo3k.ini` [default] → bit flags in 0x816840 | confirmed |
| 0x428db0 | `movie_open_amstream` | DirectShow AMMultiMediaStream (video into DDraw + primary audio to the default renderer) | confirmed |
| 0x430ec0 | `gfx2d_load` | loads `tbl.rom` (tilemaps) and `cg.rom` (16×16 tiles) | confirmed |
| 0x430fb0 | `gfx2d_draw_sprites` | software-blits 207 tilemap "sprites" into the 16-bit back buffer | confirmed |
| 0x431e70 | `gfx2d_free` | | confirmed |
| 0x431880 | `spr_set_pos(slot, x, y)` | | confirmed |
| 0x4318a0 | `spr_set_palette(slot, pal)` | | confirmed |
| 0x4318c0 | `spr_set_map(slot, tbl_index)` | -1 hides the slot | confirmed |
| 0x4318f0 | `spr_set_map_rect(slot, rec)` | map index plus a sub-rectangle | confirmed |
| 0x431920 | `spr_set_map_pal(slot, idx, pal)` | | confirmed |
| 0x425dc0 | `dbg_print(str, x, y, hl)` | **empty stub; returns 0** (arcade test-mode text printer, compiled out) | confirmed |
| 0x4376b4 / 0x4376d9 | MFC `CDC::ExtTextOut` / `CDC::TabbedTextOut` | library code, no direct callers | likely |

## 1. DirectSound setup (0x430230)

- The whole audio system lives on one worker thread, created by `audio_thread_start` 0x430090
  from 0x40e910 (boot) and 0x421cf0. The game thread only posts thread messages (0x464–0x469).
  **confirmed**
- `effect.rom` is loaded whole into the heap (0x42fe40, pointer at 0x816990). `..\data\bgm.rom`
  (string 0x48c1c4) is **memory-mapped** with CreateFileMappingA/MapViewOfFile (pointer at
  0x8169a0). Both ROM2 tables are relocated to absolute pointers (0x42fef0). **confirmed**
  - Minor bug: the cleanup closes the mapping handle twice and never closes the file handle. Harmless.
- `DirectSoundCreate(NULL, …)` (DSOUND ordinal 1). On failure the flag 0x816ae8 = 0 and every
  sound request becomes a no-op. **confirmed**
- `SetCooperativeLevel(hwnd 0x81682c, 1 = DSSCL_NORMAL)` (vtbl +0x18). The primary buffer is never
  created or formatted, so the device default applies. On Win9x DirectSound that is 22.05 kHz 8-bit
  under NORMAL. **confirmed** (the call); **likely** (that the default was the original mix format).
  This is a fidelity footnote, not something to emulate.
- 10 secondary buffers are created up front (`CreateSoundBuffer`, vtbl +0xc). DSBUFFERDESC has
  dwSize 0x14 and flags 0x1a8 = CTRLPOSITIONNOTIFY | CTRLVOLUME | CTRLFREQUENCY | LOCSOFTWARE.
  There is **no CTRLPAN**, so nothing is panned. **confirmed**
  - slots 0–7 (SE): PCM, mono, 22050 Hz, 8-bit; buffer 0x2000 bytes, notifications at 0 and 0x1000.
  - slots 8–9 (BGM): PCM, stereo, 44100 Hz, 16-bit (block 4, 176400 B/s); buffer 0x10000 bytes,
    notifications at 0 and 0x8000.
  - Each buffer is QueryInterfaced for IID 0x47f438 = `{b0210783-89cd-11d0-af08-00a0c925cd16}`
    (IDirectSoundNotify). `SetNotificationPositions(2, …)` gets two events per voice. **confirmed**
- Voice object (10 × 0x20 bytes at 0x8169a8): +0x04 event A (position 0), +0x0c event B (half),
  +0x10 IDirectSoundBuffer*, +0x14 s16 current code (0 = free), +0x18 s32 byte position
  (negative = drain countdown), +0x1c s16 pitch parameter, +0x1e s16 volume parameter. **confirmed**
- Loop: `MsgWaitForMultipleObjects(20 events, …, INFINITE, QS_POSTMESSAGE)`. Event index *i*
  → `voice_fill_half(i>>1, i&1)`. Index 20 means a posted message: it drains the queue, and WM_QUIT
  ends the thread. While the global 0x856356 (low byte) is non-zero, sound messages are dropped.
  **confirmed**

## 2. Streaming / refill (0x430a40)

Every voice, SE included, is streamed through a looping double buffer. Nothing is uploaded whole.
When the event for half *h* fires, the code fills the **other** half (`Lock((h^1)*half, half)`).
`voice_play` primes half 0 by calling `fill(slot, 1)`, then calls `Play(0,0,DSBPLAY_LOOPING)`. **confirmed**

SE refill (half = 0x1000 bytes, silence = 0x80):
1. If pos < 0: pos += 1. If it reaches 0, Stop, rewind and free the slot. Otherwise write silence.
2. Else if pos < loop_end: copy from pos, wrapping to loop_start whenever loop_end is reached,
   until 0x1000 bytes are written. One-shot samples have loop_end = 0, so they skip this branch.
3. Else (tail): copy up to the sample's length. If the end falls inside this half, pad with 0x80 and
   set pos = -2, which gives one more silent half and then the stop.

BGM refill (half = 0x8000 bytes, silence = 0): the same tail logic on the raw data. Only **code
0x19e** (bgm entry 7) loops, wrapping to offset 0. Every other BGM entry plays once. **confirmed**
(the `code == 0x19e` test in the BGM branch of 0x430a40)

`voice_release` (0x430870, "key off"):
- SE with loop_end == 0, or with loop_end == length: Stop now.
- SE with loop_end < length, while pos < loop_end: Stop, set pos = loop_end, refill, Play. The
  release tail (loop_end..length) then plays out. If pos is already past loop_end, it does nothing.
- BGM: Stop now. **confirmed**

Retrigger: playing a code that is already sounding reuses its slot. The slot is stopped and rewound,
then pos = 0, pitch parameter = 0, frequency = native rate, volume parameter = 0x7f. If no slot is
free, the request is dropped silently. There is no priority or voice stealing. **confirmed**

## 3. Sample formats

### bgm.rom: ROM2, 18 entries, sound codes 0x197–0x1a8 (index = code − 0x197)
- Raw **signed 16-bit little-endian stereo PCM at 44100 Hz**. There is no header, no compression and
  no ADPCM. The refill code memcpy's bytes straight into a buffer declared as 44.1k/16/2.
  **confirmed** (0x430a40 BGM branch plus the WAVEFORMATEX built in 0x430230). The entropy of 7.6 just reflects CD-quality PCM.
- Every entry size is a multiple of 4. Durations (size/176400): 5.5, 5.8, 4.2, 6.5, 5.7, 4.5, 36.3,
  **138.6 (0x19e, looped)**, 140.4, 42.7, 25.2, 168.5, 4.3, 3.4, 9.7, 7.4, 10.4 (0x1a7, also used as
  a chime inside voice sequences), 8.7 s. **confirmed**
- Table lookup in code: `bgm_base - 0xcb0 + code*8` == entry (code − 0x197). **confirmed**

### effect.rom: ROM2, 407 entries, sound codes 1–0x196 (entry index = code − 1)
- **Correction (confirmed, 0x430a40 / 0x430e00):** the lookup is `*(base + code·8)` on the ROM2 image, whose
  8-byte header (`"ROM2"`, count) precedes the entry table, so code *n* uses entry *n*−1. Code 0 means
  "no sound" (the fill routine returns early). Entry 406 is therefore never played by code. An earlier
  draft of this note said "index = code"; that was off by one.
- Each entry has a 16-byte header of four u32 values: `sample_rate`, `length` (= entry size − 16),
  `loop_start` and `loop_end` (both 0 for a one-shot). After it come the **unsigned 8-bit mono PCM**
  samples. **confirmed** (header read in 0x430870/0x430a40/0x430e00; the `+0x10` data offset;
  0x80 silence; every entry checks out as `size − 16 == length`).
- Rates vary per sample. 295 entries are 10000 Hz (the voice/announcement phrases, codes 0x03f–0x0d3 and
  0x0fe–0x18e, plus 0x195), and the rest include 44100, 17857, 17361, 9000, 19531, 16000, 15625 Hz and others.
  22 entries loop. Example: entry 3 is 44100 Hz, loop 16902–71437. **confirmed** (data)
- Code 0 is never playable, because 0 marks a free slot. **confirmed**
- Reimplementation: decode u8 → (s − 128)/128, then resample each voice in the mixer at
  `snd_pitch_hz`.

## 4. Pitch, volume and mixing

- **Pitch** (0x430e00): `hz = rate * (256 − p) / 256` (signed integer, rounded toward zero), where
  `p` is the 0..127 parameter. A larger p gives a lower pitch, down to about 0.5×. **Code 0x31 is
  inverted** (`p = 127 − p`). BGM codes always return 44100, so the "BGM_TEMPO" test item has no
  effect on PC. **confirmed**
- **Volume** (0x430da0; Ghidra lost the float maths, so this is from the disassembly at
  0x430da0–0x430df8):
  `g = (tbl[code] / 127) * (v / 127)`, `mB = −10000 − trunc(−10000 * g)`, clamped to [−10000, 0],
  then `IDirectSoundBuffer::SetVolume(mB)`.
  - `tbl[code]` = signed byte at `0x486b4e + 2*code`. It is a per-code "CODE VOLUME" table of 2-byte
    records, and only the low byte is read (the high byte, mostly 0xff/0x01, is never referenced).
    Typical values are 0x6b–0x7f. Constants: 1/127 at 0x47c8d8 (double), −10000 at 0x47c8d0
    (double). **confirmed**
  - This is **linear in millibels**: v = 64 on a 0x7f code gives about −50 dB. For cpal: amplitude
    = 10^(mB/2000). Keep this curve for arcade fidelity.
- There is no pan, no master or BGM/SE volume at runtime, and no software mixing of its own: each
  voice is a separate DirectSound buffer mixed by DSound. The test-menu strings "MASTER VOL",
  "EFFECT VOL" and "BGM VOL" (0x486274…) belong to the arcade test mode, which is stubbed (see §7).
  **likely**
- At most 8 simultaneous SE voices and 2 BGM voices. **confirmed**
- Pause (0x4301e0): Stop on every active buffer; un-pause calls Play(LOOPING) again. **confirmed**
- The INI/command-line option `nosound` is bit 9 of 0x816840 (table 0x47c7a0, index 9). No reader
  of that bit was found in the decompiled code. **guess**: it is unused.

## 5. Sequences (chained phrases: announcements, signal calls)

- `snd_request(code | 0x1000)` (0x41fda0) starts a **sequence** instead of a single sound.
  There are 4 sequence slots at 0x84fb00, each 0x38 bytes: s16 step, s16 counter, then up to 12
  (code, delay) s16 pairs, terminated by (−1, −1). **confirmed**
- The sequence pointer table is at **0x486190** (47 entries, 0x00–0x2e); the lists live at
  0x46bfe8–0x46c360. Examples: seq 0x13 = {0x149/100, 0x14a/130, 0x14e/144, 0x14f/112,
  0x150/62, 0x111/40, 0x108/40, 0x112/0}. Seqs 0x19–0x23 start with BGM code 0x1a7 (chime,
  delay 420) followed by 10 kHz voice phrases (0x16e…0x18f). These are the in-train
  announcements. **confirmed** (data); **likely** (that they are announcements)
- Tick (0x41ff60): for each active slot, once `counter >= steps[step].delay` it moves to step+1,
  resets the counter and plays that step's code, unless its delay is −1, which ends the sequence.
  The counter is incremented on every call. The step-0 sound is played when the sequence starts.
  `snd_seq_tick` runs twice per call of 0x430210, which is called once per frame-pump from 0x42a130.
  So the delay units are half-frames. **confirmed** (the logic); the absolute unit depends on the
  game tick (timeSetEvent 33 ms at 0x436f30, per the architecture notes), which **suggests** about
  60 units/s. **guess**
- Callers also build sequences on the fly: `voice_call_pair` (0x41ffe0, table 0x46c368 of
  (code, delay) pairs) and `voice_call_signal_speed` (0x420180: aspect call 0xc2/0xa8/0xa9/0xaa/
  0xab/0xc1/0xc3 followed by a speed number phrase 0xae–0xcd, chosen by kmh/5). **likely**
- So all spoken text (announcements, the driver's signal and speed calls) is **pre-recorded 10 kHz
  audio** in effect.rom, chained by code. There is nothing to translate in data except optional
  subtitles, which would have to be keyed by sequence/code ID.

## 6. Train sounds (motor, running noise)

- The motor sound code is the global s16 at **0x8570b6** (part of the stage/vehicle struct at
  0x8570a0; written by a struct copy, so the source table is still open). Its base pitch parameter
  is `0x45adb0[stage_idx 0x8570a4]`, which is 1 for every entry inspected. Started in 0x4229c0
  and 0x4080d0/0x40a0a0. **confirmed** (the use); **open** (where 0x8570b6 is loaded from)
- Code **9** is a second continuous running/rail noise, started with p = 0x7f and v = 1.
- Per-tick update (inside `train_task_update`, 0x423bee–0x423c74, only reachable by disassembly):
  - motor pitch parameter `[task+0x90] = clamp(1..127, trunc(X * k * env * 127))`, where
    `X = [task+0x30]` (a speed-like float), `k` = 0.0005 (0x472660) or 1/3000 (0x472698)
    depending on a mode flag, and `env = [task+0x38]`. That ramps by ±0.02 per tick (0x458bd0)
    between 0 and 1, rising while powering (0x856fba == 0). Then
    `snd_post_pitch(motor_code, [+0x90])`. **likely** (formula); **guess** (meaning of X and the
    flag). Because a larger p lowers the frequency, either the samples are pitched high or X is
    not plain speed. Confirm with the ddg-hook oracle.
  - code 9: the volume parameter `[+0x92]` and pitch parameter `[+0x94]` slew toward targets that
    depend on speed (0x857234), clamped at 0x70, 0x40, 80 and 127, in steps of ±1/±3/−5 per tick.
    Sent every tick through `snd_post_volume(9, …)` and `snd_post_pitch(9, …)`. **likely**
  - rail-joint clicks: the per-vehicle table at 0x471f88 (stride 0x20: period, 4 thresholds, 2
    codes at +0x18/+0x1c) triggers code 0x856d98 (from 0x459e60[vehicle]) at 4 phases of a
    distance counter. **likely**
  - Other per-tick triggers: 0x19 (every ≥0x50 ticks while 0x84c408 > 2.1), 0x30, 0x28 (pitch =
    0x857230/2), 0x2c/0x2e/0x29 (brake/air sounds whose volume follows state). **likely**
- The DirectSound frequency resolution is fine for this. Our mixer should apply pitch and volume
  changes at the sim tick and interpolate per sample to avoid zipper noise. That interpolation is an
  enhancement, so it needs its own config flag.

## 7. Text: where Japanese comes from

- **No font rendering in the game.** GDI32 imports only MFC-internal text calls (TextOutA,
  ExtTextOutA, TabbedTextOutA, DrawTextA, GrayStringA). There is **no CreateFont / CreateFontIndirect
  / GetGlyphOutline** import. FUN_004376b4/004376d9 are MFC CDC wrappers with no direct callers.
  **confirmed** (imports); **likely** (unused for game text)
- The arcade **test-mode text printer `dbg_print` 0x425dc0 is an empty stub** (`return 0`). It has
  43 callers, among them the sound test at 0x421180 ("SOUND CODE : %03XH %s"), and they draw nothing on
  PC. **confirmed**
- **All in-game 2D (menus, HUD, station names, messages) is a tile/sprite layer**, the arcade board's
  "FIX/sprite" plane emulated in software. **confirmed** (mechanism); **likely** (covers all visible text)
  - `tbl.rom` (ROM2, 1365 entries) holds **tilemaps**: byte width, byte height (both in 16 px tiles),
    then w×h u16 cell indices into cg.rom (0 = empty). At load, the pixel sizes `w<<4, h<<4` are cached
    at 0x850aa8. **confirmed** (0x430ec0, 0x430fb0)
  - `cg.rom` (u32 count = 0xad12 = 44306, then 44306 u32 offsets, relocated to pointers) holds
    **16×16 8-bit palettised tiles**. Byte 0 is the compression type:
    - `1`: RLE. Control byte c: if `c & 0x80`, copy `(c & 0x7f) + 1` literal pixels; otherwise
      repeat the next byte `c + 1` times. Pixel 0 is transparent (skipped).
    - `2`: LZ. If `c & 0x80`, `(c & 0x7f) + 1` literals; otherwise copy `c + 1` pixels from
      `next_byte + 1` pixels back in the 256-pixel output. Pixel 0 is transparent at blit time.
    **confirmed** (0x430fb0)
  - Palettes: 256 × u32 (xRGB) per palette at 0x829d60 + pal*0x400, converted to RGB555 or RGB565 to
    match the surface. Colour 0 maps to 1, so black stays opaque. **confirmed**
  - 207 sprite slots of 16 bytes at 0x816f88: y s16, x s16, src col/row s8, w/h s8 (sub-rect of the
    map), palette u8, layer u8 (0..3, drawn 3→0), tbl index s16 (−1 = off). Coordinates are in a
    640×480 space. With flag 0x855ffc & 0x1000 the layer is **centred** on larger modes
    (offset (W−640)/2, (H−480)/2, tables 0x48c1e0/0x48c1f0). That matters for HUD anchoring in
    widescreen. **confirmed**
  - So Japanese menu text, station names, signal/limit captions and so on are **pixels baked into
    cg.rom tiles and composed by tbl.rom maps**. A scan of tbl.rom, cg.rom, the `.COM` route files and
    `tex*.rom` found no Shift-JIS strings, only byte-pattern noise. **likely**
- Japanese strings that *are* in the exe (Shift-JIS, .data):
  - MessageBox/UI: 0x486ed4 "CD-ROM 『%s』 をドライブにセットしてください。" (CD prompt; part of
    the off-limits CD check, not analysed), 0x4870b8 VRAM ≥16 MB / no Voodoo2, 0x487118 "device
    not found", 0x48762c "DirectX 6 or later required", 0x48c148/0x48c17c replay load/save failures.
  - Window title 0x48756c 「電車でＧＯ！２高速編３０００番台」 and registry key 0x4875d0
    `Software\TAITO\電車でGO！２ 高速編 ３０００番台`.
  - Vehicle names 0x484fa4–0x4854f0 (e.g. "485系3000,特急はくたか（ブルー）") and 0x486b44
    "キハ５８系". These are labels for the `modelviewer` debug option. **likely**
  **confirmed** (addresses, from `strings-dengo3k.txt` / Shift-JIS scan)
- **Translation implications:** none of the gameplay text can be pulled from data as strings. For
  English we either (a) replace or overlay specific tilemaps (tbl index → translated string drawn by
  our fontdue renderer at the same rect), keyed by tbl.rom index, which needs a catalogue of which
  maps contain text, or (b) ship translated replacement images, which is not allowed in the repo
  because they would be derived from game data. (a) fits the project rules: `locales/en.toml`
  maps `tbl.<index>` → text, and the renderer suppresses the original tiles when a translation
  exists. Voice stays Japanese; optional subtitles are keyed by sequence/code ID. Exe-resident
  error strings just become our own locale strings.

## 8. komachi.dat (intro movie)

MPEG-1 system stream: pack 0x1ba, video stream 0xe0 and **MPEG audio stream 0xc0** (both seen in the first 2 MB).
Played by 0x428db0 through DirectShow `AMMultiMediaStream`: AddMediaStream(DDraw, primary video) and
AddMediaStream(NULL, MSPID_PrimaryAudio, AMMSF_ADDDEFAULTRENDERER). The audio goes straight to the
system renderer and does not pass through the DSound voices. **confirmed** (imports and stream ids). The
reimplementation needs an MPEG-1 video + Layer I/II audio decoder, or it can skip the movie.
The allowed dependency list has no decoder, so ask before adding one.

## Open questions

1. What fills the motor code 0x8570b6 (stage/vehicle struct copy) and the meaning of `[task+0x30]`
   in the motor pitch formula. Measure with ddg-hook: log the 0x467 messages against speed.
2. The absolute duration of a sequence-delay unit. It depends on how often 0x42a130 runs per sim tick.
3. A full catalogue of which tbl.rom maps carry text, for translation.
4. Whether `nosound` (bit 9 of 0x816840) is read anywhere outside the decompiled functions.
5. The high byte of the 0x486b4e per-code records (unreferenced, maybe priority or a leftover).

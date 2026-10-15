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
| 0x420180 | `voice_call_prefix_speed(prefix, kmh)` | "prefix + speed" call (e.g. speed limit): kmh/5 picks code 0xae..0xcd (§5) | confirmed |
| 0x4229c0 | `train_task_create` | creates the player-train task; picks the running-noise mode by train type; posts the motor's base pitch | confirmed |
| 0x422c20 | `train_task_update` (not a Ghidra function) | per-tick train update; horn, brake air, motor/running-noise pitch and volume, rail joints, squeal (0x4234eb–0x423eb0, §6) | confirmed (objdump) |
| 0x424070 | `train_task_sound_reset` (label) | resets code 9 to vol 0x7f and the motor code to its base pitch | confirmed |
| 0x407c10 | `track_probe_tick` | gradient, and the track-class ambient sounds 0x2c/0x2e/0x29 (§6.6) | confirmed (objdump) |
| 0x40d040 | `stage_setup` | copies the stage's motor/joint/horn codes from 0x459e38/0x459e60/0x459e88 | confirmed |
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

When a voice frees its slot (this matters for the 8-slot limit and for later release/pitch/volume
messages, which look the code up). **confirmed** (0x430720, 0x430a40 control flow), derived from
the fill order:
- The fill for half *k* runs when playback enters half *k−1*. The half that receives the last
  sample sets pos = −2. The next fill writes a silent half (pos = −1). The fill after that,
  which runs when playback *reaches* the silent half, stops and frees the voice. So the silent
  half is never heard, and the voice frees exactly at the end of the half that holds its last
  sample: after `ceil(n / H) · H` streamed samples, where `n` is the number of samples streamed
  since the (re)start and H = 0x1000 (SE, samples) or 0x2000 (BGM, frames). A sample that ends
  exactly on a half boundary gets no extra silent half. An empty sample lasts one half.
- Release-to-tail restarts the buffer (Stop, SetCurrentPosition(0), prime half 0, Play). The
  half alignment therefore restarts at the release point.
- **Guess**: the first notification (offset 0) fires when Play starts, so half 1 is filled in
  time. If it did not, half 1 would hold stale data. Wine's behaviour should be checked with the
  oracle.

Every voice-level message looks the code up across all 10 slots (0x4307f0). Pitch and volume
messages act only when the parameter differs from the cached one (0x430960 / 0x4309b0).
A SetFrequency outside DirectSound's 100–100000 Hz range fails, which leaves the old frequency
(the parameter is still cached). If it fails inside `voice_play`, the voice is never started. No
code reaches either case with the parameters the game sends. **confirmed** (code), **likely**
(no game path reaches it).

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
  - 0x31 is the **motor sound of every stage** (table 0x459e38 holds 0x31 twenty times). Thanks to
    the inversion, its pitch rises with the motor parameter: 0.51× at p = 1 and 1.0× at p = 127.
    That settles the "larger p lowers the frequency" puzzle in §6. **confirmed** (data)
- The code-volume table at 0x486b4e covers codes 0..0x1af, of which 0..0x1a8 are used. Code 0 is 0.
  The other values seen are 0x63..0x7c, and the data test checks that all lie in 0x40..0x7f.
  **confirmed** (data)
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
  So the delay units are half-frames. **confirmed** (the logic). PumpMessages 0x42a130 runs once
  per `Task_Tick` (0x4253c0), and `Task_Tick` runs once per 33 ms timer tick, so one unit is
  16.5 ms (≈ 60.6 units/s). **likely** (it holds while the game is not dropping ticks)
- Exact timing (0x41ff60). The counter is compared *before* it is incremented, and reset to 0 on
  advance. So the first phrase change comes **delay₀ + 1** sequencer ticks after the start, and
  every later change after **max(delayₖ, 1)** ticks. The sequence ends on the tick where the step
  being entered has delay −1, and that step's code is *not* played. The step-0 sound goes through
  `snd_request`, so a step-0 code with 0x1000 set chains into another sequence; this is bounded
  by the 4 slots. Later steps use the plain play path. While the enable flag 0x84faf8 is 0
  (cleared by the stage flow 0x4080d0 around result screens, and by 0x4253c0/0x419510), starts
  are ignored and running sequences are dropped on their next tick. **confirmed**
- The sequence table has exactly 47 pointers: the string "SOUND VOLUME" follows at 0x48624c.
  Every list holds valid codes 1..0x1a8, and the data test checks them. **confirmed** (data)
- Callers also build sequences on the fly:
  - `voice_call_pair(first, second)` (0x41ffe0) plays pair `second` of the table at 0x46c368 and
    then pair `first`. The table holds **180** (code, delay) entries and ends at 0x46c638, where
    the code is 0. Entries 0–4 and 90–94 are suffix phrases (0xc1, 0xc0, 0xbf, 0xbd, 0xbc, all
    with delay 0). The rest are station-name phrases with their lengths. 0x40c4e0 calls it with a
    per-station index as `second` and 2 or 3 as `first`, which gives "<station> + suffix".
    **confirmed** (code, data); **likely** (the meaning)
  - `voice_call_prefix_speed(prefix, kmh)` (0x420180). The table is built on the stack: 7
    (code, delay) prefixes `(0xc2,0x22) (0xa8,0x28) (0xa9,0x24) (0xaa,0x26) (0xab,0x26)
    (0xc1,0x22) (0xc3,−1)`, followed by the speed phrases. The speed phrase is element
    `12 + min(kmh/5, 26)`, so it overlaps the last prefix: kmh 0–4 → 0xc3, 5–9 → none (−1),
    10–79 → 0xae..0xbb and 80–129 → 0xc4..0xcd (one phrase per 5 km/h), ≥ 130 → none. With no speed phrase
    only the prefix plays (via `snd_request`). Otherwise the sequence is
    `{prefix, prefix_delay}, {speed, 0}`. All callers (0x40acd0, 0x40b4a0) pass prefix 0, i.e.
    0xc2, and 0 for "limit lifted", so this is the speed-limit call, not a signal-aspect call.
    **confirmed** (code); **likely** (the meaning)
- So all spoken text (announcements, the driver's signal and speed calls) is **pre-recorded 10 kHz
  audio** in effect.rom, chained by code. There is nothing to translate in data except optional
  subtitles, which would have to be keyed by sequence/code ID.

## 6. Train sounds (motor, running noise, joints, horn, ambient)

Read from `train_task_update` 0x422c20 (sound part 0x4234eb–0x423eb0, objdump; Ghidra loses every
`__ftol` argument here) and from 0x407c10. Float maths runs at x87 53-bit precision (physics.md §2).
`v` is the new speed task+0x3c (m/s), `v₀` the previous tick's speed 0x857234, `fade` the
traction fade task+0x38, and `kmh` the f32 store `[esp+0x14] = v·3.6`. **confirmed** unless noted.

### 6.1 Per-stage codes and per-type settings
- 0x40d040 copies three per-stage i16 codes: motor **0x8570b6 = 0x459e38[stage]** (0x31 for all
  20 stages), rail joint **0x856d98 = 0x459e60[stage]** (0x32 for all), and horn
  **0x8570a2 = 0x459e88[stage]**: 8 8 8 4 4 4 4 **0x1a3** 4 4 6 6 4 4 4 4 4 8 8 8. The motor's base
  pitch is 0x45adb0[stage] (1 everywhere). This answers the old "struct copy" question.
  **confirmed** (code + data)
- 0x4229c0 sets the running-noise mode task+0x96 from the train type: type 9 → 1; types 2, 3, 6, 8 →
  3; types 4, 5 → 2; otherwise 0. These are code immediates. It also latches two flags from the
  rail-joint row of the type at creation (0x471f9c + 0x20·type): bit 0 → 0x84c430 (brake-air
  sounds) and bit 1 → 0x84c400 (flange squeal, departure sound). Bit 2 (rail joints) is read live,
  each tick, from the *current* type.

### 6.2 Segment start, stop and reset
- Segment start (0x4080d0): `play 9; pitch(9, 0x7f); volume(9, 1); play motor; pitch(motor, base)`.
  If 0x857230 > 0 and 0x857210 < 3 it also plays 0x28 with pitch 0x857230/2 (rain or slippery
  rail; game rules, not part of the train task).
- Reset hook 0x424070: `volume(9, 0x7f); pitch(motor, base)`.
- Not driving (flag 0x200 clear): release 0x1c, and zero the squeal counter task+0x84 and the joint
  state task+0x80.
- Driving with `v ≤ 0`: release 0x1c; noise volume/pitch parameters = 1 / 0x7f; then
  `volume(9, 1); pitch(9, 0x7f); pitch(motor, base)`.

### 6.3 Moving: motor and running noise (code 9)
1. Noise volume `nv` (task+0x92) steps ±1 toward `clamp(v₀·32·fade, 1, 127)`.
2. Motor pitch (task+0x90) = `clamp(trunc(X·k·fade·127), 1, 127)`, where `X` = task+0x30 =
   f32(`v·3.6·34.6597·1.149425287`) (0x459420, 0x472638, 0x472630). `k` = 0.0005 (0x472660) in
   mode 2 and 1/3000 (0x472698) otherwise. With code 0x31's inversion the motor plays at
   `rate·(129 + p)/256`, so it rises from 0.51× at rest to 1.0× at about 75 km/h (mode 0, full
   fade).
3. Noise pitch `np` (task+0x94) uses a target `T(d) = 128 − kmh/(top·d)·fade·127`, where top is the
   P5 speed of the current type (0x471aec), and a "slow step" divisor
   `D = 1 − trunc(rate)` (as i16):
   - **mode 0**: T(1.4) clamped to [1, 127], with rate = `v·(−0.1176470588)` (0x472678). If
     `stage_tick % D == 0` and np > T: np −1. If np > T and |T − np| > 32 (0x472670): np −1. If
     np < T: np +1. Then the coast step below.
   - **mode 1** (type 9): T = 127 while `v₀ < 8.889` m/s (0x472668), else T(1.4) clamped, with
     rate = `v₀·(−0.1176…)`. Slow step and +1 as in mode 0. Then, if decelerating
     (`v₀ > v`), `v₀ < 5.556` (0x469830) and the brake ≠ 0: np 0x7f → 0x20, then np 0x20 → 0x1f
     (so 0x7f becomes 0x1f), and nv +1 while below `clamp(v₀·30·fade, 1, 127)` (0x458b90). There
     is no coast step in that case. Otherwise the "> 32 above" step and the coast step.
   - **mode 2** (types 4, 5): if `v₀ ≤ 5` (0x4594f8) and accelerating, T = 120 (0x458b00); if
     also `trunc(v₀·3.6)` is 9 or 17, np = 0x78. Otherwise T = T(1.2) (0x457720). If accelerating,
     T = min(T, 80) (0x472658) **without** a lower clamp; else T is clamped to [1, 127].
     rate = `v·(−0.25)` (0x472650). Slow step, > 32 step, +1. Then, with power last tick
     (task+0x60 ≠ 0), nv = max(nv, 0x70); without power, the coast step.
   - **mode 3**: np = 0x7f.
   - **coast step** (power last tick = 0): np +3 if np < 0x70; nv −5 if nv > 0x40.
4. Post `volume(9, nv)` (twice, through 0x4301b0 and its wrapper; the second is a cached no-op),
   then `pitch(9, np)` and `pitch(motor, motor_p)`.

### 6.4 Rail joints, squeal, departure, brake air, horn
- Rail joints (0x423c79): `row = 0x8570c2 ? 20 : type`, `phase = trunc(odometer·10) % period`
  (in 0.1 m). If `phase ≤ click₀` and state = 4, state = 0. If the current type's flag bit 2 is set,
  each of `click₀..click₃` plays the joint code when `phase ≥ clickᵢ` and state = i. The checks
  run in sequence within one tick and advance the state. At `v ≥ 22.2` m/s (80 km/h, 0x472648)
  the first and third clicks advance the state by 2, so only two clicks per period remain.
  Every click also bumps a counter 0x84c3dc and calls 0x433260 (force feedback, which has no
  effect; input.md). Rows on this disc: period 120/360/1200/1500, clicks e.g. 12/24/94/104.
- Flange squeal (flag 0x84c400): if `0x84c408 ≥ 2.1` (0x472640) and the counter ≥ 0x50, play 0x19
  and reset. Otherwise count up to 0x51. **Departure**: with the same flag, `v₀ == 0` while moving
  plays 0x30, so it fires on the first moving tick.
- Brake air (flag 0x84c430; any phase): brake-cylinder moving counter task+0x58 == 1 → play 0x14,
  == 0 → release 0x14. Brake notch falling to 0 (`B < B_prev && B == 0`) → play 0x13.
- Horn (start of 0x422c20; skipped while 0x855ff8 & 0x4000). Press edge → play the horn code,
  except that 0x1a3 (a BGM-slot melody horn, stage 7) only plays if task+0x6c > 99, which then
  resets. Release edge → release, except for 0x1a3 and 8. task+0x6c counts every tick. The edges come
  from input bit 0x100 (0x48c42c pressed / 0x48c430 released) or the replay byte.
- Code 0x1c is released here but no starter was found (open question 6).

### 6.5 What the sounds probably are (guess)
0x31 motor whine, 9 running/rail roar, 0x32 joint click, 0x14/0x13 brake air / release, 0x19
flange squeal, 0x30 departure, 4/6/8 horns, 0x1a3 melody horn.

### 6.6 Track-class ambient sounds (0x407c10)
Each tick (before the physics) the class at the points 16 behind, 4 ahead and 24 ahead
(`stages.track_sounds[point & 0x1f]`) goes to 0x857200[3]. On a change, "entered" = new class
(0x8570a8[i]) and "left" = old class (0x857218[i]); both are 0 otherwise.
- Classes 2 / 3 (codes 0x2e / 0x29), probe +4: entered → play; left → release (class 2 also gets
  volume 0x7f); while current → `volume = min(trunc(v₀·3.6 + 64), 127)`.
- Class 1 (code 0x2c), a point sound with distance-based volume and pitch (probably a level
  crossing):
  - entered at +24 → anchor = trunc(odo + 25), state 0, play, pitch 1, volume 1;
  - current at +4 → anchor = trunc(odo), state 1, volume 0x7f;
  - left at +4 → anchor = trunc(odo), state 2. Otherwise, if state = −1, stop here.
  - `d = |anchor − odo|`, `x = clamp(trunc(d)·2)` and `y = clamp(trunc(d))`, where clamp means
    `>127 → 127, ≤0 → 1`. State 0: volume `(−x) & 0x7f`. State 2: pitch `x`, volume
    `(−y) & 0x7f`, and once `anchor − odo < −64`, release, anchor = −1, state = −1.
  - The anchor is an **i16** (0x856d94), so past 32767 m it wraps and the class-1 sound becomes
    inaudible or is released at once. Faithful behaviour; whether any route reaches it is open.
  - Segment reset 0x40cf40: anchor = −1, state = 0.

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

## 9. Reimplementation (`crates/ddg-audio`)

- **No CD audio.** The disc has three Mode 1 data tracks and no CD-DA (recon.md §1), so all music
  comes from `bgm.rom`. `SoundBank::load` reads `effect.rom` and `bgm.rom` (117 MB) into memory
  once. The original memory-maps `bgm.rom`; there is no mmap dependency, so we read it.
- `SoundTables::from_exe` reads the code volumes, the 47 sequences, the 180 call pairs, the per-stage
  motor, joint and horn codes and base pitch, and the 21 rail-joint rows from the user's exe. Only
  addresses and the counts above are hardcoded, plus the 0x420180 prefix/speed phrase list and
  the type → noise-mode map, both of which are code immediates.
- `Mixer` (audio thread, pure) implements §1, §2 and §4: slots, retrigger, no stealing, loops and
  release tails, the half-buffer free rule, the cached pitch and volume parameters, the
  millibel volume curve and pause. Mono effects go to both channels at full level. The output is
  f32 stereo at the device rate, resampled per voice at `pitch_hz`, with linear interpolation
  by default.
  - Enhancements, each a `MixerConfig` field: `master_volume` (≠ 1), `smooth_volume` (ramps
    volume changes over 5 ms; arcade = off), `interpolation` (`Nearest` / `Linear`). Which one
    sounds closest to Win9x DirectSound software mixing is unknown.
  - Out-of-range frequencies at play time are clamped instead of leaving a stuck silent voice (§2).
    No code path in the game hits this.
- `SoundSystem` (game thread) implements `snd_request`, the post functions with their pause and
  enable gating, the 4-slot sequencer (`frame()` once per 33 ms tick = two sequencer ticks),
  `call_pair` and `call_prefix_speed`. It produces `Command`s.
- `TrainSound::tick(&TrainTick)` implements §6.1–6.4 and `AmbientSound::tick` implements §6.6. Both
  are deterministic and covered by unit tests built from the formulas above. Differential checks
  against oracle logs of the 0x464–0x469 messages are still to do.
- `AudioOutput` (cpal, default device and format, any channel count) owns the stream. It receives
  `Command`s over a channel and applies them at the start of each device buffer.
- `ddg-tools play-sound <code> [--wav out/…]` plays a code or sequence live, or renders it to a WAV.

## Open questions

1. ~~Motor code source and the meaning of task+0x30~~: answered in §6.1/§6.3 (0x459e38; X = km/h ×
   39.84). Still to do: compare the motor/noise pitch and volume messages (0x467/0x468) against
   ddg-hook traces.
2. Sequence-delay unit: 1/2 tick, ≈ 16.5 ms (likely, §5). Confirm with an oracle timestamp of
   two consecutive phrase starts.
3. A full catalogue of which tbl.rom maps carry text, for translation.
4. Whether `nosound` (bit 9 of 0x816840) is read anywhere outside the decompiled functions.
5. The high byte of the 0x486b4e per-code records (unreferenced, maybe priority or a leftover).
6. Who starts code 0x1c? It is only ever released (train task, stopped or not driving). Search
   for a computed `snd_request` argument, or log 0x464 messages in the oracle.
7. Whether the offset-0 notification fires at Play (§2) under Wine and real DirectSound. If not,
   the original would play one half of stale data on every start.
8. Win9x DirectSound resampling quality (nearest, linear or better) for the default
   `Interpolation`, and whether its software mixer clipped or wrapped when voices summed past
   full scale. We clamp.
9. Does any route's odometer pass 32767 m, where the class-1 ambient anchor (i16) wraps (§6.6)?
10. The meaning of the pair-table blocks at 0–4 / 90–94 and of the second station-name block, and
    the per-station index source (0x856d58).
11. The meaning of sequences 0x00–0x18 and 0x24–0x2e (which events request them). Needed for
    subtitles keyed by sequence ID.
12. The code 0x28 condition (0x857230 > 0 and 0x857210 < 3 at segment start; 0x40c4e0/0x40acd0
    also play it). It belongs to the rules port, not the train task.

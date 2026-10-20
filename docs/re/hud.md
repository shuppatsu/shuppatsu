# In-cab HUD while driving

Scope: every 2D element shown during a stage, from the pre-departure wait to the arrival and
time-up popups: who draws it, where, with which tilemap and palette, and which game state
drives it. Also how 2D is submitted and where text comes from. Menus and the station-result,
continue and ranking screens are in `menus.md`.

Sources: the Ghidra decomp in `re/decomp/dengo3k/`, plus `re/decomp/dengo3k_labs/` (task
update functions Ghidra had left as `LAB_` labels, decompiled with a throwaway
`DecompAt.java` script into a copy of the project). Constants were read from the exe bytes
(VA − 0x400000 = file offset for `.rdata`/`.data`), and sprite art was identified by looking at
`shuppatsu-tools export tilemaps --palette N` output. Parsers: `crates/shuppatsu-formats/src/hud.rs`.

Confidence: **confirmed** means read from code or bytes. **likely** means a strong inference.
**guess** means it needs an experiment. Nothing here has been checked against the running game.

## 1. How the HUD is built

### 1.1 Sprite slots — confirmed

The HUD is made of **sprite slots**: 207 records of 16 bytes at `0x816f88`, rendered by
`Sprite_RenderLayer` (0x430fb0, renderer.md §11). (The port composes them on the GPU from palette indices,
or on the CPU redrawing only what changed: docs/performance.md §2–3.) Slot record:

| Off | Type | Field |
|---|---|---|
| +0 | s16 | y (native pixels) |
| +2 | s16 | x |
| +4 / +5 | s8 | source column / row in the tilemap (in 16-px tiles) |
| +6 / +7 | s8 | width / height in tiles (≤ 0 = whole map) |
| +8 | u8 | palette (0..127) |
| +9 | u8 | layer 0..3 |
| +0xa | s16 | `tbl.rom` tilemap id, −1 = hidden |
| +0xc | u32 | keep flag: `0x4316b0` resets every slot whose flag is 0 |

Correction (**confirmed**, callers of 0x4316b0): the reset is not per tick. It is called only
from 0x430ec0 (init), 0x434a00 and 0x42a1c0 (screen transitions) and the debug path of
`Task_Tick` 0x4253c0. A slot set with keep = 0 (e.g. the time-up banner, the ATS lamp) keeps
its image until something changes it. The port's per-tick `SpriteLayer::reset_transient` would
wipe such slots, so `shuppatsu` (`slots.rs`) marks every slot it sets as kept.

Slot API (all **confirmed**): `0x4316e0(slot, tbl, x, y, pal, layer, keep)` sets everything;
`0x431730(slot, desc*, x, y, pal, layer, keep)` takes a 6-byte descriptor `{u16 tbl, u8 col, row,
w, h}`; `0x431790` clears (x = y = 0x800, layer 3, tbl −1); `0x431850(slot, x, y, layer, pal)`
places; `0x431880(slot, x, y)` moves; `0x4318a0(slot, pal)`; `0x4318c0(slot, tbl)` sets the
image and resets the sub-rect; `0x4318f0(slot, desc*)`; `0x431920(slot, tbl, pal)`.
`0x4317d0`, `0x4317e0`, `0x431820`/`0x431eb0` and `0x425dc0` are VRAM preload/release leftovers
from the arcade board: no-ops on PC (they return at once). They are harmless to ignore.

Blit order (0x430fb0): four passes, layer 3 first and layer 0 last, and within a pass slot
0xce down to slot 0. So **layer 0 is on top, and a lower slot number wins within a layer**.
Tile pixel 0 is transparent. Colour index `i` of palette `p` is the u32 at
`0x829d5c + p·0x400 + 4i` (bytes B, R, G; see `cg::Palette`), converted to 555/565, with black
bumped to 1 so it stays opaque.

**Coordinates are native back-buffer pixels**, with no scaling. Every HUD position is looked
up per **resolution mode** `0x816af4` (0 = 512×384, 1 = 640×480, 2 = 800×600, 3 = 1024×768;
sizes at 0x48c1e0 / 0x48c1f0). The mode-0 and mode-1 columns are identical in every HUD table,
mode 2 is shifted to the right/down, and mode 3 is a different arrangement. **This document
gives the 640×480 column.** Many element positions add a per-mode origin `0x463d78` (x) /
`0x463d88` (y), which is (0, 0) at 640×480, (88, 60) at 800×600 and (200, 144) at 1024×768.
So the HUD is a 640×480 design that is not scaled up at higher resolutions (**confirmed**).
The flag `0x855ffc & 0x1000` centres the sprite layer at higher resolutions; the driving
HUD does not set it (that flag is used by menu screens, see menus.md) (**likely**). A global
shake offset (signed bytes `0x856358`/`0x856359`) is added unless `0x855ffc & 0x800`.

### 1.2 HUD tasks

| Task / function | List | Created by | Role | Conf. |
|---|---|---|---|---|
| `Hud_Create` 0x40f7e0 | 3 | stage load (0x4080d0 step 2) | places the static slots, initial images, creates the HUD task and the ATS lamp task | confirmed |
| `Hud_Update` 0x40ff20 | 3 | 0x40f7e0 | per tick: handles, clocks, remaining-time transfer, station plates, vehicle plate, then `Hud_Distance` 0x4162c0 and `Hud_HighSpeed` 0x416d10 | confirmed |
| `Departure_Update` 0x413370 | 3 | `Segment_Hud` 0x4124f0 (each segment) | pre-departure route strip, star marks, departure signal and banner (§5) | confirmed |
| `Signal_Show` 0x411500 → task 0x411940 | 3 | rules (0x40c010, 0x40a0a0), departure task 0x413370, NaviAnim 0x410cb0 (hide), attract demo 0x40e860 | signal aspect sprite, slot 0x60 (§3.9) | confirmed |
| `Sign_Show` 0x411a00 → 0x411d40 / 0x411e00 | 3 | rules (0x40acd0, 0x40b4a0, 0x40c4e0) | gradient / whistle sign (slot 0x5e), speed-limit sign (slot 0x5f) | confirmed |
| `TimeMsg_Show` 0x415080 → 0x415810 | 3 | every bonus / penalty | message + ±seconds, slots 0x28–0x2d, and adds the seconds to pending | confirmed |
| `Hint_Show` 0x411f20 → 0x4121d0 | 3 | rules, course event 9 | navigator portrait + speech balloon, slots 0x34/0x35 | confirmed |
| `StationCaption` 0x410450 → 0x410620 | 3 | approach HUD 0x40c4e0 | small "name 駅 通過/停車", slots 0x36–0x38 | confirmed |
| `StationBanner` 0x410720 → 0x410970 | 3 | 0x40c4e0, 0x413370 | big centred banner, slots 0x39–0x3b | confirmed |
| `Landmark_Show` 0x410a70 → 0x410bd0 | 3 | course event 17 | landmark caption (bridges, rivers, signal stations), slot 0x3c | confirmed |
| `NaviAnim` 0x410cb0 → 0x411080 | 3 | EB, approach, train task, obstacle | navigator reaction animations (slot 0x61) or the emergency-stop signal (slot 0x60) | confirmed (§3.17) |
| `Popup` 0x417de0 → 0x417e80 | 3 | various | generic timed sprite: `(slot, tbl, x, y, pal, duration, delay, blink)` | confirmed |
| `AtsLamp` task 0x417d10 | 3 | 0x40f7e0 via 0x417cc0 | "ATS作動" / "ATS確認", slot 0x32 | confirmed |
| `Cab_Overlay` 0x434fd0 | — | `Frame_End` every drawn frame | speedometer (slot 0x81 + needle) and cab-signal lamp (slot 0x82) | confirmed |
| `TimeUp_Update` 0x4159c0 | 3 | 0x4158b0 at game over | "時間切れ" slide-in and advice | confirmed |

The task framework is in main-loop.md §2. All HUD tasks are in list 3, so they update after the
rule functions of the same tick and before the render task draws (main-loop.md §2,
`Task_Tick`). Many tasks use the common "show" protocol below.

**Timed-display protocol** (0x410970, 0x411940, 0x411d40, 0x4121d0, …, **confirmed**): a task
waits `+0x38` ticks before it first appears (5 or 6), then shows its sprite each tick until
either a tick counter `+0x34` runs out or, when a distance was given, until the odometer
`0x857004` passes `+0x48` (= odometer at creation + distance). Calling the creator with −1
cancels it (`+0x3c = 0`). Creating a new one replaces the old one (one global pointer per
element: `0x856d30` signal, `0x856d54` gradient/whistle sign, `0x856d04` limit sign,
`0x856d38` time message, `0x856d44` hint, `0x856c7c` caption, `0x856d40` banner,
`0x856c64` landmark, `0x856c68` navigator animation). The duration argument is passed as a
float and converted with `__ftol` (the decompiler lost the value; it is the third argument
at every call site) (**likely**). `Signal_Show` is the exception: its ticks argument is an int, loaded with `fild` and converted back with `__ftol` (0x411888), so it is unchanged (**confirmed**).

The protocol differs in detail per task (**confirmed** from each update function; the port's
unit tests pin these counts):

| Task | Hidden updates | Visible updates with `t` ticks | Distance test | Notes |
|---|---|---|---|---|
| signal 0x411940 | 5 | t + 1 | `target − odo ≥ 0` | §3.9 |
| sign 0x411d40 / 0x411e00 | 6 (`+0x38 = 5` counted down to −1) | t + 1 | `target − odo ≥ 0` | the slot is hidden by the destructor |
| hint 0x4121d0 | 5 | t (decrement before the draw) | `target − odo ≥ 0` | |
| caption / banner / landmark 0x410620 / 0x410970 / 0x410bd0 | 6 | t + 1 | `odo − start < distance` | ends as well when `0x856d00 = 0` |
| popup 0x417e80 | `delay` | t + 2 (one draw at delay 0, then t + 1) | — | |

Captions and banners take a kind with flag **0x10 = distance mode** (`+0x3c = 1`): the third
argument is then metres, not ticks. Every call in the game passes the flag (0x14 / 0x15 from
0x40c4e0, `aspect | 0x10` from the departure task), so the approach banner shows for 100 **m**
and the first caption until `f5 + 150` m from the stop (correction of §3.13). Landmarks always
use distance mode.

### 1.3 Flags that gate the HUD — confirmed

| Global | Meaning | Set / cleared |
|---|---|---|
| `0x856c80` | clocks visible | 1 at segment start (0x4080d0), 0 at arrival (0x40a0a0, 0x4080d0) |
| `0x856c6c` | clock freeze ticks: clocks are not redrawn while > 0 | 90 when passing a station (0x40d620) |
| `0x856d1c` | remaining-time readout active | 1 at segment start, 0 on result/continue/ending |
| `0x856d0c` | "driving panel" active (distance readout, ATC panel) | 1 on every update of the departure task (so from the first pre-departure tick of a segment, not only while driving), 0 at the loop exit after an arrival (0x4080d0), on the station result (0x413f30), the continue screen (0x415ab0) and every time-up update (0x4159c0) |
| `0x856c70` | distance panel on (1) / "Professional" plate (0) | toggled by the confirm button (bit 0x100000 pressed) while the ATS is idle (0x40c4e0, 0x4080d0) |
| `0x856d5c` | vehicle name/picture shown | 1 at HUD creation |
| `0x856d00` | station captions/banners allowed | 1 when one is created, 0 at arrival |
| `0x855ff8 & 0x100` | hides captions/banners | (phase flag) |
| `0x856d38 ≠ 0` | a time message is up: hides captions, banners and landmarks | |
| `0x856c80` | (see above) also cleared at the end of 0x40a0a0 and after the arrival hold (0x4080d0) | |
| `0x855ff8 & 0x400` | time-up popup running: time messages end, the departure task ends, and 0x4080d0 loops `Task_Tick` until it clears before the continue screen | 0x4158b0 / destructor 0x415a80 |

Implementation: `crates/shuppatsu/src/cab.rs` keeps these flags (`clock_freeze`,
`panel_active`, `panel_on`, `clocks_visible`, `lamp_lit`, `speed_mode`, `atc_index`) and
feeds them to `Hud::update` through `HudState`.

## 2. Static layout table

`Hud_Create` places 43 slots from the table at **0x464c90** (rows of 4 resolution modes × 10
bytes `{s16 x, y, layer, palette, slot}`, ending with x = −1 at 0x465348; parsed by
`hud::parse_slot_layout`, **confirmed**). 640×480 column:

| Slot | x, y | L | Pal | Element (§) |
|---|---|---|---|---|
| 0x0a | 66, 56 | 2 | 4 | label 現在時刻 (current time), §3.3 |
| 0x0b–0x10 | x 66, 90, 122, 146, 178, 202; y 72 | 2 | 4 | current-time digits hh:mm:ss |
| 0x11 | 66, 16 | 2 | 4 | label 次駅到着時刻 / 次駅通過時刻 |
| 0x12–0x17 | same x; y 32 | 2 | 4 | scheduled time digits |
| 0x18–0x1a | x 241, 281, 321; y 56 | 2 | 5 | remaining-time digits, §3.2 |
| 0x1b | 361, 56 | 2 | 5 | 秒 |
| 0x1c | 257, 32 | 2 | 4 | label 持ち時間 (remaining time) |
| 0x1d | 233, 16 | 2 | 4 | line + rolling-stock name, §3.8 |
| 0x1e | 411, 16 | 2 | 9 | train picture |
| 0x86 | 403, 40 | 1 | 4 | "自動運転" (replay playback) |
| 0x7c, 0x7d, 0x20–0x24 | x 494, 508, 522, 536, 552, 568, 584; y 440 | 2 | 4 | distance digits, §3.4 |
| 0x5a | 496, 408 | 3 | 4 | distance panel header |
| 0x5e, 0x5f | 528, 265 | 2 | 4 | gradient/whistle sign, speed-limit sign |
| 0x60 | 528, 113 | 2 | 4 | signal |
| 0x61, 0x62 | 80, 145 | 2 | 4 | navigator animation / "乗務完了" |
| 0x75 | 232, 424 | 3 | 4 | high-speed digital readout |
| 0x76 | 97, 424 | 3 | 4 | ATC speed |
| 0x77 | 17, 424 | 3 | 4 | "ATC" |
| 0x78 | 17, 456 | 3 | 4 | ATC margin bar |
| 0x34, 0x2e | 376, 376 | 3 | 4 | navigator portrait |
| 0x35, 0x2f | 152, 408 | 3 | 4 | navigator speech balloon |

The placements in the table are overridden at runtime for some slots (e.g. 0x5a, 0x7c, 0x7d in
centimetre mode, §3.4). Slots placed from other per-mode i32 tables:

| Slot | x, y (640) | Table (x / y) | Element |
|---|---|---|---|
| 0x83 | 493, 8 | 0x464c18 row | station plate "from" |
| 0x84 | 530, 48 | 0x464c40 row | arrow 0x4b8 |
| 0x85 | 552, 8 | 0x464c68 row | station plate "next stop" |
| 0x1f | 590, 16 (two-handle) / 11, 16 (one-handle) | 0x466cf8, 0x466d08 / 0x466d38, 0x466d48 | brake handle |
| 0x7e | 11, 16 / 11, 128 | 0x466d18, 0x466d28 / 0x466d58, 0x466d68 | mascon handle |
| 0x80 | — / 3, 8 | 0x466d78, 0x466d88 | one-handle frame 0x553 |
| 0x7f | — / 11, 96 | 0x466d98, 0x466da8 | one-handle "N" lamp |
| 0x81 | 16, 345 (dial) / 17, 432 (digital) | 0x47c9e8, 0x47c9f8 / 0x47ca28, 0x47ca38 | speedometer |
| 0x82 | 80, 144 | 0x47ca48, 0x47ca58 | cab-signal lamp |

## 3. Elements

### 3.1 Handles (notch / brake) — confirmed

`Hud_Update` every tick, from the **displayed** notches `0x856cf8` (power) / `0x856cfa`
(brake), which the train task 0x422c20 copies from the input notches each tick.

* Brake: slot 0x1f = `0x46540c[brake]` = tbl 0x09..0x11 for B0 (解除 "released") .. B8, and
  **0x12 (非常, EB) for both 9 and 10**. Each sprite is a tall bar gauge with a 7-segment
  number.
* Mascon: slot 0x7e = `0x46543c[notch]` = 0x50d (切, "off") .. 0x512 (P5).
* Layout by control type (`input_is_one_handle` 0x4332a0, i.e. input modes 3 and 5, input.md
  §4): two-handle puts brake at the top right (590, 16) and mascon at the top left (11, 16), and
  hides slots 0x7f/0x80. One-handle stacks both on the left (brake (11, 16), mascon (11, 128))
  inside the frame 0x553 (slot 0x80, (3, 8)) with an "N" lamp in slot 0x7f at (11, 96):
  0x53e (lit) when both notches are 0, else 0x53d (dark).
* Palette 4, layer 2.

### 3.2 Remaining time (持ち時間) and pending transfers — confirmed

Slots 0x18–0x1a show `0x856fb0` as three digits **with leading zeros** using
`0x4653d4[d]` = tbl 0x401..0x40a (big yellow-boxed digits); slot 0x1b shows 秒 (0x40d at
creation, 0x40e from the first update). Palette: **5 (white) while remaining > 14, 4 (yellow)
at ≤ 14** (`cVar1 = (remaining > 14) + 4`, 0x40ff20). Only drawn while `0x856d1c == 1`.

While a pending change `0x856d7c ≠ 0` exists, the readout cycles with the HUD counter
`task+0x34 % 3`: 0 → all three digits show mask 0x40b, 1 → mask 0x40c, 2 → one second is
transferred (rules.md §3: +1 with sound 0x26, cap 999; −1 with sound 0x27) and the real value is
shown. So **the digits flash in a 3-tick rhythm during every transfer**. Sounds go through
`snd_request` 0x41fda0 (audio-text.md).

Label: slot 0x1c = 0x261 (持ち時間, green) at (257, 32), from the per-stage row
`0x46594c + 16·stage` (first word).

Note: rules.md §3 says the digits blink while remaining ≤ 14. 0x40ff20 only switches the palette
at that threshold; the only blinking found is the transfer cycle above. See open questions.

### 3.3 Clocks — confirmed

`Clock_Draw(ticks, which)` 0x417140, called each tick for the clock `0x8570b0` (`which` = 0) and
the scheduled arrival `0x8571e8` (`which` = 1), unless `0x856c6c > 0` (the clocks freeze for
90 ticks after a station is passed, 0x40d620) or `0x856c80 == 0` (then all clock slots are
hidden).

* Digits: hours `t / 108000`, minutes `(t / 1800) % 60`, seconds `(t / 30) % 60`. Hours
  tens/ones and the ones digits use `0x465370[d]` (0x1f2, 0x1f4, …, 0x204: plain digit); the
  minutes and seconds **tens** use `0x4653a8[d]` (0x1f3, 0x1f5, …: the same digit with a colon
  in front), so the colons are part of the digit art.
* Current time: label slot 0x0a = 0x206 (現在時刻), digits slots 0x0b–0x10 at y 72.
  Palette **4 (red) once `clock ≥ scheduled + 30`** (late by ≥ 1 s), else 5 (yellow).
* Scheduled time: label slot 0x11 = 0x207 (次駅到着時刻, next arrival) at a stopping station, or
  0x208 (次駅通過時刻, next passing time) with palette 6 (white) digits when the next station is
  passed (`0x856d74 == 0`); digits slots 0x12–0x17 at y 32, palette 5 otherwise.
* Passing-time countdown: when the next station is passed and the scheduled time is 1..5
  whole seconds away (`(sched − clock + 30) % 30 == 0`), sound 0x15 plays once per second.
  The test is inside `Clock_Draw`, so it is skipped while the clocks are frozen, and both calls
  (clock and schedule) request the sound (the sound module, `sound/stage.rs`, plays it; the HUD
  draws nothing for it).
* Freeze: 0x40d620 sets `0x856c6c = 0x5a`; `Hud_Update` then skips both `Clock_Draw` calls
  and decrements it, so the slots keep the digits of the tick before the pass for 90 updates
  (the passing time stays readable). `cab.rs` `clock_freeze`, test
  `hud::tests::frozen_clocks_keep_their_digits`.

### 3.4 Distance to the stop point (停止位置) — confirmed

`Hud_Distance` 0x4162c0, each tick while `0x856d0c == 1`. Option byte `0x856355`
(eeprom +0x335, settings item 5, menus.md): 0 = off, 1 = centimetres, 2 = metres.

* Off, or the panel toggled off (`0x856c70 == 0`): slot 0x5a = **0x2c7 "Professional" plate** at
  (496, 408), all digits hidden.
* Metres (option 2): value = rounded distance `0x85722c` (clamped to −999 below −999), five
  digits slots 0x20–0x24 at x 522, 536, 552, 568, 584, y 440, header slot 0x5a at (496, 408).
* Centimetres (option 1): value = `trunc(0x85708c × 100)` (f32 100.0 at 0x4676e0), clamped to
  −99999; seven digits (slots 0x7c, 0x7d, 0x20–0x24, x 496, 504, 520, 536, 552, 568, 584), header
  at (472, 408).
* Digits `0x465540[d]` = 0x54..0x57, 0x61..0x66; **leading zeros blanked**, sign dropped
  (`hud::digits_blank_leading`).
* Header and colour by `tol` = stop tolerance `0x8570d0` (rules.md §2), with `tol·100 + 50`
  in cm mode:

| Condition | Header (m / cm) | Text | Digit palette |
|---|---|---|---|
| passing station (`0x856d74 == 0`) | 0x59 / 0x551 | 通過駅 まで | 6 (white) |
| `d > tol` | 0x58 / 0x4fe | 停止位置 あと | 4 (green) |
| `0 ≤ d ≤ tol` | 0x5a / 0x4fc | 合格範囲 あと | 4 |
| `−tol ≤ d < 0` | 0x5c / 0x4fd | 合格範囲 OVER | 4 |
| `d < −tol` | 0x5e / 0x507 | 過走 OVER | 5 (red) |

(In cm mode the overrun test is `d < −(tol·100 + 50)`.)

### 3.5 Speedometer — confirmed

Option byte `0x856354` (eeprom +0x334, settings item 4): 0 = none, 1 = analog, 2 = digital.
Drawn by `Cab_Overlay` 0x434fd0 → 0x4350b0 on every rendered frame while the stage is active
(`0x855ff8 & 2`), the train is waiting or driving (`& 0xa00`), the 3D bottom band is down
(`0x84c0f8 == 0`; it is up in the coupling minigame, §3.16, so the dial goes there) and the
section is not in ATC mode (`0x856d34 ≠ 2`). Bit 2 of `0x855ff8` is set only by
`Stage_Run`, so the attract demos never show it (menus.md §2.4).

* **Analog**: slot 0x81 = dial 0x54a (0–120 km/h, 128×128) at (16, 345), palette 4, layer 3.
  The needle is **not a sprite**: it is a filled triangle in the rect layer (§6.2), colour
  0xff000000 (opaque black). Model points (4, −20), (0, −58), (−4, −20) are rotated by
  `θ = v·π·0.0125 − π·0.75` (v in km/h, clamped to 0..120; constants 0x47c9e4 = π,
  0x47ca6c = 0.0125, 0x47ca68 = 0.75) about the pivot (80, 409) (0x47ca08 / 0x47ca18):
  `x' = x·cosθ − y·sinθ + 80`, `y' = x·sinθ + y·cosθ + 409`. So 0 km/h points down-left
  (−135°) and 120 km/h down-right (+135°). Speed in km/h = `0x857234 × 3.6` (f64 at 0x459420).
* **Digital**: slot 0x81 = 0x53 (a three-digit seven-segment "888 km/h" panel) at (17, 432). The
  digits are **drawn by palette animation**: palette 4 entries 0xdb..0xef (3 digits × 7
  segments) are set to `0x555555 + 0xa9aaab` = **0xff0000** (lit) or 0x555555 (unlit) from
  the segment masks at 0x4676a0 (`{0x77, 0x24, 0x5d, 0x6d, 0x2e, 0x6b, 0x7b, 0x25, 0x7f, 0x6f}`, bit k = segment k).
* **High-speed readout** (`Hud_HighSpeed` 0x416d10): only on stages with
  `0x466ca8[stage] ≠ 0` (stages 0–2, 7–9, 15, 17–19) and with option ≠ 0. In analog mode it shows
  slot 0x75 = 0x53 at (232, 424) once the speed reaches **120 km/h** and hides it again **below
  100 km/h** (hysteresis in `0x856c84`), because the dial ends at 120. In digital mode it stays
  hidden. It is hidden while a navigator hint is up (`0x856d44 ≠ 0`). Same palette trick.
  Detail (0x416d10): `0x856d34` is the per-stage table 0x466ca8 (1 = these stages); the
  readout is also hidden unless the panel is active (`0x856d3c = 0x856d0c`, then 0 while a
  hint is up). Both 0x416d10 (every HUD update) and 0x4350b0 write the same palette entries, so
  the readout and the digital dial always agree.

### 3.6 ATC panel (bonus section) — confirmed

When `0x856d34 == 2` (set by `Vehicle_Set(0x1000 | n)` 0x410320, i.e. the ATC run of the
Akita stages; course event 7 sets the ATC speed index `0x856cf4`): slot 0x75 always shows
the digital speed, slot 0x77 = 0x3f "ATC" at (17, 424), slot 0x76 = `0x4b + 0x856cf4` at
(97, 424) (ATC speed readouts 0, 30, 70, 110, 160, 210, 245, 275 km/h; the same numbers are the
table local to 0x416d10), and slot 0x78 at (17, 456) = margin bar `0x4a − clamp(atc − v, 0, 10)`
(tbl 0x40 empty .. 0x4a full red; it fills as you approach the ATC speed). Analog dial
suppressed.
The bar is read as `[0x4655b0 − 4·margin]` (`cab_hud::CabTables::atc_bar`). The analog dial is
not drawn while driving in ATC mode (Cab_Overlay neither draws nor clears slot 0x81; with no
per-tick reset the dial image would stay without its needle, the port clears it, **guess**).
Reached in the port through the stage-1 bonus section and a successful stage-2 coupling
(rules.md §7.5): `Rules::vehicle_code` → `CabHud` switches the plate and `speed_mode`.

### 3.7 Station plates and route strip — confirmed

* Top right, every tick (0x40f780): slot 0x83 = vertical station-name plate of the segment's
  start station, slot 0x84 = arrow 0x4b8, slot 0x85 = plate of the next **stopping** station.
  Plates are `0x46645c[name_id]` (tbl 0x4b9.. e.g. 秋田, 四ツ小屋). Name id per station:
  stage list `0x4860bc[stage]` (−1 terminated; `hud::HudTables::stage_station_ids`).
* Pre-departure route strip (0x4124f0, removed at departure tick `offset + 100`, §5): slot 0x7a
  = progress strip (発 … 着 arrows, e.g. 0x327) at (144, 136), slot 0x7b = route diagram
  (`0x48606c[stage][n]`, e.g. 0x32a 秋田…大曲) at (144, 184), where n = stations already
  served. The strip's arrows use palette 4 entries 0x50 + i: done segments are set to
  u32 0x00f00000 (green in B, R, G order, from 0x466b70), the others 0x00808000 (olive, from
  0x466b10); the current
  segment blinks between the two every 10 ticks for 60 ticks. Some stages also recolour
  entries 0x52.., 0x54.., 0x55, 0x56 from 0x466bd0. Slots 0x3f–0x4e (up to 16) are star marks
  over the diagram at y 144, x from a per-stage offset table built on the stack in 0x4124f0:
  a mark-1 station animates 0x38f..0x394 (silver), a mark-2 station 0x395..0x39a (gold),
  frame `(t / 5) % 6` for t < 91. Marks come from `0x856fe0[]` (rules.md §11: 1 = clean stop,
  2 = on-time clean stop).
* Details (0x4124f0 / 0x413370, **confirmed**): n counts the stations up to and including the
  segment start whose timetable "stops" field is 1; star slot `0x3f + i` gets the mark of the
  i-th of them (`0x856fe0[station]`), slot 0x3f (the origin) is zeroed, so marks show in
  slots 0x40..0x4e only. The x offsets (add 0x90) are row `stage + 1` of a 21 × 16 stack table
  (row 0 = stage 10 before its 8th station; from station 8 stage 10 also switches the strip list
  to 0x465c8c); `cab_hud::star_offsets` reads them from the `mov` immediates of 0x4124f0. The
  plates are set here too: `0x856d4c` = name id of the start station, `0x856c74` = the next
  station with "stops" = 1. 0x40f7e0 resets palette 4 entries 0x50..0x5f to olive at stage
  start; the green entries persist across segments. The stage-5 recolour copies 11 entries from
  0x466bd0, which holds only 4 colours followed by the jingle table 0x466be0 (an original quirk,
  copied as is).

### 3.8 Vehicle plate — confirmed

Per-stage row at `0x46594c + 16·stage`: `{持ち時間 label 0x261, name, picture, −1}`. Slot 0x1d =
name (e.g. 0x262 山手線２０５系) at (233, 16), slot 0x1e = train front picture (e.g. 0x68) at
(411, 16) with palette `0x431dc0(picture)` (10 for most pictures, 4 for picture 0x69, 9 otherwise).
`Vehicle_Set` 0x410320 switches to rows 20/21 (`0x1000 | n`) in the Akita bonus section and turns
on ATC mode.

### 3.9 Signals — confirmed

`Signal_Show(aspect, mode, ticks, distance)` 0x411500 → slot 0x60 at (528, 113). Mode: 0 =
block signal, 1 = home (場内), 2 = starting (出発), 3 = repeater (中継). Aspect 0..4 =
進行 / 減速 / 注意 / 警戒 / 停止 (rules.md §4.2). The tilemap comes from a 65-entry table built on
the stack, indexed `aspect + base`:

| Display variant | Mode 0 | Mode 1 (場) | Mode 2 (出) | Mode 3 (中継) |
|---|---|---|---|---|
| `0x8570c2 & 1 == 0` | 0x47c 0x479 0x476 0x473 0x470 (4-lamp) | 0x47d 0x47a 0x477 0x474 0x471 | 0x47e 0x47b 0x478 0x475 0x472 | set `0x466c0c[stage]` |
| `0x8570c2 & 1 == 1`, set `0x466c08[stage]` = 0 | 0x450 0x453 0x453 0x453 0x456 (3-lamp) | 0x451 0x454 ×3 0x457 | 0x452 0x455 ×3 0x458 | as above |

Repeater sets: 0 → 0x485 0x484 0x483 0x482 0x480; 1 → 0x485 0x481 0x481 0x481 0x480;
2 → 0x486 0x484 0x483 0x482 0x480; 3 → 0x486 0x481 0x481 0x481 0x480. Each sprite carries its
caption (進行信号, 注意信号 速度45km/h, …).

With variant 1 the mode 0–2 base is `set × 5 + {0x1e, 0, 0xf}`, so set 1 selects the same
4-lamp row as variant 0, and set 2 selects a third row with green 0x47f (otherwise 4-lamp).
`0x466c08` in the 3000-bandai exe holds `(1,0) (1,1) (1,1) (1,1) (1,1) (1,0) (1,1) (2,3) (2,1)
(2,1)` for stages 0–9 and `(1,1)` for stages 10–19. No stage uses the 3-lamp set 0, and only
stages 7–9 show a different sprite under variant 1. With variant 1 a green also sets `0x85744c`
(the aspect the 3D signal models show, scene.md) to 5 instead of 0. **confirmed** (exe bytes)

**Call (0x411500), step by step — confirmed (code 0x411500–0x41190f):**

1. Negative aspect: if a display task exists (`0x856d30`), its enable flag `+0x3c` is cleared
   and it ends at its next update. Returns 0.
2. Bit 0x100 marks a *forced* redisplay and is masked off (`& 0xff`). An aspect above 4 returns 0.
3. `0x85744c = aspect`. If the aspect is 0 and a green-signal hint is pending (`0x857224` ≠ −1,
   set by course event 20 and reset per segment by 0x40cf40), `Hint_Show(id, 0, 220)` is called
   (push 0xdc at 0x411774) and `0x857224 = −1`. This happens before the same-sprite test, so a
   no-op call still releases the hint.
4. The sprite is looked up as in the table above. A mode outside 0–3 returns 0.
5. If the newest task still shows the same sprite (`+0x30`), the call returns 0. Otherwise the
   newest task is either stopped (`+0x3c = 0`, unforced), or, when forced, keeps its sprite for
   5 more updates: `+0x48 = 0` and `+0x34 = 5` (0x411863).
6. A new task (list 3, update 0x411940, destructor 0x4119d0) gets `+0x30` = sprite,
   `+0x34` = ticks (pushed as an int and converted with `fild` + `__ftol`, so unchanged),
   `+0x38 = 5` (0x4118c7), `+0x3c = 1`, and `+0x48 = __ftol(0x857004) + distance`, or 0 when
   distance is 0. `0x856d30` then points to it. Returns 1.

**Task 0x411940 — confirmed (code 0x411940–0x4119c4):**

* While `+0x38 > 0`, it is decremented and nothing is drawn: 5 updates with no sprite.
* The update that finds `+0x38 == 0` sets it to −1 and does not decrement `+0x34`. Every later
  update decrements `+0x34` first.
* Without a distance (`+0x48 == 0`): it draws (`0x4318c0(0x60, sprite)`) while `+0x34 ≥ 0` and
  enabled. With ticks t, the sprite shows on updates 6 … t + 6, which is t + 1 updates.
* With a distance: it draws while `(float)+0x48 − 0x857004 ≥ 0.0` and enabled. The constant at
  0x458b10 is the f32 **0.0** (bytes 00 00 00 00), and the `fcomp` / `test ah, 1` at 0x41199a
  ends the task as soon as the odometer is strictly beyond the target.
* Otherwise it returns 0, and the destructor 0x4119d0 hides slot 0x60 (tbl −1) and clears
  `0x856d30` if it pointed to this task.

The slot keeps its last tilemap between draws. Tasks update in creation order, after the rule
calls of the tick (main-loop.md §2). So:

* **Forced handover**: the old sprite stays for 5 updates. On the 6th the old task ends (slot
  hidden) and the new one draws, so the change is seamless.
* **Unforced replacement**: the old task ends at the next update, which leaves a 5-update gap
  with no signal.
* **Same sprite**: there is no restart, and the running task keeps its own expiry. Once that
  task has ended, the same sprite can be shown again.

`shuppatsu_engine::rules::SignalHud` implements this model. The rules use it for the return value
(0x40c010 skips the limit set-up on 0), and `shuppatsu` popups use it for slot 0x60.

**Call sites (`aspect, mode, ticks, distance`) — confirmed (code):**

| Caller | Arguments | When |
|---|---|---|
| 0x40c010 sighting | `(0x8570a6, display type, 0, block[k+1].sight)` | new block; shown until the signal itself |
| 0x40c010 repeater | `(0x8570a6, 3, 0, 0x856fb4)` | `0x856fb4 > 0`, ATS ≤ 3; return value ignored |
| departure 0x413370 | `(4, 2, 100, 0)` (0x41361a) | counter = offset + 100 (§5) |
| departure 0x413370 | `(0x8570a6 \| 0x100, 2, 0, 10)` (0x413696), with `0x8570c2` set to 0 for the call | counter = offset + 0xa0 |
| 0x40a0a0 red-signal / ATS recovery | `(0x100, mode, 0 or 0x3c, distance)` (0x40a540) | after the 60 standstill ticks (rules.md §4.4) |
| 0x40a0a0 obstacle recovery | `(0, 0, 0, 5)` | before the final 30 standstill ticks |
| NaviAnim(0) 0x410cb0 | `(−1, −1, −1, −1)` | obstacle start (0x40c4e0, `0x856d80 & 0xf == 1`); the emergency-stop signal takes slot 0x60 |
| 0x4158b0, 0x4085e0 | `(−1, …)` | game over and stage-loop exit |
| attract demo task 0x40e860 | `(4, 2, 0x96, 0)` at counter 0x1e; `(0x8570a6 \| 0x100, 2, 0, 10)` at 0xa0 | demo drive created by 0x40e6e0 in list 4 (stage 5); not gameplay |

Emergency-stop signal (特殊信号発光機): `NaviAnim(0)` 0x410cb0 plays 0x44b..0x44f (flashing
pentagon, 緊急停止) in slot 0x60 during the obstacle event (course event 8).

### 3.10 Signs: gradient, whistle, speed limits — confirmed

`Sign_Show(kind, ticks, distance, blink_until)` 0x411a00, slots at (528, 265):

| Kind | Tbl | Meaning | Slot |
|---|---|---|---|
| 0 | 0x45b | 勾配なし (level) | 0x5e |
| 1 | 0x45c | 上り勾配 (up grade) | 0x5e |
| 2 | 0x45d | 下り勾配 (down grade) | 0x5e |
| 3, 4 | 0x46f | 警笛鳴らせ (sound horn) | 0x5e |
| 5 | 0x46e | 制限解除 (limit ends) | 0x5f |
| 6..21 | 0x45e..0x46d | 速度制限 20..95 km/h | 0x5f |
| 22..27 | 0x48a..0x48f | 速度制限 100..125 km/h | 0x5f |

* Limit kind = `km/h / 5 − 4 + 6`. The limit sign stays until the train has run `b` metres (the
  notice distance) and **blinks with a 30-tick period** (15 on / 15 off, `clock / 15` parity)
  until `c` metres are passed, i.e. until the limit is enforced (hint 0x141 explains this).
  It is hidden while a gradient/whistle sign is up, since they share the position.
  制限解除 shows for 90 ticks. Sources: 0x40b4a0 (limit list), 0x40acd0 (course kinds 4–6).
* Gradient signs come from 0x40c4e0 on changes of the gradient `0x84c3d8` (60 ticks each; level
  after 60 steady ticks).
* Whistle sign: course event 3 with `a = 1` (required; `a` other than 0 shows a sign only),
  shown until `b` metres are passed and **hidden while the horn is held** (input bit 0x100,
  0x411d40).

Task model (0x411a00 / 0x411d40 / 0x411e00, **confirmed**; corrections to the bullets above):

* One task list per slot. A call with the kind of the newest task is a no-op; any other kind
  disables the newest (it ends at its next update after its delay) and creates a new task. Each
  task waits **6 updates** (`+0x38 = 5` counted down to −1), then draws every update.
* Durations: `ticks` (t + 1 updates) or, with a distance, until `odometer at the call + b` is
  passed (`+0x48`). Limit signs pass `(kind, 0, b, c)`; 制限解除 `(5, 0x5a, 0, 0)`; gradient signs
  `(k, 0x3c, 0, 0)`; whistle `(4, 0, b, 0)`.
* Whistle: when the input word has the horn bit (`0x48c428 & 0x100` clear = held) the task
  **ends** instead of drawing, so the sign does not come back when the horn is released.
* Limit sign: hidden while any gradient/whistle task exists (`0x856d54 ≠ 0`, including during
  its delay). Until the blink point `+0x40` it is drawn only while `(clock / 15)` is **odd**; the
  update that finds the blink point passed clears `+0x40` and skips the draw (the slot keeps its
  last state), and the sign is steady from the next update.
* Gradient signs (0x40c4e0, driving-loop body, reading `0x84c3d8` as the physics left it on the
  previous tick): `≥ 10.0` up (once per state `0x856faa`), `≤ −10.0` down, `|g| ≤ 0.0001`
  counts `0x8570a0` and shows the level sign when it reaches 60. 0x40cf40 sets the counter to
  0x46 and the state to 0 per segment, so a level sign needs an up or down sign first.
* Implemented in `shuppatsu` `popups.rs`; tests `limit_sign_waits_six_updates_*`,
  `whistle_sign_ends_*`, `gradient_signs_follow_the_grade_force`.

### 3.11 Time messages (bonus / penalty popups) — confirmed

`TimeMsg_Show(kind, seconds, ticks)` 0x415080. Slot 0x28 = message, centred:
`x = 320 − w/2`, `y = 200`. Slot 0x29 = sign word 減点 (0x1ad) for penalties, slots 0x2a–0x2c
= digits `0x1b3 + d` (green, palette 4 for bonuses; palette 5 for penalties), slot 0x2d = 秒
(0x1bd). The number row is at `y = 264` for kinds 0–3 and 13, `y = 232` otherwise; x positions
(640) from a 6 × 5 table by layout `(bonus ? 3 : 0) + (3 − digit_count)`: penalty 3/2/1 digits
→ {216, 280, 312, 344, 376} / {232, –, 296, 328, 360} / {248, –, –, 312, 344}; bonus →
{–, 248, 280, 312, 344} / {–, –, 264, 296, 328} / {–, –, –, 280, 312} (– = x 640, i.e. off
screen). With 0 seconds only the message is shown and nothing is added. The task shows for
`ticks` (usually 60) and is removed early when the phase flag 0x400 is set.

| Kind | Tbl, width | Text | Sign | Raised by |
|---|---|---|---|---|
| 0 | 0x1be, 384 | 進行信号が現示されています 列車を進めて下さい | − | stopped at green (0x40a0a0) |
| 1 | 0x1aa, 160 | great! | + | on-time clean stop |
| 2 | 0x1ab, 160 | good | + | clean stop |
| 3 | 0x1ac, 160 | ボーナス | + | horn bonus, ending bonus |
| 4 | 0x1af, 128 | 急制動 | − | EB penalty |
| 5 | 0x1b0, 144 | 標識無視 | − | whistle sign missed |
| 6 | 0x1b1, 192 | 制限速度超過 | − | overspeed |
| 7 | 0x1b2, 176 | 停車駅通過 | − | overran a stopping station by 50 m |
| 8 | none | (number only) | − | stopped between stations, ending |
| 9 | 0x1c6, 256 | 知らせ灯確認ミス | − | false start |
| 10 | 0x1c7, 176 | 車止め激突 | − | buffer stop |
| 11 | 0x1c8, 144 | 警笛過剰 | − | excess horn |
| 12 | 0x258, 256 | セクション通過 | + | no-power section bonus |
| 13 | 0x259, 160 | 定通 | + | on-time pass |
| 14 | 0x1c5, 256 | 停止位置手前です | − | stopped short |
| 15 | 0x1ce, 176 | 車両激突 | − | obstacle collision |

Kind | 0x100 is passed through to the pending-time adder 0x4122c0 (its second argument is
unused) (**confirmed**). The kind → sign rule matches rules.md §3 (kinds 1, 2, 3, 12, 13 add).

### 3.12 Navigator hints — confirmed

`Hint_Show(id, ticks, distance)` 0x411f20 (suppressed while the ATS state ≥ 2): slot 0x34 =
portrait at (376, 376), slot 0x35 = balloon `0x135 + id` at (152, 408). Portrait 0x159 for
id 0x12, 0x15a for 0x1d/0x1e, else 0x157 (stage kind < 2, beginner badge) or 0x158. Id 0x0b
(0x140, "the horn is the foot pedal") is replaced by 0x508 ("horn is the Space key") or 0x509
("horn is the B button"): `(FUN_00433270() != 0) + 0x508`, where 0x433270 is true for joystick
modes 5–6 with `0x829d08 == 0` (the port uses modes 5–6 and ignores `0x829d08`, meaning unknown).
So the pedal balloon 0x140 itself is never shown. Balloon ids (tbl 0x135..0x156), English gist:

| Id | Gist | Id | Gist |
|---|---|---|---|
| 0 | the cab-signal lamp lights when the doors close | 0x11 | keep to the timetable at passing stations |
| 1 | obey a speed-limit sign until it is lifted | 0x12 | stalled in a dead section, cannot move |
| 2 | sign and signal together: obey the lower | 0x13 | please depart |
| 3 | slow down promptly at a restrictive signal | 0x14 | start braking |
| 4 | keep to the limit until the next signal | 0x15–0x18 | target speed 90 / 100 / 110 / 110–120 km/h |
| 5 | you did not slow down in time | 0x19 | keep the timetable even when passing |
| 6 | do not accelerate yet | 0x1a | stop at the mark, then couple |
| 7 | do not use the emergency brake | 0x1b | follow ATC speed, head for Shin-Hanamaki |
| 8 | repeater signals explained | 0x1c | target speed 120 km/h |
| 9 | (picture: cab-signal lamp) | 0x1d | next station is passed |
| 0xa | handling error: brake and power both on | 0x1e | next station is a stop |
| 0xb | the horn is the foot pedal | 0x1f | target speed 80 km/h |
| 0xc | a blinking limit sign is advance notice | 0x20 | running late, speed up a little |
| 0xd | the limit applies from here until lifted | 0x21 | running early, slow down a little |
| 0xe | sign lifted, but obey the 70 km/h signal | | |
| 0xf | coupling was too weak | | |
| 0x10 | ATC section: enjoy high speed | | |

Callers: course event 9 (`a`, `b`, `c`), the departure task (lesson stage), 0x40c4e0
(running late/early advice, ids 0x20/0x21, when a truncated float it computes equals 50 or 51;
"next station is passed / a stop", ids 0x1d/0x1e, 200–400 m or 500–600 m before the station
under speed and brake conditions; at most once per segment, `0x856fae`), 0x40b780 (id 6 while inside a restriction), the
ATS trip (id 5). The train task 0x422c20 shows id 0xa (0x13f) with portrait 0x157 in slots
0x2f/0x2e at the same positions after 11 ticks with both power and brake applied.

Details (**confirmed**, 0x411f20 / 0x4121d0 / 0x412250 / 0x40c4e0 / 0x40b780 / 0x413370):

* While the ATS state `0x857206 ≥ 2` the whole call is skipped, including `Hint_Show(-1)`.
  The rules raise `RuleEvent::HintShown { id, ticks, distance_m, start_m }` / `HintHidden`
  only for calls that are not skipped; `shuppatsu` `hints.rs` draws them.
* A new hint while one is current sets the old one's `+0x48 = 0`, `+0x34 = 5`: it stays 5 more
  updates while the new one waits its own 5, a seamless handover. The destructor hides both
  slots whatever task it ends.
* Call sites and arguments: green-signal hint `(id, 0, 220 m)`; course event 9 `(a, b, c)`;
  lesson stage station 0 at departure counter `o + 0x14` `(0, 0x78, 0)`, `o + 0x8c`
  `(9, 0x50, 0)`, `o + 0xdc` `(0x13, 0, 5 m)`; stage 1 station 0x18 at `o + 0x14`
  `(0x10, 0xb4, 0)`; ATS trip with the stop point ahead: `(5, 0x5a, 0)` when the speed exceeds
  the signal limit + 0.3 m/s (double at 0x458af8), else `Hint_Show(-1)`; stage 5 inside a signal
  restriction within 1.1 m/s under the limit: `(6, 0, dist_round − sight_pos(block) + 2)`
  (block 1 when the block index is 2), on every tick the condition holds.
* Advice (0x40c4e0 tail, stages of rank ≤ 3): on the 50 m leg marker (the +2.5 m bump of
  `0x84c40c` makes it fire once per leg), `0x21` (90 ticks) when `0x856fc0 < −2` or early
  ticks > 0xb3, with no restriction and a green aspect; `0x20` when `0x856fc0 > 3`. Then, unless
  `0x856fae` is set (cleared by 0x40b970, i.e. per segment and per passed station), the
  pass/stop advice from the speeds `FUN_00422990(n)` = i32 at `0x472278 + 4·(2n + 10·type)`
  (+1 entry with wet rails, `0x857230 > 1`; type from 0x472228): passing, braking, green, and
  `v(n) × 0.25` (double at 0x459520 used as f32, against m/s, as the code does) at most the
  speed and below both limits: `0x1d` for 150 m at 300–400 m (n = 2) and at 200–300 m
  (n = 1); stopping, speed ≥ `v(n) km/h`: `0x1e` for 120 m at 500–600 m (n = 4), 300–400 m
  (n = 2), 200–300 m (n = 1). Each matching window calls `Hint_Show` and sets `0x856fae`.

### 3.13 Station captions and banners — confirmed

The approach state `0x856d68` (0x40c4e0; `f5` = home-signal distance, timetable field 5):

| State | Trigger | Shows |
|---|---|---|
| 0 → 1 | `0x84c40c ≥ 0x4594c8` | caption: slot 0x36 = station name (green, `0x4665c4[id].sprite`) at (80, 112), slot 0x37 = 駅 (0x70) at `x + width`, slot 0x38 = 通過 (0x6c) or 停車 (0x6e) at `x + width + 32`; shown until the train is `f5 + 150` m from the stop |
| 1 → 2 | distance ≤ `f5 + 150` | banner: slot 0x39 = highlighted name (`0x4668ac[id]`), 0x3a = 駅 (0x71), 0x3b = 通過 (0x6d) / 停車 (0x6f); centred, y = 216, for 100 m (distance mode, §1.2); station announcement 0x41ffe0(2/3, id) |
| 2 → 3 | distance ≤ `f5 + 50` | caption again for 255 m |
| 3 → 4 | distance ≤ `f5 − 10` | (state only) |

Banner centring (0x410720): `x0 = 64 + 256 − (width + wordwidth + 32·(kind ≠ 3)) / 2` with the
word width 128 for kinds 0–3 and 64 for 4–6; name at x0, 駅 at x0 + width, word at
x0 + width + 32. Kind words (`0x465520` highlighted / `0x465500` plain): 0 出発進行
(0x6b/0x6a), 1 出発減速 (0x11d/0x11c), 2 出発注意 (0x11f/0x11e), 3 出発警戒 (0x121/0x120),
4 通過, 5 停車, 6 none. Station-name widths are stored with the sprite id
(`hud::StationNameSprite::width`, 64–192 px).

Landmarks (course event 17, 0x410a70): slot 0x3c = `0x466a20[id]` (秋田車両基地, 第一玉川橋梁,
…, 30 entries `{sprite, width}`, tbl 0x222..0x23f; 0x410a70 accepts ids < 0x1f, but entry 30 would
read the strip colours at 0x466b10), centred, y = 248, shown for `b` metres.

### 3.14 ATS — confirmed

* Lamp task 0x417d10 (slot 0x32, (208, 152), palette 4, layer 3) follows `0x856d08`: 0 →
  0x20e "ATS作動" (ATS tripped), 1 → 0x211 "ATS確認" (acknowledged) for 120 ticks then off,
  −1 → hidden. Correction (0x40c4e0): `0x856d08 = 0` is set only when the ATS brakes (state
  1 → 4 with a red aspect or past the stop point, or state 2 timing out), not when the bell
  starts (state 1 → 2 leaves it unchanged), so the lamp is dark during the confirmation window;
  1 when the driver confirms in time (rules.md §6); −1 when the rules reset the ATS without a
  confirmation (signal/obstacle recovery 0x40a0a0, arrival with the ATS active, loop exit
  0x4080d0). The port maps `AtsEmergency` → 0, `AtsAcknowledged` → 1, `SignalsReset` or the
  rules' ATS state back at 0 while the lamp is at 0 → −1.
* Sounds: bell 0xb on trip (stopped with 0x420650(0xb) on confirm), chime 0xd every 32 ticks
  after confirmation.
* On a trip before the stop point, if speed > signal limit + margin, hint id 5 is shown (90
  ticks).

### 3.15 Cab-signal lamp (知らせ灯) — confirmed

Slot 0x82 at (80, 144): 0x54e (dark) / 0x54f (lit), lit when `0x829d16 ≠ 0`. 0x4124f0 clears the
flag at segment setup and the departure task sets it at tick `offset + 200` (doors closed).
Shown while waiting (`0x855ff8 & 0x800`) and for 30 more ticks (`0x829d14`), not counting paused
ticks. Departing before the lamp lights is the false start (message kind 9).
Cab_Overlay runs once per drawn frame (`Frame_End`), so the 30 are frames; the port runs it
once per tick (**likely** equal, the original draws one frame per tick at 30 fps).
The dial follows the same flags: drawn while waiting or driving (`0x855ff8 & 0xa00`), once
more after the wait ends (`0x829d18`), else slot 0x81 is cleared.

### 3.16 Other popups

| What | Who | Slot, position, tbl | Timing | Conf. |
|---|---|---|---|---|
| 全区間走破 ALL ROUND CLEAR | 0x40a0a0 (final station) | 0x33, (184, 172), 0x20f | delay 10, 100 ticks, sound 0x19b; plus `0x4113e0`: slot 0x62 at (16, 313) 0x459 for 50 ticks then 0x45a (乗務完了 chibi) to tick 100 | confirmed |
| 非常ブレーキ使え | course event 8 (obstacle) | 0x33, (192, 168), 0x24b | delay 10, 90 ticks, bell 0xb | confirmed |
| 時間切れ (time up) | `TimeUp` 0x4158b0 / 0x4159c0 | 0x39, placed at (640, 244) at creation, then at `origin + (0x40 + p, 0xcc)` with `p` = 0x200 − 10 per update while `p > 0xa8`, i.e. x 566 → **226** (correction: not 232) at y 204, 0x252; at update 56 (counter 0x37) an advice sprite 0x3f2..0x3fe at (304, 344) (0x466ee8 / 0x466ef8) chosen by the failure cause `0x856d84` (0x40ddb0) with voice 0xea..0xfc | 151 updates (counter > 0x95 ends); sound 0x11 at tick 35; created after the 20 `Task_Tick`s that follow the game over; 0x4080d0 then loops `Task_Tick` until it ends (`0x855ff8 & 0x400`) before the continue screen | confirmed (cause meanings: guess) |
| Coupling minigame captions | 0x4093c0 | 0x33, 0x2d8 ボーナスゲーム / 0x2d9 時間内に衝撃なく列車を連結せよ, positions 0x459190 / 0x4591b0 tables | 70 / 80 ticks | confirmed |
| E3 coupling-ready animation | course event 23, 0x411240 | 0x61, 0x163..0x166 (E3 併合準備), palette 4 entries 0xb0.. from 0x463700 | anim table 0x46622c (60, 3, 3, 100 updates after 5 hidden), sound 0x1029 at counter 0x3c; yields to navigator reactions (§3.17) | confirmed |
| Navigator reactions | 0x410cb0(n), n from `0x45addc/0x45ade0` per stage | 0x61 at (80, 145), animation tables 0x465e04.. `{tbl, ticks}` | EB, notch slam, overrun, obstacle (§3.17) | confirmed |
| 自動運転 | replay playback (`0x816974`) | 0x86 at (403, 40), sub-rect col 6, 6×2 tiles of 0x50b | while replaying | confirmed |

**The 時間切れ sign is see-through where the art is index 0 (confirmed: code, tile data and a Wine
capture).** 0x252 is a 14×4-tile red train silhouette (palette 4: 15 red body, 10 yellow
lettering, 1 black lettering shadow, the shadow an extrusion of the strokes up and left by
about 2 px). Its window frames, doors, panel lines and bogie details are **palette index 0**
in the tiles, not a dark colour. `TimeUp` draws only this one sprite in slot 0x39 (layer 3,
palette 4, `0x4316e0`/`0x431850`, no flags beyond the keep word): no backing box or second
sprite (slot 0x3a stays empty until the advice sprite at update 0x37). The blitter 0x430fb0
has no per-slot option for opacity: it skips cell id 0 and, for every pixel, index 0 (the
palette conversion's entry 0 is 0 and the RLE/LZ paths test `!= 0` before writing), while any
other index whose colour is black is bumped to 0x0001 so it stays opaque. So the scene shows
through the silhouette's lines. A capture of the original (stage 5, no inputs, difficulty 1:
time runs out after 30 s; `out/captures/timeup5.mkv`, crop
`out/explain/text_shadow/original_timeup_crop_x4.png`) shows the sky and catenary through the
windows and lines; the port draws the same.

**Coupling minigame readout panel (0x4093c0; confirmed, code and a Wine capture).** Slots 0x3f–0x4b, palette 4,
layer 3, at 640×480: readout bar 0x17c (480×32, 東北 + km post, both speed digits on it) at
(80, 292); 0x17d (256×32: あと, the distance box, m) at (112, 324) with digits 0x41–0x43 and
the unit 0x171; the status lamp slot 0x47 at (384, 324), 160×32: 0x17e 併合準備完了 (green,
steady from 10 m), 0x180 距離注意 (yellow on black, blinking below 10 m, and on a failure),
0x17f 併合完了 (green, on a coupling); own nose 0x181 (128×64) at (80, 356); the partner train
0x182 (352×64) at x = 208 + min(272, trunc(16 · dist)), y 356; the ruler 0x183 (480×16) at
(80, 420), its 0 at x 208 and its end at x 560. So the partner train always reaches 352 px past
its nose: up to 80 px past the strip's end, where the 640-px screen cuts it. Right after placing
the slots it sets **`0x84c0f8 = 1`**; the render task 0x425680 (cases 0, 2, 3) then draws
`Draw2D_FlatQuad(0x486ec4, colours 0x47c3e8, 4, 0)` before the task lists: corners
(0,268)–(496,400) virtual (exe bytes), colour words 0 (black), depth 0, so the world below the
readout bar (pixel rows 321..480) is black and the panel stands on it; `Cab_Overlay` hides the
speedometer while it is set (§3.5). The flag is cleared at the cleanup (both exits) and before
the 100-tick wait of a too-weak coupling. The opening's phase E and the endings use the same
flag and quad and move its top edge (0x404110, 0x404920, 0x404a80, 0x4042b0; menus.md §12); a
coupling after an ending in the same session would see the edge where the ending left it
(**guess**, not run). Port: `coupling_hud.rs` (`CouplingHud::bottom_band`),
`play/cab_mask.rs` (`bottom_band`, before the weather like the top mask), `hud.rs`
(`HudState::bottom_band`). Widescreen (port): with `wide_coupling_band` (default on, docs/settings.md) the band spans the
visible width; off, it keeps the 4:3 width (virtual x 0..496, the centred design). Either way
the coupling slots are cut at the design's edges on the enhanced layout's wide canvas
(`SpriteLayer::frame`), so the panel is the same 4:3 picture in both layouts: past x 640 the
train art (to x 832 at 17 m) ends in a cut through a car and a wheel that the original never
shows (shots `wideband_train_*`). The 持ち時間 label (slot 0x1c) stays up: Coupling_Run only
sets `0x856d1c = 0`, `Hud_Update` skips the readout (0x4100a5) and nothing clears the label,
while the minigame's timer writes the digits (`HudState::remaining_label_kept`). `NaviAnim`
is gated on the flag (`Coupling::bottom_band`; cab.rs and the stage sounds).
Wine (stage 17 from its start at difficulty 1, a port-autopilot input script replayed by the
hook; `out/explain/coupling_panel/original_*.png`): rows 322..479 black across x 0..639 from
the intro to the outro, no dial, 0x180 blinking without a box at 8.1 and 1.35 m, and the train
ending at 560 + 16 · dist (x ≈ 582 at 1.36 m), cut only by the screen edge from 5 m out. Port
shots: same folder (before/after, both layouts, en/ja, 16:10, 16:9, 21:9, 4:3).

Details (**confirmed**):

* Time-up creation (0x4158b0) cancels the signal, signs, hint, caption and banner tasks, clears
  `0x856d00` and `0x856d0c`, and sets `0x855ff8 |= 0x400`, which ends the time message. The
  advice sprite of 0x40ddb0 by cause (`0x856d84`, forced to 2 when statistic 2 exceeds 1): 0 →
  0x3f6, 1 → 0x3f6 if the early accumulator `0x8570c4 ≥ 1` else 0x3fe, 2 → 0x3f9, 3 and 0xb →
  0x3fa, 4 → 0x3f3, 6 → 0x3f2, 7 → 0x3f4, 9 → 0x3f7, others (5, 8, 10, −1) → 0x3fe.
* Popups 0x417de0: the slot is cleared and placed at creation, the image set once on the update
  where the delay reaches 0 (no blink for these two), the destructor 0x417f00 clears the slot:
  visible `ticks + 2` updates after `delay` hidden ones.
* The chibi task 0x411450 moves slot 0x62 to `(origin.x + 0x10, origin.x + 0x139)` (the y adds
  the x origin; equal at 640×480) with palette 4, image 0x459 for counter < 0x32 then 0x45a,
  ends after counter 100; the destructor moves the slot back to (0x50, 0x91).

The station-result screen after a stop (停止位置 / m×1秒 / 合格! etc.) and the
continue/game-over screens are in menus.md.

### 3.17 Navigator reactions — confirmed

`NaviAnim(n)` 0x410cb0 creates task 0x411080 (destructor 0x4111f0) in list 3. The "navigator"
here is a passenger in a seat, drawn in slot 0x61 at (80, 145) (`Hud_Create` placement, layer 2),
who reacts to rough driving. Animation 0 is the emergency-stop signal (特殊信号発光機) and draws
in the signal's slot 0x60 instead. Port: `crates/shuppatsu/src/navigator.rs` (model and
triggers), drawn by `cab.rs`, sounds by `sound/stage.rs`. Both use the same model.

**Creation (0x410cb0, confirmed, code).** Nothing happens while `0x84c0f8 == 1` (open
question 9). With a current animation (`0x856c68 ≠ 0`):
* `n < 0` clears its `+0x3c` (enabled). `0x856c68` stays set until the task ends, so a
  cancelled animation still blocks new ones.
* `n > 0` is ignored. Reactions never interrupt each other.
* `n == 0` replaces it (clears its `+0x3c`), unless it is animation 0 itself.

Without one, `n < 0` returns. The new task gets the following fields:
* `+0x38 = 5` (delay), `+0x3c = 1`, `+0x48 = −1` (repeats);
* `+0x4c` = the frame list, `+0x50/+0x54` = the sound counter and code;
* `+0x6c` = the palette, `+0x70` = the slot (0x60 for `n == 0`, which also calls
  `Signal_Show(−1)`; else 0x61).

The per-animation `{counter, sound, palette}` triples are stack immediates (`local_f0`).

**Update (0x411080, confirmed, code).**
* **Delay.** The first 5 updates only count down.
* **First visible update** (delay 0). The task ends at once if it is disabled. Otherwise it
  draws frame 0 with `0x431920(slot, tbl, palette)`.
* **Later updates.** Each one increments the frame tick `+0x44` and the counter `+0x34`.
  * When the counter equals `+0x50` and the sound ≠ 0, the sound is requested (0x41fda0).
  * When the frame tick reaches the frame's ticks, the next frame starts.
  * At the `{−1, r}` terminator, `r` is loaded into `+0x48` the first time. With 0 left the
    task ends without drawing. Otherwise it restarts at frame 0 with `+0x48 − 1`. So a list
    plays `1 + r` times.
  * At each frame boundary the next frame's image is preloaded (0x425dc0) and the enabled flag
    checked. The one exception is a final frame (next entry is the terminator) when no repeat
    follows (`+0x48 == 0` or `r == 0`): there is no check, so a cancelled one-shot animation
    plays its last frame to the end. A cancel therefore ends an animation at its next frame
    boundary, not at once.
* **Drawing.** Every visible update redraws the current frame. While `0x855ff8 & 0x40` (the
  station result) the slot is hidden. The destructor hides the slot, frees the frame images and
  clears `0x856c68` if it is this task.

Each frame is therefore visible for exactly its tick count of updates. Total visible updates are
`Σticks · (1 + r)`, after 5 hidden ones. The sound plays on update `5 + counter` after the call.

**Animations** (`{tbl, ticks}` lists at 0x465e04.., read at runtime; confirmed, exe data):

| n | Frames (tbl:ticks) | Repeats | Visible | Sound @counter | Pal | Used by |
|---|---|---|---|---|---|---|
| 0 | 0x44b..0x44f, 4 each | 999 | until cancelled | — | 4 | obstacle (all stages), slot 0x60 |
| 1 | 0x219:20 0x21a:15 0x21f:50 | 0 | 85 | 0x1025 @20 | 8 | slam, set A |
| 2 | 0x219:20 0x21b:4 0x21c:15 0x21b:4 (0x21d:6 0x21e:6)×3 0x21f:30 | 0 | 109 | 0xdc @20 | 8 | EB, set A |
| 3 | 0x219:20 0x220:20 0x21f:20 | 0 | 60 | 0xe2 @20 | 8 | overrun, set A |
| 4 | 0x212:20 0x213:12 0x214:15 (0x215:6 0x216:6)×3 | 0 | 83 | 0x1012 @20 | 8 | overrun, set C |
| 5 | 0x212:20 0x217:4 0x218:4 | 0 | 28 | 0xe5 @20 | 8 | EB, set C |
| 6 | 0x212:20 (0x217:6 0x214:6)×3 | 0 | 56 | 0x1011 @20 | 8 | slam, set C |
| 7 | 0x2af:5 0x2b0:5 0x2af:5 0x2b1:5 | 5 | 120 | — | 0x10 | unused |
| 8 | 0x2b0:5 0x2b2:5 0x2b3:6 (0x2b4:5 0x2b5:5)×3 | 0 | 46 | 0xd8 @10 | 0x10 | overrun, set B |
| 9 | 0x2b6:7 0x2b7:8 | 7 | 120 | 0xd7 @0 | 0x10 | unused |
| 10 | 0x2b0:6 0x2b8:10 (0x2b9:6 0x2ba:6)×4 | 0 | 64 | 0xd6 @0 | 0x10 | slam, set B |
| 11 | 0x2b0:6 0x2bb:4 0x2bc:4 0x2bd:10 0x2be:30 | 0 | 54 | 0xd6 @0 | 0x10 | EB, set B |
| 12–19 | tilemaps 0x185..0x197 | 0 or 7 | 6–96 per pass | see `navigator.rs` | 8 | unused (no caller passes them) |

**Per-stage set** (`0x45addc + 8·stage`: EB, slam, overrun ×2 by the clock's low bit; both
overrun entries are equal on every stage; confirmed, exe data). Seen in headless shots of the
port, and for set C in the original:

| Set | Stages | EB, slam, overrun | Passenger |
|---|---|---|---|
| A | 0–2, 7–9, 17–19 | 2, 1, 3 | woman in a pink suit (0x219..0x220) |
| B | 3, 4, 10–16 | 11, 10, 8 | salaryman with a newspaper on a red seat (0x2b0..0x2be) |
| C | 5, 6 | 5, 6, 4 | schoolgirl standing (0x212..0x218); the EB makes her fall over |

**Triggers** (confirmed, code; port `navigator::calls`, in this order within a tick):

| Call | Where | Condition |
|---|---|---|
| `NaviAnim(0)` | 0x40c4e0 | obstacle start (`0x856d80 & 0xf == 1`, after course event 8) |
| `NaviAnim(EB)` | 0x40c4e0 | EB held 0x28 ticks (`0x856d64 == 0x28`, approach EB < 0x14, ATS 0, no obstacle, speed ≥ threshold, the penalty branch with `0x8571e0 & 3 ≠ 0`); the every-4th-application penalty does not react |
| `NaviAnim(EB)` | 0x40c4e0 | EB 0x14 ticks inside the approach (`0x856da8 == 0x14`, `0x856d64 < 0x28`, ATS 0, no obstacle) |
| `NaviAnim(overrun[clock & 1])` | 0x40c4e0 | approach state `0x856d9a` turns 4 (`0x85708c` below the overrun threshold, `0x8570ae ≠ 1`) |
| `NaviAnim(−1)` | 0x40a0a0 | obstacle cleared (first branch) |
| `NaviAnim(−1)` | 0x4113e0 | the 乗務完了 chibi (final station) |
| `NaviAnim(−1)` | 0x40a0a0 end, 0x4080d0 | after the arrival hold, the stage-loop exit (arrival or 20 ticks after a game over) |
| `NaviAnim(slam)` | train task 0x422c20 | statistic 11 += 2 (the notch-slam test of 0x422c20, audio-text.md); not while `0x855ff8 & 0x410` |

The time-up (0x4158b0) does **not** cancel a running reaction.

**E3 coupling-ready animation (course event 23 → 0x411240; confirmed, code).** Stages 17–19
fire course event 23 shortly before the coupling partner is placed (event 12); in the port's
stage 17 run from station 15 (`out/explain/coupling/stage17_station15_inputs.txt`) at drive tick
2258, 19 ticks before event 12, about 210 m from the stop. 0x411240 sets palette 4 entries
0xb0..0xbf from the 16 colours at 0x463700 (a grey ramp; GAME_INIT 0x40e910 sets the same at
boot) and creates task 0x411300 (destructor 0x411390) at the end of list 3, so it first updates
in the tick of the event, after the HUD task. Fields: `+0x38 = 6`, counter `+0x34`, frame
`+0x40`, frame tick `+0x44`, list `+0x4c = 0x46622c`. Each update decrements `+0x38` and does
nothing while it is still ≥ 1 (five updates); every later update increments `+0x44` and `+0x34`,
draws the current frame with `0x431920(0x61, tbl, 4)` **only while `0x856c68 == 0`** (no
navigator reaction current), requests sequence 0x1029 when `+0x34 == 0x3c`, and moves to the
next frame when `+0x44` reaches its ticks; at the −1 terminator the task ends and the destructor
hides slot 0x61, again only while `0x856c68 == 0`. Frames (`{tbl, ticks}` at 0x46622c): 0x163
"E3 併合準備" with the closed nose ×60, 0x164 ×3, 0x165 ×3 (the nose cover opening), 0x166 with
the coupler out ×100. Visible updates 6..170 after the event (frame 3 is drawn in update 171 and
hidden in the same update), the sound on update 65. It has no `+0x3c` flag: `NaviAnim(−1)` and
the time-up do not stop it; only a list-3 rebuild (segment start) does. The slot keeps the
`Hud_Create` placement (80, 145). Port: `navigator::ReadyAnim`, run by `cab.rs` after the HUD
task; shots `out/explain/coupling/ready_anim_*.png` (ticks 2262 hidden, 2263 first frame, 2323
and 2326 the opening frames, 2340 and 2427 the last frame, 2428 gone; `ready_anim_sheet.png`
crops the slot). Not yet compared with the running original (it needs a full stage 17 drive in
Wine); the position, palette and timing come from the code.

**Hints and balloons.** The reaction is independent of the navigator hint (portrait slot 0x34,
balloon 0x35, §3.12) and of the time message. They are separate slots and tasks, and neither
hides the other. In the original recording (below) the EB reaction plays while the hint
"速度標識と制限信号が…" and the 急制動 message are up.

**Verification** (`out/explain/navigator/`, gitignored):
* `compare_orig_left_port_right.png`: the original (Wine capture, stage 5 = set C,
  `scripts/capture-original.sh navi5 0 0 hook_inputs.txt -` with `SHUPPATSU_CAP_RECORD=60`) next to
  the port (`shuppatsu play 5 --inputs port_inputs.txt --size 640x480 --shot …`).
* EB: 0x212 for 20 ticks, then 0x217 and 0x218 (falling) for 4 each. The same frames sit at the
  same position with the same palette, at the same in-game clock second (05:56:24–25).
* Notch slam: animation 6's alternating 0x217/0x214 gestures.
* The port's stage 0 (set A, EB and slam) and stage 3 (set B, overrun at full power) shots are
  in the same folder.

## 4. Text drawing

**There is no font renderer and no Shift-JIS text on the HUD** (confirmed: no GDI text calls
from game code, audio-text.md §7; every HUD string above is a `tbl.rom` tilemap). The only
"glyph lookups" are digit tables, each a list of tbl ids indexed by the digit value:

| Digits | Table | Tbl ids | Used for |
|---|---|---|---|
| clock | 0x465370 | 0x1f2 + 2d | hours, minute/second ones |
| clock with colon | 0x4653a8 | 0x1f3 + 2d | minute/second tens |
| remaining time | 0x4653d4 | 0x401 + d | 持ち時間 (masks 0x40b, 0x40c) |
| distance | 0x465540 | 0x54..0x57, 0x61..0x66 | 停止位置 |
| message | stack table in 0x415080 | 0x1b3 + d | ±seconds |
| seven-segment | palette 4 entries 0xdb..0xef | masks at 0x4676a0 | speedometer |

Colour variants are palette swaps of the same art (palettes 4/5/6, §3). For translation the
port can key replacements on the tbl id (all ids are in this document) and draw its own text
into the same rectangle; widths of variable-length names are in the exe tables (station names
`0x4665c4`, landmarks `0x466a20`, message widths in 0x415080).

## 5. Pre-departure sequence (departure task 0x413370) — confirmed

Counter `t` starts at 0 in the segment setup tick; `o` = 180 at station 0 of stages 0–2 and
17–19, else 0 (rules.md §8). HUD events:

| t | Event |
|---|---|
| 1 | start jingle `0x466be0[stage]` (0xc, or 0x1a5 on stages 12–15) |
| 0x30 | voice 0x1018 (first station of stages 0–2/17–19), 0x16d (other stations), 0xd5 (stages 3–16) |
| < 60 | current strip segment blinks (palette 4 entry 0x50 + n, 10-tick period) |
| < 91 | star marks animate |
| o + 20, o + 140, o + 220 | lesson stage (kind 1) station 0: hints 0, 9, 0x13 |
| o + 100 | strip, map and marks removed; signal shows red, mode 2 (出), 100 ticks (`Signal_Show(4, 2, 100, 0)`, from the 6th update); sound 0x23 |
| o + 160 | signal shows the real aspect, mode 2, until 10 m travelled (`Signal_Show(aspect \| 0x100, 2, 0, 10)` looked up with `0x8570c2 = 0`; forced, so the red stays until the new sprite appears 5 updates later; a green releases the pending green-signal hint) |
| o + 200 | banner 出発<aspect> (kind = aspect) at y 224, until 10 m travelled; lamp on; sound 0x2d (bell 0xb stopped if the ATS was tripped) |
| ≥ o + 201 | wait ends (`0x855ff8 &= ~0x800`); departure voice from {0x97, 0xce, 0xcf, 0xd0, 0xd2}[aspect] once the train moves at ≥ 0.04 m/s |

Additions (**confirmed**, 0x413370 / 0x4124f0):

* Every update sets `0x856d0c = 1` (panel active), so the distance readout shows from the first
  pre-departure update; the task returns at once while the time-up popup runs (`& 0x400`).
* Stage 1 station 0x18 (the bonus section) shows hint 0x10 for 0xb4 ticks at `o + 20`.
* The banner at `o + 200` is `StationBanner(name of the start station, aspect | 0x10, 10, −1,
  0xb8)`: distance mode, 10 m; y = 0xb8 + 0x28 = 224. Kind = aspect, so a red starting signal
  would show word 4 (通過), as the code does.
* The setup tick creates the task; its first update is in the next tick (counter 0).
* In the port: `departure_hud.rs` (strip, stars, palette), `cab.rs` (banner, lamp), the rules
  (signals, hints, false start) and `sound/stage.rs` (jingle, voices, bell).

## 6. Submission for the renderer

Three 2D layers are drawn by `Frame_End` 0x4370a0 after the 3D world (renderer.md §11):

### 6.1 Sprite layer — confirmed

Software blit into the locked back buffer (0x430fb0): per slot, per 16×16 tile of the chosen
sub-rect, decode the `cg.rom` tile (types 1/2, `cg::decode_tile`), look up colour
`palette[p][i]`, skip index 0, clip against the mode's width/height, and write the 16-bit pixel.
No blending, no scaling, no filtering, no depth. A faithful port draws each slot as a textured
quad at integer pixel positions of a 640×480 canvas (alpha test on index 0) after the 3D scene,
layers 3 → 0 and slots high → low.

### 6.2 Rect layer — confirmed

`RectLayer_Add(points, n, colour)` 0x431950 appends a fan of `n` (≤ 4) integer points to the list
at 0x816b00 (max 16 fans, count `0x816af8`, reset each tick by 0x431db0). `RectLayer_Render`
0x4319b0 draws each with `DrawPrimitive(TRIANGLEFAN, FVF 0x1c4)`: x, y in native pixels (plus
shake and the 640×480 centring offset), z = 0, rhw = 1, diffuse = the colour (ARGB), texture
NULL, Z test and Z write off (render states 7 and 14 = 0), culling off (22 = 1). It is drawn
**after** the sprite layer, so the speedometer needle is on top of the dial. When the 640×480
2D is centred at a higher resolution, it also draws the four black border quads.

### 6.3 TLVERTEX quads in 496×400 space — confirmed (renderer.md §8)

`Draw2D_FlatQuad` 0x432b50 / `Draw2D_TexQuad` 0x432da0 are drawn by task draw callbacks
before `Frame_End`: sky and ground backgrounds, fades, and weather/cab effects during driving
(wiper shapes 0x424950 / 0x424aa0 / 0x424cb0, drops 0x425040, snow layers 0x425120;
renderer.md §13, confirmed). None of the HUD elements in §3 use them, but the drive task's top mask
(§6.5) is the black panel behind the top HUD row.

### 6.4 Implications for the port

* Draw the HUD in a 640×480 canvas. The original has no anchoring and no wide layout.
* **Enhancement `wide_hud`** (port, decided by the user; only in the `enhanced` HUD
  layout, `--hud-layout`; the `original` layout stays the centred 4:3 at any canvas multiple):
  * The canvas follows the window's shape: it keeps 480 × `ui_scale` pixels of height and is
    widened to the window's aspect (`SpriteLayer::fit_window`, `SpriteLayer::wide`). It is
    mapped to the window with one scale factor for x and y (`overlay::canvas_rect_for`), so
    art keeps square pixels and the speedometer stays round; a 4:3 window gives the normal
    canvas. The 3D projection's original 7.9 % horizontal stretch (`ORIGINAL_STRETCH`) is not
    applied to the 2D layer. Measured on headless shots at `ui_scale` 2: the dial face is
    244 × 244 px at 1280×960, 203 × 203 at 1280×800 and 274 × 274 at 2560×1080.
  * Each slot, widget and fan has an anchor (`sprites::Anchor`). Positions stay in 640×480
    design pixels; on a canvas `wide` pixels wider, `Left` elements are not moved, `Centre`
    ones move by `wide / 2` (the 4:3 design centred) and `Right` ones by `wide`. Only
    positions change; sizes never do. Each group keeps its internal layout.
  * Groups (`hud::anchor_slots`, set every HUD tick): **left** = clocks and their labels
    (0x0a–0x17), high-speed readout and ATC panel (0x75–0x78), mascon, one-handle frame and
    lamp, speedometer and its needle, cab-signal lamp (0x7e–0x82), and the brake gauge when it
    is at the left (one-handle mode); **right** = vehicle picture (0x1e), station plates
    (0x83–0x85), distance panel (0x5a, 0x7c, 0x7d, 0x20–0x24), signs and signal (0x5e–0x60)
    and the brake gauge in two-handle mode; **centre** = everything else: remaining time and
    vehicle name, captions, banners, landmarks, time messages, hints and navigator, ATS lamp,
    popups, the departure strip and the coupling game's screen (cut at the design's edges,
    §3.16).
  * Screens stay a centred 4:3 inside the wide canvas: every tick starts with all anchors
    `Centre`; the driving HUD sets its own, and the slots the stage flow's screens drew (pause,
    station result, continue) are put back to `Centre`. With the HUD hidden (report card,
    ranking, run end), in the ending, opening, bonus game and the front end everything is
    centred. A pause tick runs no HUD task, so it keeps the anchors of the driving frame it
    froze: pausing moves nothing of the HUD, only the pause screen and its legend are added.
  * The wider station plates of `wide_plates` (docs/localisation.md; a translation in
    the enhanced HUD layout) take part of the right group's extra room; the vehicle picture moves
    left with them, and only then, never closer than 4 px to the remaining time (the group
    shrinks uniformly if it would; with the data it never does).
* Palette animation (speedometer digits, route strip) means palettes are per-frame state, not
  constant textures.
* Sprite ids are per slot, and the slot table can be mirrored directly as engine state; the
  rules engine already produces most of the driving state (rules.md §11).

### 6.5 The black panel behind the top HUD (drive mask) — confirmed

While driving, the top **113 pixel rows of 640×480** (virtual rows 0..94) are black, although
the 3D viewport starts at virtual y 52 (pixel 62). The extra black over the world is not a
sprite or a rect-layer fan; it is the first thing the drive task draws:

* **Who.** The drive task's draw callback 0x41e460 (set by 0x41cb00 at `task + 0x10`) starts
  with `Draw2D_FlatQuad(0x486160, 0x469858, 4, 0)` (confirmed, code; scene.md §2.1).
* **Geometry.** 0x486160 = i16 (0,0) (0,94) (496,94) (496,0): a plain rectangle, virtual
  x 0..496, y 0..94, straight lower edge, no art or texture (confirmed, exe bytes). At 640×480
  that is y 0..112.8, so rows 0..=112 are covered (the first world row is 113).
* **Colour.** 0x469858 = four colour words 0 → `0x829d5c[0]`, palette 0 entry 0 (confirmed,
  code); black (confirmed, f144: rows 0..=112 are exactly (0,0,0) in every column).
* **Depth.** Depth word 0 → z = 0 with Z test and write on (renderer.md §6), drawn before
  the main line, scenery and background, so nothing of the world or the sky gradient (which
  starts at virtual y 66) shows through. Under LESSEQUAL the wiper (depth 0, drawn later by its
  own task) can still cover it; drops (2) and snow (0x10+) cannot (likely; ZFUNC is the
  device default).
* **When.** Every rendered frame while the drive task exists: the stage intro, driving, the
  attract demo drives (f132–f230 of `out/orig_opening`: the edge is at row 112/113 in every
  frame checked: f132, f144, f160, f200, f230) and (likely, not checked on a capture) the
  opening's drive view. Not at the
  station-result replay (the drive task is killed, scene.md §8.1). The ending animates the same
  quad's y as its top letterbox bar. Render mode 2 of 0x425680 and the menu model view
  0x41c3c0 draw the same quad too.
* **Port.** `Play::cab_mask` (`crates/shuppatsu/src/play/cab_mask.rs`) adds it to the 3D band's
  effect quads before the weather overlay, from `BackgroundTables::mask_quad` (read from the
  exe). Measured on headless shots: black down to row 225 at 1280×960 (= 2 × 113), 187 at
  1280×800. **Widescreen:** the quad's left and right edges are moved to the visible Hor+ range,
  so the panel fills the full window width at every `ui_scale` (a plain black rectangle,
  so this is filling, not stretching). At 4:3 and 1× it is the original layout.

## Open questions

1. The remaining-time blink below 15 s claimed in rules.md §3: 0x40ff20 only changes the palette.
   Check with the oracle or a screenshot whether anything else blinks the digits.
2. Resolved (audio-text.md §10.4): `0x84c40c` is the distance run since the segment start or the
   last passed station (0x40d620 clears it); approach state 0 → 1 fires 50 m into the leg.
3. The durations passed through `__ftol` (§1.2) are inferred from call sites; confirm with a
   trace of the task fields.
4. Resolution mode 3 (1024×768) uses a different HUD arrangement (e.g. distance digits at
   y 143, signal at (900, 252)) that has not been viewed.
5. Resolved (§3.17): the 20 navigator animations and the per-stage sets. Still open: animations
   7, 9 and 12–19 (tilemaps 0x2af/0x2b1/0x2b6/0x2b7 and 0x185..0x197) have no caller in the
   3000-bandai exe. They may belong to the original Kōsoku-hen or be unused art. The E3
   coupling animation 0x411240 (also slot 0x61, §3.16) is drawn by the port (§3.17); no Wine
   capture of it yet.
6. Failure-cause codes `0x856d84` for the time-up advice (§3.16).
7. Resolved: the float is `0x84c40c` itself; the +2.5 m bump when it reads 50 or 51 makes the
   late/early advice fire once per leg (`Train::leg_marker`, `Rules::advice_hints`).
8. The signal display model (§3.9) comes from the code alone. No trace logs slot 0x60 or
   `0x856d30` yet. Adding the slot-0x60 tilemap to the hook trace would let the replay check the
   5-update delay, the forced handover and the expiry tick against the original.
9. `NaviAnim(0)` (which hides the signal at the obstacle start) does nothing when
   `0x84c0f8 == 1`. The flag is the 3D bottom band (§3.16: the coupling minigame, the
   opening's phase E, the endings), so neither the signal hide nor the navigator reactions
   (§3.17 creation) happen during the minigame. The port skips the reactions under the band
   (`Coupling::bottom_band`); the obstacle hide needs no gate, since 0x40c4e0 does not run
   inside Coupling_Run (nor the engine's drive step in that phase).
   Separately, the obstacle-recovery hold length (and with it the tick of its `(0, 0, 0, 5)`
   green) is a guess (rules.md §12).
10. The continue screen: 0x4080d0 waits for the time-up popup to end (`0x855ff8 & 0x400`,
    about 151 ticks) before `FUN_00415ab0`. The port's stage flow opens the continue screen 22
    ticks after the game over, so it overlaps the sliding 時間切れ banner (seen in a headless
    shot). The flow should wait for `CabHud`'s time-up to finish.
11. Phase flag `0x855ff8 & 0x100`, which also hides captions, banners and landmarks: its
    setter is not identified; the port ignores it.
12. ~~`0x85744c` and the 3D signals~~: wired (`Rules::signal_model`, traffic.md §7).
13. ~~The ATC panel unreachable~~: the bonus sections are modelled (rules.md §7.5). The
    coupling minigame's screen (slots 0x3f–0x4b, captions, timer) is `coupling_hud.rs`.
14. The handling-error hint (slots 0x2e/0x2f) counts ticks only in the train task's driving
    branch and resets when `0x855ff8 & 0x200000` is set; that flag is not identified.
15. Resolved: navigator reactions are drawn (§3.17, `navigator.rs`) and checked against the
    original for set C. Not yet compared with the original: set A and set B, the overrun
    trigger tick, and the emergency-stop signal (obstacle). The EB-held and slam conditions
    come from the rules engine models, whose own thresholds are documented in rules.md.

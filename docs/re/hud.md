# In-cab HUD while driving

Scope: every 2D element shown during a stage, from the pre-departure wait to the arrival and
time-up popups: who draws it, where, with which tilemap and palette, and which game state
drives it. Also how 2D is submitted and where text comes from. Menus and the station-result,
continue and ranking screens are in `menus.md`.

Sources: the Ghidra decomp in `re/decomp/dengo3k/`, plus `re/decomp/dengo3k_labs/` (task
update functions Ghidra had left as `LAB_` labels, decompiled with a throwaway
`DecompAt.java` script into a copy of the project). Constants were read from the exe bytes
(VA − 0x400000 = file offset for `.rdata`/`.data`), and sprite art was identified by looking at
`ddg-tools export tilemaps --palette N` output. Parsers: `crates/ddg-formats/src/hud.rs`.

Confidence: **confirmed** means read from code or bytes. **likely** means a strong inference.
**guess** means it needs an experiment. Nothing here has been checked against the running game.

## 1. How the HUD is built

### 1.1 Sprite slots — confirmed

The HUD is made of **sprite slots**: 207 records of 16 bytes at `0x816f88`, rendered by
`Sprite_RenderLayer` (0x430fb0, renderer.md §11). Slot record:

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
| `Signal_Show` 0x411500 → task 0x411940 | 3 | rules (0x40c010, 0x40a0a0, 0x413370, 0x40e860) | signal aspect sprite, slot 0x60 | confirmed |
| `Sign_Show` 0x411a00 → 0x411d40 / 0x411e00 | 3 | rules (0x40acd0, 0x40b4a0, 0x40c4e0) | gradient / whistle sign (slot 0x5e), speed-limit sign (slot 0x5f) | confirmed |
| `TimeMsg_Show` 0x415080 → 0x415810 | 3 | every bonus / penalty | message + ±seconds, slots 0x28–0x2d, and adds the seconds to pending | confirmed |
| `Hint_Show` 0x411f20 → 0x4121d0 | 3 | rules, course event 9 | navigator portrait + speech balloon, slots 0x34/0x35 | confirmed |
| `StationCaption` 0x410450 → 0x410620 | 3 | approach HUD 0x40c4e0 | small "name 駅 通過/停車", slots 0x36–0x38 | confirmed |
| `StationBanner` 0x410720 → 0x410970 | 3 | 0x40c4e0, 0x413370 | big centred banner, slots 0x39–0x3b | confirmed |
| `Landmark_Show` 0x410a70 → 0x410bd0 | 3 | course event 17 | landmark caption (bridges, rivers, signal stations), slot 0x3c | confirmed |
| `NaviAnim` 0x410cb0 → 0x411080 | 3 | EB, approach, train task, obstacle | navigator reaction animations (slot 0x61) or the emergency-stop signal (slot 0x60) | confirmed (structure) |
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
at every call site) (**likely**).

### 1.3 Flags that gate the HUD — confirmed

| Global | Meaning | Set / cleared |
|---|---|---|
| `0x856c80` | clocks visible | 1 at segment start (0x4080d0), 0 at arrival (0x40a0a0, 0x4080d0) |
| `0x856c6c` | clock freeze ticks: clocks are not redrawn while > 0 | 90 when passing a station (0x40d620) |
| `0x856d1c` | remaining-time readout active | 1 at segment start, 0 on result/continue/ending |
| `0x856d0c` | "driving panel" active (distance readout, ATC panel) | 1 by the departure task, 0 at arrival/results |
| `0x856c70` | distance panel on (1) / "Professional" plate (0) | toggled by the confirm button (bit 0x100000 pressed) while the ATS is idle (0x40c4e0, 0x4080d0) |
| `0x856d5c` | vehicle name/picture shown | 1 at HUD creation |
| `0x856d00` | station captions/banners allowed | 1 when one is created, 0 at arrival |
| `0x855ff8 & 0x100` | hides captions/banners | (phase flag) |
| `0x856d38 ≠ 0` | a time message is up: hides captions, banners and landmarks | |

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
(`0x855ff8 & 2`), the train is waiting or driving (`& 0xa00`), the debug camera is off
(`0x84c0f8 == 0`) and the section is not in ATC mode (`0x856d34 ≠ 2`).

* **Analog**: slot 0x81 = dial 0x54a (0–120 km/h, 128×128) at (16, 345), palette 4, layer 3.
  The needle is **not a sprite**: it is a filled triangle in the rect layer (§6.2), colour
  0xff000000 (opaque black). Model points (4, −20), (0, −58), (−4, −20) are rotated by
  `θ = v·π·0.0125 − π·0.75` (v in km/h, clamped to 0..120; constants 0x47c9e4 = π,
  0x47ca6c = 0.0125, 0x47ca68 = 0.75) about the pivot (80, 409) (0x47ca08 / 0x47ca18):
  `x' = x·cosθ − y·sinθ + 80`, `y' = x·sinθ + y·cosθ + 409`. So 0 km/h points down-left
  (−135°) and 120 km/h down-right (+135°). Speed in km/h = `0x857234 × 3.6` (f64 at 0x459420).
* **Digital**: slot 0x81 = 0x53 (a three-digit seven-segment "888 km/h" panel) at (17, 432). The
  digits are **drawn by palette animation**: palette 4 entries 0xdb..0xef (3 digits × 7
  segments) are set to 0xffffff (lit) or 0x555555 (unlit) from the segment masks at
  0x4676a0 (`{0x77, 0x24, 0x5d, 0x6d, 0x2e, 0x6b, 0x7b, 0x25, 0x7f, 0x6f}`, bit k = segment k).
* **High-speed readout** (`Hud_HighSpeed` 0x416d10): only on stages with
  `0x466ca8[stage] ≠ 0` (stages 0–2, 7–9, 15, 17–19) and with option ≠ 0. In analog mode it shows
  slot 0x75 = 0x53 at (232, 424) once the speed reaches **120 km/h** and hides it again **below
  100 km/h** (hysteresis in `0x856c84`), because the dial ends at 120. In digital mode it stays
  hidden. It is hidden while a navigator hint is up (`0x856d44 ≠ 0`). Same palette trick.

### 3.6 ATC panel (bonus section) — confirmed

When `0x856d34 == 2` (set by `Vehicle_Set(0x1000 | n)` 0x410320, i.e. the ATC run of the
Akita stages; course event 7 sets the ATC speed index `0x856cf4`): slot 0x75 always shows
the digital speed, slot 0x77 = 0x3f "ATC" at (17, 424), slot 0x76 = `0x4b + 0x856cf4` at
(97, 424) (ATC speed readouts 0, 30, 70, 110, 160, 210, 245, 275 km/h; the same numbers are the
table local to 0x416d10), and slot 0x78 at (17, 456) = margin bar `0x4a − clamp(atc − v, 0, 10)`
(tbl 0x40 empty .. 0x4a full red; it fills as you approach the ATC speed). Analog dial
suppressed.

### 3.7 Station plates and route strip — confirmed

* Top right, every tick (0x40f780): slot 0x83 = vertical station-name plate of the segment's
  start station, slot 0x84 = arrow 0x4b8, slot 0x85 = plate of the next **stopping** station.
  Plates are `0x46645c[name_id]` (tbl 0x4b9.. e.g. 秋田, 四ツ小屋). Name id per station:
  stage list `0x4860bc[stage]` (−1 terminated; `hud::HudTables::stage_station_ids`).
* Pre-departure route strip (0x4124f0, removed at departure tick `offset + 100`, §5): slot 0x7a
  = progress strip (発 … 着 arrows, e.g. 0x327) at (144, 136), slot 0x7b = route diagram
  (`0x48606c[stage][n]`, e.g. 0x32a 秋田…大曲) at (144, 184), where n = stations already
  served. The strip's arrows use palette 4 entries 0x50 + i: done segments are set to
  0x00f000 (green, from 0x466b70), the others 0x008080 (olive, from 0x466b10); the current
  segment blinks between the two every 10 ticks for 60 ticks. Some stages also recolour
  entries 0x52.., 0x54.., 0x55, 0x56 from 0x466bd0. Slots 0x3f–0x4e (up to 16) are star marks
  over the diagram at y 144, x from a per-stage offset table built on the stack in 0x4124f0:
  a mark-1 station animates 0x38f..0x394 (silver), a mark-2 station 0x395..0x39a (gold),
  frame `(t / 5) % 6` for t < 91. Marks come from `0x856fe0[]` (rules.md §11: 1 = clean stop,
  2 = on-time clean stop).

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
caption (進行信号, 注意信号 速度45km/h, …). The same sprite is not re-shown (keyed on the tbl id
in the current task). Aspect | 0x100 marks a forced redisplay (departure, red-signal
recovery). Showing aspect 0 also releases a pending green-signal hint (`0x857224`, course event
20) as `Hint_Show(id, 0, 220)`. Callers: sighting in 0x40c010 (distance = the signal's
sight distance, so it disappears when the train passes the signal), the departure task
(§5), and recovery 0x40a0a0.

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
("horn is the B button") for analog levers. Balloon ids (tbl 0x135..0x156), English gist:

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

### 3.13 Station captions and banners — confirmed

The approach state `0x856d68` (0x40c4e0; `f5` = home-signal distance, timetable field 5):

| State | Trigger | Shows |
|---|---|---|
| 0 → 1 | `0x84c40c ≥ 0x4594c8` | caption: slot 0x36 = station name (green, `0x4665c4[id].sprite`) at (80, 112), slot 0x37 = 駅 (0x70) at `x + width`, slot 0x38 = 通過 (0x6c) or 停車 (0x6e) at `x + width + 32`; shown until the train is `f5 + 150` m from the stop |
| 1 → 2 | distance ≤ `f5 + 150` | banner: slot 0x39 = highlighted name (`0x4668ac[id]`), 0x3a = 駅 (0x71), 0x3b = 通過 (0x6d) / 停車 (0x6f); centred, y = 216, for 100 ticks; station announcement 0x41ffe0(2/3, id) |
| 2 → 3 | distance ≤ `f5 + 50` | caption again for 255 m |
| 3 → 4 | distance ≤ `f5 − 10` | (state only) |

Banner centring (0x410720): `x0 = 64 + 256 − (width + wordwidth + 32·(kind ≠ 3)) / 2` with the
word width 128 for kinds 0–3 and 64 for 4–6; name at x0, 駅 at x0 + width, word at
x0 + width + 32. Kind words (`0x465520` highlighted / `0x465500` plain): 0 出発進行
(0x6b/0x6a), 1 出発減速 (0x11d/0x11c), 2 出発注意 (0x11f/0x11e), 3 出発警戒 (0x121/0x120),
4 通過, 5 停車, 6 none. Station-name widths are stored with the sprite id
(`hud::StationNameSprite::width`, 64–192 px).

Landmarks (course event 17, 0x410a70): slot 0x3c = `0x466a20[id]` (秋田車両基地, 第一玉川橋梁,
…, 31 entries `{sprite, width}`, tbl 0x222..0x240), centred, y = 248, shown for `b` metres.

### 3.14 ATS — confirmed

* Lamp task 0x417d10 (slot 0x32, (208, 152), palette 4, layer 3) follows `0x856d08`: 0 →
  0x20e "ATS作動" (ATS tripped), 1 → 0x211 "ATS確認" (acknowledged) for 120 ticks then off,
  −1 → hidden. 0x40c4e0 sets 0 on a trip (states 1→2/4) and 1 when the driver confirms in
  time (rules.md §6).
* Sounds: bell 0xb on trip (stopped with 0x420650(0xb) on confirm), chime 0xd every 32 ticks
  after confirmation.
* On a trip before the stop point, if speed > signal limit + margin, hint id 5 is shown (90
  ticks).

### 3.15 Cab-signal lamp (知らせ灯) — confirmed

Slot 0x82 at (80, 144): 0x54e (dark) / 0x54f (lit), lit when `0x829d16 ≠ 0`. 0x4124f0 clears the
flag at segment setup and the departure task sets it at tick `offset + 200` (doors closed).
Shown while waiting (`0x855ff8 & 0x800`) and for 30 more ticks (`0x829d14`), not counting paused
ticks. Departing before the lamp lights is the false start (message kind 9).

### 3.16 Other popups

| What | Who | Slot, position, tbl | Timing | Conf. |
|---|---|---|---|---|
| 全区間走破 ALL ROUND CLEAR | 0x40a0a0 (final station) | 0x33, (184, 172), 0x20f | delay 10, 100 ticks, sound 0x19b; plus `0x4113e0`: slot 0x62 at (16, 313) 0x459 for 50 ticks then 0x45a (乗務完了 chibi) to tick 100 | confirmed |
| 非常ブレーキ使え | course event 8 (obstacle) | 0x33, (192, 168), 0x24b | delay 10, 90 ticks, bell 0xb | confirmed |
| 時間切れ (time up) | `TimeUp` 0x4158b0 / 0x4159c0 | 0x39, slides in from x 640 to 232 (−10/tick) at y 204, 0x252; at tick 55 an advice sprite 0x3f2..0x3fe at (304, 344) chosen by the failure cause `0x856d84` (0x40ddb0) with voice 0xea..0xfc | 150 ticks; sound 0x11 at tick 35 | confirmed (cause meanings: guess) |
| Coupling minigame captions | 0x4093c0 | 0x33, 0x2d8 ボーナスゲーム / 0x2d9 時間内に衝撃なく列車を連結せよ, positions 0x459190 / 0x4591b0 tables | 70 / 80 ticks | confirmed |
| E3 coupling-ready animation | course event 23, 0x411240 | 0x61, 0x163..0x166 (E3 併合準備), palette 4 entries 0xb0.. from 0x463700 | anim table 0x46622c, sound 0x1029 at frame 60 | confirmed |
| Navigator reactions | 0x410cb0(n), n from `0x45addc/0x45ade0` per stage | 0x61 at (80, 145), animation tables 0x465e04.. `{tbl, frames}` | EB, approach, comfort events | likely |
| 自動運転 | replay playback (`0x816974`) | 0x86 at (403, 40), sub-rect col 6, 6×2 tiles of 0x50b | while replaying | confirmed |

The station-result screen after a stop (停止位置 / m×1秒 / 合格! etc.) and the
continue/game-over screens are in menus.md.

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
| o + 100 | strip, map and marks removed; signal shows red, mode 2 (出), 100 ticks; sound 0x23 |
| o + 160 | signal shows the real aspect, mode 2, until 10 m travelled |
| o + 200 | banner 出発<aspect> (kind = aspect) at y 224, until 10 m travelled; lamp on; sound 0x2d (bell 0xb stopped if the ATS was tripped) |
| ≥ o + 201 | wait ends (`0x855ff8 &= ~0x800`); departure voice from {0x97, 0xce, 0xcf, 0xd0, 0xd2}[aspect] once the train moves at ≥ 0.04 m/s |

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
(rain streaks and wiper shapes 0x424950 / 0x424aa0 / 0x424cb0, drops 0x425040) (effect
meanings: **likely**). None of the HUD elements in §3 use them.

### 6.4 Implications for the port

* Draw the HUD in a 640×480 canvas. For Hor+ widescreen, anchor groups to edges: the left group
  (handles in one-handle mode, speedometer, ATC panel), the right group (brake in two-handle
  mode, plates, signal, signs, distance panel) and the centred group (clocks, remaining time,
  messages, banners, navigator). The original has no anchoring; this is an enhancement.
* Palette animation (speedometer digits, route strip) means palettes are per-frame state, not
  constant textures.
* Sprite ids are per slot, and the slot table can be mirrored directly as engine state; the
  rules engine already produces most of the driving state (rules.md §11).

## Open questions

1. The remaining-time blink below 15 s claimed in rules.md §3: 0x40ff20 only changes the palette.
   Check with the oracle or a screenshot whether anything else blinks the digits.
2. `0x84c40c`, the trigger for approach state 0 → 1 (compared with `0x4594c8`): it is
   incremented in 0x40c4e0 and reset by the train task; its meaning is unknown.
3. The durations passed through `__ftol` (§1.2) are inferred from call sites; confirm with a
   trace of the task fields.
4. Resolution mode 3 (1024×768) uses a different HUD arrangement (e.g. distance digits at
   y 143, signal at (900, 252)) that has not been viewed.
5. Exact navigator animation tables (0x465e04 … 0x466214, 20 sequences) and the
   `0x45addc/0x45ade0` per-stage choices: structure known, per-entry meaning not catalogued.
6. Failure-cause codes `0x856d84` for the time-up advice (§3.16).
7. The float truncated to 50/51 in 0x40c4e0 that times the late/early advice (it also
   advances `0x84c40c`); likely a per-segment timer, not identified.

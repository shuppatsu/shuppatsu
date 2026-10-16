# Input subsystem: dengo3k.exe (3000-bandai, v1.00)

Scope: DirectInput setup, polling, keyboard and joystick decoding into mascon/brake notches
and button bits, calibration (`joystick.dat`), settings (`eeprom.dat`), and replay recording of
input. Sources: Ghidra decompilation in `re/decomp/dengo3k/`, `.data`/`.rdata` bytes read straight
from the exe (file offset = VA − 0x400000), and `objdump` for the few spots Ghidra didn't
decompile. The CD check is out of scope and was not examined.

Confidence tags: **confirmed** means it's read directly from code or data. **likely** means it's
inferred from usage. **guess** means it's a plausible reading that needs an experiment.

## 1. Function table

| Address | Suggested name | Role | Conf. |
|---|---|---|---|
| 0x004332e0 | `input_init` | `DirectInputCreateA(hInst, 0x500)`, QI → `IDirectInputA`, then keyboard init, joystick init, and mode sanity check | confirmed |
| 0x00433440 | `input_init_keyboard` | Creates GUID_SysKeyboard, QI → IDirectInputDevice2A, `c_dfDIKeyboard`, coop level 6 | confirmed |
| 0x00433d60 | `input_init_joystick` | Enumerates joysticks (FF preferred), `c_dfDIJoystick`, coop level 5, axis range ±1000, creates the ramp FF effect, loads `joystick.dat` | confirmed |
| 0x00434160 | `enum_ff_joystick_cb` | Copies guidInstance, sets `g_has_ff` (0x816838)=1, returns DIENUM_STOP | confirmed (objdump) |
| 0x00434190 | `enum_first_guid_cb` | Copies guidInstance or effect GUID, returns DIENUM_STOP (used for EnumDevices and EnumEffects) | confirmed (objdump) |
| 0x004333d0 | `input_poll` | Called once per frame. Polls the keyboard; polls the joystick when mode ≥ 4 and no Shift override; selects the active button word | confirmed |
| 0x004335d0 | `input_poll_keyboard` | GetDeviceState(256), per-key hold counters, keyboard notch logic for modes 0–3, keyboard button word | confirmed |
| 0x004341c0 | `input_poll_joystick` | Poll + GetDeviceState(DIJOYSTATE), digital/analog notch logic for modes 4–6, joystick button word | confirmed |
| 0x00433290 | `input_get_mode` | Returns control mode byte 0x856352 | confirmed |
| 0x00433270 | `input_is_analog_lever` | True when mode is 5 or 6 and no Shift override | confirmed |
| 0x004332a0 | `input_is_one_handle` | True for mode 3 or 5 | confirmed |
| 0x004332c0 | `input_force_notches(p, b)` | Sets power/brake unless analog levers are active (the game calls it with (0, 9) = EB at run start) | confirmed |
| 0x00434a00 | `controller_calibration_screen` | Settings menu entry 4 ("ADJUSTMENT"). Samples 6 mascon and 10 brake positions, writes `joystick.dat` | confirmed |
| 0x00434940 | `ff_play_jolt` | Sets the FF effect gain from the setting (3333/6666/10000) and Start(1). Called through 0x433260 by the bonus game (0x41b810, 0x4241d0; menus.md §12.1) | confirmed |
| 0x00434ed0 | `settings_load` | Reads `eeprom.dat` (0x35a bytes → 0x856020). Clamps mode to 0 if there's no joystick | confirmed |
| 0x00434f50 | `settings_save` | Writes `eeprom.dat` | confirmed |
| 0x0042a130 | `pump_messages_and_poll` | PeekMessage/Dispatch, then `input_poll` (unless 0x816984≠0 and 0x81696c=0), then audio update | confirmed |
| 0x004253c0 | `frame_yield` | Coroutine/task step = one game frame: runs tasks, frame counter 0x84c494++, `pump_messages_and_poll`, `replay_tick` | confirmed |
| 0x00407ac0 | `replay_tick` | Per frame: records an input event or plays one back | confirmed |
| 0x00407a00 | `replay_begin_segment` | At run/station start: restores from the replay if present, otherwise writes the initial event | likely |
| 0x0042e740 | `CReplay::record_input` | counter++; appends an event only when notch/brake/masks change | confirmed |
| 0x0042e7d0 | `CReplay::play_input` | counter++; applies the next event once its counter is reached, otherwise re-applies the current one | confirmed |
| 0x0042bd80 / 0x0042bdd0 | `CReplayEvent::set` / `::get` | Event field writer and reader | confirmed |
| 0x0042b930 | `pause_menu_select` | Menu list. Cursor from up/down edges, or **from the mascon notch** in analog-lever modes | confirmed |
| 0x0042a820 | `options_draw_controller_page` | Shows button legends per mode (and the keyboard legend while Shift is held) | likely |
| 0x00434fb0/0x00434fc0/0x00434fa0 | `lamp_flag_set/clear/reset` | Byte 0x829d16 gates the "J key → brake 0" debug path (§6) | guess |

## 2. DirectInput setup (`input_init`, 0x4332e0)

- `DirectInputCreateA(hInstance=0x816824, DIRECTINPUT_VERSION 0x0500)`, then QueryInterface with the
  GUID at 0x47ef18 (IID_IDirectInputA). Result goes to 0x8298a8. **confirmed**
- **Keyboard** (0x433440): `CreateDevice(GUID_SysKeyboard @0x47f048)`, QI to IID_IDirectInputDevice2A
  (0x47ef58), `SetDataFormat(c_dfDIKeyboard @0x4449f0)` (256-byte format),
  `SetCooperativeLevel(hwnd 0x81682c, 6 = DISCL_NONEXCLUSIVE|DISCL_FOREGROUND)`. Device ptr 0x8297a0.
  **confirmed**
- **Joystick** (0x433d60): **confirmed**
  1. `EnumDevices(DIDEVTYPE_JOYSTICK, enum_first_guid_cb, &guid, DIEDFL_ATTACHEDONLY)` picks the first
     attached joystick.
  2. `EnumDevices(DIDEVTYPE_JOYSTICK, enum_ff_joystick_cb, &guid, ATTACHEDONLY|FORCEFEEDBACK 0x101)`
     overrides it with the first FF joystick, if there is one, and sets `g_has_ff` (0x816838).
  3. If the GUID is still all-zero, there's no joystick (0x8298a4 = NULL).
  4. `CreateDevice` → QI Device2A, then `SetDataFormat(c_dfDIJoystick @0x444cd0)`, which is DIJOYSTATE (0x50 bytes)
     with DIDF_ABSAXIS.
  5. `SetCooperativeLevel(hwnd, 5 = DISCL_EXCLUSIVE|DISCL_FOREGROUND)`.
  6. `SetProperty(DIPROP_RANGE, DIPH_BYOFFSET)` for X (offset 0) and Y (offset 4): **−1000..+1000**.
     The game sets no dead zone or saturation.
  7. `EnumEffects(cb, &guid, DIEFT_RAMPFORCE=2)`, then `CreateEffect` with a DIEFFECT: flags
     OBJECTOFFSETS|CARTESIAN, duration 100 000 µs, gain 10000, no trigger, axes {X,Y}, direction
     {1,0}, and a ramp from −10000 to +10000. Pointer at 0x82979c.
  8. If `joystick.dat` opens ("rb"), it reads 24 bytes into 0x48c434 and 44 bytes into 0x48c44c (§5).
- After init, if there's no joystick and mode > 3, the mode is reset to 0 (keyboard). **confirmed**
- Access is **immediate mode only** (`GetDeviceState`). The game never sets a buffer size and never calls
  `GetDeviceData`. **confirmed**
- On a failed GetDeviceState it calls `Acquire()` once and retries. A second failure skips the device for
  that frame. The joystick also calls `Poll()` (vtbl +0x64) before each read. **confirmed**

## 3. Polling cadence

- `input_poll` (0x4333d0) is called from `pump_messages_and_poll` (0x42a130), which is called by
  `frame_yield` (0x4253c0) once per game frame. All blocking loops (menus, movies at 0x428f00,
  calibration) also spin on `frame_yield` or `pump_messages_and_poll`. **confirmed**
- So input is sampled **once per game frame**. The frame period comes from the main loop/timer (not
  covered here; see the tick-rate notes). All repeat, debounce and replay counters below count **frames**.
  **confirmed** (the frame-based counting); the rate in ms is set outside this topic.
- Order within a frame: keyboard first, then joystick. The joystick overwrites the shared notch
  variables and edge masks when it's active. **confirmed**

## 4. Control modes (byte 0x856352, settings item 0)

The mode indexes a 7×14-byte binding table at **0x47c980** (stride 14, hex dump below).
Bytes [0..3] are power-up, power-down, brake-up and brake-down. [4..7] are menu up, down, left and right.
[8..13] are button sources (§6). In modes 0–3 the values are DIK scan codes. In modes 4–6 they are
DIJOYSTATE button indices. 0xFF means unbound. **confirmed**

```
0x47c980 m0: d0 c8 cd cb | c8 d0 cb cd | 1c 39 1c 01 | 01 13
0x47c98e m1: 2c 1e cd cb | c8 d0 cb cd | 1c 39 1c 01 | 01 13
0x47c99c m2: ff ff ff ff | c8 d0 cb cd | 1c 39 1c 01 | 01 13
0x47c9aa m3: d0 c8 ff ff | c8 d0 cb cd | 1c 39 1c 01 | 01 13
0x47c9b8 m4: 11 10 13 12 | 10 11 12 13 | 00 01 00 01 | 02 13
0x47c9c6 m5: ff ff ff ff | ff ff ff ff | 00 01 00 01 | 02 13
0x47c9d4 m6: ff ff ff ff | ff ff ff ff | 01 00 01 00 | 02 13
```

Quirk (**confirmed** via objdump at 0x433b25/0x433bf4): the keyboard routine reads bytes [4..13] at
**absolute** addresses (always the mode-0 row) and only indexes [0..3] by mode. The joystick routine
indexes every byte by mode.

| Mode | Device | Levers | Conf. |
|---|---|---|---|
| 0 | keyboard | two-handle, arrows: ↓ = power+, ↑ = power−, → = brake+, ← = brake− | confirmed |
| 1 | keyboard | two-handle: Z = power+, A = power−, → = brake+, ← = brake− | confirmed |
| 2 | keyboard | direct: hold 1–5 = P1–P5 (nothing held = N); numpad 0–8 = B0–B8, numpad 9 = brake code 10 | confirmed |
| 3 | keyboard | one-handle: ↓ = toward power, ↑ = toward brake | confirmed |
| 4 | joystick, digital | stick as d-pad: Y+ = power+, Y− = power−, X+ = brake+, X− = brake− | confirmed |
| 5 | joystick, analog one-handle | single Y axis: positive = power, negative = brake (fixed thresholds) | confirmed |
| 6 | joystick, analog two-handle (DDG controller) | Y = mascon, X = brake, nearest calibrated position | confirmed |

The mode 6 = official DDG gameport controller pairing is **likely**: two axes match the INF, and only
mode 6 uses the calibration file.

### 4.1 Notch state

- `g_power` **0x829d00** (int): 0..5 = N, P1..P5. **confirmed**
- `g_brake` **0x829d04** (int): 0 = released, 1..8 = B1..B8, **9 = EB** (the highest value keyboard
  modes 0/1/3 reach, and the 10th calibrated position). **10** is produced only by mode 2 (numpad 9) and
  mode 5 (Y < −950). The cab brake sprite table at 0x46540c maps 0..10 → sprites 9..18, with 9 and 10
  **both** on sprite 18, so 10 is shown as EB. **confirmed** (values); meaning of 10 vs 9 in physics:
  **guess** (EB variant or handle-removed). This needs the physics note.
- Mascon sprite table at 0x46543c: P0..P5 → 0x50d..0x512. **confirmed**
- At each run/station start, digital modes force (P0, EB=9) via 0x4332c0. Analog modes keep the
  physical lever and wait on a game flag (0x4080d0 → 0x4137e0). **confirmed** (forcing), **guess**
  (that the wait is a "set the handle to EB" prompt).
- At the title, **EB (9) + P3** held together sets a flag passed to 0x435490 (flag 0x40000 context). This is an
  easter-egg or hidden-option trigger. **likely** (purpose unknown).

### 4.2 Key repeat and edges (keyboard modes 0/1/3, joystick mode 4)

Per-key `u16` hold counters: keyboard at 0x829b00[256] (DIK index), joystick buttons at
0x829900[32]. A counter is 0 when released and increments every frame while held. **confirmed**

A step fires when `count == 1` (press) **or** `count > 8 && count % 3 == 0`, i.e. on the press, then
on frames 9, 12, 15, … while held. **confirmed**

Limits:
- Power+: up to 5. Power−: down to 0. Repeat applies to both. **confirmed**
- Brake+ (two-handle): press reaches 9 (EB), repeat stops at **8**, so holding never slides into EB.
  Brake− goes down to 0. **confirmed**
- One-handle (mode 3), toward-power: if brake > 0 → brake−1 and power=0; else power+1 up to 5.
  **Repeat won't leave neutral**: from N it needs a fresh press. Toward-brake: if power > 0 →
  power−1 and brake=0; else brake+1 up to 9 on a press. Repeat won't leave N and stops at 8. So N and EB are
  detents. **confirmed**
- Mode 2: power is recomputed every frame from held digits (lowest held digit wins). Brake changes on the
  numpad press frame. **confirmed**

### 4.3 Mode 4 (digital joystick)

Axes become virtual buttons in DIJOYSTATE.rgbButtons: Y < −499 → btn16, Y > 499 → btn17,
X < −499 → btn18, X > 499 → btn19. After that the same repeat logic runs as for the keyboard. **confirmed**

### 4.4 Mode 5 (analog one-handle, Y axis, fixed thresholds)

| Y range | Result |
|---|---|
| ≥ 901 | P5 |
| 701..900 | P4 |
| 501..700 | P3 |
| 301..500 | P2 |
| 101..300 | P1 |
| −50..100 | N (B0) |
| −150..−51 | B1 |
| −250..−151 | B2 |
| −350..−251 | B3 |
| −450..−351 | B4 |
| −550..−451 | B5 |
| −650..−551 | B6 |
| −750..−651 | B7 |
| −850..−751 | B8 |
| −950..−851 | 9 (EB) |
| < −950 | 10 |

The thresholds are code constants in 0x4341c0. The power side always forces brake=0 and vice versa. **confirmed**

### 4.5 Mode 6 (two-handle, calibrated)

- **Mascon** from **Y**: updated only while `Y < 951`, otherwise it holds the previous value. It picks the index
  of the nearest of the 6 calibration values at 0x48c434 (P0..P5). A tie keeps the lower index. **confirmed**
  (why 951 is the gate is a **guess**: an axis pegged at +1000 means a disconnected or unread gameport.)
- **Brake** from **X**: updated only while `X ≤ cal_brake[10] + 50`. It picks the nearest of 11 values at
  0x48c44c (B0..B8, 9, 10). **EB debounce:** when the nearest is 9, counter 0x829d0c must exceed 5
  (6 consecutive frames) before brake becomes 9. Until then the previous brake is held. Any other
  position resets the counter and applies immediately. **confirmed**
- Because calibration writes `cal[10] = cal[9]` and ties favour the lower index, brake 10 is
  unreachable in mode 6 with game-written calibration. **confirmed**
- Default calibration (exe .data, used when `joystick.dat` is absent). **confirmed**:
  - mascon Y: `0, −102, −310, −480, −730, −1000`
  - brake X: `0, 39, 116, 165, 235, 285, 340, 366, 392, 426, 426`

  So power runs toward −Y and brake toward +X, using only about a quarter of the X range. This fits a gameport
  resistive lever after the driver's own scaling. **likely**

## 5. Calibration ("ADJUSTMENT") and persistent settings

- `controller_calibration_screen` 0x434a00 (settings menu 0x42a1c0, case 4). **confirmed**
  - Mascon pass: 6 prompts (sprites 0x4b1..0x4b6). Each waits for a confirm edge (bit 0x1000) and stores
    the current Y. A cancel edge (0x2000) aborts without saving.
  - Brake pass: 10 prompts (sprites 0x4a7..0x4b0), storing X. Entry 10 duplicates entry 9.
  - Sound 0x17 plays per sample and 0x13 on abort. **likely** (ids passed to 0x41fda0).
  - Writes **`joystick.dat`** ("wb", relative path = current directory): **24 bytes = 6×i32 LE mascon Y**,
    then **44 bytes = 11×i32 LE brake X**. Total 68 bytes, no header. **confirmed**
  - The raw axes (0x8298b0/b4) are only refreshed by the joystick poll, which runs only in modes 4–6 without
    Shift. In keyboard modes the screen samples stale values. **confirmed**
- Neither the registry nor an INI file holds input settings. **confirmed** for the functions above.
  Everything is in `joystick.dat` and **`eeprom.dat`** (0x35a bytes, mirrored at 0x856020, loaded at
  0x434ed0 and saved at 0x434f50). Input-related bytes. **confirmed**:
  - file +0x332 (0x856352): control mode 0..6
  - file +0x333 (0x856353): FF strength 0..3 (0 off, gains 3333/6666/10000). Only offered if `g_has_ff`.
  - +0x334, +0x335: settings items 4/5 (0x42a460). Item 5 changes HUD sprites (0x40f7e0). **likely**
  - +0x006 bits 14–15 → 0x829d10 (settings item 1; difficulty-like index used across the game). **likely**

## 6. Button word (PS1-pad style, active-low)

After each poll the game builds a 32-bit **held** mask `C` (bits set = pressed) and stores three
active-low words. Bit **clear** = true. **confirmed** (0x4335d0 tail, 0x4341c0 tail):

- `0x48c420` keyboard held (`~C_kb`), `0x48c424` joystick held (`~C_joy`)
- `0x48c428` **active held** = keyboard or joystick word (joystick when mode ≥ 4 and no Shift)
- `0x48c42c` **pressed this frame** = `~(C & ~C_prev)`
- `0x48c430` **released this frame** = `~(~C & C_prev)`

Bit sources (`[n]` = binding-table byte):

| Bit | Source | Mode 0–3 key | Mode 4/5 joy | Mode 6 joy | Use in game | Conf. |
|---|---|---|---|---|---|---|
| 0x10 | [4] repeat-pulse | ↑ | btn16 (Y−) | – | menu cursor up | confirmed |
| 0x20 | [5] repeat-pulse | ↓ | btn17 (Y+) | – | menu cursor down | confirmed |
| 0x80 | [6] repeat-pulse | ← | btn18 | – | menu left | likely |
| 0x40 | [7] repeat-pulse | → | btn19 | – | menu right | likely |
| 0x01/0x02/0x04/0x08 | [4..7] held | ↑ ↓ ← → | same | – | held directions | confirmed |
| 0x100000 | [8] held | Enter | btn0 | btn1 | driving: toggles HUD overlay (0x856c70) and acts at stop/door events (0x40c4e0); title skip | likely |
| 0x100 | [9] held | Space | btn1 | btn0 | driving: **horn** (held/press logic 0x40c4e0 L264, 0x40acd0) | likely |
| 0x1000 | [10] held | Enter | btn0 | btn1 | menu **confirm**, movie skip | confirmed |
| 0x2000 | [11] held | Esc | btn1 | btn0 | menu **cancel** | confirmed |
| 0x800000 | [12] held | Esc | btn2 | btn2 | **pause** (0x4080d0 → 0x42b7f0), also skips replay | confirmed |
| 0x4000 | [13] press only (kbd) | R | – | – | no reader found | confirmed (no use) |

- In joystick mode the keyboard still contributes: Esc → 0x802000 (pause + cancel), R → 0x4000,
  Enter → 0x1000. **confirmed**
- Bits 0x200, 0x400 and 0x8000000 are tested (0x425680, 0x4253c0) but never produced. They are arcade
  leftovers. **likely**
- **Shift override:** LShift (DIK 0x2a) or RShift (0x36) held → flag 0x829d08. The joystick poll is then skipped
  and the keyboard word becomes active. This is only allowed outside driving or while paused
  (`(0x855ff8 & 2)==0 || (0x855ffc & 4)`). **confirmed**
- **Debug letter bitmask** 0x816828: Q..P → bits 0..9, A..K → bits 8..15 (overlapping). Its only
  reader: **J** (bit 14) held while byte 0x829d16 is set forces brake 0 (0x4333d0). 0x829d16 is set or
  cleared by scripted events (0x40e860, 0x422910) and drives a lamp sprite (0x434fd0). **guess**:
  ATS/EB-reset debug aid.
- Pause menu (0x42b930): in analog-lever modes the cursor follows the **mascon notch** (moves at most
  one step per frame toward `g_power`). Otherwise it follows the 0x10/0x20 edges. **confirmed**

### 6.1 Key-mapping summary (keyboard)

| Action | Mode 0 | Mode 1 | Mode 2 | Mode 3 |
|---|---|---|---|---|
| Power + | ↓ (DIK_DOWN 0xd0) | Z (0x2c) | hold 1..5 (0x02..0x06) = P1..P5 | ↓ toward power |
| Power − | ↑ (0xc8) | A (0x1e) | release digits | ↑ toward brake |
| Brake + | → (0xcd) | → | Numpad1..8 = B1..B8, Numpad9 = 10 | ↑ (past N) |
| Brake − | ← (0xcb) | ← | Numpad0 = release | ↓ (past N) |
| Horn | Space (0x39) | Space | Space | Space |
| A / confirm | Enter (0x1c) | Enter | Enter | Enter |
| Cancel | Esc (0x01) | Esc | Esc | Esc |
| Pause | Esc | Esc | Esc | Esc |
| Menu nav | arrows | arrows | arrows | arrows |
| Force keyboard in menus | Shift | Shift | Shift | Shift |

Numpad DIKs: 0=0x52, 1=0x4f, 2=0x50, 3=0x51, 4=0x4b, 5=0x4c, 6=0x4d, 7=0x47, 8=0x48, 9=0x49.
There's no dedicated EB key in modes 0/1/3. EB is reached with one more press after B8. **confirmed**

## 7. Replays (CReplayEvent)

- `replay_tick` 0x407ac0 runs every frame from `frame_yield` (not while paused). It's gated on the replay
  manager being active (0x816980). With 0x81696c=0 it either records (0x816968=0) or plays back. **confirmed**
  (which flag means "demo/attract" vs "replay" is a **likely** reading).
- Event layout (0x42bd80). **confirmed** offsets, field names from the debug strings at 0x48bc80..:
  `+0x08 m_bReady (u8)`, `+0x0c m_nCounter (i32, input frame count)`,
  `+0x10 f32 = 0x85708c` (likely `m_fDist`), `+0x14 m_nTime = frame counter 0x84c494`,
  `+0x18 f32 = 0x857234` (likely `m_fSpeed`), `+0x1c m_nNoch (i8)`, `+0x1d m_nBrake (i8)`,
  `+0x20 m_nInpush`, `+0x24 m_nInpull`, `+0x28 m_nInbuf` (u32 active-low words).
- **Recording** (0x42e740) is delta-encoded: counter++ each frame. A new event is emitted only when
  notch, brake or any of the three masks differ from the last event. Masks are reduced to bits
  **0x100100** (A button + horn) before storing. Every other bit is stored as released. **confirmed**
- **Playback** (0x42e7d0) overwrites `g_power`, `g_brake` and all three words every frame from the
  current event, and advances when `event.counter ≤ counter`. A real Esc press during playback is
  passed through (bit 0x800000 cleared) so the player can quit. **confirmed**
- Segment start (0x407a00): if a replay exists for the station, the input and time are restored from it,
  otherwise an initial event is written. **likely**

Implication for the reimplementation: power and brake are **integer notch indices captured after
decoding**. Analog calibration and repeat are never replayed. Replays are deterministic only if
physics consumes the input at the same frame cadence.

## 8. Notes for ddg-engine / ddg-game

- Model input as `{power: 0..=5, brake: 0..=10, held/pressed/released: u32}` sampled once per sim
  frame. Apply the repeat rule (press, then every 3rd frame from frame 9) and the EB rules (no repeat
  into 9; mode 6 EB needs 6 frames).
- Arcade-faithful keyboard defaults = mode 0. The binding table, thresholds and default calibration can
  be read from the exe at runtime (0x47c980, 0x48c434..0x48c477) instead of hardcoding them.
- Real DDG controllers: the PC gameport version shows up as 2 axes. Modern USB DDG controllers
  (PS1/PS2 via adapter, or the 2000s USB TCPP) are **not** handled by this exe. Our engine
  will need its own decoders that output the same notch integers. The mode 6 nearest-point method and
  `joystick.dat` format are the faithful path for axis-style devices.

## 9. Open questions

1. What do brake codes 9 and 10 mean in physics: EB vs "handle removed"? Is 10 ever distinct?
   That needs the train-physics note (who reads 0x829d04; no direct reader outside input/replay/HUD was
   found).
2. Mode 6 reads button 2 for pause, but the INF declares only 2 buttons. Does DGOC44.DLL report a third
   (START)? This needs the driver or a real device.
3. ~~Is the FF effect ever triggered?~~ Yes: 0x433260 is called by the bonus game (0x41b810 on
   contact, 0x4241d0 on the rail joints). The port has no force feedback.
4. Frame rate (ms per poll) is outside this note.
5. What does the J-key/0x829d16 path do, and should it be kept?

## 10. Port notes (ddg-game)

Implementation: `crates/ddg-game/src/input.rs` (keyboard, shared button word) and
`crates/ddg-game/src/gamepad.rs` (joystick modes, calibration, modern controllers).

### 10.1 What is read from the exe at runtime

- Binding table rows 0–6 (0x47c980). The keyboard always uses the mode-0 button slots [4..13]
  (the absolute-address quirk of §4). The joystick uses its own row.
- Default calibration: 17 × i32 at 0x48c434 (6 mascon, 11 brake).
- The 0x4341c0 thresholds are taken from the instruction immediates, and the opcode bytes are
  checked first, so a different exe fails loudly instead of decoding garbage. **confirmed**
  (`gamepad::tests::tables_from_exe` matches §4.3–4.5):

| Constant | Instruction | Value |
|---|---|---|
| mode 4 X−/X+ | 0x43425f `cmp eax,imm32; jg`, 0x43426f `cmp eax,imm32; jl` | −500 / 500 (axis ≤ −500 or ≥ 500) |
| mode 4 Y−/Y+ | 0x434282, 0x434292 | −500 / 500 |
| mode 5 brake chain (B10..B1) | 0x434442 … 0x43450d (`3d` imm32), 0x434527 (`83 f8` imm8) | −950, −850, …, −150, −50 (`y < limit`) |
| mode 5 power chain (P5..P1) | 0x43453e … 0x43458e, 0x4345a8 (imm8) | 900, 700, 500, 300, 100 (`y > limit`) |
| mode 6 mascon gate | 0x43431c `cmp ebx,imm32; jle` | 950 (decode while y ≤ 950) |
| mode 6 brake margin | 0x43436e `add eax,imm8` | 50 (decode while x ≤ cal[10]+50) |
| mode 6 EB debounce | 0x4343d6 `cmp eax,imm8; jg` | 5 (EB after the 6th poll) |

The mode 5 split between the brake and power chains is `y < 0` (0x43443a, `cmp eax,edx` with
edx = 0). The repeat rule (press, then from frame 9 every 3rd frame) is the same code pattern
repeated inline. It is kept as a constant in `input::repeat_pulse`.

### 10.2 Behaviour reproduced

- Mode 4: the axes set virtual buttons 16–19, OR'd over the device buttons. The levers and the menu
  pulses then run through the keyboard repeat rules, including "press reaches EB, repeat stops at B8".
- Mode 5: Y < 0 picks the brake chain with power = 0, otherwise the power chain with brake = 0.
- Mode 6: nearest calibrated value, with ties going to the lower index. Each handle has its gate and
  holds its value outside it. EB needs 6 consecutive polls (the counter clamps to 5 after applying),
  and any other position applies at once and resets the counter. Code 10 is unreachable with a
  game-written calibration.
- Station start: P0/EB is forced only in digital modes. Modes 5 and 6 keep the handle (0x4332c0).
- Button word: per-device edges (pressed = held & ¬prev). In joystick modes the keyboard still
  contributes Esc → pause+cancel, Enter → menu confirm (0x1000 only, so **not** the driving A
  button 0x100000), and R → 0x4000. Keyboard Space does **not** horn in joystick modes.
- Shift override: `Controls::poll(down, shift_allowed)`. Shift skips the joystick poll and the
  keyboard word becomes active. The levers hold.
- Mode fallback: if a joystick mode is selected with no gamepad connected at start, the port drops
  to keyboard mode 0, as `input_init` does.
- Calibration (`Calibrator`): 6 mascon Y samples, then 10 brake X samples, each on a menu-confirm
  edge. Confirm wins over cancel in the same tick, and cancel aborts. cal[10] = cal[9].
  `joystick.dat` is the same 68-byte LE image. A short file only overlays the whole i32s it contains
  (like the two `fread`s into the live table).
- `ButtonState` now also carries `menu_confirm_pressed` (0x1000), `pause_pressed` (0x800000)
  and the whole `word` (held/pressed/released, active-high, bits in `input::bits`).

### 10.3 Port choices (deviations, each a candidate config option)

| Choice | Original | Port | Why |
|---|---|---|---|
| Unplugged mid-run | GetDeviceState fails twice, so the stale word is kept, edges included | levers hold, buttons release | a stale "pressed" edge would re-fire every tick |
| Calibration file location | current directory | caller passes the path (`--joystick-dat`) | no writes into the install dir |
| gilrs filters | DirectInput, no dead zone | gilrs default filters off (no dead zone or jitter filter) | keeps the thresholds meaningful |
| FF ramp effect | created, never started (§1 0x434940 has no callers) | not implemented | dead code in the original |

### 10.4 Modern controllers (enhancements, all opt-in)

**Generic gamepad (`PadProfile::Generic`)**: produces a `DIJOYSTATE`-like `JoyState`:
- X/Y come from the left stick, scaled to ±1000. +Y is stick **back**, as in DirectInput, so back means power+
  in mode 4 and the power side in mode 5, like the arcade one-handle lever. The d-pad (buttons or
  hat axes) overrides the stick with ±1000.
- Face buttons are placed on the DirectInput index that the mode's binding row names, so roles
  are the same in every mode: **South (A/✕) = A + menu confirm, East (B/○) = horn + cancel
  (shared, as in the original), Start = pause.**
- In mode 6, a USB lever device that reports its handles as two plain axes (gameport-style) works
  through the faithful nearest-calibrated path after `ddg-game pad calibrate`.
- Device choice: the first gamepad with force feedback, otherwise the first one (like 0x433d60).
  Hot-plug re-picks.

**Button-combination controllers (`PadProfile::Combo`)**: the PS1/PS2 Densha de GO! two-handle
controllers (and similar), seen through a PS→USB adapter, report notch positions as **combinations of
buttons/d-pad** rather than axes. The local research came up with:
- gilrs' bundled SDL_GameControllerDB has no Densha de GO! entries. The only Taito entries are Egret II.
- The hwdata `usb.ids` lists Taito Corp. as VID **0x0ae4**, with no product IDs.
- No offline source gives the combination tables, so **no built-in table is shipped**. Any table
  written from memory would be unverified.

Instead the port **learns** the table: `ddg-game pad learn --combo <file>` records the set of
active raw inputs at P0..P5 (brake handle left alone) and at B0..EB (power handle left alone).
Raw inputs are gilrs button codes, plus axis codes past ±0.5 for hat-style d-pads. An input
belongs to a handle if it changes across that handle's positions. Inputs that never change are
ignored. When decoding, an exact match on the handle's inputs gives the notch. An unknown pattern
(mid-transition) holds the handle. The notches go through mode 6, so the EB debounce and the
"analog lever" behaviour (no forced EB at station start, pause cursor follows the mascon) apply.
The file format is text, starting with `ddg-combo 1`, then `power <n> <hex inputs…>` and `brake <n> …`.

**Needs hardware**: everything in 10.4, and the generic mapping on a real pad (stick sign
conventions per gilrs backend). It also needs: whether a USB one-handle controller (for example the Switch-era
ones) reports its lever as an axis compatible with mode 5's fixed thresholds or needs a calibrated
one-handle mode (not implemented, open), and the button mapping for the combination pads
(currently South/East/Start, excluding any input a handle uses).

### 10.5 Tools

`ddg-game pad monitor|calibrate|learn [--mode 4..6] [--joystick-dat F] [--combo F]`.
`monitor` prints notch and word changes (Enter = station-start reset). `calibrate` runs the
ADJUSTMENT sequence with Enter as the confirm and writes `joystick.dat`. `learn` records a combination table.

# dengo3k.exe: menus and game-flow screens

Scope: every 2D screen outside the in-cab HUD (that is `hud.md`): title menu, options, records,
route/stage select, the pre-run screens, pause, the station result, continue/game over, the run
report card, ranking and name entry, the bonus game, attract, and where the save data lives.
Sources: the Ghidra decomp in `re/decomp/dengo3k/`, task-update functions that Ghidra left as labels
(decompiled separately by a headless script into `re/decomp/dengo3k_labs/<addr>.c`), `objdump` of
`BIN/dengo3k.exe`, and contact sheets of the `tbl.rom` maps (`ddg-tools export tilemaps`, palette 4)
to identify the art. VA = file offset + 0x400000 for `.rdata`/`.data`.

Confidence: **confirmed** = read from code or bytes; **likely** = strong inference; **guess** =
plausible, needs a run of the original. Nothing here has been checked against the running game.

Strings are named by `tbl.rom` index (`tbl 0x513`) or by a sub-rectangle of one (see §1.2). A short
English gloss is given where it helps; no screen text exists as character strings (audio-text.md §7).

## Function table

| Address | Suggested name | Role | Conf. |
|---|---|---|---|
| 0x42a1c0 | `TitleMenu_Run` | Title menu loop and dispatch of its result | confirmed |
| 0x42b210 / 0x42b2f0 / 0x42b530 | `TitleMenu_Create` / `_Update` / `_Draw` | Title menu task (list 3) | confirmed |
| 0x42a520 / 0x42a560 / 0x42a730 | `OptionsMenu_Create` / `_Update` / `_Draw` | Options list (6 items) | confirmed |
| 0x42abb0 / 0x42ac00 / 0x42ad10 | `OptionsControl_*` | Value page for item 0 (control mode), with controller legend 0x42a820 | confirmed |
| 0x42afa0 / 0x42b020 / 0x42b180 | `OptionsValue_*` | Value page for items 1–5 | confirmed |
| 0x42a3f0 / 0x42a460 | `Options_Get` / `Options_Set` | Read / write the six settings | confirmed |
| 0x41f5e0 / 0x41f910 / 0x41fc60 / 0x41fd00 | `Records_Create` / `_Update` / `_Draw` / `_Destroy` | Ranking viewer (title item 2 and attract) | confirmed |
| 0x41e520 | `Ranking_FromEeprom` | Unpacks the eeprom ranking into the display table; sets `0x8500b0 = 0x3f9da` | confirmed |
| 0x41e670 | `Ranking_ToEeprom` | Packs one ranking table back into the eeprom mirror | confirmed |
| 0x41e7a0 / 0x41e4c0 | `Name_Filter` / `Name_Replace` | Replaces names from three lists with a per-route default | likely |
| 0x41e8d0 / 0x41ebe0 / 0x41f5a0 | `NameEntry_Create` / `_Update` / `_Destroy` | Inserts the score, name entry, then the ranking page | confirmed |
| 0x434a00 | `ControllerAdjust_Run` | Calibration screen (input.md §5) | confirmed |
| 0x4122d0 / 0x4123c0 / 0x412490 | `PressStart_*` | Arcade "press START" screen; a no-op on PC | confirmed |
| 0x435490 / 0x435510 / 0x436210 / 0x436640 | `RouteSelect_Create` / `_Update` / `_DrawHeader` / `_Destroy` | Line select → train select | confirmed |
| 0x4137e0 / 0x413900 / 0x413940 | `LeverCheck_*` | "仕業検査" lever-position prompt, analog-lever modes only | confirmed |
| 0x4139c0 / 0x413b10 / 0x413d30 | `DepartureBoard_*` | Intro: departure-board roulette | confirmed |
| 0x42b7f0 | `PauseMenu_Run` | Pause dialog sequence | confirmed |
| 0x42b930 | `Dialog_Run` | Generic blocking list dialog (pause, confirmations, exit) | confirmed |
| 0x42bb90 | `ExitConfirm_Run` | "終了しますか?" yes/no | likely (return value) |
| 0x413d90 / 0x413f30 / 0x4148b0 | `StationResult_*` | Per-station stop evaluation screen | confirmed |
| 0x415ab0 / 0x415ba0 / 0x415e80 | `Continue_*` | Continue countdown | confirmed |
| 0x4149d0 / 0x414b70 / 0x415050 | `ReportCard_*` | 運転評価 five-part grade screen | confirmed |
| 0x415f00 / 0x416270 / 0x4162a0 | `RunEnd_*` | 運転中止 / 運転終了 banner | confirmed |
| 0x41b540 / 0x41b810 / 0x41c2e0 | `BonusGame_*` | Coupling bonus game (EF63) | likely (purpose from art) |
| 0x4040a0 | `Ending_Run` | Ending after clearing a 4–5 star stage | guess (contents not traced) |
| 0x402700 | `Attract_Run` | Attract sequencer over 0x484e30 (main-loop.md §5) | confirmed |
| 0x401910 | `Credits_Init` | Sets `0x8573e0 = 1` (free play) | confirmed |
| 0x41fda0 | `Sfx_Play` | Plays a sound code if sound is on (`0x816ae8`); bit 0x1000 → jingle sequence 0x41fdd0 | confirmed |
| 0x430100 / 0x430130 | `Snd_Play` / `Snd_Stop` | Thread message 0x464 / 0x465 (wrappers 0x42ff20 play, 0x420630/0x420650 stop) | confirmed |
| 0x434ed0 / 0x434f50 | `Eeprom_Load` / `Eeprom_Save` | `eeprom.dat` (input.md §5) | confirmed |

## 1. Common machinery

### 1.1 Screens are tasks plus a blocking wait — confirmed

Every screen follows the pattern of main-loop.md §2: a creator allocates a task in list 3
(`0x84c1a4`; records and attract use list 4), places sprites, sets a bit in a flag word, and the
caller spins `while (flag) Task_Tick();`. The task's destroy callback clears the bit and the
sprites. Two exceptions block inside the call instead: `Dialog_Run` 0x42b930 and the calibration
screen 0x434a00 call `Task_Tick` themselves.

| Wait flag | Screen |
|---|---|
| `0x855ff8 & 0x100000` | route select |
| `0x855ff8 & 0x4000000` | lever check |
| `0x855ff8 & 0x1000` | departure board |
| `0x855ff8 & 0x40` | station result, report card |
| `0x855ff8 & 0x10` | continue |
| `0x855ff8 & 0x2000` | name entry + ranking, records viewer |
| `0x855ff8 & 0x20` | run-end banner |
| `0x855ff8 & 0x4000` | bonus game |
| `0x855ff8 & 0x80000` | press START |
| `0x816834 == -1` | title menu, options pages (result value) |

`0x855ffc & 0x1000` (the 640×480 centring of the sprite layer, renderer.md §11) is set by the
ranking and records screens and cleared on exit; route select and the report card clear it.

### 1.2 Sprite descriptors and text — confirmed

Menus use the sprite slots of renderer.md §11 through `0x4316e0` (full set), `0x431730` (set from
a 6-byte descriptor), `0x431850` (position, layer, palette), `0x4318c0` (tbl id, −1 hides) and
`0x431790` (clear). A **descriptor** is `{u16 tbl, i8 col, i8 row, i8 w, i8 h}` in 16-px tiles; it
cuts one label out of a large "string sheet" map (w ≤ 0 = full width). Menu text therefore comes
from a handful of sheets:

| Sheet | Contents | Descriptor table |
|---|---|---|
| tbl 0x513 | Title items 1–6, 6 tiles high each | 0x47c778 (5 × 6 bytes, rows 0,6,12,18,24) |
| tbl 0x4fa / 0x4fb | Options labels and values (selected / normal colouring) | 0x47c584 / 0x47c664 (38 each) |
| tbl 0x544 | Pause and dialog texts, Yes/No, calibration prompts | 0x48bbb0 (29) |
| tbl 0x54d | Screen headers (路線選択, 電車選択, メインメニュー, …) | e.g. 0x48c584 |
| tbl 0x4a6 / 0x503 | Button legends (keyboard / pad glyphs) | 0x47cab0.. |

Index lists for the option values are at 0x48766c (one pointer per item, first byte = label
index, then value indices, −1 terminated). A translation layer can key on (sheet, descriptor index).

### 1.3 Positions — confirmed

Positions are native pixels of the 640×480 sprite space. Screens that are laid out for 640×480 add
a per-resolution offset `x0 = 0x463d78[res]`, `y0 = 0x463d88[res]` = (0,0), (0,0), (88,60),
(200,144) for res modes 0–3 (512×384, 640×480, 800×600, 1024×768). Some screens add
`(640 − *0x855ae8)/2` as well (guess: 0 in every shipped mode). All coordinates below are for res
mode 1 with those offsets at 0. Several layouts are `{x, y, layer, pal, slot}` × 4 res modes per
row, −1 terminated (the HUD tables use the same shape).

### 1.4 Navigation conventions — confirmed

Input words are active-low (input.md §6); "pressed" = the edge word `0x48c42c`.

* Cursor: **up 0x10 / down 0x20** (edge with auto-repeat), **no wrap** (clamped at both ends).
* **Confirm 0x1000** (Enter / pad A), **cancel 0x2000** (Esc / pad B).
* Analog-lever modes (input mode 5/6, `0x433270`): the cursor **follows the power notch**
  (`0x829d00`), one step per tick toward it.
* Sounds: cursor move 0x16, decide 0xd (title, route select) or 0x17 (options, records),
  cancel 0x13, dialogs 0x26 (sent straight to the sound thread, not gated by the sound option).
* Scrolling lists (title, route select) use 106-px rows (0x6a). The view scrolls by half the
  remaining distance per tick (the whole distance with an analog lever), window 2 rows in res mode 1
  and 3 rows otherwise (`((res != 1) + 2) · 0x6a`).
* Cursor frame: tbl 0x504 at (x0, row + 0x86), arrows 0x505 (more below, at +240,+0xef) and 0x506
  (more above, at +240,+0x6e), palette 0x18. After confirm the frame blinks (hidden when
  `t % 6 > 3`) for **30 ticks**, then the result is taken.

## 2. Boot, attract, press START

* Attract: `Attract_Run` 0x402700 walks the table at 0x484e30 (main-loop.md §5): 0x402e60,
  0x402880 (title logo), 0x402e80, 0x404e10 (ranking; may chain to the Komachi movie), 0x40e450
  (demo route), 0x40e4b0 (demo drive), 0x40e6c0, 0x403f40 ×4, 0x403da0, 0x40e6e0. START (0x1000)
  ends it. **likely** (individual items not traced in this pass).
* Demo route (0x40e450): collects the routes 0–12 whose bit is set in `0x8500b0` (→ 1, 3, 4, 6, 7,
  8, 9, 10, 12) and takes entry `attract_counter (0x8573c8) mod 9`. **confirmed** (objdump).
* The arcade "insert coin / press START" machinery is disabled on PC: `Credits_Init` 0x401910 (called
  unconditionally from `Game_Init`) sets `0x8573e0 = 1`, and every credit routine (0x401dc0,
  0x401e40, 0x401ef0, 0x401f40) returns at once when it is set. **confirmed**
* `PressStart` 0x4122d0 (`Game_MainFlow` after the title): places a cabinet picture (tbl 0x246
  "スタートボタンを押して下さい", 0x247, 0x15, 0x14, 0x1b, 0x38, 0x22 at the table 0x463f18) and
  its update returns 0 on the first tick when `0x8573e0 != 0`. **On PC it lasts one tick.** In
  the arcade build it blinks 0x246 every 15 ticks and plays 0xe9 every 200 ticks until START.
  **confirmed**

## 3. Title menu (0x42a1c0, task 0x42b210)

* BGM 0x19e starts (0x42ff20). Header and legends: `0x42a820(0)`.
* Items: descriptors 0x47c778 on tbl 0x513 (palette 0x18), slot 0x93+i at
  (x0 + 20, 0x8e + 106·i − scroll). Five items are drawn when input mode is 6 (the DDG controller),
  four otherwise. **confirmed**

| Index | Label (tbl 0x513 row) | Result → action |
|---|---|---|
| 0 | 1 アーケードモード / Arcade Mode | return 0 → route select |
| 1 | 2 ゲーム設定 / Game Setting | options (§4) |
| 2 | 3 ランキングを見る / Ranking | records viewer (§5) |
| 3 | 4 ゲーム終了 / Game Exit | `ExitConfirm_Run`, Yes → `DestroyWindow` |
| 4 | 5 専用コントローラー設定 / Controller Adjust (mode 6 only) | `ControllerAdjust_Run` 0x434a00 |
| (5) | 6 LOVE特急こまち | movie `FUN_00429110`; **not reachable**: the cursor max is 3 or 4 |

* Confirm: sound 0xd, 30-tick blink, stop 0xd, `0x816834 = index`. Cancel: sound 0x13,
  `0x816834 = −2` → `TitleMenu_Run` returns −2, `Game_MainFlow` sets `0x855ffc |= 0x10` and shows
  the title again without attract. **confirmed**
* Start (0): BGM 0x19e stops, all non-persistent sprites cleared, 3 ticks, return 0.
* Exit confirm 0x42bb90: `Dialog_Run(0x47c848, cursor 1)`: frame tbl 0x542, title "終了しますか?"
  (0x544 idx 0x0a), はい/いいえ (idx 0x0c/0x0e), cursor starts on いいえ. Yes destroys the window
  (→ `WM_QUIT` → shutdown, eeprom/replay saved). **likely** (Ghidra garbled the return value; the
  caller treats 1 as quit).

## 4. Options (0x42a520 → value pages)

List of six items, label at (x0+96, y0+160+48·i) slot 0x90+i and current value at
(x0+336, same y) slot 0x9a+i; the selected row uses sheet 0x4fa, the others 0x4fb. **confirmed**

| Item | Label | Values (0x4fa index) | Storage | Skipped when |
|---|---|---|---|---|
| 0 | コントロール | キーボードA–D, ジョイパッド, ジョイスティック, 専用コントローラ (22–28) | eeprom +0x332 (`0x856352`) | — |
| 1 | 難易度 | イージー, ノーマル, ハード, ベリーハード (6–9) | eeprom +6 bits 14–15; mirrored to `0x829d10` | — |
| 2 | 振動 | 切, 弱, 並, 強 (10–13) | eeprom +0x333 | no force-feedback device (`0x816838 == 0`) |
| 3 | サウンド | オン, オフ (14, 15) | `0x856356` byte 0 (eeprom +0x336) | no sound device (`0x816ae8 == 0`) |
| 4 | 速度メーター | 非表示, ノーマル, デカデジ (16, 7, 19) | eeprom +0x334 | — |
| 5 | 距離メーター | 非表示, cm表示, m表示 (16–18) | eeprom +0x335 | — |

* So the four difficulty columns of the rule tables (rules.md) are **Easy / Normal / Hard /
  Very hard**. **confirmed** (labels in tbl 0x4fa rows 6–9, `Options_Set` case 1).
* List page: confirm (0x17) opens the value page for the item, cancel (0x13) returns to the title.
  Value page: up/down picks, confirm (0x17) applies through `Options_Set` 0x42a460, cancel (0x13)
  discards. The control-mode page allows 0–3 without a joystick and 0–6 with one; with an analog
  lever, pushing the lever to P3 steps the mode cyclically (mod 7). It also shows the controller
  legend page 0x42a820 (pad picture tbl 0x50c etc.). **confirmed**
* Sound off → stop all (0x430160); sound on → restart the title BGM 0x19e. **confirmed**
* Labels 20/21 ロードする/セーブする exist in the sheet but are not in any item list (leftover).
  **confirmed** (lists at 0x47c744–0x47c76c)

## 5. Records viewer (0x41f5e0)

* Created from the title (item 2, with `0x855ffc |= 8` → interactive) and from attract.
  Sets `0x855ffc |= 0x1000`, `0x855ff8 |= 0x2000` (and `|= 4` in attract), dims the 3D layer
  (0x428990(0x70, 0x7070)). **confirmed**
* Pages: the route index `r` steps through 0, 1, 2, … and shows a page when bit `r` of `0x8500b0`
  is set and the ranking-table index `0x46bbe8[r] ≠ −1`; it ends after the first `r > 12` that fails.
  With `0x8500b0 = 0x3f9da` this gives **routes 1, 3, 4, 6, 7, 8, 9, 10, 12, 13, 14, 15, 16, 17**
  (routes 8 and 9 both show ranking table 5). **confirmed** (0x41f910)
* Layout: header maps `0x46bc10[r]` (slot 0x67) and `0x46bc64[r]` (slot 0x68) slide in at y = 40
  (x from −176 and 576, 8 px per tick, 33 ticks); column titles tbl 0x413–0x418 (slots 0x6c–0x71);
  10 rows at y = 92 + 32·rank, 8 cells per row (4 name characters, 4 score digits) at
  x = 144, 176, 208, 240, 360, 392, 424, 472 (0x46be42), character sprites `0x46bd30[c]`; badge
  tbl 0x2c8 ("PRO") at slot 0x5d+rank, x = 272, when the row's flag is 1. Cells reveal one per 2
  ticks, flickering between tbl 0x274/0x275 for 16 ticks. Legend tbl 0x545 (keyboard) or 0x546
  (pad) at (128, 432). Black side bars from the rect layer (0x41fc60). **confirmed**
* Timing: a page lasts 200 ticks. Confirm (0x17): jump to tick 175 (reveal all) or, if already
  there, to the next page. Cancel (0x13): leave. In attract the viewer ends when `0x855ff8 & 4`
  clears. **confirmed**

## 6. Route / stage select (0x435490)

### 6.1 Structure — confirmed

Two levels in one task (`+0x30` level, `+0x38` cursor, `+0x34` item count, `+0x44` chosen line,
`+0x48` chosen stage):

* **Line select** (header 路線選択: descriptor 0x48c584 on tbl 0x54d): 5 groups. Each group box =
  frame pieces tbl 0x30b, 0x308, 0x30a, 0x309, a line-name header and up to 4 train thumbnails.
  Group table 0x47cb10, stride 20 bytes `{i32 header, i32 thumb[4]}`:

  | Group | Header | Stages (0x47cb74, 6 × i32 per group) |
  |---|---|---|
  | 0 | 0x126 北陸路線 | 5, 6, 8, 9, 7 |
  | 1 | 0x127 東北路線 | 0, 3, 17, 18, 4, 1 |
  | 2 | 0x124 関東路線 | 11, 10, 20, 16 |
  | 3 | 0x125 関西路線 | 13, 15, 12, 14 |
  | 4 | 0x127 東北路線 | 2, 2, 2, 2 (see open questions) |

* **Train select** (header 電車選択: descriptor 0x48c58a): the stages of the chosen group, one
  106-px row each, drawn from the **stage table 0x4656ac** (21 × 32 bytes):

  | Off | Field |
  |---|---|
  | +0x00 | train thumbnail tbl (palette from `0x431dc0`) |
  | +0x04 | line + series caption (tbl 0x262–0x270, e.g. 山手線205系) |
  | +0x08 | from-station board (tbl 0x3c9–0x3f1) |
  | +0x0c | to-station board |
  | +0x10 | train-type board (快速/普通/特急はくたか/こまち …) |
  | +0x14 | −1 |
  | +0x18 | difficulty 1–5 (stars; also the "stage kind" of rules.md) |
  | +0x1c | route index written to `0x8570a4` |

  Stage 20 maps to route 10 with `0x48ddf0 = 1` (a variant of route 10; the departure board then
  shows a different train type). Row layout relative to the row top (+0x86 + 106·i): thumbnail
  (48,16), caption (128,24), tbl 0x30c (128,40), from board (136,56), tbl 0x30d "〜" (264,64),
  to board (300,56), type board (300,16), tbl 0x30f 難易度 (424,80), stars tbl 0x30e × difficulty
  from (424+16k, 64), rank badge (428,20) = 0x314 初級 (1–2 stars), 0x315 中級 (3),
  0x316 上級 (4), 0x317 特級 (5), clear star (36,12), replay mark (40,40). Stages 0x11/0x12 add a
  雫石停車/通過 tag (tbl 0x549/0x548). Header bar tbl 0x515 at (x0, 0); legends slots 0x87–0x8e,
  0x9c, 0x9d (descriptors 0x47cab0–0x47cb08, keyboard vs pad variant by input mode).

### 6.2 Navigation and results — confirmed

* Up/down (or the power notch) moves; confirm plays 0xd, blinks 30 ticks, stops 0xd. At line
  level that opens the train list (count = entries until −1, max 6); at train level it ends with
  `+0x48 = cursor`. Cancel (0x13) at train level returns to the line list with the cursor on the
  previous group; at line level it ends with `+0x44 = +0x48 = −1`.
* **R key (bit 0x4000)** toggles `0x816974` ("自動運転", replay playback) while in the train list. When
  on, each row whose stage has a stored replay (`0x42e450(stage) ≥ 0`) shows tbl 0x50b (row 6) at
  (40,40). This is the only reader of bit 0x4000 found (input.md lists none).
* Destroy 0x436640: if both indices are valid, `0x8570a4 = stage[+0x1c]` (stage 20 → 10,
  `0x48ddf0 = 1`); with replay mode on, `0x42e490(&0x829d10, rank)` loads the replay's difficulty or
  replay mode is switched off when none exists. Clears `0x855ff8 & 0x100000` and `0x855ffc & 0x1000`.
  Otherwise `0x8570a4 = −1` → `Stage_Run` sets `0x855ffc |= 0x40` (back to title). BGM 0x19e runs
  from creation and is stopped by `Stage_Run`.
* The creation argument (1 when EB + P3 were held at the title with `0x855ff8 & 0x40000`) is
  **ignored**: 0x435490 never reads its stack argument. **confirmed** (objdump)

### 6.3 There is no route lock — confirmed

* Every group and stage is always selectable; the code has no lock test.
* `0x8500b0` is **not** an unlock mask. It is a constant written once, `0x3f9da`, by
  `Ranking_FromEeprom` 0x41e520 (at eeprom load), and read only by the records viewer (§5) and the
  attract demo-route picker (§2). Bits set: 1, 3, 4, 6–10, 12–17 = routes that own a ranking table
  (`0x46bbe8[r] ≠ −1`) minus 11, 18, 19 (which share a table with 10 and 17).
* What *is* stored per route is the **clear mark** `0x85635a[route]` (eeprom +0x33a, 20 bytes),
  written by `Stage_Run` after a clear: 3 = cleared without a continue, 2 = 1–3 continues,
  1 = 4 or more continues; only ever raised. Route select shows it as the star tbl 0x54b with
  sub-row `(3 − mark)·2` (three star colours). The attract code 0x404cf0 also tests it
  (**guess**: to pick a demo stage the player has not cleared).

## 7. Pre-run screens

Order in `Stage_Run` 0x4080d0 after the route select (main-loop.md §5):

1. **Lever check** 0x4137e0 (**analog-lever modes only**; digital modes just force P0/EB). Shows
   "仕業検査" (tbl 0x249, slot 10 at (224,108)), "各レバーをスタート位置に合わせて下さい" (tbl 0x248,
   slot 11 at (160,364)), controller picture tbl 0x50a (slot 0x11 at (156,160), palette 0x18) and
   arrows tbl 0x3b/0x3c. Ends when the brake has been ≥ 9 with power 0 on **50 ticks** (not
   necessarily consecutive) or after **400 ticks**. **confirmed**
2. BGM 0x19e stopped, route data load `0x40ed90`, 15 ticks, setup calls (main-loop.md §5).
3. **Departure board** 0x4139c0 (skipped on retry): board tbl 0x3d8 "発車案内" (slot 58 at
   (112,206)) with four flaps: slot 54 series (128,256), 55 train type (376,256), 56 from (136,320),
   57 to (328,320). From tick 10 (sound 0x18) flap k starts at tick 10+8k, spins 32 ticks
   (alternating tbl 0x3d6/0x3d7 with random entries from its list), then lands on the route's entry
   from `0x467040[route·20 + k]` (lists 0x466fa0 series, 0x466fd8 type, 0x466ff8 stations; with
   `0x48ddf0` the type is forced to entry 2). When all four have landed sound 0x18 stops; the
   screen ends after **70 more ticks**. **confirmed**
4. Route load, then the per-station loop. At the first segment the HUD and the departure task
   take over (hud.md, rules.md §8).

## 8. Pause (0x42b7f0, dialogs 0x42b930)

* Opened by **bit 0x800000** (Esc / pad button 2) in the pre-departure wait and the driving loop.
  Sets `0x855ffc |= 4` (only the render list runs), pauses sound (0x4301e0(1)), saves the notches.
  **confirmed**
* `Dialog_Run(desc, cursor)` is a blocking loop. Descriptor: `{u16 frame tbl, i8 title idx,
  i8 title y, i8 cursor x, i8 items y, i8 spacing, i8 count, u32 → item idx list}`, texts on
  sheet 0x544 (descriptor table 0x48bbb0; item idx + 1 = highlighted variant), centred on the screen
  (frame centred using the cached map size at 0x850aa8). Cursor = star (desc 0) in slot 0x87, layer 0
  (on top). Up/down/confirm/cancel all play 0x26. Pressing pause again `longjmp`s out (= resume).
  **confirmed**

| Dialog | Frame | Title | Items (default) |
|---|---|---|---|
| 0x47c810 main | 0x541 | PAUSE★ | 運転再開 / 運転中止 / 始発駅に戻る (0) |
| 0x47c820 quit | 0x542 | 運転を中止しますか? | はい / いいえ (いいえ) |
| 0x47c830 retry | 0x542 | 始発駅に戻りますか? | はい / いいえ (いいえ) |
| 0x47c848 exit | 0x542 | 終了しますか? | はい / いいえ (いいえ) — title EXIT |
| 0x47c858 | 0x542 | 運転を記録しますか? | はい / いいえ — no caller found |

* Result: resume (item 0, cancel, or pause key) → 0 (cancel actually returns −1, which `Stage_Run`
  also treats as resume); quit + はい → 1 (`0x855ffc |= 0x10`, back to the title); retry + はい → 2
  (restart the route from station 0 without the departure board, `0x42e890(1)`); いいえ goes back to
  the main dialog. On exit: 3 ticks, sound resumes, notches restored, a held horn is released.
  **confirmed**

## 9. Station result (0x413d90, task 0x413f30)

Created after the arrival wait (rules.md §7.3 gives the deductions). Timing and layout — **confirmed**:

* Creation: penalties are computed (overrun `−d` capped at 999, re-power 0x45a8b0, EB-in-approach
  10 s unless lesson stage) and sound **0x195** (perfect stop: |dist| < 1 cm and a clean-stop mark)
  or **0x2d** is played. Sets `0x855ff8 |= 0x40`.
* Phase A, countdown `+0x38` from 96: at 94 the result camera preset is applied (0x419730) and the
  layout placed (table 0x464298, lesson stages 0x4646a8); the monitor frame tbl 0x25e/0x25f/0x260
  (by resolution) is slot 0x57 at (16 + shake, 192 − shake) and "運転評価" tbl 0x1a0 slot 0x4b at
  (64,22). At 90 sound 0x23. At 0 the lamp flag is set (0x434fc0(1)).
* Phase B, counter `+0x34` from 0 (rows at x = 80, palette 4; digits palette 5):

  | Tick | Slot / position | Content |
  |---|---|---|
  | 0 | 0x4c (80,268), digits 0x3f–0x41 at x 296/328/360 | Overrun: tbl `0x4654ec[0x856fb8]` "オーバーラン m×N秒" + metres. Else: cm < 50 → tbl 0x1c9 "停止位置 nn Cm 合格!" with the cm digits (palette 4 if stopped short by > 1 cm, guess); cm ≥ 50 → tbl 0x1a6; cm = 0 with a clean mark → tbl 0x1cc "停止位置 EXCELLENT!" + voice 0x193 or 0x192 by route. Sound 0x18 starts |
  | 10 | 0x4d (80,308) tbl 0x1c0 駅構内再加速; 0x4e (320,308) tbl 0x1c4 減点; 0x4f (512,308) tbl 0x1bf 秒; digits 0x42/0x43 at 448/480 | re-power penalty, or tbl 0x1c3 なし in slot 0x42 |
  | 30 | 0x53 (80,348) tbl 0x1ca 非常制動停車; 0x54/0x55; digits 0x46/0x47 | EB penalty or なし (row skipped on lesson stages) |
  | 40 | 0x56 (80,380) tbl 0x1a9 "合計 マイナス … 秒"; digits 0x48–0x4a at 416/448/480 (y 396) | total (cap 999, lesson stages 20); the remaining time is reduced, +exact-stop bonus |
  | 0–56 | HUD slots 0x18–0x1a (remaining-time digits) | flicker tbl 0x40b/0x40c; from 40 each digit k settles when the counter passes 40+5k; sound 0x18 stops when all three settled. Palette 4 + (remaining > 14) |
  | 120 | — | task ends; destroy stops 0x195/0x2d, clears `0x40` |

* Skip: confirm or cancel pressed with the counter in 1..80 jumps to 80; pressed after 80 jumps
  to 120 (the check runs after the counter is incremented). **confirmed**
* Correction (0x413f30, implemented in `crates/ddg-game/src/screens/result.rs`): the EB row's
  step sets the counter from 20 to **30** before drawing, so the "30" and "40" rows of the table
  above are counter values, reached on phase-B ticks 20 and 30; lesson stages jump straight to
  40 at tick 20. Without skipping the screen lasts 96 + 20 + 90 + 1 = **207 ticks** (lesson
  stages 197). A skip to 80 before the EB row has run is undone by that bump (the counter goes
  back to 30), so an early press only shortens the screen by a few ticks; a second press after 80
  ends it. **confirmed** (code reading; unit-tested, not checked against the running game)
* The result camera preset 0x419730 is not a simple camera change: it creates a task
  (update 0x41a010, destroy 0x41a470) that picks one of four scripted views from the timetable
  row field +0x40 (camera constants 0x84c100–0x84c144, curve tables 0x469454–0x469514, a
  per-stage spline 0x45735c by 0x4596e0) and rebuilds 121 heights at 0x850300; the monitor frame
  tbl 0x25e is drawn around it. **Open:** not traced or built; the port keeps the cab view.
* **Finding for rules.md §7.3:** the overrun deduction shown and charged is `metres ×
  0x856fb8`, where `0x856fb8 = 0x459910[stage][difficulty]` (i16, values 1–3, set in 0x40d040),
  not always 1 s per metre. The banner names the factor (m×1秒 / m×2秒 / m×3秒 = tbl
  0x1a3/0x1a4/0x1a5). Stage 0 is 1 at every difficulty, which is why the stage-0 oracle traces
  matched. **confirmed** (0x413f30: `+0x50 = 0x856fb8 · +0x50` before the total)

## 10. Continue and game over (0x415ab0)

* Shown when the remaining time ran out before the last station (`0x855ff8 & 0x10` after the
  station flow, and no replay is playing). Clears the HUD flags. **confirmed**
* Layout: tbl 0x24a at (x0+240, y0+200) slot 0x37; the countdown is drawn with the HUD
  remaining-time digit slots 0x18–0x1a (all three show the same digit `t/40`, tbl 0x401+…), so
  it counts **9 → 0, 40 ticks per digit** from 399, blinking 0x40b/0x40c in the **first** 4 ticks of
  each digit (`t % 40` ≥ 36 while the counter counts down; digit 9 shows from 395). Free play (§2) means the "press START" branch always applies: tbl 0x3f8 at (304,344)
  slot 0x38. **confirmed**
* Confirm (0x1000) → continue chosen: tbl 0x24d at (240,200), counter reset to 0. Horn button
  (0x100) skips to the next digit. At 0: digits 0, tbl 0x491 (continue) or 0x492 (no) at
  (304+192, 344); then 31 more ticks; continue → `0x855ff8 |= 2` (Stage_Run resumes at the last
  stopping station via 0x40d930, continues += 1, clock skew reset), else `0x855ff8 |= 0x20`
  (run over). **confirmed**
* The arcade "insert coin" variant (tbl 0x254 + credits digit at (348,344)) is unreachable on PC.
  **confirmed**

## 11. End of a run

### 11.1 Report card 0x4149d0 — confirmed

* Computes the five grade parts (0x40dfd0, rules.md §9) and their sum `0x856c88` (0–100).
  Background tbl 0x444 (運転評価 table) slot 43 at (80,104). Rows at y = 152, 184, 216, 248, 280:
  ダイヤの正確さ (timing), ブレーキ(乗り心地) (handling), 停止位置の正確さ (stopping),
  信号・標識を守る (safety), 安全性への配慮 (comfort). Per row: a stamp at x = 360,
  `[0x448, 0x447, 0x446, 0x445][part / 6]` (0x445 優, 0x446 良, 0x447 可, 0x448 lowest), then a
  bar of tbl 0x449 blocks (one per 4 points) and a half block 0x44a if `part % 4 ≥ 2`, in the
  slots at x = 392…520. Total in slots 40–42 at (384/416/448, 320).
* Timing: 30-tick delay; sound 0x18 at 0; rows at ticks 25, 50, 75, 100, 125 (sound 0x16 each);
  the total digits spin (`tick % 10`) and settle from tick 150 (sound 0x18 stopped); end at 250.
  Confirm or cancel jumps to 225. Leading zeros are hidden.

### 11.2 Ranking insert and name entry 0x41e8d0 — confirmed

* Ranking value (rules.md §9) capped at 9999; the run's "PRO" flag `0x85723c` is 1 when the
  distance driven with the distance meter shown (`0x8571f0`, accumulated by the HUD at
  speed/30 per tick) is ≤ 30 m, i.e. the run was driven essentially without the meter
  (likely meaning). Routes with no ranking table (`0x46bbe8[r] = −1`: 0, 2, 5) skip this screen.
* Insert position: first row whose score is lower, or equal with a lower PRO flag; not in the
  top 10 → skip. Rows below shift down; the new row gets name `0xb ×4` (blank) and the score.
* Name entry page: rank label `0x46bcd8[rank]`, headers tbl 0x41d (80,72), 0x41c (288,324),
  0x30 (112,392), 0x34 ×2 (80,360)/(496,360), slot 0x6d = legend descriptor 0x486184 (keyboard)
  or 0x48618a (lever); BGM 0x19d. A **59-symbol wheel** (`0x46bd30[0..58]`: 0–9, 10 = "00",
  11 = blank, A–Z, `. - ! ? /`, train-class abbreviations DD DF ED EF HK and katakana pairs,
  系, 形, 57 = ← (delete), 58 = 終 (end)) scrolls horizontally at y 360, 32 px per symbol; the
  current symbol is enlarged at (304,352). Entered letters show at x = 272 + 32k, y 252.
  * Keyboard/pad: hold left (held bit 4) / right (8) to scroll, accelerating 2 → 4 → 8 → 16 px/tick
    after 5/10/15 ticks; confirm (0x1000) takes the symbol.
  * Analog lever: the brake notch sets the scroll speed (B0 −16 … N/B4–6 stop … B10 +16 px/tick);
    raising the power notch from N takes the symbol.
  * Sounds: 0x15 per symbol passed, 0x17 letter, 0x25 delete, 0x1d done.
  * Done when 4 letters are in or 終 is taken (rest blank); timeout **930 ticks** (31 s), shown by
    the HUD timer routine 0x417040. Then 30 ticks, the name filter 0x41e7a0 (three 4-character
    lists at 0x46bf64/0x46bf88/0x46bf94; a match is replaced by the route's default name at
    0x46bf14), and `Ranking_ToEeprom`.
* Ranking page: as §5 for this route, the new row blinking every 4 ticks, 150 ticks; confirm (Enter)
  or horn ends it early. Destroy clears `0x2000`, stops BGM 0x19d.

### 11.3 Run-end banner 0x415f00 — confirmed

* `RunEnd(0)` (game over / quit by time): tbl 0x250 "運転中止 GAME OVER" at (x0+240, y0+204).
* `RunEnd(1)` (cleared): tbl 0x251 "運転終了 GAME OVER" at (240,204) and the conductor girl,
  tbl 0x3fc (routes 2–4, 8–10, 12–18, palette 0x12) or 0x400 (others, palette 4) at (384,48).
  Grade 100 → tbl 0x3fd "鉄ちゃん感動です!" at (304,344) and voice 0x193; grade ≥ 90 → tbl 0x3fb
  "素晴らしい運転でした!". Voice/jingle by grade, continues and stage difficulty: grade < 70 without
  continues → jingle 0x1004 or 0x1002 (difficulty > 3); with continues → voice 0xfe / 0xf9;
  grade ≥ 70 with continues → 0xf7 / 0xf8 / 0xfd by difficulty; without → jingle 0x1000 / 0x1001 /
  0x1003. **confirmed** (sound meanings guess)
* Lasts **150 ticks**; confirm or cancel ends it at once.

### 11.4 Order — confirmed (`Stage_Run`)

* Game over: report card → (ranking value without the remaining-time bonus) name entry → banner 0.
* Clear (`0x855ff8 & 8`): report card → clear mark → ending 0x4040a0 if the stage difficulty is
  > 3 (flag `0x855ffc & 0x800`) → ranking value with `+remaining·1000` → name entry → banner 1.
* Quit from pause: straight back to the title, no report.
* Replay playback runs skip the report/ranking (`0x42e910 ≠ 0`).

## 12. Bonus game and ending

* **Bonus game** 0x41b540: started inside the station loop at station `0x459e10[route]` when no
  continue was used (routes 3: st 5, 4: 4, 6: 9, 10: 8, 11: 5, 12–15: 10, 16: 9; 99 = none).
  Art: ボーナスゲーム tbl 0x2d8, instruction "時間内に衝撃なく列車を連結せよ" 0x2d9, shock meter
  0x2da (EF63), 0x2db, 連結成功 0x2dc / 連結失敗 0x2dd, 準備/開始 0x2de/0x2df; time limit 474 ticks
  drawn with 0x417040; stops all sound, scripted camera. Waits on `0x855ff8 & 0x4000`. Route 6
  then branches on 0x40e350 (≥ 4 → an extra section with flag 0x2000000). **likely** (task
  internals 0x41b810 not traced; rules.md mentions its penalty/bonus tables 0x45a4f0/0x45ac70).
* **Ending** 0x4040a0: 0x404920 or 0x404110 depending on `0x855ff8 & 0x40000000` (set from
  registry option bit 0x10), waits on `0x855ff8 & 8`, then kills lists 4, 5 and 7. **guess**
  (staff roll; not traced).

## 13. Save data

### 13.1 `eeprom.dat` (0x35a bytes, mirror at 0x856020) — confirmed unless noted

Loaded at startup (0x434ed0, then `Ranking_FromEeprom`), **saved only at exit** (0x434f50 from
`WinMain`'s shutdown path). No header or version; a short read leaves the defaults in memory.

| File off | VA | Content |
|---|---|---|
| +0x000–0x01f | 0x856020 | arcade settings block (coin setup bytes +6/+8/+0xa/+0xb/+0xc, read by 0x401910); +6 bits 14–15 = difficulty |
| +0x020 | 0x856040 | checksum of +0x00..+0x1f (computed by a stub that returns 0 on PC) |
| +0x022 | 0x856042 | names: 13 tables × 10 rows × 4 symbol indices (0x46bd30 alphabet) |
| +0x22a | 0x85624a | scores: 13 tables × 10 × u16, bits 0–14 = score 0–9999, bit 15 = PRO flag |
| +0x330 | 0x856350 | checksum of +0x22..+0x32f (stub, 0) — likely |
| +0x332 | 0x856352 | control mode 0–6 |
| +0x333 | 0x856353 | vibration 0–3 |
| +0x334 | 0x856354 | speed meter 0 hidden / 1 normal / 2 big digits |
| +0x335 | 0x856355 | distance meter 0 hidden / 1 cm / 2 m |
| +0x336 | 0x856356 | sound on/off; bytes +0x338/+0x339 are an x/y screen offset (`0x856358/59`, renderer.md §11) — likely |
| +0x33a | 0x85635a | clear marks, 1 byte per route 0–19 (§6.3) |
| +0x34e–0x359 | | unknown |

Ranking tables per route (`0x46bbe8`, i16 per route): 0:−1, 1:3, 2:−1, 3:10, 4:11, 5:−1, 6:0,
7:4, 8:5, 9:5, 10:7, 11:7, 12:9, 13:8, 14:2, 15:12, 16:1, 17:6, 18:6, 19:6. In memory the tables
are unpacked to `0x84fca0 + 0x50·table + 8·row` (4 name bytes + 4 decimal digits) and
`0x84fc00 + 10·table + row` (PRO flag). Where the default (first-boot) ranking comes from was not
found (open question).

### 13.2 Other stores

* Registry `HKLM\Software\TAITO\電車でGO！２ 高速編 ３０００番台`: `Option` (0x80-byte blob →
  `0x816840`: resolution, windowed, nosound, …) and the fog values; written by the separate
  `DengoSetting.exe`, not by the game's menus (main-loop.md §1). Option bits used by the flow:
  0x20 = debug start (route 1, station 7, no select), 0x18 = skip the departure board block,
  0x10/0x08 = ending variant. **confirmed** (bits), meanings **guess**.
* `..\BIN\REPLAY.DAT`: replays per difficulty/stage (main-loop.md §7); route select's 自動運転 mark
  and the pause-dialog text "運転を記録しますか?" belong to it.
* `joystick.dat`: calibration (input.md §5).

## Open questions

1. Group 4 of the line select (header 東北路線 again, stage list `2, 2, 2, 2`, thumbnails 104, 103,
   0): is it really reachable as a fifth group, and why stage 2 four times? Stage 19 (route 19) is
   in no group. Needs a run of the original.
2. `0x855ae8` (subtracted from 640 in route-select and title x positions): its value per mode.
3. Default ranking contents on first boot (no `eeprom.dat`): not located.
4. The exact meaning of the PRO flag threshold (`0x8571f0 ≤ 30.0`, accumulated only while the
   distance meter is drawn): verify with a run with the meter on and off.
5. Attract items 0x402e60, 0x402e80, 0x40e6c0, 0x403f40, 0x403da0, 0x40e6e0, the ending
   (0x404110/0x404920) and the bonus-game internals (0x41b810) were not traced.
6. Sound code meanings (0xd, 0x13, 0x16–0x18, 0x23, 0x2d, 0x195, 0xf7–0xfe, jingles 0x1000–0x1004,
   BGM 0x19d/0x19e) are from context only; map them with the audio notes.
7. `ExitConfirm_Run` return value: Ghidra lost it; confirm that はい = 1.
8. Whether the station-result palette choice (`dist > 0.01` → palette 4) reads the double at
   0x4676d0 as 0.01 (low dword 0x47ae147b fits) — confirm in the disassembly.

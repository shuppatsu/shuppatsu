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
| 0x41f5e0 / 0x41f910 / 0x41fc60 / 0x41fd00 | `Records_Create` / `_Update` / `_Draw` / `_Destroy` | Ranking viewer (title item 2) | confirmed |
| 0x41e520 | `Ranking_FromEeprom` | Unpacks the eeprom ranking into the display table; sets `0x8500b0 = 0x3f9da` | confirmed |
| 0x41e670 | `Ranking_ToEeprom` | Packs one ranking table back into the eeprom mirror | confirmed |
| 0x41e7a0 / 0x41e4c0 | `Name_Filter` / `Name_Replace` | Replaces names from three lists with a per-route default | confirmed |
| 0x418ef0 | `Eeprom_CheckDefaults` | Copies the default image (0x467768/0x467790/0x467aa0) when the 2-byte magic differs | confirmed |
| 0x41e720 / 0x418e70 | `Ranking_Reset` | Restores the default ranking tables (debug paths) | confirmed |
| 0x418930 | `Eeprom_Checksums` | Writes the (always 0) checksum words +0x20/+0x330 | confirmed |
| 0x41e8d0 / 0x41ebe0 / 0x41f5a0 | `NameEntry_Create` / `_Update` / `_Destroy` | Inserts the score, name entry, then the ranking page | confirmed |
| 0x434a00 | `ControllerAdjust_Run` | Calibration screen (input.md §5) | confirmed |
| 0x4122d0 / 0x4123c0 / 0x412490 | `PressStart_*` | Arcade "press START" screen; a no-op on PC | confirmed |
| 0x435490 / 0x435510 / 0x436210 / 0x436640 | `RouteSelect_Create` / `_Update` / `_DrawHeader` / `_Destroy` | Line select → train select | confirmed |
| 0x4137e0 / 0x413900 / 0x413940 | `LeverCheck_*` | "仕業検査" lever-position prompt, analog-lever modes only | confirmed |
| 0x408ec0 / 0x4091a0 / 0x408f40 | `StageIntro_Run` / `_FlyBy` / `_Chase` | Pre-run stage intro (scene.md §9): fly-by with an AI train, or a chase of the player's train | confirmed |
| 0x4139c0 / 0x413b10 / 0x413d30 | `DepartureBoard_*` | Intro: departure-board roulette | confirmed |
| 0x42b7f0 | `PauseMenu_Run` | Pause dialog sequence | confirmed |
| 0x42b930 | `Dialog_Run` | Generic blocking list dialog (pause, confirmations, exit) | confirmed |
| 0x42bb90 | `ExitConfirm_Run` | "終了しますか?" yes/no | likely (return value) |
| 0x413d90 / 0x413f30 / 0x4148b0 | `StationResult_*` | Per-station stop evaluation screen | confirmed |
| 0x415ab0 / 0x415ba0 / 0x415e80 | `Continue_*` | Continue countdown | confirmed |
| 0x4149d0 / 0x414b70 / 0x415050 | `ReportCard_*` | 運転評価 five-part grade screen | confirmed |
| 0x415f00 / 0x416270 / 0x4162a0 | `RunEnd_*` | 運転中止 / 運転終了 banner | confirmed |
| 0x41b540 / 0x41b810 / 0x41c2e0 | `BonusGame_*` | EF63 coupling bonus game (§12.1); physics task 0x4240d0 / 0x4241d0 | confirmed |
| 0x4093c0 | `Coupling_Run` | In-cab coupling minigame (rules.md §7.5) | confirmed |
| 0x4040a0 / 0x404110 / 0x404920 | `Ending_Run` / `_Full` / `_Plain` | Staff roll after clearing a stage of difficulty above 3 (§12.2); tasks 0x4042b0, 0x404a80 | confirmed |
| 0x402700 | `Attract_Run` | Attract sequencer over 0x484e30 (§2.1) | confirmed |
| 0x402880 / 0x402e80 / 0x40e4b0 / 0x40e6e0 / 0x403f40 / 0x403da0 | `Attract_Logo` / `_Title` / `_DemoRoute` / `_DemoLesson` / `_Photo` / `_Poster` | Attract items (§2.2–§2.5) | confirmed |
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
| `0x855ff8 & 0x20000` | stage intro (set and cleared inside 0x408f40 / 0x4091a0, no wait) |
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
| tbl 0x503 / 0x4a6 | Legend key pictures: ↑↓ keys, pad cross, lever (0x503, 3 tiles wide); Esc, Enter, pad A/B/C, R (0x4a6, 2 tiles wide) | 0x47c524.. (menu header), 0x47cab0.. (route select) |
| tbl 0x50b | Legend words 選択 / キャンセル / 決定 / おわり / モード切替 (rows 0, 2, 4, 6, 8; 選択 4 tiles wide, the rest full width), replay mark 自動運転 (col 6) | 0x47c538 (5), the same five at 0x47cac4 |

Index lists for the option values are at 0x48766c (one pointer per item, first byte = label
index, then value indices, −1 terminated). A translation layer can key on (sheet, descriptor index).

### 1.3 Positions — confirmed

Positions are native pixels of the 640×480 sprite space. Screens that are laid out for 640×480 add
a per-resolution offset `x0 = 0x463d78[res]`, `y0 = 0x463d88[res]` = (0,0), (0,0), (88,60),
(200,144) for res modes 0–3 (512×384, 640×480, 800×600, 1024×768). Some screens add
`(640 − *0x855ae8)/2` as well: `0x855ae8` is the pixel width of tbl 0x504 (the cursor frame) in
the tilemap-size cache at 0x850aa8 (16 bytes per tilemap, filled by 0x430ec0), so the term
centres the cursor frame. **confirmed** (nothing writes 0x855ae8 directly; 0x850aa8 + 16 · 0x504
= 0x855ae8) All coordinates below are for res
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

### 2.1 The walk — confirmed (0x402700)

`Attract_Run` 0x402700 walks the null-terminated table 0x484e30. An item creates a task (list 4)
and sets `0x855ff8 & 4`; the walk runs `Task_Tick` while the bit stays set, then one more
`Task_Tick`, then the next item. Items that only call a loader clear the bit at once and cost
that one tick. At the end of the table the sprites are cleared (0x431790(0)), three ticks run,
the walk counter `0x8573c8` goes up by one and the walk starts again at **item 0**.

| # | Item | What it is | Length |
|---|---|---|---|
| 0 | 0x402e60 | `stage_assets_load(0x15)`: area 2 plus partsTR2 / partsTR9 (the opening's sets) | instant |
| 1 | 0x402880 | TAITO logo with two locomotives (§2.2) | ≈ 470 ticks |
| 2 | 0x402e80 | the title: the cold-start opening (§2.3) unless `0x855ffc & 0x10`, then the title picture 0x4037f0 | 566 + 301 |
| 3 | 0x404e10 | the Komachi-movie check (below) | instant |
| 4 | 0x40e450 | picks the demo route (below) and loads its assets | instant |
| 5 | 0x40e4b0 | demo drive on it (§2.4) | 1600 |
| 6 | 0x40e6c0 | `stage_assets_load(5)` | instant |
| 7–10 | 0x403f40 ×4 | a photo each (§2.5) | 102 each |
| 11 | 0x403da0 | a poster (§2.5) | 202 |
| 12 | 0x40e6e0 | the stage-5 lesson demo (§2.4) | 3250 |

The records viewer is **not** an attract item: the only caller of 0x41f5e0 is the title menu
(0x42a1c0).

**START** is the confirm edge `0x48c42c & 0x1000` (free play, `0x8573e0 = 1`; 0x401dc0 then
returns 1). While an item runs, the walk tests START unless the item handles it itself
(`0x855ffc & 0x100`, the logo only): START clears `0x855ff8 & 4`, the item's task dies in the
next `Task_Tick` (0x4027f6), one more runs (0x4027ff), the walk counter goes up (0x402829), and
then
* with the title picture up (`0x855ffc & 0x20`, set by 0x4037f0): sound 0x1e and `Attract_Run`
  returns, the title menu follows;
* otherwise `0x855ffc |= 0x10` (0x40284c), one `Task_Tick` (0x40270c), and the walk restarts at
  item 2, whose title then skips the opening.
Cancel (`0x2000`) during the title picture sets `0x48de00`, which nothing reads. **confirmed**
(The input word is active-low: 0x402762 / 0x4027cf treat a *clear* 0x1000 bit as START.)

**START per item — confirmed** (disassembly of 0x402700 0x402723–0x402878, 0x402e80,
0x402a70, 0x404e10, 0x428f00; no Wine capture needed, the code has no other path). `0x855ffc &
0x10` is set on *every* START abort and cleared only by 0x402e80 when it consumes it, so the
opening never comes back after START; it plays again only when a walk runs to the table's end
untouched (item 0 onwards with the bit clear).

| START during | Original | Walk counter |
|---|---|---|
| logo, states 0–2 | jumps to state 3 (logo alone); the logo goes on and the title item then plays the opening | — |
| logo, states 4–7 | 0x402a70 sets `0x10` and ends the item; the walk goes on to item 2 → title picture | — |
| logo, state 3 | nothing | — |
| cold-start opening (item 2 before 0x4037f0) | abort, `0x10`, restart at item 2 → title picture | +1 |
| title picture | sound 0x1e → title menu | +1 |
| Komachi movie (item 3) | 0x428f00 sets `0x10` and returns; 0x404e10 leaves `0x855ff8 & 4` set, the walk sees the same edge → abort → title picture | +1 |
| route demo, lesson demo | abort → title picture | +1 |
| photos, poster | abort → title picture | +1 |

The ranking pages are not an attract item (the records viewer is reached from the title menu
only), so there is no attract case for them.

**Port** (`front/attract.rs`, `Attract::restart_at_title`): until 2026-10-19 only the logo and
the movie skip set `skip_opening`; START in a photo, the poster or a demo after the opening had
played once restarted the walk at item 2 with the flag clear and **replayed the opening**, and
START in the opening, a demo or the title picture did not count the walk counter on (it feeds the demo route,
item 4, and the poster parity). Now every abort goes through one path that counts the walk and
sets the flag. Tests: `attract::tests::start_in_any_item_restarts_at_the_picture_without_the_opening`,
`a_full_walk_plays_the_opening_again`, `late_start_in_the_logo_goes_to_the_picture`, and on the
real data with the app's loop and a scripted Enter `app::tests::start_in_attract_never_replays_the_opening`
(opening, route demo, photo, poster, then the picture → title menu) and `app::tests::start_in_the_logo`.

* Item 3, 0x404e10, calls 0x404cf0, which returns 1 when every select stage of the five
  route-select groups has a clear mark on its route (`0x85635a[stage +0x1c]`), and then plays the
  Komachi movie `FUN_00429110`, its only reachable caller (§3). **confirmed**
* Item 4, 0x40e450, collects the routes 0–12 whose bit is set in `0x8500b0` (`0x3f9da` from
  0x41e520: routes 1, 3, 4, 6, 7, 8, 11, 12) and takes entry `0x8573c8 mod 8`. **confirmed**
* The arcade "insert coin / press START" machinery is disabled on PC: `Credits_Init` 0x401910
  (called unconditionally from `Game_Init`) sets `0x8573e0 = 1`, and every credit routine
  (0x401dc0, 0x401e40, 0x401ef0, 0x401f40) returns at once when it is set. **confirmed**
* `PressStart` 0x4122d0 (`Game_MainFlow` after the title): places a cabinet picture (tbl 0x246
  "スタートボタンを押して下さい", 0x247, 0x15, 0x14, 0x1b, 0x38, 0x22 at the table 0x463f18) and
  its update returns 0 on the first tick when `0x8573e0 != 0`. **On PC it lasts one tick.** In
  the arcade build it blinks 0x246 every 15 ticks and plays 0xe9 every 200 ticks until START.
  **confirmed**
* After a run `Stage_Run` sets `0x855ffc |= 0x10` only on the quit path (0x408b72), so attract
  starts from item 0 (with the opening) after a game over or a clear, and from the title picture
  after a quit, a menu cancel or a declined exit. **confirmed**

### 2.2 The logo item 0x402880 (task 0x402a70) — confirmed

Slots 0x11 / 0x12 = two locomotives at (0x280, 0x13e) / (0x300, 0x13e), palette 4; slot 0xd =
TAITO tbl 0x160 at (0x380, 0x66), palette 0xb (res modes 0/1; 0x161 / 0x162 and other locomotive
art for modes 3 / 2). Locomotive k shows bit k of the difficulty `0x829d10`: tbl 0x433 (bit 0)
or 0x436 (bit 1) from a stack table, so the pair spells the difficulty in binary. Sets
`0x855ffc |= 0x100` (START handled by the item). With A = locomotive width 0x4579c0[r] (128),
B = logo width 0x4579e0[r] (208), W = screen width 0x48c1e0[r], C = (W − B)/2, y = 0x457a00[r]
(318) for the locomotives and 0x457a10[r] (102) for the logo, x = 5·counter:

| State | Motion | Ends |
|---|---|---|
| 0 | the three slide in from the right: loco 0 at W − x, loco 1 +A, logo +2A | x ≥ 2A − C + W (136 ticks) |
| 1 | pause | counter > 0x13; sound 0x2a |
| 2 | the locomotives leave to the left (C − A − x) | x ≥ C |
| 3 | the logo alone at C, locomotives parked at (0x800, 0x13e) | counter > 0x3b |
| 4 | the locomotives come back from the left (x − A) | x ≥ C; sound 0x2a |
| 5 | pause | counter > 0x13 |
| 6 | all three slide off to the right (C + x − 2A …) | x ≥ 2A − C + W |
| 7 | | counter > 4: the item ends |

START before state 3 jumps to state 3 (counter 0, not counted that update); START after state 3
sets `0x855ffc |= 0x10` and ends the item, so the title skips the opening. Destroy 0x402e00
clears the slots and both flags.

### 2.3 The cold-start opening (item 2, 0x402e80 → 0x403750) — confirmed (code)

One task in list 4 whose update moves through seven functions; each switches with the counter
reset, and the next function first runs on the next tick. 0x425dc0, 0x431eb0 and 0x4317d0 are
no-ops on PC (arcade VRAM preloads). The create also runs `0x40eed0(2, 0)` (palette 3 for area
2, plus one `Task_Tick`, so phase A's first update happens inside the create), BGM 0x1a1,
render mode `0x84c128 = 2`, slot 0x26 placed at (0x170, 0x154).

| Phase | Function | Updates | Content |
|---|---|---|---|
| A | 0x402fb0 | 145 | render mode 2 (eye (0.5, 0.13, −2.488), target (−1.041, 0.08, 2.551), 45°): draw 0x403920 = backdrop gradient 0x4692e0 (palette 3 index 1 → 255: sky (213,213,238) to ground (200,200,200)), black rect bars 0–40 and 440–480 px, the track bed 0x2f83 at the origin and the Komachi consist (`*0x4573dc` = 0x450f58, 28-byte records `{part[5], offset, spacing}`) at (0.15, −0.2, t + 0.5·L − offset); t from 10.0, −0.04 per update (f64 0x458ac0); update 2 sets the draw callback and slot 0x10 = tbl 0x198 (platform photo: the left platform, the rest transparent) at (96, 56), palette 0x11. **Where the 3D goes — confirmed (code + Wine capture):** mode 2's viewport is the full virtual frame (centre (248, 200), half (248, 200)), which `Viewport_Apply` 0x436e14 draws as 496×400 *unscaled* pixels at ((W − 480)/2 + 16, (H − 400)/2 + 16) whenever the resolution index `0x816afc` ≠ 0 (renderer.md §5), i.e. (96, 56) at 640×480: exactly the photo's rectangle, so the 3D platform lines up with the photo and the train passes in front of its edge; everything outside x 96–592, y 56–440 is black (measured on the capture). Projection 45°, aspect 1.25 (0x3fa00000) |
| B | 0x403080 | 57 | black 3D; slot 0x26 = tbl 0x123 (3000番台 badge); slot 0x10 at (80, 124): frames 0x457a20[0..5] (0x439–0x43d), palette 0xe, the first for 20 updates, then each new frame starts with one update in palette 0x14 (all white: `0x436c20(0x14, 0xffffff)`) at counts 20, 26, 32, 40 |
| C | 0x4031d0 | 94 | as A with t from 7.0, −0.1953125 per update (0x458ac8): the train rushes past |
| D | 0x4032a0 | 57 | as B with frames 0x457a20[5..10] (0x43e–0x442); at its end `0x408080(1, 24)` (Morioka on AKITA1C), FOV 30°, the camera jumps 150 units |
| E | 0x403420 | 89 | render mode 0 on the route, black bars x 0–64 and 576–640 and the bottom band (268–400 virtual) plus the drive callback's own top mask 0–94 (0x486160, scene.md §2.1), so the view is x 64–576, virtual y 94–268 (pixels 113–321, measured); update 0: `0x406e10(spawn 2, flags 0x11, 270 km/h, 0, model 7)` (an E3 set; rear bogie 12 points back instead of 10) and v = 0.1852 units/tick (f32 0x3e3da512); then +0.0038892 per update (0x458ae0) up to 0.51 (0x458ad8); speed readout 0x416f50(`trunc(v · 0.5 · 1079.913607)`), slot 0x75 = tbl 0x53 at (64, 112) and the 7-segment palette entries; at its end list 5 is killed |
| F | 0x403540 | 91 | white studio (`0x40eed0(8)`, palette 3 all white): the platform set 0x2df3, the 489 car 0x327a at z40 and the EF63 pair 0x2df2 at z20 and z20 + 3.9 (x −1.5); three camera keys (counts 0, 20, 40) with FOV 16°, 12°, 22°; from count 41 z20 and z40 −0.15 per update and slot 0x10 = tbl 0x443 (3000番台 scroller) at (20c − 1376, 140), palette 0xe |
| G | 0x403750 | 33 | black 3D; slot 0x10 at (80, 124), palette 0xe: 0x457a20[10..12] for counts 0–2, 0x2c2 to 23, 0x2c3 (the navigator girl's salute and wink) to 31; then the title picture 0x4037f0 in the same task |

566 updates before the title picture. BGM 0x1a1 plays on through the title picture; the title
task's destroy 0x403d20 kills list 5, releases 0x1a1, clears the sprites and the flags. START
during the opening ends the task the same way (no title picture; the walk restarts at item 2).

### 2.4 Demo drives (0x40e4b0, 0x40e6e0) — confirmed

Both build a stage run without the stage loop: `0x40d040(0)`, the plates of station 0 and of the
first stopping station (0x40f780), the running sounds, `0x408080(stage, 0)`, the HUD
(0x40f7e0), the train task in its **replay mode** (`0x4229c0(stage, 2 or 3)` → task+0x74 = 2,
table pointer task+0x88), 0x40dad0, the signal blocks (0x40b970), `0x856fb0 = 30`, the distance
panel and the clocks on, and the attract overlay 0x4023b0: slot 6 = 0x25a "スタートボタンで運転開始
/ PUSH ENTER KEY" at (0xe0, 0xac) and slot 8 = 0x25b "一日乗車券 FREE PLAY" at (0xf0, 0xc4),
shown in the first / third 30-tick quarter of every 120, and slot 0x26 = 0x123 at
(0x4579a0, 0x4579b0)[r] every update (in the arcade build 0x253 / 0x251).

The demo's notches: every train-task update reads one byte of the table, `brake << 4 | horn << 3
| power` (0x422c71). The end test always looks at 0x467b20 (`cmp byte [edi+0x467b20], 0xff`),
which holds no 0xff within either demo's length, so neither table wraps. The demo inputs are
**not** `REPLAY.DAT` / CReplayEvent (input.md §7, the player's recordings).

* **Route demo** 0x40e4b0 (task 0x40e630, list 4): the route item 4 picked, table 0x467b20
  (2652 bytes), the GMQ logo 0x402590 (slot 7 at (0x457980, 0x457990)[r], palette 0xb, frame
  `[0x304, 0x305 ×3, 0x306, 0x307 ×3][0x84c3dc % 12]`, 0x84c3dc counting the rail-joint clicks of
  the train task), the departure lamp lit (`0x434fb0(1)`). Each update while the counter is below
  0x640 (1600): 0x40dad0, 0x40b710 (AI-train events), clock +1.
* **Lesson demo** 0x40e6e0 (task 0x40e860): stage 5, table 0x46857c, the course events replaced
  by the list 0x459338 (`0x857088`; hints 1, 2, 14, 4, 12, 13, 11 and two speed limits). Each
  update while the counter is below 0xcb2 (3250): at count 30 hint 0 (180 ticks) and
  `Signal_Show(4, 2, 150, 0)` (a red starting signal), at 160 `Signal_Show(aspect | 0x100, 2, 0,
  10)`, at 200 the lamp on and hint 9 (90 ticks); then 0x40dad0, 0x40acd0, 0x40c010, 0x40b780 and
  clock +1. No ATS and no AI-train events.

The destroy 0x40e670 kills lists 2, 3 and 4, stops all sound, clears the sprites and the flags.

**No speedometer in the demos — confirmed** (code, and a Wine capture of the original with no
`eeprom.dat`: `out/orig_opening/f144.png`). Both creators set `0x855ff8 |= 0x204` (0x40e530,
0x40e75d) and the destroy 0x40e670 clears 0x204 again; the only writes of bit 2, "stage run", in
the disassembly are `Stage_Run`'s `mov 0x855ff8, 2` at 0x4081dd and 0x408b48. `Cab_Overlay`
0x434fd0 draws the speedometer only with bit 2 and `& 0xa00`, else it clears slot 0x81 (`0x829d18` is 0), so the demos show no dial
whatever the option. The option itself is not the cause: the default image byte +0x334
(0x467aa0 + 2) is **1 = analog** (the image there is `00 02 01 02 00`: control 0, vibration 2,
speed meter 1, distance meter 2 = m, sound 0 = on), and nothing but the options page writes
0x856354. The port: `Hud::stage_run` (false in `CabHud::demo_setup`) gates the overlay; the
headless and fresh-install runs take their options from that default image (`save.rs`), a
demo from the save like a run (`--speedometer` overrides only `ddg-game play`). The cold-start
opening has bit 2 clear as well, so it shows no dial either (the port draws none there).

### 2.5 Photos and poster — confirmed

* 0x403f40 (task 0x404030): `{tbl, w, h}` i16 × 35 at 0x4578ac, entry `0x8573cc mod 35`
  (`0x8573cc` then +1), slot 0x10 centred on (320, 240), palette 7; the task runs 102 updates.
* 0x403da0 (task 0x403ec0): `{tbl, palette}` i16 × 2 at 0x4578a0 by the parity of `0x8573c8`
  (0x412 / palette 26, 0x40f / palette 13), full screen in slot 0xf at (x0, y0), and slot 7 =
  tbl 0x122 (the line list) at (0x196, 0xf0) or (0x34, 0x78), palette 4; 202 updates.

### 2.6 The title picture 0x4037f0 — confirmed

Tbl 0x410 (0x411 in res mode 2) full screen at (x0, y0), palette 0x17; palettes 0x17 / 0x16 /
0x15 fade in from black over 0x40 / 0x50 / 0x50 ticks towards the palette data at 0x48b290 /
0x488e90 / 0x488290 (task 0x4367c0 → 0x436820: each channel × `(n − remaining) / n`, truncated,
the last tick an exact copy); the title call 0x1024 at tick 100; the item ends after 300 ticks.

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
| (5) | 6 LOVE特急こまち | movie `FUN_00429110`; **not reachable from the menu**: the cursor max is 3 or 4 and 0x47c778 holds only five descriptors. Attract plays it once every select stage is cleared (§2). The label is on the sheet at row 30 (the cell under item 5). Port: setting `movie_in_menu` makes it the last row (movie.md §6) |

* Confirm: sound 0xd, 30-tick blink, stop 0xd, `0x816834 = index`. Cancel: sound 0x13,
  `0x816834 = −2` → `TitleMenu_Run` returns −2, `Game_MainFlow` sets `0x855ffc |= 0x10` and shows
  the title again without attract. **confirmed**
* Start (0): BGM 0x19e stops, all non-persistent sprites cleared, 3 ticks, return 0.
* After options (1), records (2) and calibration (4) the menu is re-created with the cursor on
  that item (0x42a1c0 passes its last result back to 0x42b210). Records: BGM stopped, sprites
  cleared, 10 ticks, then the viewer; the BGM restarts afterwards. **confirmed**
* Every menu task's destroy (0x42a710) clears slots 0x90..0xb6; the header slots 0x87..0x8f and
  the background 0xb7 stay until the next screen redraws them. The header 0x42a820 draws the
  full-screen background tbl 0x501 (res mode 1) in slot 0xb7. **confirmed**
* Exit declined (いいえ, cancel or the pause key) does **not** return to the menu: 0x42a1c0 falls
  into case 0 with result −1 (BGM stop, clear, 3 ticks), `Game_MainFlow` sets `0x855ffc |= 0x10`
  and attract restarts at the title picture, the same as a title cancel. **confirmed**
  (The port returns to the menu instead: decided port behaviour, §14.1.)
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
* Sound off → stop all (0x430160); sound on → restart the title BGM 0x19e. While the byte is
  non-zero the sound thread drops the play/stop/pitch messages (`cmp 0x856356` at 0x4305bd).
  **confirmed**
* List-page skips (0x42a560): with no FF device the cursor jumps over item 2, with no sound
  device over item 3, in both directions; the analog branch maps notch 3 → 2 → 1 the same way.
  The control page (0x42ad10) always draws all seven labels at (x0+0x20, y0+0x88+48 i); its
  pictures: slot 0x9a = `0x487684[mode]` at (x0+0x118, y0+0x90) and slot 0x9b = 0x494, 0x495,
  0x496, 0x497, 0x498, 0x493 for modes 0–5 at per-mode offsets; mode 6 shows 0x49d and 0x4a1.
  **confirmed**
* Labels 20/21 ロードする/セーブする exist in the sheet but are not in any item list (leftover).
  **confirmed** (lists at 0x47c744–0x47c76c)

## 5. Records viewer (0x41f5e0)

* Created from the title (item 2, with `0x855ffc |= 8` → interactive) and from attract.
  Sets `0x855ffc |= 0x1000`, `0x855ff8 |= 0x2000` (and `|= 4` in attract), sets the grey clear
  colour 0x707070 (0x428990(0x70, 0x7070); no 3D task runs behind it, so the page is on grey).
  **confirmed**
* Pages: the route index `r` steps through 0, 1, 2, … and shows a page when bit `r` of `0x8500b0`
  is set and the ranking-table index `0x46bbe8[r] ≠ −1`; it ends after the first `r > 12` that fails.
  With `0x8500b0 = 0x3f9da` (bits 1, 3, 4, 6, 7, 8, 11–17) this gives **routes 1, 3, 4, 6, 7, 8,
  11, 12, 13, 14, 15, 16, 17**: thirteen pages, one per ranking table. **confirmed** (0x41f910,
  immediate at 0x41e52c; an earlier version of this note listed 9 and 10 instead of 11)
* Layout: header maps `0x46bc10[r]` (slot 0x67) and `0x46bc64[r]` (slot 0x68) slide in at y = 40
  (x from −176 and 576, 8 px per tick, 33 ticks); column titles tbl 0x413–0x418 (slots 0x6c–0x71);
  10 rows at y = 92 + 32·rank, 8 cells per row (4 name characters, 4 score digits) at
  x = 144, 176, 208, 240, 360, 392, 424, 472 (0x46be42), character sprites `0x46bd30[c]`; badge
  tbl 0x2c8 ("PRO") at slot 0x5d+rank, x = 272, when the row's flag is 1. Cells reveal one per 2
  ticks, flickering between tbl 0x274/0x275 for 16 ticks. Legend tbl 0x545 (keyboard) or 0x546
  (pad) at (128, 432). Black side bars from the rect layer (0x41fc60). **confirmed**
* Timing: in attract a page lasts 200 ticks. From the title (interactive) the counter only runs
  up to 175 (all cells shown, header in place) and **holds** there: confirm (0x17) jumps to 175
  (and the header slide to its last step) or, if already there, to 200 = the next page; the
  viewer ends after the last page. Cancel (0x13) undims and ends the task on the next tick. In
  attract the viewer ends when `0x855ff8 & 4` clears. Cells: cell u appears at counter 2u.
  **confirmed**

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
  from (424+16k, 64), rank badge (428,20) = 0x314 初級 (1 star), 0x315 中級 (2–3; the
  stack table is 0x314, 0x314, 0x315, 0x315, 0x316, 0x317 indexed by difficulty), 0x316 上級 (4),
  0x317 特級 (5), clear star (36,12), replay mark (40,40). Stages 0x11/0x12 add a
  雫石停車/通過 tag (tbl 0x549/0x548). Header bar tbl 0x515 at (x0, 0); legends slots 0x87–0x8e,
  0x9c, 0x9d (descriptors 0x47cab0–0x47cb08, keyboard vs pad variant by input mode).

### 6.2 Navigation and results — confirmed

* Up/down (or the power notch) moves; confirm plays 0xd, blinks 30 ticks, stops 0xd. At line
  level that opens the train list (count = entries until −1, max 6); at train level it ends with
  `+0x48 = cursor`. Cancel (0x13) at train level returns to the line list with the cursor on the
  previous group; at line level it ends with `+0x44 = +0x48 = −1`.
* **R key (bit 0x4000)** toggles `0x816974` ("自動運転", replay playback) while in the train list. When
  on, each row whose stage has a stored replay (`0x42e450(stage) ≥ 0`) shows tbl 0x50b (row 6) at
  (40,40). This is the only reader of bit 0x4000 found (input.md lists none). It plays no sound
  and is ignored at the line level.
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
  attract demo-route picker (§2). Bits set: 1, 3, 4, 6, 7, 8, 11–17: one route per ranking table
  (`0x46bbe8[r] ≠ −1` minus 9, 10, 18, 19, which share tables 5, 7 and 6 with 8, 11 and 17).
* What *is* stored per route is the **clear mark** `0x85635a[route]` (eeprom +0x33a, 20 bytes),
  written by `Stage_Run` after a clear: 3 = cleared without a continue, 2 = 1–3 continues,
  1 = 4 or more continues; only ever raised. It is written right after the report card of a
  clear, before the ending and the name entry (§11.4). For select stage 20 (route 10 with
  `0x48ddf0 = 1`) `Stage_Run` sets `0x8570a4 = 0x14` around the write, so that variant's mark is
  a 21st byte at `0x85635a + 20` = eeprom **+0x34e**, then restores route 10. **confirmed**
  (0x4080d0). Route select shows it as the star tbl 0x54b with
  sub-row `(3 − mark)·2` (three star colours). The attract code 0x404cf0 also tests it
  (**guess**: to pick a demo stage the player has not cleared).

## 7. Pre-run screens

Order in `Stage_Run` 0x4080d0 after the route select (main-loop.md §5):

1. **Lever check** 0x4137e0 (**analog-lever modes only**; digital modes just force P0/EB). Shows
   "仕業検査" (tbl 0x249, slot 10 at (224,108)), "各レバーをスタート位置に合わせて下さい" (tbl 0x248,
   slot 11 at (160,364)), controller picture tbl 0x50a (slot 0x11 at (156,160), palette 0x18) and
   arrows tbl 0x3b/0x3c (only preloaded by 0x4317e0(0x465abc); 0x4137e0 images slots 10, 11 and
   0x11 only and merely places 13, 14, 18, 19, so who draws the arrows is open). Ends when the brake has been ≥ 9 with power 0 on **50 ticks** (not
   necessarily consecutive) or after **400 ticks**. **confirmed**
2. BGM 0x19e stopped, route data load `0x40ed90`, 15 ticks and one more.
3. **Stage intro** 0x408ec0 (scene.md §9; skipped on retry): the fly-by 0x4091a0 (stages 1, 5–8,
   11–15, 18; 144 ticks) or the chase 0x408f40 (the others; 145 ticks), the train's caption
   card (slot 0x1d at (24, 408)), BGM 0x197; horn or A skips. Then two ticks and the setup
   calls 0x40d040, 0x407900, 0x40f7e0 with a tick after each of the last two (main-loop.md §5).
   **confirmed** (code; oracle trace of stages 0, 1, 13: 16 ticks before, 4 after)
4. **Departure board** 0x4139c0 (skipped on retry): board tbl 0x3d8 "発車案内" (slot 58 at
   (112,206)) with four flaps: slot 54 series (128,256), 55 train type (376,256), 56 from (136,320),
   57 to (328,320). From tick 10 (sound 0x18) flap k starts at tick 10+8k, spins 32 ticks
   (tick s of the spin: s % 3 = 0 → tbl 0x3d6, 1 → 0x3d7, 2 → entry `(s / 3) % len` of its list;
   deterministic, not random), then lands on the route's entry
   from `0x467040[route·20 + k]` (lists 0x466fa0 series, 0x466fd8 type, 0x466ff8 stations; with
   `0x48ddf0` the **destination** flap (k = 3, slot 57) is forced to station entry 2; an earlier
   version said the train type). Every tick after the fourth landing stops 0x18 again; the screen
   lasts 136 ticks in all. When all four have landed sound 0x18 stops; the
   screen ends after **70 more ticks**. **confirmed**
5. Route load, then the per-station loop. At the first segment the HUD and the departure task
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
  per-stage car list 0x45735c by 0x4596e0) and, after an overrun of 10 m or more, replaces the
  stop-distance record at 0x850300 with a made-up 121-sample approach. It is a replay of the
  last 4 s of the approach in a platform set at the origin, in two viewports (orbit view right,
  platform monitor inside the frame tbl 0x25e); the route is not drawn. Traced and built:
  scene.md §8. **confirmed** (code)
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

Implemented in `crates/ddg-game/src/screens/ranking.rs` (tables read from the exe) and wired by
`screens/flow.rs` / `play.rs`; headless shots of stage 1 (time-out run, value 70 → rank 6)
show the art lining up with these positions. Compared with a Wine capture of the original
(stage 1, train left standing until game over, `scripts/capture-original.sh` with a 200 s
recording; 2026-10-17): the Japanese name entry at 640×480 matches pixel for pixel except the
timer digits, on the same black. Shots in `out/explain/name_entry/`.

* Ranking value (rules.md §9) capped at 9999. Routes with no ranking table
  (`0x46bbe8[r] = −1`: 0, 2, 5) skip this screen.
* **PRO flag** `0x85723c`: cleared at a run start (0x40d040, together with `0x8571f0`), set by
  `Stage_Run` after the station loop when `0x8571f0 ≤ 30.0` (f64 at 0x458b90). `0x8571f0` is
  accumulated only by `Hud_Distance` 0x4162c0, on the path where the distance panel is drawn
  (panel active `0x856d0c == 1`, panel on `0x856c70 == 1`, option `0x856355 ≠ 0`):
  `+= 0x857234 × (f64 0x4676f0 ≈ 1/30)`, and `0x857234` is the speed in m/s, so it is **metres
  driven with the distance meter shown**. The flag therefore marks a run driven (almost) entirely
  with the 停止位置 meter hidden, i.e. with the "Professional" plate tbl 0x2c7 showing; the badge
  on the ranking page is tbl 0x2c8. **confirmed** (code), meaning **likely** (not seen in a run).
  It survives continues (0x40d040 is not called on a continue).
* Insert position: first row whose score is lower, or equal with a lower PRO flag; not in the
  top 10 → skip (a tie with row 10 does not enter). Rows below shift down with their PRO flags;
  the new row gets name `0xb ×4` (blank), the score and the run's flag. **confirmed**
* Task fields: +0x30 counter, +0x34 page (0 entry, 1 ranking), +0x38 route, +0x3c symbol,
  +0x40 letters entered, +0x44 done tick (99999 = not done), +0x48 rank, +0x4c wheel offset,
  +0x50 table, +0x54 previous power notch, +0x56 lever-take flag, +0x58 hold counter.
* Name entry page (first update, counter 0): placements `0x46bec4` (`{x, y, layer, pal, slot}`,
  −1 terminated: slot 0x6b (80,124), 0x6c, timer digits 0x18–0x1b at (240..360, 84) layer 2
  palette 5, label slot 0x1c at (256,60)); slot 0x1c = tbl 0x261 (持ち時間), slot 0x6b =
  `0x46bcd8[rank]` (a train picture with four letter boxes: 0x419 ranks 1–3, 0x41a ranks 4–9,
  0x41b rank 10), then 0x4316e0 with palette 4 layer 3: 0x6c = 0x41d 乗務記録 (80,72), 0xc =
  0x41c selection frame (288,324), 0xf = 0x30 (112,392), 0x10/0x11 = 0x34 (80,360)/(496,360)
  (black covers at the wheel ends), BGM 0x19d (0x42ff20). Every tick: slot 0x6d = legend
  descriptor 0x486184 (keyboard/pad) or 0x48618a (analog lever) at (408,60).
* Background: **black**. The 3D view is gone (`Stage_Run` killed the world lists before the
  report card, §11.4) and the clear colour is still black, so the covers 0x34 and 0x30 (palette
  colour 0,0,8) are invisible and only cut off the wheel's outer symbols. **confirmed** (capture;
  the port once drew the frozen cab view behind the entry, which showed the covers as black
  boxes over the scenery)
* The **59-symbol wheel** (`0x46bd30[0..58]`: 0–9, 10 = "00", 11 = blank, A–Z, `. - ! ? /`,
  train-class abbreviations and katakana pairs, 系, 形, 57 = ← (delete), 58 = 終 (end)):
  * Keyboard/pad (input modes without the analog lever): left held (held bit 4) checked first:
    the hold counter goes −1, −2, … (reset to 0 first if positive) and the offset moves −2 px for
    counts −1..−4, −4 for −5..−9, −8 for −10..−14, −16 from −15; right held (bit 8) the same
    upwards. Neither: the counter resets and a non-zero offset keeps moving **2 px/tick in its
    own direction** until the step completes, so a tap always ends one whole symbol further.
  * Analog lever (0x433270 ≠ 0): `0x829d04` (brake) 0 → −16, 1 → −8, 2 → −4, 3 → −2, 4–6 like no
    key, 7 → +2, 8 → +4, 9 → +8, 10 → +16 px/tick.
  * Wrap: offset ≥ 32 → −32 and symbol −1 (0 → 58); offset < −31 → +32 and symbol +1 (58 → 0);
    either plays 0x15. So left/brake-released brings the following symbols in from the right.
  * Take: confirm (pressed 0x1000), or in analog modes the power notch rising from 0
    (`+0x54 == 0 && 0x829d00 > 0`). 終: sound 0x1d, the remaining letters blank, done; ←: sound
    0x25 (also with nothing to delete), last letter blanked; else sound 0x17, letter stored, done
    at the fourth (sound 0x1d). Done = done tick `counter + 30`, the filter, then
    `Ranking_ToEeprom` 0x41e670. Input is ignored once done.
  * Drawing (every tick): letters slots 0x55–0x58 at (272 + 32k, 252), blank for positions not
    yet entered. Previous symbols: slots 0x45–0x4b, `x = off + 192 − 32i`, drawn at
    `x + 64 (+ trunc(off·0.5) for i = 0 when off > 0)`, y 360, only while `x ≥ 16` (an
    off-screen slot keeps its last image and position). Next symbols: slots 0x4d–0x53,
    `x = off + 288 + 32i`, at `x + 64 (+ trunc(off·0.5) for i = 0 when off < 0)` while
    `x ≤ 464`. Current symbol slot 0xd at (304,352) when `off = 0`, else
    `(304 − trunc(off·−1.5), 360)` (f64 0.5 at 0x457730, −1.5 at 0x458c08): at rest the current
    symbol sits 8 px higher in the frame with 48 px gaps either side (not a larger sprite).
  * Timer 0x417040(930 − counter): `s = t/30`, `t%30 < 26` → digits `0x4653d4[...]` on
    0x18–0x1a, 26–27 → mask 0x40b ×3, 28–29 → 0x40c ×3; slot 0x1b = 0x40e (秒). No blanking of
    leading zeros.
  * Page change when the counter reaches **930** (31 s) or the done tick: page 1, counter 0,
    the filter again. **On a time-out `Ranking_ToEeprom` is not called**: the row (with the letters
    entered so far, filtered) exists only in the unpacked table `0x84fca0` until some later
    completed entry on the same table packs it, and is lost at exit otherwise. (The port saves
    it like a completed entry: decided port behaviour, §14.1.)
* Name filter 0x41e7a0 (then `Name_Replace` 0x41e4c0 writes `0x46bf14[route]`, 4 bytes): list
  `0x46bf64` = exact matches (blank ×4, KKKK, the three single-blank variants, `SEX ` / ` SEX`);
  list `0x46bf88` = match when symbols 1–3 are equal and symbol 0 **differs** from the entry's
  (entries start with 0x3c, no symbol, so in effect `?KKK`, `?SEX`); list `0x46bf94` = match
  when symbols 0–2 are equal and symbol 3 differs (`KKK?`, and `SEX` + anything but Y — the
  `SEXY` entry itself passes). **confirmed** (objdump of the compares).
* Ranking page (page 1), first update: placements `0x46be54` (headers 0x67/0x68 at (80,40) and
  (320,40), column titles 0x6c–0x71), slot 0x6b hidden, 0x67/0x68 = `0x46bc10[route]` /
  `0x46bc64[route]` (line and series), column titles tbl 0x413, 0x415–0x418, 0x414; all 80 cells
  (slots 0xd–0x5c, 8 per row at `0x46be42` x = 144 176 208 240 360 392 424 472, y 92 + 32·row)
  set to the blank symbol; PRO badge slots 0x5d+row placed at (cell 3 x + 32, y). The draw
  callback becomes 0x41fc60 (black rect-layer bars x 0–80 and 560–640, adjusted by the screen
  offset bytes +0x338/+0x339) and the clear colour becomes grey 0x707070
  (`Gfx_SetClearColour` 0x428990(0x70, 0x7070), renderer.md); destroy sets it back to black.
  With no 3D task left (§11.4) the page therefore sits on grey between the black bars.
  **confirmed** (Wine capture: 115,113,115 in the 16-bit frame, bars 0,0,0)
* Each tick while the counter < 140: cell `u` (row `u/8`, column `u%8`) appears at counter `u`:
  for 16 ticks it alternates tbl `0x46be1c`/`0x46be20` (0x274/0x275), then shows the symbol of
  the unpacked byte (name 0–3, score digits 4–7, leading zeros shown); with column 3 the row's
  badge slot gets tbl 0x2c8 or −1. The new row's rank label `0x46bd04[rank]` (slot 0xc at
  (96, row y)) is shown when `(counter/4)` is odd, hidden otherwise.
* The page ends when the counter passes **150** or on the A button (0x100000, Enter in keyboard
  modes) or the horn (0x100). Destroy 0x41f5a0: undim, reset every non-keep sprite slot,
  clear `0x2000`, stop BGM 0x19d, clear `0x855ffc & 0x1000`. **confirmed**

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
* Before any of these, right after the station loop, `Stage_Run` kills task lists 2 (world),
  5 (traffic, weather) and 3 (HUD, UI) (`0x425380` on 0x84c198, 0x84c1bc, 0x84c1a4), runs one
  `Task_Tick` and stops every sound. The report card, name entry, ranking page and banner are
  therefore drawn over the clear colour only: black, grey 0x707070 on the ranking page. The
  continue countdown before still has the cab view. **confirmed** (code at 0x4080d0; Wine
  capture). The port: `StageFlow::world_view` is `Hidden` whenever `hud_visible` is false; the
  ending (0x4040a0) builds its own world and keeps the cab view while it runs.

## 12. Bonus game, coupling minigame and ending

The in-cab coupling minigame of stages 2 and 17–19 (0x4093c0) and the stage-1 bonus section are
rules: rules.md §7.5.

### 12.1 Bonus game (EF63) 0x41b540 / 0x41b810 / 0x41c2e0 — confirmed (code)

**When.** `Stage_Run`, after a station result and 0x40d340, when `0x856d78 == 0x459e10[stage]`
(i16[20]: stage 3 → 5, 4 → 4, 6 → 9, 10 → 8, 11 → 5, 12–15 → 10, 16 → 9, else 99) and no
continue was used: `0x40ee50(0x14)` (the TR sets of row 0x14: partsTR9), `0x430160` (stop all
sounds), `BonusGame_Create`, `0x407a00(1)` (the replay recorder takes the bonus as pseudo-station
0xff), wait on `0x855ff8 & 0x4000`, `0x407ab0()`. Then stage 6 checks the marks of stations 1–d78
(`0x40e350(1, d78)`): four or more load set 0x16 and set `0x855ff8 |= 0x2000000` and traffic
mask state `0x857216 = 2`; stages 10 and 12–15 reload with variant 1, the others with 0.

**What.** The player drives a pair of EF63 locomotives onto a standing 489-series car in a set
of its own (draw 0x41c3c0: mask 0x486160, white backdrop 0x4692e0, parts 0x2df3 at the origin,
0x327a at (−1.5, 0, car z), 0x2df2 at (−1.5, 0, pair z) and again 3.9 units behind; FOV 45°,
aspect 1.28). The route is not drawn (list 5 died at the arrival). `Vehicle_Set(0x1002)`: the
plate 信越本線EF63 (row 22) and picture 0x69. Timer 474 ticks (`0x417040`), remaining time and
distance panel off (`0x856d1c = 0`, `0x856d0c = 0`).

**Physics task** 0x4240d0 / 0x4241d0 (list 2, before the bonus task): train type 16, the
player's notches of the previous tick, no gradient, curve or traction fade, the brake cylinder
easing at 1.15 × its rate (0x4726e0), the usual resistances and brake force, no top-speed cap;
the EF63 horn (sound 3) on the horn button. The speed is `0x84c43c`.

**Bonus task** (counter `+0x38`, one update per tick):
* updates 1–60: nothing (black 3D); at 60 BGM 0x19a, the draw callback, slot 0x36 = needle 0x2db
  at (0x4696ac[r] + 248, 0x4696bc[r]), slot 0x37 = meter 0x2da at (0x46968c[r], 0x46969c[r]),
  slot 0x38 = 0x2d8 ボーナスゲーム at (160, 172);
* intro while `+0x38 ≤ 0xd2` (speed held at 0): at 0x50 the instruction 0x2d9 at (136, 172), at
  0xb4 0x2de 準備, at 0xd2 0x2df 開始 (272, 172) and sound 1; from 120 the camera moves from key 0
  towards key 2 (`t = (n − 120) × 0.0266…`, keys 0x4693c8: 5 × `{cx, cz, r_eye, r_target,
  y_eye, y_target, angle}`, eye `(sin(a + π)·r_e + cx, y_e, cos(a + π)·r_e + cz)`, target
  `(sin a·r_t + cx, y_t, cos a·r_t + cz)`);
* play: the instruction goes at 0xf0; before contact the pair moves `v × 0.00666666` units per
  tick (1 unit = 5 m) from z 4.4, the timer counts down; the gap is `z + 0.4` (24 m at the start),
  shown as `trunc(gap × 5)` m (header tbl 0x58 at (88, 38), digits 0x465540 from x 112, y 70),
  the camera between keys 3 and 2;
* contact (gap < 0): shock `S = trunc(min(v, 5.5556) × 0.18 × 216 + 248)`; `S ≤ 328` (below
  7.5 km/h) → 連結成功 0x2dc at (240, 164), else 連結失敗 0x2dd and sound 0x1b; sound 0x2a either
  way; the needle sweeps 2 px per tick to S; the pair slides on (−0.0004 per tick), then the car
  recoils (≤ 0.01 units); the camera turns from key 3 to key 4;
* time-out (timer 0 before contact): 0x2dd at (240, 200), 時間切れ 0x252 at (232, 240), sound
  0x11;
* 28 updates after settling: points `P = min((328 − S)/8 + 1, 10)` (0 and sound 0x11 when too
  hard, else the jingle 0x199) and `TimeMsg_Show(0x103, P)`: P bonus seconds; 30 updates later
  the prize picture (0x25d 玄米弁当 for 10, 0x25c 峠の釜めし for 1–9) at (0x46966c[r],
  0x46967c[r]), palette 15; 90 updates later the task ends.

Destroy 0x41c2e0: the stage's vehicle again, the slots cleared, flag 0x4000 cleared, FOV 30°.
The derail pose table 0x4695f4, `0x8502e0` and 0x41c540's tables have no reader on PC.

### 12.2 Ending 0x4040a0 — confirmed (code)

`Stage_Run` on a clear, after the report card and the clear mark: registry option bit 0x08
clears `0x855ff8 & 0x40000000`, 0x10 sets it; `0x40eed0(area)`; if the stage difficulty
`0x4656c4[stage]` is above 3 (stages 1, 2, 6, 7, 12, 14, 16): `0x855ffc |= 0x800` and
`Ending_Run(full = !(0x855ff8 & 0x40000000))` (rules.md §7.5 for who sets the bit). It runs the
full ending 0x404110 (task 0x4042b0, list 4) or the plain one 0x404920 (task 0x404a80, list 3),
waits on `0x855ff8 & 8`, kills lists 5, 4 and 7, two ticks; then the ranking follows.

**Credits** (both): text tables 0x45811c / 0x457a5c, 0x30-byte records of four 12-byte
sub-records (one per resolution mode) `{tbl (0 blank, −1 end), x, y, fade kind, n, duration}`.
A group of n records shows in slots 0x6e+ (palette 0x17, layer 3; y − 8 in the plain table) for
the first record's duration; the palette fades in over 24 updates from the title palette data
(kind 4 → 0x488290, kind 11 → 0x488e90) and out over the last 24. A blank record closes the
letterbox masks: `h = trunc((1 − count/duration) × C)`, top `94 + h`, bottom `base − h` (full: C
100, base 288; plain: 90, 268). The masks are depth-0 black quads over the 3D (0x486160 top,
0x486ec4 bottom, virtual y 0–94 and 268/276/288–400). Credits: 電車でGO!2 高速編 STAFF, the staff
names, JR東日本企画, 北越急行, おぎのや, Stack Lab, then GM R&D SECT#2 and TAITO.

**Full ending** (BGM 0x1a2, render mode 3: eye 0.04 and target 0.06 units lower than the cab
camera): t from −200; 0x408080(stage, 0), masks 94 / 276. While t < 0 the camera departs at
+0.3 km/h per tick (to 60 km/h). Then every 160 updates the next timetable row with `f4 ≠ −1`
(so stage 1 shows stations 1–8 and 25): update 0 closes the masks (194 / 188) and kills list 5;
update 1 places the camera at the previous station (`0x408080`) and jumps it `J` m in one tick
(`v = J × 0.1 × 1081.081`; stop: `J = to_next − 100`, pass: `to_next − f5`); update 2 opens the
masks (94 / 288) and sets the approach (stop: v = 21620/160, a = −21620/160² per tick, to stop
at the mark; pass: f5 m per 160 ticks, +0.2 per tick), the station plate (0x417ab0: name
`0x4665c4[id]`, 駅, 通過 0x6c / 停車 0x6e at y 16, centred) and the clock panel (0x417410:
label 0x20a–0x20d at (X+64, 48), scheduled time in the orange digits 0x465d8c, the run's time
from the arrival record `0x8570e8` in yellow 0x465dd0 or orange when 30 ticks late, and for a
stop the stop error `trunc(dist × 100)` in cm below 100 or m). After the last station the final
shot of `0x4587dc[stage]` (9 × i32: station, jump m, camera km/h, spawn, speed, param, decel,
flags, model): the camera at that station, an AI train spawned (`0x404f10(spawn, flags | 5,
speed, param, model)`; model 8 for stage 2 with train type 0xe), then the camera moving
backwards at `cam` km/h ahead of the oncoming train, braking by `decel × 0.1` per tick from t =
4040. List 5 dies at t = 4340; the task ends at t = 4590.

**Plain ending** (BGM 0x19f, render mode 0): t from −160, the camera departs station 0 and
accelerates +0.1 km/h per tick to 110; list 5 dies at t = 3540, the task ends at 4010. Its
destroy releases 0x1a2 instead of 0x19f (a game bug; Stage_Run's stop-all ends it).

**Skip.** Enter or Esc ends either from its 11th update (`t > −190` / `t > −150`).

**Not the ending:** 0x4240d0 is the bonus game's physics task (§12.1), not an ending train
(physics.md corrected). The stage intro (0x408ec0 → 0x4091a0 / 0x408f40, §7, scene.md §9)
runs before a run, not after it.

## 13. Save data

### 13.1 `eeprom.dat` (0x35a bytes, mirror at 0x856020) — confirmed unless noted

Loaded at startup (0x434ed0, then `Ranking_FromEeprom`), **saved only at exit** (0x434f50 from
`WinMain`'s shutdown path). No header or version. `fopen("eeprom.dat")` + `fread(0x35a)`; only a
full read sets the difficulty (`+6 >> 14`), unpacks the rankings and resets a control mode > 3
when no joystick is present (`0x8298a4 == 0`).

**First boot / defaults — confirmed.** After the load, the boot sequence 0x40e910 calls 0x418ef0,
which compares the first **two bytes** of the mirror with the default image (0x418eb0, a memcmp)
and on a mismatch copies the whole default image in three pieces: `0x467768` → +0x00..+0x22
(starts with the magic `"CG"`), `0x467790` → +0x22..+0x332 (the 13 default ranking tables),
`0x467aa0` → +0x332..+0x35a (options: control 0, vibration 2, speed meter 1, distance meter 2,
sound 0; no clear marks). So a missing file (mirror still zero) or a foreign one gets the
defaults; a short file with the right magic keeps its partial bytes over zeros. The same ranking
table is restored alone by 0x418e70 / 0x41e720 (debug/test-mode resets, callers 0x4021f3 and
0x4223d2). The default tables are each sorted by score; table 3 (route 1) is 124, 113, 100, 90,
77, 64, 54, 41, 28, 18 with name `E3系 `. `crates/ddg-game/src/save.rs` reads the image from the
exe at runtime.

The checksum words +0x20 and +0x330 are rewritten by 0x418930 after every pack; its sum routine
0x425dc0 is `xor eax,eax; ret`, so both are always 0 on PC. **confirmed**

Our port keeps the file byte-compatible and stores it at `$XDG_CONFIG_HOME/ddg/eeprom.dat`
(else `~/.config/ddg/`, `%APPDATA%\ddg\` on Windows); headless shots use the defaults and never
write. Besides the exit write, the port writes the file right after every change (§14.1).

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
| +0x34e | 0x85636e | clear mark of select stage 20 (route-10 variant, §6.3) |
| +0x34f–0x359 | | unknown (0 in the defaults) |

Ranking tables per route (`0x46bbe8`, i16 per route): 0:−1, 1:3, 2:−1, 3:10, 4:11, 5:−1, 6:0,
7:4, 8:5, 9:5, 10:7, 11:7, 12:9, 13:8, 14:2, 15:12, 16:1, 17:6, 18:6, 19:6. In memory the tables
are unpacked to `0x84fca0 + 0x50·table + 8·row` (4 name bytes + 4 decimal digits) and
`0x84fc00 + 10·table + row` (PRO flag). The defaults come from the exe (above).

### 13.2 Other stores

* Registry `HKLM\Software\TAITO\電車でGO！２ 高速編 ３０００番台`: `Option` (0x80-byte blob →
  `0x816840`: resolution, windowed, nosound, …) and the fog values; written by the separate
  `DengoSetting.exe`, not by the game's menus (main-loop.md §1). Option bits used by the flow:
  0x20 = debug start (route 1, station 7, no select), 0x18 = skip the departure board block,
  0x10/0x08 = ending variant. **confirmed** (bits), meanings **guess**.
* `..\BIN\REPLAY.DAT`: replays per difficulty/stage (main-loop.md §7); route select's 自動運転 mark
  and the pause-dialog text "運転を記録しますか?" belong to it.
* `joystick.dat`: calibration (input.md §5).

## 14. The port's front end

`ddg-game` with no subcommand (`crates/ddg-game/src/app.rs`) sequences `Game_MainFlow`: attract →
title menu → route select → lever check (modes 5/6) → stage intro (fly-by or chase,
`play/intro.rs`, its own run) → departure board → the run (`crates/ddg-game/src/play.rs`,
built from the app's shared window, sprite bank, layer, save,
input and sound device) → attract again. The screens are `crates/ddg-game/src/front/`, one state
machine per task, with the exe tables read at runtime (`front/tables.rs`). Headless checks:
`ddg-game --inputs <script> --shot <ticks>:<file.ppm>` (script keys as for `play`, plus `r`).

Faithful (code reading, unit-tested, art lined up in headless shots; not compared with the
running original): the title menu (list scroll, clamps, 30-tick blink, sounds, five items with
mode 6 only), the options list and both value pages with `Options_Get` / `Options_Set` on
`eeprom.dat`, the records viewer, the exit dialog, the route select (both levels, settle-on-back,
clear stars, rank badges, the 雫石 tags, R toggle), the lever check, the departure board, the
calibration screen (writes `joystick.dat` next to `eeprom.dat`), the tick gaps between screens
listed above, BGM 0x19e on the title / options / route select, and the attract title picture.
The stage intro (both kinds, `play/intro.rs`) is compared frame by frame with Wine recordings
of stages 0, 1, 9 and 13 (scene.md §9.3).

Approximated or left out:

* Attract walks its whole table: the logo, the cold-start opening, the title picture, the demo
  drives (`play/demo.rs`: a `Play` on `Drive::start_demo` with the overlay), the photos and the
  poster, with the walk and photo counters kept across walks. The opening is `play/opening.rs`
  on a stage-1 `Play`. Approximated: the opening's E3 spawn uses the normal spawner (rear bogie
  10 points back, not 12); the GMQ logo of the route demo animates on
  the rail-joint clicks of the sound module, so it stands still without sound (headless). The
  Komachi movie plays at item 3 (movie.md); a skip restarts the walk at the title picture, its
  end goes on to the demo route.
* The opening on other window shapes: render mode 2 (A, C) is the 496×400 window scaled with
  the centred 640×480 design (it has to stay on the photo), black around it; in E and F the 3D
  outside the inset x 64–576 is masked black to the window edges and the canvas bars reach the
  edges of a wide canvas, so every shape shows the 4:3 picture (compared frame by frame with a
  Wine capture of the original at 640×480).
* The ending (`play/ending.rs`) and the bonus game (`ddg_engine::bonus`, `play/bonus.rs`) run in
  the end-of-run flow and between stations. Approximated: the registry ending bits, the bonus
  game's force feedback and motor-pitch sounds, the bonus aspect 1.28; the bonus game is not
  recorded for replays (`REPLAY.DAT` is not implemented). `ddg-game play <stage> --ending
  full|plain` starts with the ending to check it.
* Replays (`REPLAY.DAT`) are not implemented: R toggles the flag, no row ever shows the 自動運転
  mark, and leaving the select switches the flag off (what 0x436640 does without a replay).
* Select stage 20 runs route 10 with its own clear mark (+0x34e) and the departure-board
  destination; the other effects of `0x48ddf0` (replay slot 0x14 in 0x407900, the rules branch at
  0x40aa42, which also reads the clear mark of the route) are not modelled.
* No force feedback: the vibration item is skipped as on a PC without an FF device. The Shift
  override's keyboard legends (0x829d08) are not drawn. The difficulty value page writes the
  eeprom; a run reads it at its start.
* The 4:3 layout is always centred (the port has no non-centred mode), so the
  `0x855ffc & 0x1000` toggling has no visible effect.
* **Enhancement `wide_menus`** (port, approved by the user as widescreen presentation; setting
  `wide_menus`, on by default, `--no-wide-menus` keeps the 4:3 menus over black; it shows
  whenever the window is wider than 4:3, in both HUD layouts and every `--ui-scaling`): the
  front end's canvas takes the window's shape (`App::fit_front`) with every slot still
  centred, so items, header text, legends and cursor keep their 4:3 places and a 4:3 window is
  unchanged (compared pixel for pixel at 1280×960). Only the **backdrops** are continued to
  the window edges, never stretched (`front::mark_backdrops`, `SpriteBank::set_wide_fill`,
  `sprites::Repeat`):
  * tbl 0x501, the 40×30-tile background of slot 0xb7 (title menu, options and control page,
    calibration): the diagonal 3000番台 logo pattern is a block of **14×14 tiles (224 px)**
    that repeats from column 14 to the right edge (and in rows 14–29 from column 0); the
    top-left block (rows 0–13, columns 0–13) uses other tile ids. **confirmed** (tile ids of
    tbl.rom entry 0x501);
  * the header bands of 0x42a820 (per-page i16 table 0x47c50c, e.g. tbl 0x516/0x517) and
    the route-select bar tbl 0x515: 40×7 tiles, all one black tile (0x2569) except the
    coloured stripe in columns 3–4, so they repeat with a period of one column.
    **confirmed** (same).

  The period is not hardcoded: `Repeat::of` finds the shortest period of the columns at the
  right edge of the map in its tile ids, and every column outside the map maps into that run
  (`start + (c − start) mod period`), so the tiling phase continues the original's centred
  picture on both sides. A slot is continued only when it shows the whole map across the
  full 640-px design.

  The **attract title picture tbl 0x410** (§2.6, slot 0xf at (0, 0)) has no tile pattern
  (`Repeat::of` finds none) but plain paper texture at its edges, so it continues with
  **paper synthesised from that texture** (`SpriteBank::set_wide_paper`, `sprites::Paper`,
  `sprites/paper.rs`; constants in `front/attract.rs`), on the CPU at art resolution, for
  the palette it is drawn with (rebuilt when the palette changes, e.g. during its fade-in):
  * **exemplars**: rows 256–479 of the 128 outermost columns on each side, minus their light
    (blurred row and column means), made tileable by offset-and-blend (each wrap edge
    cross-fades with a copy offset by half a period, contrast-preserving);
  * **synthesis**: tiling-and-blending by example (Heitz & Neyret 2018): every vertex of a
    triangle lattice (40-px edges) shows one exemplar at a hashed offset, and each pixel mixes
    the three vertices of its triangle by (sharpened) barycentric weight with a
    variance-preserving blend, so nothing repeats even at 32:9 and the joins do not go flat;
  * **light**: the mean colour of 12 columns at each edge (skipping the outermost, which on
    the right is lighter than the paper next to it), row by row, blurred vertically (σ 12 px
    at the picture, turning to σ 40 px over the first 96 px out, so the edge's blotches do
    not run on as streaks): the picture's vertical lighting gradient continues;
  * **ramp**: the 24 outermost columns of the picture fade into the fill (smoothstep,
    contrast-preserving). In the logo band (rows 48–255) the glow left of 電 reaches x ≈ 15
    and the shadow of 番台 / the signal head x ≈ 631, but the art itself stays inside
    x = 32–609, so only glow and shadow fade out; outside the band "PRESS START BUTTON", the
    TAITO logo and the copyright lines lie in x ≈ 141–500. **confirmed** (measured on a
    640×480 shot of tbl 0x410 in palette 0x17).

  This replaces a mirrored continuation (tile columns flipped about the edges, the logo band
  borrowing the edge of rows 16–28), which showed a line at the mirror axis and a brightness
  step at the borrowed band. Before/after crops of both seams at 1280×800, 2560×1080 and
  3840×1080 are in `out/explain/title_wide/seam_*`.

  The 21:9 window (2560×1080) needs 249 original pixels per side, 32:9 (3840×1080) 533; both
  show only texture (`out/explain/title_wide/`).

  Everything else stays centred with black beside it: the records viewer's framed table (it
  draws its own black side bars), the route/train select (its background is black; the 0x515
  band continues black on black), the departure board (black around the board), the TAITO
  logo item (black), the photos (small, centred on black), the posters tbl 0x412 / 0x40f
  (photographs to their edges: a mirror would show mirrored trains and signs) and the
  stage-flow screens over the 3D view (their panels, e.g. the report card tbl 0x444, are
  framed art).

  At 4:3 nothing changes (1280×960 smooth, integer and nearest and 1024×768 compared pixel
  for pixel). With `--ui-scaling integer` a 4:3 window that is not a whole multiple of
  640×480 (e.g. 1600×1200) gets a canvas as wide as the window (`wide_canvas_width`), so
  there the backdrops, and now the title picture, fill the side margins too, as since the
  menu backdrops were added.

### 14.1 Decided port behaviour (deviations, on by default)

The user decided these deviations from the original. Each is on by default and has its own
setting (docs/settings.md); off restores the original's behaviour.

| What | Original | Port | Code |
|---|---|---|---|
| Name entry time-out (`save_timed_out_names`) | after 930 ticks the row is shown on the ranking page but `Ranking_ToEeprom` (0x41e670) is skipped, so it is never written (§11.2) | packed and saved like a completed name | `screens/ranking.rs` (`NameEntry::committed`), `save.rs` (`Save::finish_ranking`) |
| When `eeprom.dat` is written (`save_on_change`) | only at exit (0x434f50, §13.1) | also right after a ranking entry, a clear mark, an options change (options list and value pages) and a calibration; same 0x35a-byte format, written to `eeprom.dat.tmp` and renamed. The exit write is unchanged. Headless `--shot` runs have no save path and never write. A failed write is a warning; play goes on | `save.rs` (`Save::persist`, `write_atomic`), `play.rs`, `app.rs` (`apply`, `save_calibration`; `joystick.dat` is written atomically too) |
| Keyboard with a controller (`keyboard_with_controller`) | in modes 4–6 only Esc, Enter (menu confirm) and R reach the game (input.md §6) | the keyboard's horn (Space) and A / ATS confirm (Enter) also work while driving, OR-ed with the pad's buttons; the levers stay the controller's | `gamepad.rs` (`Joystick::poll`), input.md §10.3 |
| Exit dialog いいえ (`exit_no_to_menu`) | attract restarts at the title picture (§3) | the title menu comes back after the same 3 ticks, cursor on ゲーム終了, menu BGM restarted | `app.rs` (`exit_declined`) |

Evidence: unit tests `screens::ranking::tests::timeout_filters_and_commits`,
`screens::flow::tests::time_out_offers_continue`, `save::tests::saves_after_every_change`
(a temporary `$XDG_CONFIG_HOME`, passed to `save::config_dir_in`, never the real home),
`save::tests::headless_saves_never_write`,
`gamepad::tests::keyboard_drives_buttons_alongside_a_mode_4_pad`,
`app::tests::exit_no_returns_to_the_menu_on_quit`; with the settings off
`screens::ranking::tests::timeout_without_the_setting_is_not_committed`,
`save::tests::without_save_on_change_only_the_exit_writes`,
`gamepad::tests::keyboard_buttons_off_is_the_original_mask`,
`app::tests::exit_no_without_the_setting_goes_to_attract`; and a headless run (title menu → ゲーム終了 →
いいえ → title menu on ゲーム終了), shots in `out/explain/save_input/`.

## Open questions

1. Group 4 of the line select (header 東北路線 again, stage list `2, 2, 2, 2`, thumbnails 104, 103,
   0): the code makes it reachable (the line level is created with 5 items, 0x435490, and the
   cursor clamps at 4), so the port shows and selects it; why stage 2 four times is open. Stage 19
   (route 19) is in no group. Needs a run of the original.
11. Who draws the lever-check arrows tbl 0x3b / 0x3c (§7), and what 0x40aa42 does with a clear
    mark of 3 at station 7 of the route-10 variant.
2. ~~`0x855ae8`~~: resolved, the cached pixel width of tbl 0x504 (§1.3).
3. ~~Default ranking contents on first boot~~: resolved, the image at 0x467768/0x467790/0x467aa0
   copied by 0x418ef0 when the magic differs (§13.1).
4. The PRO flag (`0x8571f0 ≤ 30.0`, metres driven with the distance meter drawn, §11.2): the
   code is clear; that it is meant as "driven with the Professional plate" is still to be seen
   in a run with the meter on and off.
9. ~~Whether the name-entry art leaves the 3D view undimmed~~: resolved, there is no 3D view
   after the run (§11.4); 0x428990(0x70, 0x7070) is the grey clear colour of the ranking page
   and the records viewer (§11.2, §5).
10. Whether the A-button/horn end of the ranking page and the confirm of the name entry can
    collide with a key still held from the report card (both read edges, so probably not).
5. Attract, the opening, the demos, the bonus game and the ending are read from code (§2, §12);
   none of it has been compared with the running game. Worth a trace: the opening's +150-unit
   camera jump at the hand-off, the bonus game's timings (0x41b810 +0x20/+0x34/+0x38/+0x44,
   `0x84c43c`) and one full ending (0x4042b0 +0x3c/+0x44/+0x74 and the mask words).
6. Sound code meanings (0xd, 0x13, 0x16–0x18, 0x23, 0x2d, 0x195, 0xf7–0xfe, jingles 0x1000–0x1004,
   BGM 0x19d/0x19e) are from context only; map them with the audio notes.
7. `ExitConfirm_Run` return value: Ghidra lost it; confirm that はい = 1.
8. Whether the station-result palette choice (`dist > 0.01` → palette 4) reads the double at
   0x4676d0 as 0.01 (low dword 0x47ae147b fits) — confirm in the disassembly.

# dengo3k.exe — main loop, timing, game flow, replay

Scope: WinMain, frame pacing, task scheduler, top-level game flow, the per-tick driving step
(call sites only), threads, replay system. Source: Ghidra decomp in `re/decomp/dengo3k/` plus
`objdump -d` of `BIN/dengo3k.exe` for code Ghidra did not turn into functions.
Confidence: **confirmed** = read directly from code; **likely** = strong inference;
**guess** = plausible, needs an experiment.

The CD check is called from WinMain (`FUN_00425ca0` wraps `FUN_00425bf0`/`FUN_00425bc0`). It is
copy protection and is deliberately not analysed here.

## Key functions

| Address | Suggested name | Role | Conf. |
|---|---|---|---|
| 00438ecb | `WinMainCRTStartup` | CRT entry; calls WinMain with `GetModuleHandleA(0)` | confirmed |
| 00429bf0 | `WinMain` | Plain Win32 WinMain (not MFC `CWinApp::Run`): single-instance check, CD check, DDraw caps check, registry, window, replay load, `setjmp`, init, game flow, shutdown, replay save | confirmed |
| 0042a0e0 | `MainWndProc` (label) | Window procedure of class `"DENGO3K"` | confirmed |
| 0040e910 | `Game_Init` | Loads resources step by step (`"GAME_INIT_00_24_STEP"` debug markers), starts the sound thread, creates the render task | confirmed |
| 00419510 | `Game_MainFlow` | Never-returning top-level flow: attract → title menu → mode/route select → stage → loop; soft-reset `setjmp` target | confirmed |
| 004253c0 | `Task_Tick` (a.k.a. `WaitFrame`) | One game tick: runs every task's update, pumps messages, polls input, handles soft reset | confirmed |
| 00425490 | `Task_WaitTicks(n)` | Calls `Task_Tick` n times | confirmed |
| 00425260 | `Task_Alloc(list, extra)` | `malloc(extra+0x30)`, append to a task list | confirmed |
| 00425320 | `TaskList_Update(list)` | Runs `update` of each task, honours sleep counter, frees finished tasks | confirmed |
| 00425ac0 | `TaskList_Draw(list)` | Runs `draw` of each task | confirmed |
| 004252a0 | `Task_Unlink` | Unlink+free a task | likely |
| 00425380 | `TaskList_KillAll(list)` | Destroys every task of a list (used on scene change) | likely |
| 0042a130 | `PumpMessages` | Handles at most one message per tick (PeekMessage+GetMessage); `WM_QUIT` → `longjmp(0x8167a4)` | confirmed |
| 00407ac0 | `Input_PollAndReplay` | Reads controls into globals, then records or plays back replay events | confirmed |
| 004254b0 | `RenderTask_Create` | Allocates the render/camera task in list 6 (`0x84c1c8`) with update fn `0x425680` | confirmed |
| 00425680 | `RenderTask_Update` | Per tick: asks the pacer whether to draw; if yes sets viewport/camera for view mode and runs all `draw` callbacks, then ends the frame | confirmed |
| 00436fd0 | `FramePacer_BeginFrame` | 33 ms pacing + frame skip (max 2 in a row) + device-lost check + `BeginScene` | confirmed |
| 004370a0 | `Frame_End` | Viewport, 2D overlay layers (`FUN_00434fd0`, `FUN_00430fb0`, `FUN_004319b0`), `EndScene`, flip (`FUN_004286b0`) | likely |
| 00436f20 / 00436f30 | `FrameTimer_StaticInit` / `FrameTimer::FrameTimer` | Static ctor (CRT init table entry at VA 0x484098) of global object at `0x849d78`: auto-reset `CEvent` + `timeSetEvent(33,33,hEvent,0,TIME_PERIODIC|TIME_CALLBACK_EVENT_SET)` | confirmed |
| 00436f90 | `FrameTimer::~FrameTimer` | `timeKillEvent`, close event (registered with atexit by 0x436fb0) | confirmed |
| 00437372 | `CSyncObject::Lock` (MFC) | `WaitForSingleObject(h, timeout)` | confirmed |
| 00433140 | `LowerMainThreadPriority` | If D3D init (`FUN_00428820`) succeeded, `SetThreadPriority(current, -1 /*BELOW_NORMAL*/)` | confirmed |
| 00428820 | `Gfx_InitDevice` | D3D/DDraw setup wrapper (returns 0 on success) | likely |
| 00430090 | `SoundThread_Start` | `CreateThread(SoundThread_Main)` | confirmed |
| 00430230 | `SoundThread_Main` | Maps `effect.rom` / `..\data\bgm.rom`, creates DirectSound buffers, then a `MsgWaitForMultipleObjects` loop over 20 buffer-notify events + thread messages `0x464..0x469` | confirmed |
| 004300b0 | `SoundThread_Stop` | `PostThreadMessage(WM_QUIT)`, wait 1 s, close | confirmed |
| 00402700 | `Attract_Run` | Attract-mode sequencer over a function table at `0x484e30` until START is pressed | confirmed |
| 0040e4b0 | `Attract_DemoDrive` | Attract demo run: picks a route, runs the driving step from a task for ≤1600 ticks | likely |
| 0042a1c0 | `TitleMenu_Run` | Title menu loop; result in `DAT_00816834` | confirmed |
| 004122d0 | `ModeSelect_Create` | Creates the mode/route-select task (flag `0x80000` in `DAT_00855ff8`) | likely |
| 004080d0 | `Stage_Run` | Plays one game (route) from intro to results; contains the per-tick driving loop | confirmed |
| 0042b7f0 | `PauseMenu_Run` | Pause dialog (sets `DAT_00855ffc` bit 4); returns 0 resume / 1 quit / 2 retry | likely |
| 0042bb90 | `ExitConfirm_Run` | Yes/No dialog used by title EXIT | likely |
| 0042e180 | `CReplayMgr::Load(path)` | MFC `CFile`+`CArchive` load of `..\BIN\REPLAY.DAT` | confirmed |
| 0042e2c0 | `CReplayMgr::Save(path)` | Store counterpart | confirmed |
| 0042e010 / 0042df00 | `CReplayMgr::Serialize` store / load | Header + list of ranks | confirmed / likely |
| 0042d940, 0042d150, 0042c9a0 | `CReplayRank/Round/Station::Serialize(store)` | Nested lists | likely (which class is which: guess) |
| 0042c020 | `CReplayEvent::Serialize(store)` | 31-byte binary record or text dump | confirmed |
| 0042e740 | `CReplayMgr::RecordTick` | Appends an event when any input changed | confirmed |
| 0042e7d0 | `CReplayMgr::PlaybackTick` | Applies the next event's inputs when its tick is reached | confirmed |
| 00407a00 | `Replay_BeginSegment(final)` | At the start of each station segment: choose playback or record | likely |
| 004079c0 | `Replay_EndRun` | At the end of a run: decide whether to keep the recording | guess |
| 0042bcb0 | static init | Sets version string `"RPLY1.3"` into `CReplayMgr+0xe8` (`DAT_00816988`) | confirmed |

## 1. Entry and startup (`WinMain` @ 00429bf0) — confirmed

The game does **not** use MFC's `CWinApp::Run`/`OnIdle`. MFC is linked for helpers (`CString`,
`CFile`, `CArchive`, `CEvent`), but the entry is a hand-written `WinMain` called from the CRT
entry `0x438ecb`. Order:

1. `FindWindowA("DENGO3K")`: if already running, focus that window and exit.
2. `FUN_00425ca0`: CD check (off-limits).
3. `FUN_00429fb0`: `DirectDrawCreate` capability check. On failure: `MessageBox "Not found"`.
4. Registry `HKLM\…` values `Option` (an 0x80-byte blob), `FOGENABLE`, `FOGTABLESTART`, `FOGTABLEEND`,
   `FOGTABLEDENSITY`. The option bits (`DAT_00816840`) select the resolution mode `DAT_00816af4` = 1/2/3
   (bits 0x200000/0x400000/0x800000) and windowed mode (bit 0x4000 clears the fullscreen flag
   `DAT_00816d80`).
5. Registers window class `"DENGO3K"`. The window is full-screen sized (`GetSystemMetrics`), `WS_POPUP` +
   `WS_EX_TOPMOST` when fullscreen, the IME is disabled and the window starts hidden.
6. `CReplayMgr::Load("..\BIN\REPLAY.DAT")` on the global replay manager at `0x8168a0`.
7. `setjmp(0x8167a4)`. On the first pass: `LowerMainThreadPriority` (0x433140), `Game_Init`
   (0x40e910), `ShowWindow`, then `Game_MainFlow` (0x419510), which never returns.
8. The quit path is a `longjmp` back to step 7 when `PumpMessages` sees `WM_QUIT`. Then:
   `SoundThread_Stop`, misc teardown (`FUN_00434f50`, `FUN_00430e50`, `FUN_00431e70`), and
   `CReplayMgr::Save("..\BIN\REPLAY.DAT")`.

## 2. Execution model: blocking flow and cooperative tasks

The design is the classic arcade one. **Game flow is plain sequential code** (functions that loop
calling `Task_Tick` until a flag clears), while **everything on screen is a "task"** in one of 9
intrusive lists. There is no explicit state enum for the top level. The "state" is the call stack
plus two flag words.

### Task object (allocated by `Task_Alloc` @ 00425260) — confirmed
| Offset | Field |
|---|---|
| +0x00 | prev |
| +0x04 | next |
| +0x08 | `update(task) -> next_update_fn`. Returning **its own address** keeps the task alive. Returning another function is a state change. Returning 0 kills the task. |
| +0x0c | `destroy(task)` (called on kill) |
| +0x10 | `draw(task)` (called only on rendered frames) |
| +0x14 | `int16 sleep`. If >0, the update is skipped and the counter decremented. |
| +0x30.. | task-specific data. Many tasks use +0x34 as a local tick counter. |

So **each task is itself a small state machine** whose state is the update function pointer (for example
`FUN_00403420 → FUN_00403540 → FUN_00403750 → FUN_004037f0`). Ghidra shows these as `code *FUN_x(int)`;
about 25 such functions exist in 0x402000–0x41e000.

### Task lists (head, tail, `int16 disabled`; 12 bytes each, `0x84c180..0x84c1ec`)
| # | Addr | Typical creators | Role (guess unless noted) |
|---|---|---|---|
| 0 | 0x84c180 | 00420470, 004205c0 | screen fades/transitions |
| 1 | 0x84c18c | none | unused |
| 2 | 0x84c198 | 004229c0 (route load), 0040e370, 004240d0 | 3D world/route |
| 3 | 0x84c1a4 | ~30 creators: menus, HUD, results, title menu (0042b210) | UI/2D (likely) |
| 4 | 0x84c1b0 | attract screens (00402e80, 00402590, 004023b0), 0041f5e0 | attract/title |
| 5 | 0x84c1bc | 00424f20 (random positions), 00425070, 00404f10 | effects/particles |
| 6 | 0x84c1c8 | `RenderTask_Create` only | system: render task (confirmed) |
| 7 | 0x84c1d4 | 004367c0, 00436920, 00436a60 | palette/2D post-effects |
| 8 | 0x84c1e0 | none | unused |

### `Task_Tick` @ 004253c0 — confirmed
One call = one game tick:
1. If the global freeze `DAT_0084c490 == 0`: for every list whose `disabled` is 0, run `TaskList_Update`. While
   **pause** (`DAT_00855ffc & 4`) is set, only list 6 runs. The render task keeps drawing, so the
   world freezes but stays on screen. A debug hotkey path follows (`DAT_00856d60` and input bit
   0x8000000 → `FUN_00421e10`) (guess).
2. If not paused: `DAT_0084c494++`, the **global tick counter**. The replay stores it.
3. `PumpMessages` (0x42a130): handles at most **one** window message per tick. Also calls
   `FUN_004333d0` / `FUN_00430210` (likely input device/sound housekeeping).
4. If not paused: `Input_PollAndReplay` (0x407ac0).
5. Soft reset: if `DAT_00855ffc & 1` and the reset `jmp_buf` at `0x84c200` is armed and bit 2 is clear, then
   `longjmp(0x84c200)` back into `Game_MainFlow`. That path kills lists 2–5, clears the flags and returns to
   the title.

Rendering happens **inside** step 1, when list 6's render task runs. The update of every task
in lists 0–5 therefore precedes the draw in the same tick. List 7 updates after the draw.

## 3. Timing: fixed 33 ms tick, render skipping — confirmed

* **Timer** (0x436f30, static object at `0x849d78`): an auto-reset Win32 event, and
  `timeSetEvent(uDelay=33, uResolution=33, hEvent, 0, TIME_PERIODIC|TIME_CALLBACK_EVENT_SET)`. The event
  is signalled every 33 ms. Teardown with `timeKillEvent` at 0x436f90.
* **Pacer** `FramePacer_BeginFrame` (0x436fd0) is called once per tick by the render task. State: `deadline`
  `DAT_00849d70`, `late_count` `DAT_00849d74`, `device_lost` `DAT_00849d84`.
  * If `timeGetTime() > deadline` (behind): `late_count++`, `deadline += 33`. If `late_count < 3`, return 0 and
    **skip drawing this tick, without waiting**. On the 3rd consecutive late tick it draws anyway and
    resyncs `deadline = now + 33`, so the backlog is dropped and the game runs slower than real time.
  * Else (on time): `WaitForSingleObject(timerEvent, INFINITE)` (blocks until the next 33 ms timer pulse),
    then `deadline += 33`.
  * Before drawing: `IDirectDraw::TestCooperativeLevel` (vtbl +0x68 on `gfx+0x11c`). If it fails, mark
    `device_lost` and skip the frame. On recovery, `FUN_00427b90` restores surfaces. Then reset
    `late_count = 0` and call `IDirect3DDevice3::BeginScene` (vtbl +0x24 on `gfx+0x12c`). Returns 1.
  * Nuance (likely): because the event is auto-reset and periodic, a pulse that fired while the
    previous tick was still running is consumed immediately, so the wait is "until the next 33 ms
    boundary", not "33 ms after the last frame".
* **Consequences for the reimplementation:**
  * The simulation is **strictly fixed-step**: exactly one update per `Task_Tick`, with no delta time. All
    game logic counts ticks.
  * Under load, up to **2 consecutive draws are skipped** to catch up. Beyond that, game time slows down
    (no unbounded catch-up).
  * Nominal tick = 33 ms → **30.30 Hz**. The game itself treats **30 ticks = 1 second**: the replay
    `m_nTime` is formatted as `t/108000 h`, `(t/1800)%60 m`, `(t/30)%60 s` at 0x42c020, and the stage timer
    `DAT_008570b0` uses `% 0x1e`. So game seconds run ~1 % faster than wall time (30 ticks = 990 ms)
    (likely; verify with the oracle).
  * Recommended engine design: run the sim at 1000/33 Hz with a 33 ms accumulator, interpolate rendering
    (as CLAUDE.md requires). "Arcade" mode could optionally emulate the frame-skip/slowdown rule.
* The render task @ 0x425680 also contains debug camera modes selected by `DAT_0084c128` (0 normal,
  1 split dual viewport, 2 fixed alt camera, 3 free camera moved with input bits 0x200/0x400/0x1000/0x2000).
  It increments its own `+0x34` counter per drawn frame. With the pacer returning 0 it just clears
  `DAT_00816af8` (`FUN_00431db0`) and draws nothing (confirmed code; mode meanings guess).

Open question: before `Game_Init` creates the render task (0x4254b0, near the end of init), the
`Task_WaitTicks(0x5a)` etc. calls inside `Game_Init` have no pacer, so they may run unthrottled. Check whether
another task paces the logo screens (guess).

## 4. Threads

| Thread | Created at | Priority | Role | Conf. |
|---|---|---|---|---|
| Main | CRT | lowered to `THREAD_PRIORITY_BELOW_NORMAL` by 0x433140 when the 3D device came up | game, render, input, messages | confirmed |
| Sound | `SoundThread_Start` 0x430090 (called from `Game_Init`) | normal | DirectSound streaming/SFX. Waits on 20 notification events (10 buffers × 2 halves → `FUN_00430a40(buf, half)` refill). Commands arrive as thread messages `0x464` play, `0x465` stop, `0x466`, `0x467` volume, `0x468` pan/freq, `0x469` pause (exact meanings to be confirmed by the audio agent). Exits on `WM_QUIT` (0x12). | confirmed (structure), guess (command names) |
| winmm timer | `timeSetEvent` | (system) | only sets the frame event | confirmed |

Communication with the sound thread happens through `PostThreadMessage`. There are no locks around sim
state, and the sim is single-threaded.

## 5. Top-level game flow (`Game_MainFlow` @ 00419510) — confirmed structure

```
init → setjmp(0x84c200) [soft-reset target: kill lists 2..5, clear flags]
loop forever:
    unless coming back from "retry/continue" (flag 0x40):
        Attract_Run()                     # 0x402700, until START
        kill attract tasks (list 4)
    flags |= 0x400
    r = TitleMenu_Run()                   # 0x42a1c0
    if r != 0: flags |= 0x10; repeat title (attract skipped next time)
    ModeSelect_Create(); wait while DAT_00855ff8 & 0x80000     # 0x4122d0
    FUN_00401ef0()                        # credits/coin bookkeeping (arcade build only, DAT_008573e0==0)
    Stage_Run()                           # 0x4080d0
```

### Flag words (bits named by usage; mostly likely/guess)
* `DAT_00855ffc` "system flags": 1 = soft-reset request, 4 = pause/dialog (only list 6 runs),
  8, 0x10 = skip the attract opening / returned from the menu, 0x20, 0x40 = go straight back to the title menu
  (quit from stage), 0x100, 0x400 = in title menu, 0x800, 0x1000, 0x2000 = attract ranking display.
* `DAT_00855ff8` "phase flags": 2 = stage active, 4 = attract item running, 0x10 = run cleared/continue,
  0x20 = results screen active, 0x40 = waiting on a results/info task, 0x200 = driving, 0x400,
  0x800 = pre-departure (doors/countdown), 0x1000 = stage intro task, 0x2000 = score tally,
  0x4000 = ending, 0x80000 = mode select, 0x100000 = route select, 0x4000000 = loading screen,
  0x40000 = "special" input combo seen at the title (enables a hidden option if
  `notch==3 && brake==9`, 0x4080d0).
  Waits always take the form `while (flag) Task_Tick();` (confirmed pattern).
* Input globals (from `Input_PollAndReplay`): `DAT_00829d00` = power notch, `DAT_00829d04` = brake notch
  (likely, from the replay field names `m_nNoch`/`m_nBrake`), `DAT_0048c42c` = buttons held (active-low:
  a bit is 0 while pressed, confirmed by usage), `DAT_0048c430`, `DAT_0048c428` = second/third button words
  (edge or alternate device; guess).

### Attract (`Attract_Run` @ 00402700, table @ VA 0x484e30) — confirmed table, guessed meanings
Null-terminated list of 15 entries, each run in turn until bit 4 of `DAT_00855ff8` clears, while polling
for START (`FUN_00401dc0`). Pressing START ends attract. In the arcade build (`DAT_008573e0 == 0`) a coin
is also needed:
`0x402e60` (screen 0x15 via `FUN_0040ed90`), `0x402880` (title logo task, chooses graphics by resolution mode),
`0x402e80`, `0x404e10` (ranking, can chain into the Komachi movie `FUN_00429110`),
`0x40e450` (choose demo route from the bitmask `DAT_008500b0`, rotating with the attract counter `DAT_008573c8`),
`0x40e4b0` (demo drive), `0x40e6c0` (screen 5), `0x403f40` ×4, `0x403da0`, `0x40e6e0`.
`Attract_DemoDrive` creates a list-4 task (update `0x40e630`) that calls `FUN_0040dad0` and `FUN_0040b710` once per tick
for at most 1600 ticks (≈53 s) and increments `DAT_008570b0`. The train is driven by replay playback
(likely: see §7).

### Title menu (`TitleMenu_Run` @ 0042a1c0) — confirmed values, likely meanings
It creates the menu task `FUN_0042b210` and waits until `DAT_00816834 != -1`.
* 0 → start the game (return 0).
* 1 → options submenu (`FUN_0042a520`), with sub-pages 0 (`FUN_0042abb0`) and 1..5 (`FUN_0042afa0`).
  `FUN_0042a460` applies the changes.
* 2 → `FUN_0041f5e0` (records/route-status view; uses the route bitmask `0x8500b0`) (guess).
* 3 → EXIT: `ExitConfirm_Run`, then `DestroyWindow` (→ `WM_QUIT` → `longjmp` to WinMain shutdown).
* 4 → controller/key assignment `FUN_00434a00` (records a key per prompt into a table).
* 5 → bonus movie `..\data\Komachi.DAT` through DirectShow `IMultiMediaStream` (`FUN_00429110` → `FUN_00428f00`).
  The movie loop calls `PumpMessages` directly, not `Task_Tick`.

The arcade operator-menu strings (MONITOR TEST, CONFIGURATION, DIFFICULTY EASY/NORMAL/HARD/HARDEST,
COIN/CREDIT, …) at file offsets 0x577xx, 0x718xx–0x71axx and 0x84dxx are present in the exe. They were not
traced in this pass. They are probably used by the leftover arcade test mode and by the
`"******* CONFIGURATION *******"` debug display (`FUN_00402280` references the 0x84dxx block) (guess).

### Stage (`Stage_Run` @ 004080d0) — confirmed sequence
1. Route select (`FUN_00435490`, wait on `0x100000`). The route index goes to `DAT_008570a4`, where −1 means back.
   Route data pointer: `DAT_00856fd0 = table[0x485610 + 4*route]`.
2. Loading (`FUN_004137e0`, wait on `0x4000000`). `FUN_0040ed90(route)`, `FUN_00408ec0`, `FUN_0040d040`,
   `FUN_00407900`, `FUN_0040f7e0`, intro (`FUN_004139c0`, wait on `0x1000`), and route load
   `FUN_004229c0(route, 0)`.
3. **Per station segment** loop (`DAT_00856d78` = current station/segment index):
   * Setup: BGM/ambient sounds, `FUN_00408010`, `FUN_0040b970`, `FUN_004124f0(station)` (HUD/station task),
     set 0x800, then one priming call each of `FUN_0040dad0` and `FUN_0040b710`. Then `Replay_BeginSegment(0)`.
   * Pre-departure wait: `while (ff8 & 0x800)`. Each tick: pause check, `DAT_008570b0++`, `Task_Tick`.
   * Driving loop, **one iteration per tick**, while `ff8 & 0x200`, at 0x4084f5:
     ```
     if (!paused && pause button) PauseMenu_Run()  -> quit / retry
     FUN_00407c10  FUN_0040dad0  FUN_00409fc0  FUN_0040acd0  FUN_0040b4a0
     FUN_0040b710  FUN_0040b780  FUN_0040c010  FUN_0040c4e0
     if (!DAT_00856d80) DAT_008570b0++            # stage tick timer
     if (DAT_008570b0 % 30 == 2) DAT_008570b0 += DAT_00856fac   # time penalty/bonus hook (guess)
     if (DAT_00856fb0 > 0) FUN_0040a0a0()        # failure/over-run handling (guess)
     Task_Tick()                                  # tasks + render + input for next tick
     ```
     These calls are the per-tick rules/track lookups (confirmed): `FUN_00407c10` computes the gradient
     and the ambient-sound class at three probe points (`DAT_00857200[3]`, not signal aspects), then
     distance-to-stop (`FUN_0040dad0`, `DAT_0085708c`), then events and checks. **The speed
     integration itself is not here**: it is the list-2 task `0x422c20`, which runs inside
     `Task_Tick` after these calls (see `physics.md` §1).
     Input for tick N is polled at the **end** of tick N−1 (inside `Task_Tick`), so the sim always sees
     the inputs that were latched one tick earlier (confirmed by order).
   * Arrival: wait 20 ticks, `FUN_004158b0` (stop evaluation), kill HUD tasks, accumulate the distance error into
     `DAT_00857004`, `FUN_00413d90` (station result, wait on 0x40), `FUN_0040d340` (next segment),
     `FUN_00407ab0` (replay segment end), and continue (`FUN_00415ab0`, wait on 0x10) or game over.
   * Endings and special routes: `FUN_0041b540` (wait on 0x4000). Route 6 and routes 10/12–15 have special cases.
4. Results: `FUN_004149d0` (wait on 0x40), `FUN_0041e8d0(route, score)` (wait on 0x2000), name entry or ranking
   `FUN_00415f00` (wait on 0x20). Then `Replay_EndRun` / `FUN_0042e900`, clear lists and return to `Game_MainFlow`.
5. Pause result 1 (quit) → title. Result 2 (retry) → `FUN_0042e890(1)`, restart from the stage setup.

## 6. Pause

`PauseMenu_Run` (0x42b7f0) sets `DAT_00855ffc |= 4` (so `Task_Tick` only runs the render list, does not count
ticks and does not poll replay input), `setjmp(0x816858)` as an escape, and runs dialog pages
`FUN_0042b930(&0x47c810/…/…)`. It returns 0 resume, 1 quit or 2 retry, then restores the notch/brake globals
saved on entry (confirmed).

## 7. Replay system (`CReplayMgr`, global object @ 0x8168a0)

### Purpose — likely
Inputs are recorded **per station segment**, kept per difficulty "rank" and route ("round"), and
saved to `..\BIN\REPLAY.DAT` at exit. Playback drives the attract-mode demo, and probably a best-run
ghost/replay feature. Playback is input-based: inputs are re-injected into the same globals the
live controls write, so **deterministic replay relies on the fixed-step sim**. This is useful for our
differential tests, because a REPLAY.DAT from the original is effectively a recorded input script.

### Manager fields (offsets from 0x8168a0; DAT_ names in decomp) — confirmed unless noted
`+0x04 m_bReady`, `+0x10 m_listRank` (CObList-style), `+0x18` rank count, `+0x58 current event`
(`DAT_008168f8`), `+0xc4` (`DAT_00816964`, "viewing replay" — likely), `+0xc8` (`DAT_00816968`, playback
mode), `+0xcc` (`DAT_0081696c`, record disabled for this run), `+0xd0` (`DAT_00816970`, segment tick
counter), `+0xd4`, `+0xe0` (`DAT_00816980`, replay system enabled), `+0xe4` (`DAT_00816984`),
`+0xe8` CString version `"RPLY1.3"`.

### Per-tick hook (`Input_PollAndReplay` @ 00407ac0) — confirmed
If enabled and not record-disabled:
* playback mode → `CReplayMgr::PlaybackTick` (0x42e7d0): `counter++`. If the current event is ready and
  `event.tick <= counter`, it copies the event's notch, brake and 3 input words into the live input
  globals, **overwrites the global tick counter `DAT_0084c494` with the event's stored value**, and advances.
  Otherwise it re-applies the current event's inputs (inputs are sticky).
* record mode → `CReplayMgr::RecordTick` (0x42e740): `counter++`. If notch, brake or any of the 3 input words
  (bits 0x100100 masked to 1, i.e. ignored) differ from the last event, it appends a new event.
  **Recording is change-based (run-length), not per-tick.**
`Replay_BeginSegment` (0x407a00) writes or reads the first event of the segment
(`FUN_0042ea10` / `FUN_0042ea50`) and selects the mode depending on whether data exists for this station
(`FUN_0042e940`).

### Event record (`CReplayEvent`, 0x42c020 / 0x42bd80) — confirmed layout
| Mem off | Field (debug name) | Binary | Meaning |
|---|---|---|---|
| +0x04 | m_nIndex | not stored | position in list |
| +0x08 | m_bReady | u8 | valid |
| +0x10 | m_fDist | f32 | distance-to-stop `DAT_0085708c` at the event |
| +0x0c | m_nTime | i32 | segment tick counter (`+0xd0`); printed as h:m:s at 30 ticks/s |
| +0x14 | m_nCounter | i32 | global tick counter `DAT_0084c494` |
| +0x18 | m_fSpeed | f32 | train speed `DAT_00857234` |
| +0x1c | m_nNoch | i8 | power notch |
| +0x1d | m_nBrake | i8 | brake notch |
| +0x20 | m_nInpush | i32 | input word `DAT_0048c42c` |
| +0x24 | m_nInpull | i32 | input word `DAT_0048c430` |
| +0x28 | m_nInbuf | i32 | input word `DAT_0048c428` |

Binary store order (0x42c020): `u8 bReady, f32 fDist, i32 nTime, i32 nCounter, f32 fSpeed, i8 notch,
i8 brake, i32 push, i32 pull, i32 buf` = **31 bytes**, little-endian. `FUN_0042f100` is called first
(purpose unknown).

### File format (high level) — likely
MFC `CArchive` over `CFile` (4 KB buffer):
```
"RPLY1.3\r\n"                       # CArchive::WriteString; load uses ReadString + strcmp
Mgr:     u32 bReady, count(ranks)    # MFC WriteCount (u16, or 0xFFFF + u32)
 Rank:   u32 nIndex, u32 bReady, count(rounds)
  Round: u32 nIndex, u32 bReady, count(stations)
   Station: u32 nIndex, u32 bReady, u32 nCounter, count(events)
    Event × n (31 bytes each, see above)
```
The rank/round/station mapping to 0x42d940/0x42d150/0x42c9a0 is a guess, and so is the field order beyond
"nIndex, bReady, …, count". `FUN_0042e450` scans ranks 0..3, so there are 4 ranks (probably the four difficulty
levels EASY/NORMAL/HARD/HARDEST; guess). If the file name does **not** end in `DAT`, every class instead writes a
human-readable text dump with the `// CReplay… //` headers (debug feature, confirmed by the extension test
against the string at 0x48be54). Load and save failures show the Japanese message boxes at 0x48c148 / 0x48c17c.

## 8. Open questions / next experiments
* Measure the real tick period under Wine with the oracle (expected 33.0 ms) and confirm that 30 ticks are
  treated as 1 game second.
* Confirm the input-word semantics (`0x48c42c/430/428`: held / pressed edge / released edge?).
* Map the remaining `DAT_00855ff8` / `DAT_00855ffc` bits by tracing with the hook.
* Decode a real `REPLAY.DAT` (after the user plays) to validate §7 and to use it as an input script.
* Determine whether logo waits in `Game_Init` run before any pacer exists.

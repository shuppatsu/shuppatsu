# AI trains ("traffic")

Scope: the other trains (and road vehicles) that run past the player: where the game spawns
them, which track they use, how they move, how they are drawn and heard, and what they have to
do with the rules. Addresses are VAs in `dengo3k.exe` (3000-bandai). Code:
`crates/ddg-formats/src/traffic.rs` (model table), `crates/ddg-engine/src/traffic.rs`
(simulation, poses, sounds), `crates/ddg-game/src/traffic.rs` (drawing).

Confidence: **confirmed** = read from the code (and checked against the data where that
applies), **likely** = strong inference, **guess** = needs an oracle recording.

## 1. Where they come from

### 1.1 The event list (0x40b710) — confirmed

Per stage, `0x4858a0[stage]` points at records of 8 × i16
`{station, distance_m, mask, spawn, flags, speed_kmh, param, model}`, sorted like the course
events and ended by station 99 (routes.md §9). The driving loop calls 0x40b710 once per
iteration (after the limit events, before the enforcement 0x40b780), and the segment setup
makes one priming call after 0x40dad0's. Each call looks at the record under the cursor
`0x856d90` only:

- it fires when `0x85722c` (rounded distance to the stop, m) ≤ `distance_m` **and**
  `station == 0x856d78`; the cursor then moves on;
- it spawns only if `mask & (1 << 0x857216)` is set (one bit test, `mov cl, byte [0x857216]`
  at file offset 0xb730);
- one record per tick at most.

The cursor is set to the list head and moved past `station < current` records at the stage
start (0x40d040) and at a continue (0x40d930); 0x40d340 only does that for the stage-1
"jump to station 24" special. A record whose station is passed without firing (passing a
station before reaching its distance) blocks the list for the rest of the run, as in the
original.

**The mask state `0x857216`** (i16, written as a word, read as its low byte):

| value | set by |
|---|---|
| 1 | stage start (0x40d040); after each station result when no continue was used (`0x8570b8 < 1`, 0x4080d0) |
| 0 | continue (0x40d930); after each station result once a continue was used |
| 2 | stage 6 only, when the summed station marks `0x856fe0[1..=station]` (0x40e350) reach 4 (0x4080d0, with the train switch `0x40ee50(0x16, 0)`) |

So mask 2 = "no continue yet", 1 = "after a continue", 3 = both, 4 = the stage-6 variant,
and **mask 0 never fires**: 0x857214 is also written, but as a word (`66 a3`), so it does not
touch 0x857216. A large share of the events (e.g. most of stage 10's station-0 list) have
mask 0 and are dead data. **confirmed** (code bytes)

### 1.2 Other callers of the spawner

- **Scripted red-signal recovery** (0x40a0a0), §6.
- **Stage intro** (0x4091a0, called by 0x408ec0 before stages 1, 5–8, 11–15 and 18 start; scene.md
  §9): a 144-tick pre-run fly-by on the stage's route, `0x404f10(0x458c80[stage].spawn, 5,
  km/h, param, −1)` towards the camera, which runs back; the caption card (slot 0x1d) and BGM
  0x197; skipped by the horn or A. Ported (`play/intro.rs`, the normal spawner); matches Wine
  captures of stages 1 and 13. **confirmed** (code, trace, captures)
- **0x406960** (the other stages' intro 0x408f40, scene.md §9.2): the player's own train placed
  at an explicit track point ahead of the camera (mode 1, 100 km/h, model `0x4596e0[stage] |
  0x1000`, 12-point bogies; the cars apparently stepped along the track by their offsets, not
  traced in detail). Not ported. **confirmed** (call and arguments) / **likely** (placement)
- **0x4042b0** (the full ending's task, menus.md §12.2; table 0x4587dc, 0x24 bytes per stage):
  the final shot spawns with `flags | 5`; on stage 2 with `0x84c3fc == 0xe` the model is forced
  to 8. Ported (`Drive::ending_spawn`). **confirmed**
- **0x406e10** (the attract opening's phase E, menus.md §2.3): spawn 2 of AKITA1C, flags 0x11,
  270 km/h, model 7, rear bogie 12 points back (0x404f10 uses 10). Ported with the normal
  spawner. **confirmed**
- **0x406c50** (course event 12, the coupling minigame's waiting set): its own placement of a
  parked set on the player's track ahead of the stop, with a second car list for the coupler
  frame `0x857244` (rules.md §7.5). Ported as `Traffic::place_waiting_set`. **confirmed**

### 1.3 Spawn points and the model table

`spawn` indexes the `.COM` traffic table (routes.md §7: `u16 type, u16 flags, slot`).
The model row is `model` if it is not −1, else `type − 1` (`type < 1` = nothing).

The model table at **0x457358** has 79 rows (0..=78) of
`{car list for direction 0, car list for direction 1, i32 kind}`. A car list is a run of
28-byte records `{u32 part[5], f32 offset, f32 spacing}` ended by a record with `part[0] = 0`
(all terminators are zero). `part[0..5]` are the five distance bands (§4), `offset` the
car's distance behind the head (cumulative, ≤ 0) and `spacing` the gap to the car before
(−2.0 units = 20 m for most cars; −2.45 for row 65; −1.05..−2.15 for the locomotive rows).
Kinds: 0 = road vehicles (rows 68–78, area parts 0x29a5..0x2abb), 1–8 = rail (TR parts
0x2eb2..0x3279). Data test: `traffic_model_table`. **confirmed**

Which trains these are is visible in the headless shots: e.g. a 701-series 2-car EMU at
Akita, 211-series and a 15-car Tōkaidō set with Green double-deckers at Yokohama, an EF66
with a blue sleeper train near Tsukamoto, a Shinkansen along the Yamanote stage, an HK100
railcar on the Hokuhoku line.

## 2. The spawner (0x404f10; 0x405e80 for row 0x43) — confirmed

Arguments `(spawn, flags, speed_kmh, param, model)`. `flags & 0xf` is the speed mode,
`0x10` places the cars spaced out instead of letting them emerge, `0x20` reverses the
spawn's direction, `0x40` randomises the speed; mode 6 is mode 0 with a random speed.

1. Direction: `dir = spawn.flags & 1` (1 = towards the end of the segment, "forward"),
   toggled by `0x20`. The car list is `cars[dir]`, except for kinds 3, 7 and 8, which take
   `cars[dir ^ 1]` in reverse order with offsets and spacings recomputed (0x404ea0) — the
   same consist seen from its other end, so a locomotive leads either way.
2. Rows above 0x45 start one point further on in the running direction.
3. Random speed (mode 6 or `0x40`): `speed += 15 − rand() % 31` (< 0 → 10), then
   `srand(rand() × 0x84c494)`.
4. Per car, a task in list 5 (0x84c1bc, update 0x405420, draw 0x405640; 0x405b10 for rows
   0x47 and 0x4b):
   - mode 5: first skip `param` points in the running direction, then run as mode 0;
   - **emerging** (no `0x10`, mode ≠ 0): every car starts at the spawn point with a delay
     `|offset|` (in units of distance run): the train comes out of the spawn point car by
     car, so spawn points sit off-screen;
   - **spaced** (`0x10`, or mode 0): each car is walked back from the previous one by
     `|spacing|`, carrying the fraction;
   - the rear bogie point is 10 points behind the front one (12 for row 0x43);
   - first car of rows 0x1e–0x21, 0x34, 0x35 running backwards: flag 0x1000 (turned round).
5. Speeds: `+0x54 = speed · 0.000926` units/tick (km/h; 0x458b78), `+0x58 = speed · 0.926`
   (the same × 1000; 0x458b70), and per mode `+0x5c` (change of `+0x58` per tick):

| mode | behaviour | `+0x5c` |
|---|---|---|
| 0 | constant speed (speed 0 = parked) | — |
| 1 | accelerates by `param` × 0.001 km/h/s, without limit | `param · 0.000926 · 0.033333` |
| 2 | brakes to a stop within `param` m | `v² / (param · 0.2) · 1000` |
| 3 | runs, then stops at `param` m from the spawn: brakes over 300 m (or `param` < 300 m: from the start) | `v²/60 · 1000` or as mode 2 |
| 4 | as 3 with 100 m, **only if the obstacle event is armed** (`0x856d76`), else mode 0; such a train restarts (mode 1 with the braking value) when the obstacle is cleared and cruises from 90 km/h (0.08334 units/tick) | `v² · 0.05 · 1000` or as mode 2 |
| 5 | skip ahead, then mode 0 (the update's mode-5 case, "sound 0x196 when 0x85722c < 1", is unreachable) | — |

Row 0x43 (0x405e80): no random speed, no `0x20`, the car direction is the *opposite* of the
spawn's flag and it uses `cars[flag]` (only list 1 exists, so it only spawns from flag-1
points), emerging delay `−offset`. Its update 0x406220 also sets the sound gate to 9999 when
the distance to the stop drops below 1 (no effect on the sounds). Row 0x43 appears in no
event of this exe; it can come from a `.COM` spawn of type 68 via the intro or 0x4042b0.

## 3. The car task (0x405420) — confirmed

Every tick of `Task_Tick` (driving, holds, result and loading ticks alike; not while paused):

1. If the train waits for the obstacle and `0x856d76 == 0`: mode 1.
2. Mode step (table above); modes 3/4 switch to 2 when `+0x48 − +0x4c` (stop distance −
   distance run) drops below 30 / 10 units.
3. Move: while the delay `+0x60` is positive it is reduced by the speed and the car stays
   (the overshoot carries into the fraction); otherwise the fraction `+0x50` moves by the
   speed (+ forwards, − backwards). `+0x4c += |speed|` in both cases.
4. Normalise: while `+0x50 > 0.125` step both bogies to the next point (0x407880), while
   `< −0.125` to the previous one (0x4078c0). These walks go through segment markers to the
   segment's link (`0xe000` tail marker → linkB, `0xf000` head marker → linkA) and stay put
   where the link is null. If the front bogie does not move on the first step of a loop, the
   task ends: **cars disappear one by one at the end of their track.**

Task list 5 is cleared two ticks after an arrival (with the stop error, 0x4080d0 before the
station result), when the continue screen closes, and at the stage end. So all traffic of a
leg vanishes at the station stop and parked trains are re-spawned by the next leg's events.

## 4. Drawing (0x405640; 0x406420 for row 0x43, 0x405b10 for rows 0x47/0x4b) — confirmed

- Bogie points: each of the two points is interpolated towards its neighbour in the running
  direction by `|+0x50| · 8`; nothing is drawn while the front point has no neighbour.
- Model matrix 0x4186a0: origin = midpoint of the two bogie points, y = mean of the two
  interpolated heights; local z = the horizontal direction from the rear to the front bogie
  for forward cars and the reverse for backward ones — i.e. **always towards higher slots**;
  the car lists provide direction-specific parts (cab ends). Flag 0x1000 turns the car round
  (`0x418080(sin π, cos π)`); row 0x43 turns every car except parts 0x2eb8/0x2ebc.
- Pitch: only when the raw heights of the two bogie points differ, `0x418000(sin a, cos a)`
  with `a = atan(Δy · 1/len)` (the call 0x437d40 takes `(y₁ − y₂) · 1/len`, operands in
  `[esp+0x18]`/`[esp+0x1c]`, swapped for backward cars). Which height is which, and so the
  sign, is **guess**; the port tilts the car so that it follows the track.
- Distance bands from the view-space position of the origin (`0x856bd8` = z, `0x856bd0` = x;
  forward = −z; world units of 10 m): z < −15 or z ≥ 2.5 not drawn; −15.. → LOD 4, −12.. → 3,
  −9.. → 2, −6.. → 1, −3..2.5 → 0; |x| > 16 / 24 / 32 adds 1 / 2 / 3, |x| > 64 or a sum above 4
  hides it (doubles 0x458c30..0x458bf8). **Cars are drawn from 150 m ahead to 25 m behind
  only**, with no frustum test, so the widescreen view needs no extra culling work. The port
  draws them up to the far plane by default (decided enhancement `far_traffic`, renderer.md
  §13.9), in LOD 4 beyond 150 m; `--original-traffic` keeps the original bands. Only drawing
  changes: the engine's sound windows still use these bands.
- Rows 0x47/0x4b (road vehicles) also draw part 0x24 at local (0, 0.05, −0.02) and make no
  sound.

The port: `Drive::traffic_poses(alpha)` interpolates each car between the last two ticks by
replaying the tick's move from the previous bogie state, so cars follow curves and segment
links like the cab camera; the game (`traffic::add_cars`, called from `RouteScene::compose`)
picks the band from its own (interpolated) cab camera. The stage's TR sets are loaded after
the area sets, `texTR(k+1)` then `partsTR(k+1)` (0x40ed90, loaders.md §8).

## 5. Sounds — confirmed (code), loudness/feel unverified

From the draw step (so only for drawn cars, at the camera after this tick):

| code | when |
|---|---|
| 0x33 | backward (oncoming) car, `|x| ≤ 1`, speed ≥ 0.02 units/tick (21.6 km/h), z in −0.2..0 **or** −1.9..−1.6: every tick it is inside a window |
| 0x33 | forward car, same lateral/speed test: z in 0..0.1 if the gate `+0x3c` > 999 (gate = 100), z in 1.6..1.7 if the gate > 99 (gate = 10); the gate starts at 10000. Overtaking a car gives two clicks, being overtaken one |
| 0x1028 | row 0x43, first car, z in −0.4..0, once: sequence 0x28 = `{0x196 /235, 0x12}` |

The scripted recovery (§6) adds 8 (at hold-loop tick 40, released at 100) or 1 + 4 (tick 45, 4
released at 100). The port emits all of these as `TrafficSound`s; `Sound::after_tick` plays them
after the task-list sounds. The original's frame skipping (renderer.md) would also skip draw-step
sounds; the port draws every tick.

## 6. Rule interactions

### 6.1 The scripted "train ahead" stop (0x40b970 / 0x40a0a0) — confirmed

When running early paints a scripted pattern (kind 0 of the list at 0x459234, rules.md §4.3),
0x40b970 also sets `0x855ff8 |= 0x80000000` and copies the row's `train` / `train_distance`
to 0x857000 / 0x8570cc. The next stop at a red or after the ATS (0x40a0a0) then:

- with the ATS involved (`0x857206 > 0`): drops the flag; the usual 60-tick standstill;
- otherwise: drops the flag, **30** standstill ticks, then if `d = 0x85722c − 0x8570cc` is
  1..30 (the train stopped 1–30 m before `train_distance`) a train passes and the standstill
  continues for its hold, else for 60 more ticks:

| `train` | condition | spawn call | hold | sounds |
|---|---|---|---|---|
| 0x38 | — | `(0x38, 1, 160, 0, model 0x42)` | 360 | — |
| 1 | d < 17 or d > 22 | `(1, 1, 100, 0, model 0xf)` | 200 | 1 + 4 at 45, 4 off at 100 |
| 1 | 17 ≤ d ≤ 22 | `(1, 1, 120, 0, own type)` | 300 | 8 at 40, off at 100 |
| other | — | `(train, 1, 80, 0, own type)` | 360 | — |

The three rows in the exe: stage 4 station 0 (train 1, 2410 m), stage 6 station 8 (0x38,
1932 m), stage 6 station 11 (63, 1342 m). Then, as for every recovery, the green and 10 more
ticks. The engine reproduces the hold lengths in `Rules::signal_stop_recovery` (it used to
hold 60 always) and `Drive` hands the spawn to the traffic module 30 ticks after the stop.
No trace covers this yet.

### 6.2 Obstacle trains — confirmed

Mode-4 trains (stages 10 and 12–15, whose scripted list arms the obstacle event) only stop
short if `0x856d76` is set when they spawn, and restart when 0x40a0a0's obstacle branch clears
it. In the port the flag clears at the start of the obstacle hold, a few ticks earlier than in
the original (it is cleared after its first `Task_Tick`s there). **likely** (timing)

### 6.3 Nothing else

AI trains have no signals of their own and do not occupy blocks: the "train ahead" of the
signal patterns is purely the patterns of rules.md §4.3. They do not collide or stop for the
player. 0x40b710 is the only traffic call in the rule functions.

## 7. The 3D signal models (0x85744c) — confirmed (code)

Animated scenery parts of classes 0–5 (0x44e820: 0x2af5, 0x2afb, 0x2353, 0x2354, 0x2363,
0x2364; scene.md §4) are tasks whose update 0x401140 copies **0x85744c** into their frame
index; frames `0x4841ec[k]` hold 7 parts (green, YG, Y, YY, red, the variant-1 green, one
unused). 0x85744c is written by:

- `Signal_Show` (0x411500): the shown aspect 0–4, or 5 for a green under display variant 1
  in modes 0–2 (stages with `0x466c08` set > 0 for that variant);
- 0x40c4e0, station HUD state 3 → 4 (`0x85708c ≤ f5 − 10` of the target row) at a stopping
  station: 4 — the platform's signals turn red as you arrive;
- 0 at the stage start (0x40d040) and in the intro/fly-by tasks (0x404110, 0x404920).

So **every** 3D signal in view shows the aspect of the last signal display, not its own block.
The port keeps it in `Rules::signal_model` and the scene draws those parts with that frame.
Approximation: the HUD states 0–3 of 0x856d68 are not modelled, so the red latch fires on
the first tick under `f5 − 10` (the original needs one tick per earlier state).

## 8. The port

- `ddg_formats::traffic::TrainModels` (in `StageTables`), `ddg_engine::traffic::Traffic`
  driven by `Drive` (events in the loop body and the setup's priming call, car tasks and
  draw-step sounds every tick, clears as in §3), `Drive::traffic_poses`,
  `Drive::traffic_sounds`; `ddg-game` draws them and `Sound::traffic` plays them.
- The global tick counter 0x84c494 (`Drive::global_tick`) reseeds the random speed.
  **Approximation:** `rand` starts from the CRT seed 1 at each run; the original's sequence
  depends on every earlier `rand` call of the session (weather drops, effects), so random
  speeds differ.
- `ddg-tools trace-replay --drive` stays exact on all three traces (no traffic state is in
  them). `oracle/full.schema` now logs `traffic_cursor` (0x856d90), `traffic_state`
  (0x857216), `signal_model` (0x85744c) and the scripted train (0x857000 / 0x8570cc); the
  replay compares `traffic_state` and `signal_model` when present.
- `ddg-game view` has no stage, timetable or rules, so it shows no traffic.

## 9. Open questions

- Car positions in a trace: the cars are tasks in list 5; the hook would need to walk that
  list (head in 0x84c1bc) and log `+0x64/+0x68/+0x50/+0x54` per car.
- The pitch sign (§4) and which local end of a part is the front (checked only by eye: the
  EF66 and the HK100 lead correctly towards the camera).
- Task pool size of list 5 (allocation can fail in 0x425260; the long consists of stage 10
  put 25+ cars in the list at once, plus the animated scenery that shares it).
- State 2 after stage 6's bonus game with four marks or more is modelled
  (`Traffic::set_state`, menus.md §12.1); the event of stage 6 station 12 (mask 4) depends on it.
- The stage intro chase (0x408f40 → 0x406960) is not ported; the fly-by (0x4091a0), 0x4042b0's
  and the opening's spawns are.
- Row 0x43's sequence codes (0x196, 0x12): which sounds these are.

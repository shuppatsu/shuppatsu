# Train physics, position and the stage clock

Scope: how the player train's speed changes each tick (notches → forces → speed), how distance
and track position advance, and how the stage clock runs. Signals, speed limits, ATS, penalties
and scoring are covered in `rules.md`; this note only names the rule hooks that live inside the
physics code. Implementation: `crates/shuppatsu-engine/src/physics.rs`, `time.rs`.

Confidence: **confirmed** = read from the code (Ghidra plus `objdump` of the x87 sequences);
**likely** = strong inference; **guess** = needs an oracle trace. Addresses are VAs in the
3000-bandai `dengo3k.exe`. The original Kōsoku-hen exe has not been examined; its addresses will
differ.

## 1. Where the physics runs — confirmed

The driving-loop functions at 0x4084f5 (main-loop.md §5) do **not** integrate speed. The
physics is a **task** in task list 2 (0x84c198), created by the route loader 0x4229c0 with
update function **0x422c20** (Ghidra had not made a function there; it is 5192 bytes, up to
0x424067). Draw 0x424070 and kill 0x4240c0 belong to the same task. The task object is
0x68 + 0x30 bytes; its fields are listed in §8.

One tick of the stage loop is therefore:

1. Driving-loop calls (0x407c10 … 0x40c4e0), which read the speed **of the previous tick**.
   0x407c10 computes the gradient (§4.5); 0x40dad0 the distance to the stop (§5).
2. Clock increment (§6).
3. `Task_Tick` (0x4253c0): list 2 runs; the physics task runs before the in-cab camera task
   (0x41d160, created later per segment by 0x41cb00, so it sits after the physics task in the list
   — **confirmed** by the oracle: 0x8500c8 drops by the 0x850208 of the same tick). The camera then moves by the distance the physics just produced.
4. Input poll at the end of `Task_Tick`: the notches the physics uses next tick.

So the physics reads notches latched one tick earlier, and the track position, gradient and
heading of the previous tick.

Helper functions of 0x422c20 (all **confirmed**): 0x422610 traction, 0x422750 brake force,
0x422860 speed-dependent resistance, 0x4228b0 curve term, 0x422740 (writes 1.0 to a factor at
task+0x34 — an unused adhesion hook), 0x422910 (sets three lamp/indicator bits from the notches),
0x422990 (per-type table read by the rules).

### Driving-loop functions, for orientation (likely; details belong to rules.md)

| fn | role |
|---|---|
| 0x407c10 | gradient 0x857228; ambient sound class at three probe points −16/+4/+24 slots (0x857200[3], from the per-stage 32-byte track-type table 0x857220 = `stages.track_sounds`) and their sound starts/stops. Not signals: main-loop.md's "aspect/limit bytes" is a misreading. |
| 0x40dad0 | distance to stop 0x85708c / 0x85722c; passing a non-stop station (0x40d620); overrun below −50 m (0x857206 = 1); once a second after the scheduled arrival time a penalty (0x4122c0). |
| 0x409fc0 | 400-entry ring 0x850300 of distance-to-stop samples; zeroes 0x857234 inside a configured zone. |
| 0x40acd0 / 0x40b4a0 / 0x40b710 | course, limit and traffic event lists (stages.rs). Course event 7 sets the ATC limit 0x84c424, events 10/11 the power cut 0x856fba. |
| 0x40b780, 0x40c010, 0x40c4e0 | limit/signal overspeed checks, signal blocks, overrun state machine 0x857206, slippery-rail counter 0x857230, misc announcements. |

## 2. Numerics — confirmed (oracle, Wine)

The oracle logs the x87 control word **0x027f** on every tick: 53-bit (double) precision,
round to nearest. `DDSCL_FPUSETUP` (renderer.md §3) does not switch the FPU to single precision
under Wine. Intermediate values therefore live on the FPU stack as doubles and become `f32` only
where the code stores them to a float variable (`fst`/`fstp dword`). Two consequences matter for
bit-exactness:

* `fst` stores a rounded copy but keeps the unrounded value on the stack. Several comparisons
  test the unrounded value: the traction fade against 1.0 and 0, the released brake cylinder
  against −0.4, the gradient force against 100, the curve value against 200, the new speed
  against 0 and the top speed, and the coast-down speed against 0.
* Some results are never stored before use. The brake force from 0x422750 stays on the stack
  through the force sum and the brake-deceleration readout. The force sum `T − R − R2 − Fb` is
  stored to task+0x44, and the gradient force is subtracted from the unrounded sum.

`shuppatsu-engine` computes in `f64` and rounds at exactly the original's stores. The CRT helpers
used here are `__ftol` (0x437b30, truncation), `floor` (0x437b57, CW 0x173f), `ceil` (0x437e0b,
CW 0x1b3f) and `fmod` (0x437eea, `fprem`).

**Open:** on real Windows, Direct3D with `DDSCL_FPUSETUP` would normally select 24-bit
precision. Results would then differ in the last bits; the size of the difference is unmeasured.
The Wine oracle is the reference for now.

**Result:** replaying `yamate-drive.csv` (6350 driving ticks, stage 5 = hokuho1a despite the file name, including
EB use, overrun handling and coast-down) gives **158 700 of 158 700 values bit-exact**. That
covers 25 fields: speed, task fields, forces, distances, brake readouts and history.
Not exercised by that trace: a non-zero gradient (the segment is flat), the power cut, slippery
rail and an ATC trip.

## 3. Train tables — confirmed

`0x471ad8`: 20 records of 15 `f32` (stride 0x3c). The train type of the run is
`0x472228[stage]` (i32[20]) → 0x84c3fc, copied to task+0x5c every tick; 0x40d340 switches it to
12 at stage 1 station 7 (a stage that changes trains mid-run). Record layout:

| off | name (engine) | meaning | type 0 |
|---|---|---|---|
| +0x00 | `base_speed_kmh` | end of the low-speed traction region | 30 |
| +0x04..+0x14 | `notch_speed_kmh[5]` | balancing speed of P1..P5; P5 is also the top speed | 10 30 50 80 100 |
| +0x18 | `force_top` | traction force reached at top speed | 95 |
| +0x1c | `force_base` | traction force at the base speed | 65 |
| +0x20 | `brake_rate` | brake cylinder change per tick | 0.08 |
| +0x24 | `brake_per_notch` | cylinder target per notch (×0.1) | 4 |
| +0x28, +0x2c | `brake_gain[2]` | brake force gains (multiplied) | 16.9, 3.8 |
| +0x30 | `mass` | divisor of the v² resistance; copied to task+0x48 | 449.5 |
| +0x34 | `drag` | v² resistance coefficient | 7 |
| +0x38 | `jolt_limit` | per-tick deceleration limit for the jolt penalty (§7) | 0.045 |

Stage → type: 16 16 16 10 10 11 11 9 8 8 5 5 2 4 6 17 3 16 16 16. Type 7 has `drag` 0.05 and
`jolt_limit` 0 (looks like a data slip; unused by any stage). Types 12–15 and 19 are the fast
types (top speed 250–330).

Per stage × difficulty (`[stage·4 + difficulty]`, difficulty = settings value 0x829d10), set by
0x40d040: gradient scale **0x459b90 → 0x84c404** and curve scale **0x459a50 → 0x84c42c**
(1.0–1.3). Traction scale 0x84c3f4 and brake scale 0x84c438 are 1.0 (only the bonus game's
physics task, 0x4240d0, resets them, also to 1.0; menus.md §12.1).

`0x471f88` (21 × 8 i32) is the rail-joint sound table (period in metres, click offsets); not physics.

## 4. One physics tick (0x422c20) — confirmed unless noted

Notation: `v` = speed in m/s (task+0x3c, a working copy of 0x857234 taken at entry), `V` =
`v × 3.6` km/h, `T` = the record above. Forces are in the game's unit; §4.6 converts them.

### 4.1 Notch overrides
* Power `P` = 0x829d00, brake `B` = 0x829d04 (or the attract script, task+0x74 = 2).
* If 0x857206 ≥ 4 (overrun handling): `B = 9`, `P = 0`.
* **ATC trip**: if `0x857234 > (L + 3) × 0.27777777` (L = 0x84c424 in km/h, 1000 at route load,
  set by course event 7), latch task+0x8c and zero 0x856fb0 (the rules end the run); the latch
  clears when `0x857234 < L × 0.27`. While latched `B = 8` (even over EB).
* Coast-down (flags 0x400 or 0x10 of 0x855ff8): `v −= 0.15`, `P = 0`, clamp at 0, and `v = 0` once
  `trunc(0x8500c8 × 10)` ≤ the next timetable row's field +0x20.

### 4.2 Resistances (from `v` at the start of the tick)
* Running: `R = 0.0164 V + ((drag + 1)·0.0078 + 0.028) V² / mass + 1.32` for `V > 10`;
  `R = 3.0` for `0 < V ≤ 10`; 0 at rest. Evaluation order: `((c·V)·V)/mass`, then
  `0.0164V + that`, then `+ 1.32`.
* Speed term (0x422860): `k = top × 0.2777777 × 1.02` (m/s), `x = (k² − v²)/k²`,
  `R2 = 5x² + 0.2` — 5.2 at rest, 0.2 near top speed. It acts like breakaway resistance.

### 4.3 Traction (0x422610), `n = P`
* `n = 0` → 0.
* `V > notch_speed[n]`: `F = (1 − 0.1·min(V − notch_speed, 10)) × force_base × 0.25`
  (25 % of `force_base`, fading to 0 over 10 km/h above the notch's balancing speed).
* `base < V ≤ notch_speed`: `F = force_base + 1.1 × (force_base − force_top)/(top − base) × (V − base)`
  (a dead branch returns 6.0 if `notch_speed < base`).
* `V ≤ base`: `h = 1.2 × top`, `p = notch_speed × (1 + 0.6 (h − V)/h)`,
  `F = (p − V)/p × force_base × 1.25`.
* Every branch × 0x84c3f4. Note the low region ends at `force_base·1.25·(p−base)/p`, not at
  `force_base`, so the curve has a step at the base speed (reproduced as is).
* Then × task+0x34 (always 1.0) × the **traction fade** task+0x38: +0.02 per tick up to 1 while
  0x856fba = 0, −0.02 per tick down to 0 while a power cut is active. It starts at 0 at route
  load, so the first 50 ticks (pre-departure) fade it in.

### 4.4 Brake cylinder (task+0x4c) and brake force
Cylinder `c`, starts at −0.4 (`0xbecccccd`):
* If **last tick's** effective brake (task+0x64) is 0: `c −= 1.3 × brake_rate`, floor −0.4
  (`c ≤ −0.4` → −0.4f). The negative range is a release delay: re-applying takes `0.4/rate` ticks
  (5 ticks for 0.08) before any force.
* Otherwise: traction is set to **0** this tick; `target = min(B, 9) × 0.1 × brake_per_notch`
  (B1..B8 → 0.1..0.8, EB and 10 → 0.9); `c += brake_rate`; if `c ≥ target` then
  `c −= 1.2 × brake_rate` and clamp up to `target`. Net effect: rises at `rate` per tick, holds at
  the target, and when easing to a lower notch falls only `0.2 × rate` per tick.
* task+0x58 counts ticks with `|Δc| > 1e-5` (brake-air sound).

Brake force (0x422750), zero unless `c > 0`. `vl = top × 0.2777777f`, `v' = min(v, vl)`,
`x = (vl² − v'²)/vl²`:
```
fade  = 0.45 − 0.18·v'²/vl²          (0.45 at rest → 0.27 at top speed)
low   = 1 + 0.8·x                     (1.8 at rest → 1 at top speed)
shape = 0.9 + 0.12·x
load  = 1 − 0.0018·N                  (N = 0x857084, timetable row field +0x10; 0 in the rows seen)
Fb    = (gain1·gain0·c) · shape · (fade·low [·0.8 if 0x857230 > 1] · load) [·4 if 0x857206 ≥ 4] · 0x84c438
```
For type 0 at B8 (c = 3.2) this gives ≈169 at rest (≈5.5 km/h/s) and ≈50 at top speed: braking
fades strongly with speed.

### 4.5 Track terms (only while driving and a stage is set, 0x8570a4 ≥ 0)
* **Gradient** 0x857228 (0x407c10): `(h[slot+6] − h[slot+4]) / 4096`, `h` = raw `i16` height of
  the main-line slots ahead of the camera slot (`TrackPos::slot`). Force
  `G = 0x84c404 × gradient × 2000`; `G ≥ 100` → 0 (glitch guard); `0 ≤ G < 100` → `G × 0.8`
  (uphill is softened); negative (downhill) as is. Stored in 0x84c3d8 and kept when not updated.
* **Curve** (0x4228b0): `s = min(| prev − |heading| | × 700, 200)` → 0x84c408, where heading
  0x850288 = `atan2(Δz, Δx)` of the camera's track chord (written by 0x41d160) and `prev` =
  task+0x54 (the previous |heading|; at route load the raw heading). `R += 0x84c42c × s`.
  So curve resistance is proportional to the change of direction per tick, i.e. to speed ÷ radius.
  0x84c408 ≥ 2.1 also triggers flange squeal (sound 0x19) on trains with flag bit 1 of
  0x471f9c.

### 4.6 Integration
```
A      = F·fade − R − R2 − Fb − G            (task+0x44; F·fade is 0 if the brake was on)
a      = A × 0.032425421 × 0.27777777        (task+0x40, m/s²)
v_new  = a × 0.03333333 + v
v_new  < 0              → 0
v_new ≥ top × 0.277777  → top × 0.2777777   (speed cap)
odometer 0x857004 += v_new × 0.03333333      (metres)
leg      0x84c40c += v_new × 0.03333333      (metres since the last station event)
0x850208 = v_new × 0.003333333               (track units for the camera, = V × 0.000925926)
0x857234 = v_new;  task+0x60 = P;  task+0x64 = B (after overrides)
```
`0.032425421 × 0.27777777` ≈ 1/111: a force of 100 is ≈ 0.9 m/s² (3.2 km/h/s). There is **no
wheel slip** model, no separate notch lag besides the brake cylinder and the traction fade, and
no mass/inertia term beyond the resistance divisor. **confirmed**

### 4.7 Not driving (flag 0x200 of 0x855ff8 clear: pre-departure, stop sequences)
Everything in 4.2–4.4 is still computed (so the cylinder follows the handle during the doors
sequence), then: speed, acceleration, force, 0x850208, 0x84c40c, 0x84c41c, 0x84c3f8, 0x84c420,
0x84c434, 0x84c3ec, the ATC latch and several sound counters are zeroed; task+0x54 = |heading|;
0x857228 = 0; the brake history 0x84c410[0..3] = B. The odometer is kept.

## 5. Position and distance — confirmed

* **Track position**: the in-cab camera task (0x41d160, scene.md §3) keeps a slot pointer and a
  fraction `t`; each tick `t += 0x850208` and **0x8500c8 −= 0x850208** (track units left to the
  next stop). Engine index 0x8502a4 = slot − main-line base. `shuppatsu_formats::scene::CabWalker`
  reproduces the walk.
* **Distance to the stop** 0x85708c = `0x8500c8 × 10 + 0x85709c` (metres; 0x85709c is only ever
  set to 0). The integer 0x85722c is that rounded half away from zero (`fmod(|d|,1) ≥ 0.5`
  → floor/ceil away from zero). Both are computed by 0x40dad0 at the start of the next tick.
* At a non-stop station (`d < 0` and the next row's `stops` field is 0) 0x41c820 adds the next
  stop's distance to 0x8500c8 and 0x40d620 records the passing time.
* **Odometer** 0x857004 starts at `row[station].position − row[0].position` (0x40d040, 0x40d930)
  and grows by `v/30` per tick.

## 6. Stage clock — confirmed

0x8570b0, i32 ticks since midnight (timetable rows use the same unit; 30 ticks = 1 s; the HUD
formats `t/108000`, `(t/1800)%60`, `(t/30)%60`).
* Start/continue at station n (0x40d040, 0x40d930): `clock = row[n].time + 690` (690 ticks = 23 s
  of dwell after the scheduled arrival).
* Pre-departure wait (0x4080d0): `clock += 1` per tick.
* Driving loop: `clock += 1` unless 0x856d80 ≠ 0 (end-of-line sequence); then if
  `clock % 30 == 2`, `clock += 0x856fac`.
* **Speed-up 0x856fac**: after each stop evaluation, if the stage's rank `0x4656c4[stage·0x20]`
  is > 0 (true for all 20 stages), the station index > 3 and `(station − 4)` is even, it becomes 1
  (it never exceeds 1). With 1 the clock gains one tick each time it passes phase 2, i.e. every 29
  driving ticks (≈3.4 % fast). Reset to 0 on continue.
* Next segment after a stop (0x40d340): `clock += 690`.
* Arrival at a stop (0x40a0a0) and passing (0x40d620): `0x856fc0 = clock/30 − sched/30` (each
  divided separately, truncating); `sched` = 0x8571e8 = next row's time. |0x856fc0| goes to the
  penalty function (0x40dd60, kind 1). Passing within `0 ≤ clock − sched ≤ 29` gives the on-time
  bonus; arriving early (`clock < sched` with 0x85722c > −20) accumulates `sched − clock` into
  0x8570c4.
* Time-attack mode (0x8571f4) overrides the clock with fixed values (0x1a25c, 0x1a50e, 0x1a5e0).

## 7. Rule hooks inside the physics task (for rules.md)

Computed here, consumed by the rules (**confirmed** code; meanings likely):
* 0x84c3f8 = `0x84c41c − 0x857234` = speed lost over the previous tick (0x84c41c holds the speed
  two ticks back); max in 0x84c3f0, copy in 0x84c3e8.
* 0x84c420 = 0x84c434 = `Fb × 0.032425421 × 0.277777777 × 0.03333333` = brake deceleration per
  tick; max in 0x84c3e0, copy in 0x84c3e4.
* **Jolt penalty**: `0x856fb0 > 0 && 0x857206 < 2 && 0x856d80 == 0 && jolt_limit ≤ 0x84c3f8 &&
  jolt_limit ≤ 0x84c420` → `0x40dd60(3, 1)`.
* Brake history 0x84c410[3] (written at `clock % 3` with task+0x64) and 0x84c3ec =
  `task+0x64 − 0x84c410[0]`. **Notch slam**: moving, `|0x84c3ec| ≥ 5`, 0x8571f8 > 150, brake < 9
  → announcement + `0x40dd60(0xb, 2)`; also B8 from below 8 under 12 m/s.
* Crossing 95 % of top speed (`0x857234 < 0.95·top/3.6 ≤ v_new`) → `0x40dd60(0, 2)`.
* EB counters: 0x856d64 (consecutive ticks at EB while ≥ 4 m/s), 0x8571e0 (EB applications),
  0x856da8 (EB ticks near the stop), 0x8571f6 (power during the stop approach).
* ATC trip (§4.1) zeroes 0x856fb0.
* **Leg distance 0x84c40c** is also written by the driving loop before the physics runs, in
  the same tick. 0x40d620 clears it when the train passes a non-stop station. 0x40c4e0 adds
  2.5 m on stages of rank ≤ 3 (0x4656c4) once `trunc(leg)` is 50 or 51; the 2.5 m skip makes
  the 50 m check fire only once, and it may trigger announcements 0x20/0x21. The engine
  provides these as `TrainState::pass_station` and `TrainState::leg_marker`. **confirmed**
  (the replay is exact with both).

## 8. Oracle trace schema

Log at entry and exit of 0x422c20 (the task pointer is its argument; or find the list-2 task
whose update is 0x422c20). `i16` fields are words.

| address | type | meaning |
|---|---|---|
| FPU CW | u16 | `fnstcw` at entry (precision check, §2) |
| 0x829d00 / 0x829d04 | i32 | power / brake input (latched) |
| 0x855ff8 | u32 | phase flags (0x200 driving, 0x400/0x10 coast) |
| 0x8570a4 | i16 | stage |
| 0x84c3fc | i32 | train type |
| 0x84c404 / 0x84c42c | f32 | grade / curve scale |
| 0x84c3f4 / 0x84c438 | f32 | traction / brake scale |
| 0x857206 | i16 | overrun state (≥ 4 forced stop) |
| 0x856fba | i16 | power cut state |
| 0x857230 | i16 | slippery-rail counter |
| 0x84c424 | f32 | ATC limit, km/h |
| 0x857084 | i32 | brake load |
| 0x857228 | f32 | gradient |
| 0x850288 | f32 | heading, rad |
| 0x857234 | f32 | speed, m/s |
| task+0x38 | f32 | traction fade |
| task+0x3c | f32 | speed working copy |
| task+0x40 | f32 | acceleration, m/s² |
| task+0x44 | f32 | net force |
| task+0x4c | f32 | brake cylinder |
| task+0x54 | f32 | previous \|heading\| |
| task+0x58 | i32 | brake-moving tick count |
| task+0x60 / +0x64 | i32 | power / effective brake of the last tick |
| task+0x8c | i16 | ATC latch |
| 0x84c3d8 | f32 | gradient force |
| 0x84c408 | f32 | curve sharpness |
| 0x857004 | f32 | odometer, m |
| 0x84c40c | f32 | leg distance, m |
| 0x850208 | f32 | track units this tick |
| 0x84c41c, 0x84c3f8, 0x84c3f0 | f32 | speed two back, speed lost, max |
| 0x84c420, 0x84c3e0 | f32 | brake decel per tick, max |
| 0x84c410[3], 0x84c3ec | i32 | brake history, delta |
| 0x856fb0 | i32 | remaining-time/continue counter (zeroed by the ATC trip) |
| 0x8570b0 | i32 | stage clock |
| 0x856fac | i16 | clock speed-up |
| 0x856d80 | i16 | clock freeze (end-of-line) |
| 0x8500c8 | f32 | track units to the stop |
| 0x8502a4 | i32 | engine index |
| 0x85708c | f32 | distance to stop, m |
| 0x85722c | i32 | distance to stop, rounded |
| 0x856d78 | i32 | station index |
| 0x8571e8 | i32 | scheduled time at the next station |
| 0x856fc0 | i32 | seconds late at the last stop/pass |

### Row alignment (confirmed by the replay)

The hook writes one row per game tick at the end of `Task_Tick`, before that tick's input
poll. Row *k* therefore shows the state after tick *k*'s physics. The inputs of tick *k* come
from row *k* as well: the notches latched at the end of tick *k−1*, and the driving-loop
globals (gradient, flags, ATC limit, overrun state, power cut, slippery, brake load, clock). The
heading is the exception: the camera task writes it after the physics, so the physics of tick
*k* sees the heading of row *k−1*. `shuppatsu-tools trace-replay` (`crates/shuppatsu-tools/src/trace.rs`)
replays the trace with this alignment.

**Two notch timings.** The notches of a scripted run are written by the hook in the same call
that logs the row. `Task_Tick` (0x4253c0) runs the task lists, then `0x84c494++`, then
`PumpMessages` (0x42a130, whose `PeekMessageA` the hook redirects), then the input poll
0x407ac0; the physics task reads `0x829d00`/`0x829d04` directly (0x422c20). A value the hook
writes in row *k* is therefore first used by tick *k+1*. That is what every capture made with
`scripts/capture-original.sh` (Xvfb, Wine virtual desktop) shows: at a logged notch change the
physics task's copy of the notches it used (`t_last_power` / `t_last_brake`, task+0x60/+0x64)
still holds the old value (depart13-*.csv, rails13.csv; the hook log names a single thread for
all rows). The traces of 2026-10-15 (hilly, stops, yamate; run with `run-oracle.sh` on the
desktop) show the new value in the same row instead. Why is **open** (which `PeekMessageA`
caller saw the new tick first is the likely difference; the hook now logs the thread that
writes rows). The replays read the timing from the trace (`Trace::notch_timing`: the first
driving row whose notch changed) and, for the later kind, take tick *k*'s notches from row
*k−1*, except on a segment's setup row, where 0x4080d0 has forced N / EB (`0x4332c0(0, 9)`)
before that tick. Buttons are read by the game in the pump after the hook (0x4333d0) and keep
the row-*k* alignment. **confirmed** (all three replays of depart13-mark.csv are exact with it,
and miss on the first notch change without it)

## 8a. Whole-run replay: `shuppatsu_engine::drive::Drive`

`Drive` (`crates/shuppatsu-engine/src/drive.rs`) runs one stage from the route load to the end:
rules, physics, the in-cab camera walk and every track-derived input (gradient, heading, units
to the stop, sound class ahead, station passing, segment changes, odometer). `shuppatsu-tools
trace-replay <csv> --drive` (`crates/shuppatsu-tools/src/trace/drive.rs`) feeds it only the logged
notches and buttons plus stage, difficulty and start station, and compares every column.

**Result** (all floats bit for bit):

| trace | stage | ticks | values | mismatches | covers |
|---|---|---|---|---|---|
| hilly-drive.csv | 0 | 7 155 | 815 657 | 0 | 2 segments, passing stations, red-signal/ATS recovery, game over while driving, continue |
| stops-drive.csv | 13 | 10 522 | 1 170 663 | 0 | 3 segments, 2 arrivals with results, game over on the result screen, continue, final time over |
| yamate-drive.csv | 5 (hokuho1a) | 6 771 | 338 537 | 0 | passing station, recovery, game over inside the approach, continue to the next station |
| depart13-mark.csv, -mark-d1, -b, -c, -d | 13 | 13 673–13 963 each | ≈ 1.6 M each | 0 | 4 legs, 3–4 results, false starts on later legs, notches set during the wait (rules.md §14) |
| depart17g.csv, depart17h.csv | 17 | 24 604 / 24 577 | 2.84 M each | 0 | 4 legs from station 0 (offset 180), passing stations, horn signs missed |
| depart3e.csv, depart3f.csv | 3 | 14 457 each | 1.65 M each | 0 | 4 legs, cut at the weather start (rules.md §13) |

Not compared: the camera globals before the first setup tick (they belong to the stage
intro's camera, as does the physics copy of the heading on the first setup row), and rule
columns while stopped at a station (the result screen deducts over several ticks; `Drive`
deducts in `finish_station`). yamate-drive.csv has no button columns: the engine missed the
whistle sign at row 3898 where the original lost no time, so the replay holds the horn from the
sign to that row (reported as an inferred input). The HUD transfer phase (counter of the HUD
task since the stage load, which depends on the intro) is inferred from the first ±1 s step.

Findings this replay needed (all **confirmed** by code and by the traces above):

* **Heading rounding**: 0x41d160 stores Δz to a float temporary (`fstp [esp+0xc]` at
  0x41d31f) and reloads it for `fpatan` (0x41d362), while Δx stays on the FPU stack.
  `atan2(f32 Δz, f64 Δx)`; the double Δz gave a 1-ulp heading error at yamate row 6431.
* **Gradient** 0x857228 is zeroed by the not-driving branch of the train task (§4.7) and only
  recomputed by 0x407c10 in driving iterations; it keeps its value while the train coasts after
  a game over.
* **Track units** at a segment start: `(stop[s+1] − stop[s]) × 0.125f` (0x41c7b0, one f32
  rounding); passing: `units += leg × 0.125f` in double, stored once (0x41c820); the camera
  task subtracts `0x850208` with one f32 rounding (0x41d181).
* **Red-signal / ATS recovery** (0x40a0a0, second branch) keeps `0x855ff8 & 0x200` set: the
  train task runs its driving branch throughout. 0x857234 is zeroed before each of the 60
  standstill ticks only; the 10 ticks after them and the loop's own `Task_Tick` are not forced
  (`rules::RECOVERY_RELEASE_TICKS` = 11). The arrival branch clears `0x200` (0x40a590).
* **Flow tick counts** of 0x4080d0, all between rows of the traces: route load → setup tick:
  2 idle ticks; result screen closed → 0x40d340: 1 tick, then 4 idle ticks to the setup tick;
  continue screen closed → 0x40d930: 2 ticks (no coast, `0x200` cleared), then 3 idle ticks (13
  when the time ran out inside the approach: 0x40d340 + 0x40d930 + 10 ticks); arrival:
  `0x857206 = 0`, `0x8570d4 = −1` (0x4085e0) before the first tick after the stop sequence and
  `0x857004 += 0x85708c` before the second.
* **Game over while driving**: 20 `Task_Tick`s including the game-over tick, then the pending
  time is discarded and 0x4085e0 resets the ATS; the train coasts (`0x210`) with the camera
  walking until the continue screen closes.
* `0x8570d4` (ATS chime start) is not reset by 0x40cf40; it is 0 before the first departure
  and −1 after every wait / loop exit.
* 0x40d340 on a game over (remaining < 1 after a result) still updates the target, resets the
  segment state and adds the 690-tick dwell (stops-drive.csv row 4381).
* The signal blocks (0x40b970) are built in the segment setup, not by 0x40d340/0x40d930: the
  old blocks stay visible in between (hilly-drive.csv rows 7778–7780).

Not exercised by any trace: arrival at the final station (+120 ticks, 0x40aaa5), the obstacle
sequence, weather, the power-cut fade, the ATC trip.

## 9. Open questions

* Precision on real Windows (§2): Wine runs in double precision.
* 0x857084 (brake load) is 0 in the rows checked; are other values used anywhere?
* ~~Attract/demo mode 2 and the ending train~~: the replay mode (task+0x74 = 2, one input byte
  per update from 0x467b20 / 0x46857c) is `shuppatsu_engine::demo`; 0x4240d0 is the bonus game's
  physics task (`shuppatsu_engine::bonus`), not an ending train (menus.md §2.4, §12.1).
* Which settings value 0x829d10 holds (difficulty-like; stage tables are indexed by it).

## Decision: numeric reference (user, 2026-10-15)

The reference is the measured Wine behaviour: x87 at 53-bit precision (control word 0x027f),
which the engine reproduces bit for bit. Real Windows with D3D's `DDSCL_FPUSETUP` probably ran at
24-bit precision; the other platforms (PS1 fixed point, PS2/Dreamcast single precision) differed
anyway. The visible effect is a fraction of a km/h over minutes. A 24-bit mode may be added later
as an option and can be verified with the oracle by having the hook set the precision as
DirectX would.

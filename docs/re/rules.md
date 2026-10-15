# Game rules: signals, speed limits, ATS, remaining time, stop evaluation, grades

Scope: everything in the driving loop that is not train physics. Source: Ghidra decomp in
`re/decomp/dengo3k/` and `objdump -d` of `BIN/dengo3k.exe` (3000-bandai), used where the decomp
lost float expressions. Implementation: `crates/ddg-engine/src/rules.rs`. Tests:
unit tests in that file and `crates/ddg-engine/tests/rules_data.rs` (needs `DDG_DATA_DIR`).

Confidence: **confirmed** = read from code (and constants checked in the exe);
**likely** = strong inference from code; **guess** = needs an oracle trace. Nothing here has been
checked against the running game yet: every "confirmed" means "this is what the code says".

All tuning numbers live in tables read at runtime by `RuleTables::read`. Most are
`[stage][difficulty]` arrays of `i16` indexed `difficulty + 4·stage` (160 bytes each), with the
difficulty in `0x829d10` (settings item 1, loaded from `eeprom.dat`). Column 0 is the easiest
(e.g. 90 s starting time vs 20 s in column 3) and columns 1 and 2 are often identical
(**likely**: the four arcade levels EASY / NORMAL / HARD / HARDEST).

## 1. Who does what in the driving loop

One loop iteration per tick (0x4084f5, see main-loop.md §5). Order and roles:

| Order | Function | Role | Conf. |
|---|---|---|---|
| 1 | 0x407c10 | Track-sound lookup (`0x857200[3]`, class 3 = tunnel), gradient. Not a rule; feeds the weather "wet rails" flag only | confirmed |
| 2 | 0x40dad0 | Lateness drain; distance to stop `0x85708c` and its rounded copy `0x85722c`; **passing stations** (calls 0x40d620, then rebuilds the signals); overrun > 50 m trips the ATS; approach latch | confirmed |
| 3 | 0x409fc0 | Forced stops: past timetable field 8 (buffer stop), obstacle event. Also records a stop-distance graph (ring buffer 0x850300, presentation) | confirmed |
| 4 | 0x40acd0 | **Course events** (one per tick), horn sign timers, no-power section | confirmed |
| 5 | 0x40b4a0 | **Speed-limit events** (one per tick) | confirmed |
| 6 | 0x40b710 | Traffic (AI trains) events — not a rule | confirmed |
| 7 | 0x40b780 | **Enforcement**: speed limit and signal speed, overspeed penalties, signal overspeed → ATS | confirmed |
| 8 | 0x40c010 | **Signal blocks**: advance the block index, sight the next signal, set the signal limit | confirmed |
| 9 | 0x40c4e0 | Station approach HUD and state, **EB penalty**, **ATS state machine**, weather, hint messages | confirmed |
| 10 | loop | `clock++` unless the obstacle event runs; `if clock % 30 == 2: clock += skew` | confirmed |
| 11 | loop | if remaining time > 0: **0x40a0a0** (stopped-train handler: arrival, stopped short, red-signal recovery) else **game over** | confirmed |
| 12 | `Task_Tick` | Player-train task 0x422c20 (physics plus EB counters, re-power detection, derailment) then the HUD task (remaining-time transfer) | confirmed |

0x40a0a0 and parts of `Stage_Run` contain **blocking loops** (`for n: speed = 0; Task_Tick()`).
During them the rule functions don't run and the clock (`0x8570b0`) does not advance, but the HUD
keeps transferring time. The engine models them as `Phase::Holding`.

## 2. Tables

`[stage][difficulty]` i16 unless noted. Names are ours.

| VA | Name | Use | Conf. |
|---|---|---|---|
| 0x459870 | limit margin, km/h | Added to every limit and signal speed (3 or 5) | confirmed |
| 0x4597d0 | stop tolerance, m | `0x8570d0`: ±window for a valid stop (0–3 m) | confirmed |
| 0x4599b0 | start time, s | Remaining time at start/continue | confirmed |
| 0x45a310 | late drain, s | Lost per second behind schedule | confirmed |
| 0x45a3b0 | limit overspeed, s | Speed-limit violation | confirmed |
| 0x45a450 | EB penalty, s | Emergency brake | confirmed |
| 0x45a590 / 0x45a630 | signal overspeed min / cap, s | −1 in the min table selects the cap rule | confirmed |
| 0x45a6d0 | false start, s | Power before departure | confirmed |
| 0x45a770 | stopped between stations, s | Every 50 ticks after the grace period | confirmed |
| 0x45a810 | horn missed, s | Whistle sign not obeyed | confirmed |
| 0x45a8b0 | re-power, s | Power during the station approach | confirmed |
| 0x45a950 | horn bonus, s | Full horn bonus | confirmed |
| 0x45a9f0 | exact stop bonus, s | Stop within 1 cm | confirmed |
| 0x45aa90 | on-time stop bonus, s | + streak | confirmed |
| 0x45ab30 | clean stop bonus, s | | confirmed |
| 0x45abd0 | on-time pass bonus, s | + streak | confirmed |
| 0x45ad10 | coasting bonus, s | No-power section | confirmed |
| 0x45db58 | limit notice, m | Default advance notice of a speed limit (300/250/250/200 typ.) | confirmed |
| 0x459d20 | weather minimum time, s | Weather starts only if this much time is left | confirmed |
| 0x459eb0, 0x459f50, 0x459ff0, 0x45a090, 0x45a130, 0x45a1d0, 0x45a270 | early thresholds, ticks | Signal-pattern selection (§4.3) | confirmed |
| 0x459cd0 | i16[20] block length, m | Signal spacing (330, 300, 250) | confirmed |
| 0x459cf8 | i16[20] weather station | 99 = never, 0 = from the start | confirmed |
| 0x459dc0 | i32[20] weather duration, ticks | | confirmed |
| 0x4656c4 | i32, stride 0x20 | Stage kind: 1 = lesson stage (stage 5), > 3 = special ending, < 4 enables hint messages | likely |
| 0x4591d0 | 8 × i16[5] | Aspect patterns | confirmed |
| 0x459220 / 0x45922a | i16[5] | Scripted patterns (other stages / stage 4) | confirmed |
| 0x459234 | rows of 6 × i16, until stage −1 | Scripted signals `{stage, station, kind, blocks_back, train, train_distance}` | confirmed (layout) |
| 0x45951c, 0x459518, 0x459514 | f32 70, 45, 25 | Signal speeds above the margin for aspects 1–3 | confirmed |
| 0x45a4f0, 0x45ac70 | | Penalty / bonus in the ending sequences (0x41b130, 0x41b1e1) — not modelled | likely |

## 3. Remaining time (持ち時間) and the clock

* `0x856fb0` **remaining time in whole seconds**, shown with 3 digits, capped at 999. Starts from
  0x4599b0 at stage start (0x40d040) and after a continue (0x40d930). **confirmed**
* Every bonus and penalty goes through **0x415080(kind, seconds)**, which shows the HUD message
  and calls 0x4122c0 to add ±seconds to the **pending** counter `0x856d7c`. The sign comes from
  the kind (table built on the stack in 0x415080: kinds 1, 2, 3, 12, 13 add, others subtract).
  `seconds == 0` shows the message without a number and changes nothing. **confirmed**
* The **HUD task** (update 0x40ff20, created by 0x40f7e0 at stage load, counter at task+0x34
  starting at 0) moves pending time into the remaining time **one second every third tick**
  (`counter % 3 == 2`, 0x4100b2): positive → `+1` (cap 999, sound 0x26), negative → `−1`
  (sound 0x27; at 0 the unit is dropped). The digits blink while `remaining ≤ 14`. **confirmed**
* **Game over**: after the rule functions, `if remaining ≤ 0` the stage ends (0x40857b) and
  `0x855ff8` gets 0x10 (the train coasts to a stop under 0x210). The HUD keeps transferring for
  the 20-tick wait; only then is the rest of the pending change discarded (`0x856d7c = 0` at
  0x4085d0). **confirmed** (oracle: hilly-drive.csv still shows pending −1 on the game-over row
  and the HUD draining it 2 rows later). A continue is offered unless it was the final
  station. Remaining time can also hit 0 by derailment (§8). Between stations,
  `remaining < 1` at the station result also ends the run (0x40d340), unless the station was the
  last one (then the run is cleared). **confirmed**
* **Lateness drain** (0x40dad0): while `clock > scheduled` and the stage is active, every
  30 ticks of lateness (`(clock − scheduled) % 30 == 0`) costs 0x45a310 seconds (1 or 2) via
  the pending counter. So arriving late costs time continuously; there is no separate
  "late arrival" penalty. **confirmed**
* **Clock** `0x8570b0`, ticks since midnight (30/s). Starts at `row[s].time + 0x2b2` (23 s dwell);
  `+1` per driving tick and per pre-departure tick; frozen during blocking sequences and while the
  obstacle event runs. At each station departure `clock += 0x2b2` (0x40d340). **confirmed**
* **Clock skew** `0x856fac`: after each stop at stations 4, 6, 8, … (`(station−4)` even) it is
  raised to 1 (max 1), so the clock runs `31/30` fast from then on (`clock % 30 == 2 →
  clock += skew`). Reset to 0 on continue. Condition `stage kind > 0` is always true in data.
  **confirmed** (code); the gameplay intent (pressure late in the run) is a **guess**.
* Scheduled arrival `0x8571e8 = row[station+1].time`. **confirmed**

## 4. Signals

### 4.1 Blocks (0x40b970, at each segment start and after passing a station)

The segment to the next station is cut into blocks of `L = 0x459cd0[stage]` metres. Record
`{sight_pos, sight, aspect}` (12 bytes) at `0x856dc0 + 12·i`, positions are distances to the stop
point. With `D = row[s].to_next`, `H = row[s+1].f5` (home-signal distance), `n = (D − H) / L`,
`S = trunc(L × 0.66666)` (f32 at 0x459510):

* blocks `n+2 … 4`: `sight_pos = S + D − L·m` (m = 1, 2, …), `sight = S`;
* block 3: `S + H` (if not below the last boundary, else `boundary − 20` and `sight = pos − H`);
* block 2: `{H, 50}` — the **home signal**; never painted by patterns;
* block 1: passing station → `{trunc(H·0.66666)}` (the starting signal after the platform),
  stopping station → `−999` (none);
* block 0: `−999`.
* Current index `0x857080 = max(n,1) + 2`, also stored in `0x85720c`; block `k+1` is a copy of
  block `k`. **confirmed**

A signal is **sighted** when the distance drops below `sight_pos` (0x40c010: `k−1` per tick at
most). The aspect in force (`0x8570a6`) becomes that block's aspect, the HUD shows it
(0x411500, skipped if the same sprite is already shown) and the signal speed limit
`0x857090 = speed[aspect] / 3.6` is armed; it is **enforced from `odometer + sight`** onwards,
i.e. from the signal itself (`0x856fcc`). Aspect 0 sets 300 km/h. No new sighting happens while
the ATS is in emergency or for block 1. **confirmed**

### 4.2 Aspects and speeds — confirmed

| Aspect | Meaning | Speed (km/h) |
|---|---|---|
| 0 | 進行 G | 300 (none) |
| 1 | 減速 YG | margin + 70 |
| 2 | 注意 Y | margin + 45 |
| 3 | 警戒 YY | margin + 25 |
| 4 | 停止 R | 0 |

### 4.3 Where restrictive aspects come from: running early

`0x8570c4` accumulates how **early** you are, in ticks: at a stop it adds `scheduled − clock`
when early (and the stop was not overrun by ≥ 20 m), else resets to 0; passing stations do the
same (0x40d620). At segment start, if no restriction carries over:

* `early < 0x459eb0` → all green.
* `early ≥ 0x459ff0` and the stage/station is in the scripted list 0x459234: kind 0 paints the
  scripted pattern `blocks_back` blocks before the first block (a train ahead, with a passing-train
  effect), kind 1 arms the obstacle event (§8). `early` resets to 0. **confirmed**
* Otherwise a pattern from 0x4591d0 is chosen (**confirmed**):

| Left a stopping station | Threshold reached | Pattern | Start | Restricted blocks `0x856d70` |
|---|---|---|---|---|
| yes | 0x45a270 | 6 | `clock % n + 4` | 3 |
| yes | 0x45a1d0 | 5 | `k + 1` (first aspect applies at once) | 2 |
| yes | 0x45a090 | 3 | `k + 1` | 2 |
| yes | 0x459f50 | 1 | `k + 1` | 1 |
| yes | below | 0 | `clock % (n − c) + 3 + c` | 1 |
| no | 0x45a270 | 6 | as row "below" | 3 |
| no | 0x45a130 | 4 | 〃 | 2 |
| no | 0x459ff0 | 2 | 〃 | 2 |
| no | below | 0 | 〃 | 1 |

  The pattern is written from the start block downwards (skipping block 2), advancing through
  its 5 aspects until it reaches 4 (red), which then fills the remaining blocks. Patterns all end in
  red, so running early **always ends at a red signal** before the station. If the start is above
  the current block the first aspect is in force immediately (enforced from `odometer + 10`).
* If an aspect is still in force when a passing station rebuilds the blocks, it propagates:
  every block gets `min(aspect+1, 4)`, then 4 (3 is skipped). **confirmed**
* Each sighting under an enforced restriction decrements `0x856d70`; at 0 all blocks turn green
  (0x40bfd0) — the train ahead has moved on. **confirmed**
* Course event kind 0 overrides: current block = `a`, the blocks ahead `a+1, a+2…` up to 4
  (`b == 0` skips 3), `0x856d70 = c`. Only 2 such events in the data. **confirmed**
* Kind 14 (`0x856fb4 = a`, 33 events, a = 100): a **repeater** — at the next sighting the HUD shows
  the signal in mode 3 and the block is re-entered `a` metres later. **likely** (code confirmed,
  name a guess). Kinds 15/16 set/clear the display variant `0x8570c2`. **confirmed** (meaning guess)

### 4.4 Stopping at a red / after an ATS stop (0x40a0a0)

When stopped (|v| ≤ 0.0001) with the ATS in emergency (`0x857206 ≥ 4`) or aspect 4, and not at
the station (short by more than the tolerance, or a passing station): all signals turn green,
signal limit cleared, notches forced to P0/B9, **60 ticks** standstill (or 30 + a passing-train
animation of 200/300/360 ticks for scripted signals), the HUD shows a green, then 10 ticks. If
no signal is in sight ahead, re-powering during the station approach is excused (`0x857238 = 1`).
**confirmed** (structure), hold lengths **likely**.

## 5. Speed limits

Limit events (0x485850, kinds 4/5/6; course events can carry the same kinds) — **confirmed**:

* `{station, distance, kind, a = km/h, b = notice distance shown, c}`. Valid only if `a/5 − 4 ∈
  0..=21`; an invalid record **stalls the list** (not in data).
* `c == −1`: trigger at `distance`, enforced immediately. `c == 0`: trigger
  `distance + notice` with the difficulty's notice (0x45db58), enforced `notice` metres later.
  Other `c`: trigger at `distance`, enforced `c` metres later. Enforcement starts at
  `0x856da0 = trunc(odometer + offset)` → `0x856fc4 = 1`.
* Kind 4 applies only when no limit is active (`0x856fc4 == 0`), kind 5 only when one is. Both set
  `0x8570bc = (a + margin) / 3.6` and re-arm (`0x856fc4 = 0`). Kind 6 ends the limit (300 km/h).
* While enforced, `speed > limit` costs **0x45a3b0 seconds once** (`0x856fc4 = 2`, no more checks
  until the next limit event), stat 5 += 5. Overspeed against a speed limit does **not** trip the
  ATS. A stop at a stopping station resets the limit at the next segment.
* The list is consumed in order, one record per tick, firing when `station == current` and
  `rounded distance ≤ trigger`. A record whose station is below the current one never fires and
  blocks the rest of the list (stage 0's limit list ends with three such station-0 records).

## 6. Signal overspeed and the ATS

* Enforced signal (`0x856fa0` 1/2) and `speed > signal limit` (0x40b780): penalty
  `v = trunc((speed − signal_limit + margin/3.6) × 3.6)` = km/h above the nominal aspect speed;
  if 0x45a590 ≠ −1 the penalty is `max(v, 0x45a590)`, else `min(v, 0x45a630)`. stat 5 += 5,
  `0x856fa0 = 3`, and **the ATS trips** (`0x857206 = 1`). Passing a red at any speed > 0 trips it.
  **confirmed**
* Other trips: overrunning a stopping station by more than 50 m (0x40dad0, message kind 7 without
  time), and the pre-departure false start (§7). **confirmed**
* ATS state `0x857206` (0x40c4e0), counter `0x857240` (ticks since trip) — **confirmed**:
  * 1 → bell (sound 0xb). Before the stop point: stat 7 += 2·aspect; **red → 4 at once**,
    otherwise → 2. Past the stop point: stat 6 += 10, → 4.
  * 2 → while `0x857240 < 90` (3 s): **confirm (A) pressed with brake ≥ 1** clears it (→ 0, bell
    stops, chime every 32 ticks from `0x8570d4` until the next sighting). Else → 4.
  * 4 → **emergency brake**: the train task forces B9/P0 (0x422e3b); when stopped → §4.4.
* The ATS bell itself costs no time; the time cost is the overspeed penalty that tripped it.

## 7. Stations

### 7.1 Approach

* `0x856d74` = target station stops (timetable field 3). `0x856d9a` approach state: 1 when the
  distance drops below field 6 (and the speed in km/h is latched in `0x856d8c`), 2 next tick,
  3 below 80 % of field 6, 4 when overrun by > 20 m (not at the final station). **confirmed**
* **Re-power** (train task 0x422eb3): during the approach (`0x856d9a > 0`), raising the power
  notch while no ATS, or any power once in state ≥ 3, sets `0x8571f6` (once) and stat 3 += 5.
  **confirmed**
* **EB in the approach**: ticks with B ≥ 9 and v > 0 during the approach count in `0x856da8`.
  **confirmed**
* Passing stations: when the distance becomes negative, 0x40d620 runs: `late = clock/30 −
  sched/30` (stat 1 += |late|); if `0 ≤ clock − sched ≤ 29` → **on-time pass bonus**
  `0x45abd0 + streak` (`0x857094`, +1 each), else streak 0; the early accumulator is updated;
  station + 1; blocks rebuilt. **confirmed**
* Field 8 (`row[s+1].f8`, −999 = none, −15/−40 at terminals): rounded distance ≤ f8 → speed
  forced to 0 and `0x8570b4 = 1` (buffer stop). **confirmed**

### 7.2 Stopped (0x40a0a0, every tick while time remains)

Let `d` = distance rounded to metres half away from zero (`0x85722c`), `tol` = 0x4597d0.

* **Short** (`d > tol`) at a stopping station, or anywhere for a passing station: "stopped
  between stations" (`0x8571ec` ticks). Not yet in the approach: hint at 50 ticks, a message
  every 50 ticks from 150, and from tick **650** on −0x45a770 s every 50 ticks (5 s per 1.67 s).
  In the approach: messages every 50 ticks from 30, time from tick **430**. Stopping inside a
  no-power section costs 10 s once. **confirmed**
* **At the station** (`d ≤ tol`, overrun allowed): the stop is accepted.
  1. If past field 8: −20 s (message only at the final station), 80 ticks hold, stop error + 20.
  2. Wait until the pending time is 0.
  3. Stop error sum `0x856fa4 += |dist|`, stop count `0x856fc8 += 1`, `late = clock/30 −
     sched/30` (stat 1 += |late|), early accumulator.
  4. **Clean stop** = `d == 0` (|dist| < 0.5 m) and no EB in the approach and no EB held and no
     un-excused re-power. On time (`|late| < 3`): **+0x45aa90 + streak** (`0x8570c0`), mark 2;
     otherwise **+0x45ab30**, mark 1. Not clean: streak 0. 100 / 30 ticks hold.
  **confirmed**

### 7.3 Station result (0x413d90 + task 0x413f30)

Deducted directly from `0x856fb0` (not through the pending counter) — **confirmed**:

* **Overrun**: `d < −tol` → `−d` seconds (1 s per metre of overrun, cap 999).
* **Re-power** in the approach: 0x45a8b0 s.
* **EB** in the approach: 10 s (stat 4 += 2); not on lesson stages.
* Sum capped at 999 (20 on lesson stages), then `+0x45a9f0` (**10 s**) if `trunc(|dist|·100) == 0`
  (within 1 cm) and the station got a clean-stop mark. Clamped at 0.

So: the **合格範囲** is ±`tol` metres (short side: you must creep up; long side: costs a
second per metre beyond it), the bonus window is ±0.5 m, the perfect window ±1 cm.

### 7.4 Final station, continue

Reaching the last station clears the stage regardless of the remaining time. A continue
(0x40d930) rewinds to the last **stopping** station, resets the remaining time, the per-continue
stats (`0x857020`), the early accumulator and the clock skew, and `0x8570b8 += 1`. **confirmed**

## 8. Other rule events

| Kind | Course event | Effect | Conf. |
|---|---|---|---|
| 0 | signal override | §4.3 | confirmed |
| 1 / 2 | sound `a` / station announcement `a` | presentation | confirmed |
| 3 | horn | `a = 1`: whistle sign, horn (held) required within `b` m (odometer deadline `0x856d6a`, an **i16**), else −0x45a810 s and stat 10 += 2. `a = 0`: horn bonus zone of `b` m: pressing the horn gives `max(1, trunc(0x45a950 × min(1, metres_left × 0.04)))` s. Other `a`: sign only | confirmed |
| 4 / 5 / 6 | speed limit | §5 (course variant: `c` = enforcement offset) | confirmed |
| 7 | derail speed | sound 0xe, `0x856cf4 = a`, ATC / derailment speed `0x84c424 = b` km/h (default **999** = 0x4479c000, 210 in the bonus section; oracle: 999) | confirmed |
| 8 | obstacle stop | only if armed by a scripted signal kind 1 and not used yet: bell, stop target `odo + a·1.25 − 5`; the clock stops; reaching it forces v = 0; when stopped, ~36 ticks of scripted recovery, wet rails | confirmed (code), hold length guess |
| 9, 13, 17, 23 | HUD/screen effects | presentation | likely |
| 10 / 11 | no-power section | 10 enters (`0x856fba = 1`), any power → 2; 11 exits: still 1 → **+0x45ad10 s**. Stopping inside costs 10 s | confirmed (name **guess**: AC/DC dead section, only Akita/Ōu stages have bonuses) |
| 12 | scenery train event | presentation | likely |
| 14 / 15 / 16 | repeater, display variant | §4.3 | likely |
| 18 / 19 | `0x84c400 = 1 / 0` | unknown flag | unknown |
| 20 | `0x857224 = a` | message shown at the next green | confirmed |
| 21 / 22 | weather on / off | 21 starts only if `remaining ≥ 0x459d20`; level `0x857230` ramps to 254, > 199 sets wet rails `0x84c2e0` (cleared in tunnels); ends after 0x459dc0 ticks | confirmed (physics effect: physics agent) |

* **EB penalty** (0x40c4e0 with counters from the train task): `0x8571e0` counts EB
  applications (B < 9 → B ≥ 9), `0x856d64` counts ticks with B ≥ 9 at ≥ 4 m/s. The penalty
  (0x45a450 s, stat 4 += 6) fires when the EB has been held **40 ticks at ≥ 4 m/s**, unless the
  application count is a multiple of 4 — in which case it fires on the **first** tick instead. Not
  during the ATS or the obstacle event, and not when > 20 EB ticks were already counted in the
  approach. **confirmed** (code); the intent of the `% 4` is unclear.
* **Derailment** (train task 0x422e51): `speed > (0x84c424 + 3) × 0.27777777` sets the remaining
  time to 0. **confirmed**
* **Departure task** (update 0x413370, created per segment; counter task+0x34 from 0 in the
  segment setup tick, offset task+0x38 = 180 at station 0 of stages 0–2 and 17–19, else 0):
  * **False start**: counter in (32, offset+200), power > 0, ATS idle → ATS = 1, stat 8 += 2, and a
    70-tick freeze (task+0x50) during which the counter holds at 199. The pre-departure loop then
    turns ATS 1 into 2 and charges 0x45a6d0 s (kind 9); leaving the wait resets the ATS to 0.
  * **Brake released**: every tick with counter ≥ offset+160 and applied brake (`0x856cfa`) = 0
    counts in task+0x40; at counter = offset+200, if any → **stat 2 += 1**.
  * At counter ≥ offset+201 the wait ends (`0x800` cleared); the task itself ends once the train
    moves at ≥ 0.04 m/s.
  **confirmed** (code 0x4133c5–0x413790; oracle hilly-drive.csv: 381 waiting rows at offset 180,
  stat 2 = 1 at counter 380, a false start with ATS = 2 on the last waiting row of the continue).
* **Ride comfort** (train task, after the physics step; inputs `TrainReadout`):
  * jolt: remaining > 0, ATS ≤ 1, no obstacle, spec jolt limit (+0x38) ≤ `0x84c3f8` and
    ≤ `0x84c420` → stat 3 += 1. **confirmed** (oracle: stat 3 matches).
  * notch slam: speed > 0, `0x8571f8` > 150, and |`0x84c3ec`| ≥ 5 with last brake ≤ 8, or B8 from
    below 8 under 12 m/s → `0x8571f8` = 0, stat 11 += 2. `0x8571f8`: 100 at segment start,
    +1 per driving tick up to 200 (0x40cc04), 0 under ATS emergency or obstacle. **confirmed**
    (code; not exercised by the trace).
  * accelerating through 95 % of the spec top speed (+0x14) → stat 0 += 2. **confirmed** (code; not
    exercised).
  * EB applications also count the B9 the game forces at run start (input_force_notches(0, 9)):
    `0x8571e0` = 1 already on the first waiting row. **likely** (oracle)

## 9. Statistics, grades and ranking

`0x40dd60(cat, n)` adds to `0x857020[cat]` (since the last continue) and `0x857050[cat]` (run).
Not on stage 2 station 24. **confirmed**

| Cat | Source |
|---|---|
| 0 | ride comfort (train task 0x423587) |
| 1 | \|lateness\| in s at each stop/pass |
| 2 | brake released before departure +1 (departure task 0x4136fd) |
| 3 | re-power +5; comfort +1 (0x4233d4) |
| 4 | EB penalty +6, EB in approach +2 |
| 5 | overspeed +5 |
| 6 | ATS past the stop +10 |
| 7 | ATS 2·aspect |
| 8 | false start +2 |
| 9 | ending sequence +50 (0x41b134) |
| 10 | horn missed +2 |
| 11 | brake jerk (train task 0x4234d9 / 0x423536) +2 |

Categories 0, 3 (+1) and 11 come from the train task's comfort checks; the rules compute them from
the physics readout passed to `Rules::end_tick` (§8).

**Grade** (0x40dfd0), five parts, `div(x) = trunc(x / (continues + 0.2))` when continues > 0:
`M = clamp(P − continues/3, 1, 20)` with progress `P = (station+1)/stations × 21` (stage 1 before
station 9: `(s+1)/8 × 18`; stage 10 before 8: `s/8 × 16`).

| Part | Bad = | Cap | Value |
|---|---|---|---|
| timing | `div(max(T1 − 1, 0))` (0 if no stops) | 50 | `trunc((50−bad)·0.02·M)` |
| handling | `div(T3/8 + T11 + T4)` | 50 | `trunc((50−bad)·0.02·M)` |
| stopping | `div(trunc(stop error sum))` (0 if no stops) | 30 | `trunc((30−bad)/30·M)` |
| safety | `div(T7 + T5 + T6 + T9)` | 20 | `trunc((20−bad)·0.05·M)` |
| comfort | `div(T11/2 + T3/16 + T8 + T2 + T0)` | 30 | `trunc((30−bad)/30·M)` |

Sum `0x856c88` (0–100). **Ranking value** (0x4080d0 → 0x41e8d0):
`trunc((grade·1000/(continues+1) + odometer at the last stop [+ remaining·1000 if cleared]) ×
0.01)`, capped at 9999. **confirmed** (part names are ours).

## 10. Timetable fields 4–8 (routes.md §9)

| Field | Meaning | Conf. |
|---|---|---|
| 4 | `0x857084`, read by the train task (0x4227ac: `1 − f4·k` factor in the brake/adhesion maths) — physics | likely |
| 5 | Home-signal distance (block 2), base of the station-approach HUD (`f5+150`, `f5+50`, `f5−10`) | confirmed |
| 6 | Station-approach distance (approach state, re-power / EB-in-approach detection) | confirmed |
| 7 | 0–3: preset for the station-result camera (0x419730 picks one of four view setups) | likely |
| 8 | Forced-stop overrun distance (−999 none; −15/−40 at terminals) | confirmed |

## 11. Trace schema additions (per tick)

| VA | Type | Meaning |
|---|---|---|
| 0x856d78 | i32 | station index (segment start) |
| 0x8570b0 | i32 | clock, ticks |
| 0x8571e8 | i32 | scheduled arrival |
| 0x856fb0 | i32 | remaining time, s |
| 0x856d7c | i32 | pending time change, s |
| HUD task +0x34 | i32 | transfer phase counter (task allocated in 0x40f7e0; pointer not kept in a global — hook 0x40f7e0's `Task_Alloc` result) |
| 0x856fac | i16 | clock skew |
| 0x85708c | f32 | distance to stop, m |
| 0x85722c | i32 | rounded distance, m |
| 0x857004 | f32 | odometer, m |
| 0x857234 | f32 | speed, m/s |
| 0x857088 / 0x8571fc | ptr | course / limit event cursor |
| 0x857080, 0x85720c | i32 | block index, initial index |
| 0x856dc0 | 40 × {i32, i32, i32} | blocks (log on change) |
| 0x8570a6 | i16 | aspect in force |
| 0x857090 | f32 | signal limit, m/s |
| 0x856fa0 | i32 | signal enforcement state |
| 0x856fcc | i32 | signal enforcement odometer |
| 0x856d70 | i32 | restricted blocks left |
| 0x856fb4 | i32 | repeater offset |
| 0x8570c2 | i16 | signal display variant |
| 0x8570bc | f32 | speed limit, m/s |
| 0x856fc4 | i32 | limit state |
| 0x856da0 | i32 | limit enforcement odometer |
| 0x857206 | i16 | ATS state |
| 0x857240 | i32 | ATS ticks |
| 0x8570d4 | i32 | ATS chime start |
| 0x857208, 0x856d6a | i16, i16 | horn state, horn deadline |
| 0x856fba | i16 | no-power section state |
| 0x8570c4 | i32 | early accumulator, ticks |
| 0x856d74, 0x8570ae | i16, i16 | target stops, final station |
| 0x856d9a, 0x856d8c | i16, f32 | approach state, latched km/h |
| 0x856d64, 0x8571e0, 0x856da8 | i32 | EB ticks, EB count, EB ticks in approach |
| 0x8571f6, 0x857238 | i16 | re-power, excuse |
| 0x8571ec | i32 | stopped ticks |
| 0x8570b4 | i16 | forced stop |
| 0x856d80, 0x856d76, 0x85721e, 0x856dac | i16, i16, i16, f32 | obstacle state, armed, used, target |
| 0x84c424 | f32 | derail speed, km/h |
| 0x84c3a0, 0x857230, 0x84c2e0 | i16, i16, i32 | weather on, level, wet rails |
| 0x856fc0, 0x8570c0, 0x857094 | i32, i16, i32 | last lateness, stop streak, pass streak |
| 0x856fa4, 0x856fc8 | f32, i16 | stop error sum, stop count |
| 0x857020[12], 0x857050[12] | i32 | statistics |
| 0x8570b8 | i16 | continues |
| 0x856fe1[] | u8 | station marks |
| 0x855ff8 | u32 | phase flags (0x200 driving, 0x800 pre-departure, 2 stage active) |

Added for the rules replay (`ddg-tools trace-replay <csv> --rules`, `crates/ddg-tools/src/trace/rules.rs`):
`buttons_held` 0x48c428 u32, `buttons_pressed` 0x48c42c u32, `track_class_ahead` 0x857202 i16,
`stat0`..`stat11` 0x857050.. i32, and `units_to_stop` 0x8500c8 f32 in rules.schema. The HUD
transfer counter is not loggable (no global, not the physics task); the replay infers its phase
from the first ±1 s transfer in each stretch. Blocks are not logged; the replay rebuilds them at
each segment start and checks `block_index` / `aspect`.

Hook points for event-level traces: 0x415080 (every time message: kind, seconds), 0x4122c0
(pending change), 0x411500 (signal shown), 0x40dd60 (stats).

## 12. Engine notes and approximations

* `Rules::step` runs the rule functions of §1 (inputs as the loop sees them before this tick's
  physics); then the caller steps the physics and calls `Rules::end_tick(readout)` for the train
  task's rule parts, the departure task and the HUD transfer (`Rules::tick` = both, without a
  readout). The pre-departure wait is ended by the departure task, not by the caller.
* The blocking sequences are modelled as holds of the documented length. The arrival is split into
  `Holding`(80, forced stop only) → `Draining` (until pending = 0) → evaluation → `Holding`(31/101)
  → `Arrived`. The exact number of `Task_Tick`s around 0x407ff0 and the obstacle sequence are
  **likely/guess**.
* `finish_station` counts the remaining pending time in before the result deductions (the HUD
  keeps draining it during the result screen).
* The signal HUD de-duplication is keyed on `(aspect, display mode, variant)`; the original keys
  on the sprite id (adds the per-stage table 0x466c08) — **likely** equivalent.
* Not modelled: hint messages (0x40c4e0 tail, 0x422990), the station-approach HUD state
  `0x856d68`, scripted weather start at a segment change (needs the global frame counter % 4),
  special stage endings (stages 1, 2, 10, 17–19 at fixed stations), the Komachi section display.
* x87 extended-precision intermediates are computed in f64 then stored to f32 where the original
  stores; rare double-rounding differences are possible.

## 13. Open questions

* Oracle coverage so far (§14): stations reached by stopping, the arrival evaluation and result
  deductions, the approach state, EB penalty, weather, obstacle, notch slam and 95 % checks, the
  clock skew and continues past a stop are not yet exercised. The hold lengths and the EB `% 4`
  rule still need a trace.
* The horn deadline `0x856d6a` is an i16: with an odometer above 32 767 m a whistle sign would
  expire immediately. No whistle sign lies that far in the shipped data (only the Akita bonus
  section beyond 32 km), so it is latent.
* Meaning of kinds 18/19 (`0x84c400`) and of display variant `0x8570c2`.
* Difficulty names for columns 0–3.

## 14. Oracle replay (hilly-drive.csv)

`ddg-tools trace-replay <csv> --rules` (`crates/ddg-tools/src/trace/rules.rs`) drives the engine
from the logged inputs (row k: notches, buttons, tunnel class; row k−1: speed, odometer,
`units_to_stop × 10`) from the first row of each pre-departure wait, and compares 65 rule globals
per tick (integers exactly, floats bit for bit). The HUD transfer phase is inferred from the first
visible transfer. A stretch ends when the engine arrives or the game ends.

Result on `hilly-drive.csv` (stage 0, two waits + driving, ending in time over; the continue):
**2 stretches, 6911 ticks, 0 mismatches in all 65 fields.** Fixes the trace required:

| Fix | Evidence | Conf. |
|---|---|---|
| ATC/derail default 999 km/h, not 1000 (0x4479c000) | `derail_kmh` = 999 on every row | confirmed |
| Pending time is not discarded on the game-over tick | pending −1 on row 7540, drained by the HUD at 7542 | confirmed |
| Departure task: wait length, false start, stat 2 | 381 waiting rows; stat 2 at the 380th; ATS 2 at row 8162 | confirmed |
| Jolt statistic (stat 3 += 1) from the physics readout | stat 3 matches on all rows | confirmed |
| `0x857094` (pass streak) is an i16; `0x857096` is a separate turnout flag | logged i32 read 65536 when passing a turnout | confirmed |

Fields that never changed in this trace (so only their initial value is checked): approach state
and speed, stop count / error sum / streak, EB-in-approach, re-power, forced stop, obstacle,
weather, clock skew, display variant, horn state (no whistle sign reached), ATS confirmation
(the chime start never left −1), stats 0, 4, 6, 9, 10, 11. Exercised: lateness drain, pass
stations and the on-time pass streak, signal blocks and aspects (3 values), restriction counter,
repeaters, speed limits with a violation, signal overspeed → ATS bell → emergency brake →
red-signal recovery, no-power section, early-running accumulator, false start, departure brake
release, jolt, game over.

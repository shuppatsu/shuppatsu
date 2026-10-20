# Game rules: signals, speed limits, ATS, remaining time, stop evaluation, grades

Scope: everything in the driving loop that is not train physics. Source: Ghidra decomp in
`re/decomp/dengo3k/` and `objdump -d` of `BIN/dengo3k.exe` (3000-bandai), used where the decomp
lost float expressions. Implementation: `crates/shuppatsu-engine/src/rules.rs`. Tests:
unit tests in that file and `crates/shuppatsu-engine/tests/rules_data.rs` (needs `SHUPPATSU_DATA_DIR`).

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
| 0x459910 | overrun factor, s per m | Station-result overrun deduction (`0x856fb8`, §7.3) | confirmed |
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
(0x411500, skipped if the newest live display task already shows the same sprite, hud.md §3.9) and the signal speed limit
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
signal limit cleared, notches forced to P0/B9, **60 ticks** standstill (after a scripted "train
ahead" signal and no ATS: 30 ticks, then 60, or 200/300/360 while a train passes; traffic.md §6.1), the HUD shows a green, then 10 ticks. If
no signal is in sight ahead, re-powering during the station approach is excused (`0x857238 = 1`).
**confirmed** (structure), hold lengths **likely**.

The green is `Signal_Show(0x100, mode, ticks, distance)` at 0x40a540, computed after the hold
(0x40a45e–0x40a53b, **confirmed** from the code):

* `0x856fb4 == 0` (a repeater was just shown): mode 3, ticks 0, distance
  `0x85722c − block[0].sight_pos + 1`.
* Otherwise blocks `i = 0x85720c … 1` (skipping 2) are searched for one whose sighting point is
  ahead, `0 < sight_pos[i] − 0x85722c < block[k+1].sight`. If block `i` has no signal (−999) and
  the train is past the home signal (`0x85708c < block[2].sight_pos`), `0x857080 = 0` and mode 2.
  Otherwise the mode is 2 at block 0, 1 at blocks 1–2, and 0 beyond.
  * Found: ticks 0, distance `block[i].sight − (sight_pos[i] − 0x85722c)`, i.e. until that
    signal.
  * Not found: ticks 0x3c, distance 0, and the re-power excuse above.


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
  distance drops below field 6 (and the speed in km/h is latched in `0x856d8c`, which is **never reset**: 0x40cf40 clears 0x856d88, not 0x856d8c; confirmed, stops-drive.csv carries 78.28 km/h into the next segment), 2 next tick,
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
  2. Wait until the pending time is 0 (0x407ff0: loop while pending ≠ 0, then one more
     `Task_Tick`).
  3. Stop error sum `0x856fa4 += |dist|`, stop count `0x856fc8 += 1`, ATS reset; then one
     `Task_Tick`; then `late = clock/30 − sched/30` (stat 1 += |late|), early accumulator. So
     the lateness lands one tick after the stop count (**confirmed**, stops-drive.csv rows
     4138/4139).
  4. **Clean stop** = `d == 0` (|dist| < 0.5 m) and no EB in the approach and no EB held and no
     un-excused re-power. On time (`|late| < 3`): **+0x45aa90 + streak** (`0x8570c0`), mark 2;
     otherwise **+0x45ab30**, mark 1. Not clean: streak 0. 100 / 30 ticks hold.
  **confirmed**

### 7.3 Station result (0x413d90 + task 0x413f30)

Deducted directly from `0x856fb0` (not through the pending counter) — **confirmed**:

* **Overrun**: `d < −tol` → `min(−d, 999) × f` seconds, with `f` = `0x856fb8`, set by 0x40d040
  from the `[stage][difficulty]` i16 table **0x459910** (values 1–3, rising with the
  difficulty, e.g. stage 0: 1 1 2 2, stage 1: 2 2 2 3, stage 13: 1 1 1 2). **confirmed** (code: 0x413d90 stores
  the metres in task+0x50; task 0x413f30 at 0x41436e shows the factor sprite
  `0x4654ec[f]` (the "m×1/2/3秒" banner) and at 0x4143fa multiplies task+0x50 by `f` before
  the sum at 0x4145a3 is capped at 999; oracle: stops-drive.csv, stage 13 difficulty 1, `f` = 1,
  24 s for 24 m). Read as `RuleTables::overrun_factor`.
* **Re-power** in the approach: 0x45a8b0 s.
* **EB** in the approach: 10 s (stat 4 += 2); not on lesson stages.
* Sum capped at 999 (20 on lesson stages), then `+0x45a9f0` (**10 s**) if `trunc(|dist|·100) == 0`
  (within 1 cm) and the station got a clean-stop mark. Clamped at 0.

So: the **合格範囲** is ±`tol` metres (short side: you must creep up; long side: costs 1–3 s
per metre beyond it, by stage and difficulty), the bonus window is ±0.5 m, the perfect window ±1 cm.

### 7.4 Final station, continue

Reaching the last station clears the stage regardless of the remaining time. A continue
(0x40d930) rewinds to the last **stopping** station, resets the remaining time, the per-continue
stats (`0x857020`), the early accumulator and the clock skew, and `0x8570b8 += 1`. **confirmed**

### 7.5 Stage endings, the stage-1 bonus section and the coupling minigame — confirmed (code)

Which leg is the last is `0x8570ae`. 0x40d340 (after a stop) and 0x40d620 (after a pass) set it
from the timetable (`row[d78+2].time == −1`) and then **override** it for fixed legs: stage 1 at
`d78 == 7`, stage 2 at `0x17`, stages 17–19 at `0xf`, and (0x40d340 only) stage 10 at 7. So
stage 1 normally ends at station 8, stage 2 at 24 and stage 10 at 8 even though their timetables
go on (stage 1's rows 9–23 are never driven). 0x40d040 sets the flag to 0; 0x40d930 (continue)
leaves it alone (the port recomputes it with the overrides, equivalent for every rewind target).

The stop handler 0x40a0a0, after the marks, decides per stage. "Perfect" means
`0x85722c == 0`, `0x856da8 == 0` (after the stage-kind-1 reset), `0x856d64 == 0`,
`0x8571f6 == 0x857238` and no continue (`0x8570b8 == 0`):

| Stage, `d78` | Perfect | Otherwise |
|---|---|---|
| 1, 7 | `0x8570ae = 0`, `0x855ff8 \|= 0x20000000` (bonus section), no caption | caption 0x20f + 120 ticks, the stage clears |
| 2, 0x17 | coupling minigame; success → `Vehicle_Set(0x1000)` + train type 0xe when bit 0x8000000 is set, else `Vehicle_Set(0x1001)` + type 0xc, `0x8570ae = 0`, 30 ticks; failure → `\|= 0x40000000`, final | `0x8570ae = 1`, bits 0x18000000 cleared, caption + 120 ticks |
| 17–19, 0xf | coupling minigame, then always `\|= 0x40000000` and final (the result only gives the bonus seconds) | final |
| 10, 7 | — | marks of stations 1–8 (`0x40e350(1, 8)`) ≥ 15 or select stage 20 (`0x48ddf0`) → `0x8570ae = 0` (on to 17), else final |

0x40d340 at stage 1, `d78 == 7`: without bit 0x20000000 it sets `0x8570ae = 1` and
`0x40000000` and **does not** advance `d78`. With it, it jumps to the **bonus section**:
`d78 = 0x18`, odometer `0x857004 = row[24].pos − row[0].pos`, clock `0x8570b0 = row[24].time`
(+0x2b2 dwell after), course cursor `0x857088` and AI-train cursor `0x856d90` skipped to
station ≥ 24 (**not** the limit cursor `0x8571fc`, so a limit record of stations 8–23 blocks
the limit list for the rest of the run), `Vehicle_Set(0x1001)` (E3 cab, ATC speedometer,
hud.md §3.6), train type `0x84c3fc = 0xc` (280 km/h; the stage's own type 16 tops out at
135), weather off (`0x857230 = −1`, `0x84c3a0 = 0`). The common path then makes 24 → 25 the
final leg (9.7 km, ATC limits from course event 7). Bit 0x20000000 is cleared on every
0x40d340. The train task reads `0x84c3fc` every tick (0x422c20), so the new type acts at once.

**The waiting set: course event 12 (0x40acd0 case 0xc) and 0x406c50 — confirmed (code, port screenshots)**

Course event 12 sits on the coupling leg (stage 17 station 15: it fired at 200 m to the stop
in the port's autopilot run). It sums the marks so far, `0x40e350(0, d78)` = Σ `0x856fe0[0..=d78]`
(i8 each; 0x856fe0[0] is never written, so these are the marks of stations 1..=d78), and:

| Marks | Flag | Rows (0x45735c[row·3]) | Head past the stop | Set |
|---|---|---|---|---|
| < 13 | `0x855ff8 \|= 0x10000000` | 0x14 / 0x15 (20 / 21) | 294.25 m (f64 0x4594e0) | 10 cars, spacing 2.55 units (25.5 m), kind 2 |
| ≥ 13 | `\|= 0x8000000` | 0x11 / 0x12 (17 / 18) | 239.5 m (f64 0x4594f0) | 8 cars, spacing 2.5 units (25 m), kind 2 |

The call is `0x406c50(0x8502a4 − ftol((0x85708c + head) × −0.8), rowA, rowB)` (0x40b15f /
0x40b19f; f64 −0.8 at 0x4594e8 = −1/1.25 m per point): the head car's front bogie stands
`head` metres past the stop marker on the player's own track, counted in track points from the
cab's engine index. 0x406c50 (0x406c50..0x406e0e) then creates one task per car of row A's
direction-1 list in task list 5 (0x84c1bc) with the ordinary traffic callbacks (update 0x405420,
draw 0x405640): `+0x30 = 1` (forward, facing away from the player), speed, mode, delay and the
accel all 0 (parked), `+0x3c = 9000` (sound gate; no sound anyway at speed 0), `+0x6c` = row A's
record, `+0x70` = row B's record of the same index. Each car is walked back from the previous
one by `|spacing|` with the 0.125-unit point steps and the carried fraction of the spawner's
spaced mode (0x4078c0 = previous point), the rear bogie 12 points behind (0x406d94; the
spawner uses 10). Last, `0x857244 = 0`.

Both sets put the last car's front bogie at ≈ 64.6 m past the stop marker (239.5 − 7 × 25,
294.25 − 9 × 25.5), its origin (mid-bogie) 7.5 m behind that and its rear end, half a car
further, at ≈ 44.5 m: the cab meets it where the minigame's distance reaches 0 (40 m past the
marker, 0x4594b0). In the port the cab eye is 15.1 m from the last car's origin at the moment
of coupling (`tests/coupling_set.rs`).

**Coupler frame `0x857244`.** The draw callback picks the part as `[task + 0x6c + 4·0x857244]
[lod]` (0x40598c), so every car draws row B's record once `0x857244 = 1`. Writers: 0 by
0x406c50, 1 at success outro tick 60 (0x409ddd) and at time-up (0x409ec4); nothing resets it
otherwise. The spawners store the same record in both fields (0x4052bc/0x4052d5, 0x4060ae,
0x406c32), so only the waiting set changes: rows 18 / 21 differ from 17 / 20 in the last car
only (parts 0x2e59.. vs 0x2e58.., 0x2e87.. vs 0x2e86..); that it is the nose with its coupler
connected is a **guess** (the change is too small to see in the port's cab shots).
Readers 0x405e20, 0x4066ea, 0x406792 and 0x4074b3 are the other draw callbacks, same pattern.

**Lifetime.** The set is ordinary list-5 traffic: it is there from the event through the stop,
the minigame and its outro, and goes with `TaskList_KillAll(0x84c1bc)` before the station result
(traffic.md §3) or when a continue closes. It neither moves nor sounds.

**`0x855ff8 & 0x40000`** is set by the minigame on a coupling (0x409d8b). Its other setter,
the game-flow loop (0x419679, when A and the horn are both held, active-low `0x48c428 &
0x100100 == 0`), only feeds the route-select secret (EB + P3, input.md, Stage_Run 0x408177),
and Stage_Run then clears all of `0x855ff8` (0x4081bd, `ebp = 0`) before the stage starts. So in
play it means "coupled". The result replay's creation (0x419730) reads it (`task+0x1cc` = 0
instead of −9999, scene.md §8.1) and clears it (0x419830); at the coupling station it also
clears 0x8000000 / 0x10000000 after using them (0x41995d, 0x4199e8). A stop that is not perfect
at the coupling station clears both set bits (0x40aa1c), so that result shows no waiting train.

Port: `traffic::WaitingSet` / `Traffic::place_waiting_set` / `set_coupler_frame`,
`Rules::waiting_set` (taken by `Drive::traffic_events` with the cab's engine index of the tick
before the camera moves), `Rules::result_coupling_flags`. The game draws the set with the other
traffic (`traffic::add_cars`; stage 17's TR sets hold the parts, shots in `out/explain/coupling/`), so with
`far_traffic` it is visible from the placement on, with `--original-traffic` from 150 m.

|= 0x10000000`, else `|= 0x8000000`; it also places the
set's model (0x406c50, presentation).

`0x855ff8 & 0x40000000` chooses the ending (menus.md §12): 0x404110 when clear, 0x404920 when
set. Besides the cases above it is set when the stage clears with a continue used, out of time,
after running into the forced-stop point (`0x8570b4`), and on stage 2 when it clears before 25
(`d78 < 0x19`). Registry option bits 0x08 / 0x10 force it clear / set (Stage_Run).

**Coupling minigame** `Coupling_Run` 0x4093c0 (blocking, one `Task_Tick` per step):

* Setup: distance panel off (`0x856d0c = 0`), timer 0x709 ticks (0x417040), caption tbl 0x2d8
  ボーナスゲーム (slot 0x33, 70 ticks); 10 ticks; BGM 0x19a; sprites slots 0x3f–0x4b at the
  per-resolution table 0x458fd0 (8 bytes × 4 resolutions per slot, palette 4, layer 3; slots
  0x45–0x4a = tbl 0x17c, 0x17d, 0x17e, 0x181, 0x182, 0x183, slot 0x40 = digit 0) and
  `0x84c0f8 = 1` (the black band under the panel, no speedometer: hud.md §3.16); 80 ticks;
  caption 0x2d9 時間内に衝撃なく列車を連結せよ (80 ticks); `0x857238 = 1`; 180 ticks with
  sounds 0x11, 0x34, 0x3f, 0x40, 0x36, 0x3f at 15, 30, 60, 70, 130, 175. Then `0x200` is set:
  the player drives.
* Each driving tick: `0x85708c = 0x8500c8 × 10`, 0x409fc0 (forced stop on the previous
  `0x85722c`, stop record), `+= 40.0` (f64 0x4594b0; the coupler is 40 m past the stop mark),
  `0x85722c = trunc`, clamp at 0. Horn while stopped before moving: +3 points; stopping
  (`|v| < 0.0001`) between 4.5 and 6.5 m: sound 0x3a, +5, and a horn after that: +2; moving
  forfeits the pending horn points. Calls 0x38, 0x11, 0x39 at 8, 7, 6.5 m; 0x3b, 0x3c, 0x3d at
  3, 2, 1 m (passing 4.3 m skips the mark). Warning 0x37 / 0x3f every 90 ticks while ≥ 9 m or
  waiting at the mark. HUD: slots 0x3f/0x40 speed km/h (tbl 0x172 + digit, tens hidden below
  10), slots 0x41–0x43 + 0x171 the distance (`dd.d` from 10 m, `d.dd` below, from 17 m, tbl
  0x167 + digit), slot 0x47 = 0x17e at ≥ 10 m else blinking 0x180 (`(t/10)` odd), slot 0x49
  x = base − 272 + min(272, trunc(16 · dist)).
* At 0 m: v < 0.5 km/h (f64 0.1389 at 0x459450) → too weak: 0x180 + 連結失敗 0x2dd, sound
  0x2b, hint 0xf (90 ticks), 100 ticks, returns −1. v ≤ 2.5 km/h (0x459448) → coupled:
  `points += 10 − trunc((v − 0.1389) × 18.18)` (≥ 1 in all), 0x17f + 連結成功 0x2dc, sound
  0x2a, jolt 0x40e370(−0.0005), `0x855ff8 |= 0x40000`, then 200 ticks (sounds 0x13, 0x10, 0xe,
  0x3e at 40, 58, 65, 95; coupler frame `0x857244 = 1` at 60; at 120 the time message
  `0x415080(0x103, points)` = ボーナス + points seconds), returns 1. Faster → too fast: 連結失敗,
  sounds 0x2b, 0x2f, jolt −0.004, 20 ticks, returns 0. Timer at 0 → `0x857244 = 1`, 20 ticks,
  returns 0. A final `Task_Tick` follows; `0x85708c` / `0x85722c` are restored.
* Maximum bonus 3 + 5 + 2 + 10 = 20 s.

Port: `shuppatsu_engine::coupling` (state machine), `Rules::stage_ending_at_stop`,
`Rules::coupling_step`, `Rules::next_segment` / `enter_bonus_section`, `Phase::Coupling`
(`Drive` keeps the physics driving while `Coupling::driving`), `Rules::vehicle_code` /
`train_type` applied by `Drive::sync_train_type`, `Rules::full_ending`. The 0x8000000 /
0x10000000 bits are `Rules::coupling_partner` (2 / 1). The autopilot soak drives stage 1
through the bonus section and couples on stages 2 and 17–19 (`tests/autopilot.rs`).
Not traced in the original yet. A Wine capture reached and won the stage-17 minigame (a
port-autopilot input script replayed by a hook that counts ticks across legs; its stop errors
at stations 6, 10, 13 and 16 matched the port to 0.1 mm; hud.md §3.16).
Its marks reached 13 by station 15 (on-time passes at 1, 2, 3, 8, 9, 12, 14, 15 from the
trace's `pass_streak`, stops 1 + 2 + 2), so it placed the 8-car rows 17/18; the port's replay of
the same script applied the inputs one tick earlier against the clock, missed four on-time
pass windows (9 marks) and placed rows 20/21. With the inputs one tick later the port has the
same 13 marks and the same waiting set (`out/explain/coupling_panel/cap17_port_inputs_aligned.txt`,
`compare_v2_original_vs_port_aligned.png`).

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
| 12 | coupling set | picks the waiting set by the marks so far and parks it on the track ahead (§7.5) | confirmed |
| 14 / 15 / 16 | repeater, display variant | §4.3 | likely |
| 18 / 19 | `0x84c400 = 1 / 0` | flange-squeal / departure-sound flag, read by the train sounds (audio-text.md §10) | likely |
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
  * **Per leg** nothing changes: the task is new at every segment setup, the counter starts at
    0 and the offset is 0 at every later station, also on stages 17–19. In wait ticks (0 = the
    setup tick, whose update runs with counter 0), power used in wait tick *j* is a false start
    for **33 ≤ *j* ≤ offset + 199**; the lamp `0x829d16` lights in the update of tick offset +
    200 and the wait ends after tick offset + 200 (201 / 381 waiting rows). The hold at 199 is
    the literal 199, so a false start at counter ≥ 200 (offset 180) does not lengthen the wait.
    **confirmed** (oracle, `from wait` probes with a one-tick power blip on each leg, logging the
    task's counter, freeze and brake count and the lamp): *j* = 199 false start and 200 not, both
    on every leg of stage 13 (legs 1–4), stage 3 (legs 1–4) and stage 17 (legs 2–4, offset 0);
    stage 17 leg 1 (offset 180): 379 false start (wait still 381 rows), 380 not; lower bound: 32
    not (stage 13 legs 1 and 3, stage 3 leg 5), 33 false start (stage 13 legs 2 and 4, stage 3
    leg 5). A false start at offset 0 adds 69 waiting rows (270). Traces: depart13-*.csv,
    depart17g/h.csv, depart3e/f.csv (§14; the stage-3 leg-5 probes are only in the raw captures).
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

Added for the rules replay (`shuppatsu-tools trace-replay <csv> --rules`, `crates/shuppatsu-tools/src/trace/rules.rs`):
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
* Outside the loops `Rules::segment_setup` (0x40b970 + the priming 0x40dad0 of a segment's setup
  tick) and `Rules::idle_tick` (train task + HUD only, for result/continue screens and loading
  ticks) complete the tick model; `shuppatsu_engine::drive::Drive` sequences them as 0x4080d0 does
  (physics.md §8a). The signal blocks are built in the setup, not by `finish_station` /
  `continue_run` (lazily on the first `step` for callers without a setup).
* Red-signal / ATS recovery (`AfterHold::Resume`) keeps `0x200` set (`Rules::train_driving`)
  and forces the speed to 0 only outside the last 11 ticks; the obstacle hold
  (`AfterHold::ObstacleCleared`) is still a guess. `continue_run` moves on to the target
  station first when the time ran out inside the approach (0x4080d0, `0x856d9a ≥ 1`;
  yamate-drive.csv). A stop at the final station holds 120 ticks longer (0x40aaa5; not traced).
  All **confirmed** by the whole-run replay unless noted.
* The blocking sequences are modelled as holds of the documented length. The arrival is split into
  `Holding`(80, forced stop only) → `Draining` (until pending = 0; the tick that finds it 0 is
  0x407ff0's extra `Task_Tick`) → `Evaluating(1)` (stop error, count) → `Evaluating(2)`
  (lateness, bonus) → `Holding`(100/30) → `Arrived`. Evaluation timing is **confirmed** by
  stops-drive.csv (pending was 0 at arrival); the drain with pending ≠ 0, the forced-stop hold and
  the obstacle sequence are **likely/guess**.
* `finish_station` counts the remaining pending time in before the result deductions (the HUD
  keeps draining it during the result screen).
* The signal HUD (`Signal_Show` 0x411500, hud.md §3.9) is modelled by `rules::SignalHud`, which
  both the rules and the HUD use: the "already shown" test compares sprite ids against the newest
  **live** display task, as the original does. Its return value matters in 0x40c010, where a
  no-op skips the `signal_state` reset and the limit set-up. The task update needs the odometer
  after the physics step, so the rules run it at the start of the next tick (`latch_input`), and
  `leave_driving_loop` runs it first. The earlier key `(aspect, mode, variant)`, which never
  expired, made a repeated restrictive aspect a no-op even after its sprite had gone (the original
  re-arms the limit), and it told the 3-lamp aspects 1–3 apart, which share a sprite.
  **confirmed** (code); no trace logs the slot or `0x856d30` yet.
* `Signal_Show` calls raised (`RuleEvent::SignalShown` with ticks, distance, start odometer,
  forced bit and variant): sighting and repeater (0x40c010), departure red and aspect (0x413370),
  red-signal / ATS recovery green after the 60 standstill ticks (0x40a540), obstacle recovery
  green `(0, 0, 0, 5)` (0x40a0a0, 31 hold ticks before the end; the hold length is a guess), and
  hides (`SignalHidden`) for NaviAnim(0) at the obstacle start (0x40c4e0 → 0x410cb0) and at the
  stage-loop exit (0x4158b0 / 0x4085e0, via `leave_driving_loop`). The attract demo drive task
  0x40e860 (list 4, stage 5) has its own two calls; `Rules::demo_tick` makes them (menus.md §2.4).
* Green-signal hint: course event 20 sets `0x857224` (`Rules::green_hint`, reset per segment
  in 0x40cf40). The next `Signal_Show` with aspect 0, whether a no-op or not, raises
  `HintShown { id, ticks: 0, distance_m: 220 }` and clears it. This includes the departure call
  and both recoveries.
* Not modelled: hint messages other than the green-signal hint (0x40c4e0 tail, 0x422990), the station-approach HUD state
  `0x856d68`, the presentation of the coupling
  set (0x406c50). The stage endings, the bonus section and the coupling minigame are in §7.5.
* x87 extended-precision intermediates are computed in f64 then stored to f32 where the original
  stores; rare double-rounding differences are possible.

## 13. Open questions

* Oracle coverage so far: see §14. Still unexercised: clean-stop and on-time bonuses, the stop
  streak, the exact-stop bonus, the EB penalty (`% 4` rule), the **horn bonus award and the
  missed-horn penalty** (stops-drive.csv has one bonus zone, horn state 2 at odometer 850–879 m,
  with no press inside it, and no required whistle sign), the **ATS confirmation** (no ATS
  acknowledged in time in either trace), forced stop, weather, obstacle, the 95 % check, clock
  skew, the drain with pending ≠ 0 at arrival, and the hold lengths.
* The horn deadline `0x856d6a` is an i16: with an odometer above 32 767 m a whistle sign would
  expire immediately. No whistle sign lies that far in the shipped data (only the Akita bonus
  section beyond 32 km), so it is latent.
* **Weather start / end runs an extra `Task_Tick`** that the engine does not model:
  0x40f320 (the palette fade, called by course event 21, by the random start in 0x40d620 and by
  the weather end in 0x40c4e0) calls `Task_Tick` itself when the stage's palette index
  `0x463c18[stage]` is < 7 (code 0x40f320 tail). The oracle shows that tick as an extra row in the
  middle of the loop body: on depart3e.csv row 16447 `0x84c3a0` is already 5 (it holds the
  weather kind + 1, set by 0x424740, not a flag) and the level 1, while the course cursor and the
  stage clock only move on the next row. Every later row is one tick off in the engine. The
  stage-3 traces are therefore cut before the weather starts. **confirmed** (code + oracle); not
  fixed.
* Meaning of display variant `0x8570c2`.
* Difficulty names for columns 0–3.

## 14. Oracle replay (hilly-drive.csv)

`shuppatsu-tools trace-replay <csv> --rules` (`crates/shuppatsu-tools/src/trace/rules.rs`) drives the engine
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

### stops-drive.csv

Stage 13, difficulty 1, three stretches, 9 277 ticks: stop at station 1 overrunning by 24.6 m (result screen
deducts it, time runs out, continue), stop at station 2 at 0.17 m (result screen deducts
20 s), then time over. Both approaches used the EB; the second also re-applied power. Since this trace the replay also checks the **station
result**: after a stretch ends at the station it runs `finish_station` and compares remaining
time, stop count / error sum / streak, continues and all statistics with the first row after the
result screen (`0x855ff8 & 0x40` set, then clear).

Before the fixes: `approach_kmh` 4 770, `last_lateness` 2, `stat1` 2 mismatching values.
Result: **0 mismatches in all 65 per-tick fields and in the 17 result fields (2 results)**.
`hilly-drive.csv` still replays with 0 mismatches.

| Fix | Evidence | Conf. |
|---|---|---|
| `0x856d8c` (approach speed) is never reset; the replay takes it from the setup row | 78.28 km/h carried into the next segment from row 4595 on | confirmed |
| Stop evaluation in two parts one `Task_Tick` apart (count/error, then lateness/bonus) | stop count at row 4138, lateness and stat 1 at row 4139 | confirmed |

Now exercised: arrival evaluation (two stops, neither clean), stop error sum and count, the
result-screen overrun deduction (24 s) and the EB-in-approach deduction and stat 4 += 2, re-power
in the approach, approach states 2–4, EB-in-approach ticks, notch slam (stat 11) and jolt
(stat 3), the horn-bonus zone timing (entered and expired, no press). Not exercised: see §13.

### depart*.csv (departures on later legs)

Issue mark 4 suggested that the original allowed an earlier departure on later legs: the mark's
notches, replayed in the original leg by leg, gave a false start in the port on leg 4 (Tachibana)
and none in the original. The cause was the measurement: the hook's input scripts then counted
each leg from its first **driving** tick and could not set the levers during the
pre-departure wait, so every leg of the replay started about 200 ticks later than in the port
(stop errors do not depend on a constant shift, which is why legs 1–3 matched). With `from wait`
scripts (crates/shuppatsu-hook `script.rs`) the original false-starts on leg 4 exactly as the
port does: power used in wait tick 197 (counter 196), ATS 1 and stat 8 += 2 in the next update,
69 more waiting rows. **confirmed**

New traces (`scripts/capture-original.sh`, `oracle/full.schema` with the departure task and the
lamp, difficulty 0 unless noted). They all have `NotchTiming::NextRow` (physics.md §8). Each run
is cut where the stage leaves the run (the attract demo after the game over); the stage-3 runs
are cut before the weather starts (§13).

| trace | stage | legs | what | physics / rules / drive mismatches |
|---|---|---|---|---|
| depart13-mark.csv | 13 | 4 | the mark's notches; leg 4 false start | 0 / 0 / 0 |
| depart13-mark-d1.csv | 13 | 4 | every notch of the port's mark run, difficulty 1 (the mark's own); odometer equal to the port's run on every tick of legs 1–4 up to the mark | 0 / 0 / 0 |
| depart13-b / -c.csv | 13 | 4 | blips at *j* = 199 / 200, alternating | 0 / 0 / 0 |
| depart13-d.csv | 13 | 4 | blips at *j* = 32 / 33, alternating | 0 / 0 / 0 |
| depart17g / h.csv | 17 | 4 | leg 1 (offset 180) *j* = 380 / 379, then 200 / 199 alternating | 0 / 0 / 0 |
| depart3e / f.csv | 3 | 4 | *j* = 199 / 200 alternating (cut at the weather start in leg 4) | 0 / 0 / 0 |

The drive and rules replays also compare the departure task's counter, freeze and brake count
(`dep_counter`, `dep_freeze`, `dep_released`) on every waiting row. The rules replay carries
the HUD transfer phase from the previous stretch when a stretch shows no transfer of its own
(depart13-mark.csv leg 3: a −1 s penalty drained in the same tick). The stage-17 runs and the
stage-3 runs were driven by scripts generated in the port and checked there first.

# Route data: `.COM` files and the exe stage tables

Scope: the 15 `DATA\*.COM` route files ("RAIL DATA VER7.0") and the per-stage tables in
`dengo3k.exe` (3000-bandai) that tie them together. Parsers: `ddg-formats::route` and
`ddg-formats::stages`. Data tests: `crates/ddg-formats/tests/data.rs`
(`every_route_parses_completely`, `stage_tables_read_and_agree_with_routes`).

Confidence levels: **confirmed** (code and data agree, or checked across all files),
**likely**, **guess**. Addresses are VAs in `dengo3k.exe`.

## 1. Summary

- A `.COM` file holds the **geometry** of one route segment: the driven line as a dense polyline
  (one point per 1.25 m), side tracks, per-position draw lists, scenery placements, AI-train spawn
  points and the station stop points.
- **Speed limits, signals, the timetable and station names are not in the `.COM` files.** They are
  per-stage tables in the exe, keyed by *(station, distance to that station's stop point in
  metres)*. See §8–9.
- Distance along the line is measured in **points**: the train position is a pointer to a point
  plus a fraction in units of 0.125 per point; 1 unit = 10 m, so 1 point = 1.25 m. §3.

## 2. File layout

Header (0x30 bytes): 8 × u32 file-relative offsets, then the 16-byte ASCII tag
`RAIL DATA VER7.0` at 0x20. All pointers inside the sections are also file-relative; the loader
(0x435320) reads the file raw into a static 0x1c3f40-byte buffer and `com_relocate` (0x435380)
adds the buffer base to each one. **confirmed**

| slot | global (0x41cb00) | section | record | terminator |
|---|---|---|---|---|
| h0 | — | relocated, never read; equals h1 in all 15 files | — | — |
| h1 | 0x85010c | traffic spawn points (§7) | 8 B | u16 0xffff (4-byte terminator) |
| h2 | 0x8500b4 | scenery placements (§5) | 20 B | u32 0xffffffff |
| h3 | 0x850210 | segment table (§4) | 24 B | i32 −1 (4-byte terminator) |
| h4 | 0x8501ec | draw-bucket pointer table (§6) | u32 | count implied (§6) |
| h5 | 0x850218 | station stop points (§3.3) | 8 B | ptr 0 (8-byte terminator) |
| h6 | 0x8501f8 | activation index, u16 per main-line point (§5) | 2 B | count = main-line points |
| h7 | 0x850144 | activation lists (§5) | i16 lists, 0-terminated | — |

Physical order in every file (**confirmed**, all 15 files tile exactly, only 2-byte zero pads
after h6/h7 when their length is not a multiple of 4):

```
header | segment table | traffic spawns | main-line slots | h6 | h7 | h4 table | bucket records
       | placements | side-track slots | stops
```

The parser records each section's byte range and the data test checks that nothing but ≤3-byte
zero padding is left unparsed in any file.

## 3. Track points, units and distance

### 3.1 Point (12 bytes) — confirmed layout

| off | type | meaning | conf. |
|---|---|---|---|
| 0 | f32 | x (world units) | confirmed (0x41d160 interpolates x from +0 and +12) |
| 4 | f32 | z (world units) | confirmed (same; `atan2(dz, dx)` gives the heading) |
| 8 | i16 | height in 1/4096 units (double 1/4096 at 0x458c38); in marker slots: segment index | confirmed |
| 10 | u16 | flags (§3.4) | confirmed (bits read at 0x407c10, 0x41cfe0, 0x41d890, 0x4078c0) |

The eye height is `height/4096 + base`, where `base` is a per-COM float at `0x469dc4 + com*8`
(0.26–0.34 units, i.e. 2.6–3.4 m; 10.0 for COM 0) set in 0x41cb00. **likely**

### 3.2 Units — confirmed

- The camera/train task keeps a pointer to the current point and a fraction `t`. `t` advances by the
  per-tick speed; each time `t > 0.125` (float at 0x458b80) the pointer moves one point and
  `t -= 0.125` (0x41d160). Interpolation factor between points is `t × 8` (0x46a7c8). So one
  point = 0.125 *track units*.
- Distance to the stop in metres is `remaining_units × 10.0` (double 10.0 at 0x458b88, 0x40dad0),
  so **1 track unit = 10 m and 1 point = 1.25 m**.
- The per-tick speed increment is `speed × 0.000925` (0x458af0, 0x4042b0). At 30 ticks/s,
  1 km/h = 1/108 m per tick = 1/1080 units = 0.000926 units. This matches km/h and confirms the
  30 Hz tick and the 10 m unit together.
- Cross-check: station-to-station point counts × 1.25 m against the exe timetable's `to_next_m`
  (§9) agree within 4.25 m over all 272 station pairs of all 20 stages (data test).
- Consecutive main-line points are ~0.123 world units apart (mean per file 0.115–0.13), so **one
  world unit ≈ 10 m** as well. Curves make the chord slightly shorter than the 0.125 nominal.

### 3.3 Stop points (h5) — confirmed

`{u32 ptr to a main-line point, u32 index}` until ptr = 0. `index` always equals
`(ptr − seg0.start)/12`, the 0-based main-line point index. One entry per station of the segment
(2 to 17). 0x41c7b0 builds a station array from it; the start position for station *s* is
`stops[s]` and the remaining distance is `(index[s+1] − index[s]) × 0.125` units.
At segment start (0x41cb00) the camera is placed up to 300 points (37.5 units, double at
0x46a7b8) **behind** the stop point and catches up immediately, which pre-runs the activation
lists (§5). **likely**

### 3.4 Point flags

| bits | meaning | conf. |
|---|---|---|
| 0–4 | **track type** (0–31). Index into the per-area rail part table (`0x486170[area]`, 4 LOD groups × 32) used by 0x41d890/0x41d9c0, and into the per-stage sound table `0x485660[stage]` (§9) by 0x407c10. 31 = draw no rail. 28–30 have a special case in 0x41de20. | confirmed (code) |
| 5–12 | on turnout points: low 8 bits of the index of the side segment that joins here (matches the segment table's links in ~89 % of turnouts; the rest have index > 255 or other links) | likely |
| 13–15 = 0x8000 / 0xa000 (`flags & 0xe000`) | **turnout**: the side segment's end (0x8000, its `linkB` is this point) or start (0xa000, its `linkA` is the previous point) joins here. Passing one sets the points-clatter flags (0x407c10, 0x41cfe0). | confirmed (code) / likely (which end) |
| 12 | set together with 0x8000/0xa000 in ~10 % of turnouts | unknown |
| 0x2000 alone (527 points), 0xc000 (126, main lines only) | carry a bits-5–12 value too; meaning unknown | unknown |
| high nibble 0xf / 0xe on marker slots | segment head / tail marker (§4) | confirmed |
| 0xe000 on the last real point of a side segment, low 3 bits *n* | end of side track; 0x41d7d0 → 0x41d400 draws a buffer-stop-like model *n* times there | likely |

Histogram over all files: high nibble 0 for 604k points; 2: 527, 8: 542, 9: 61, a: 540, b: 54,
c: 126.

## 4. Segment table (h3) — confirmed

24-byte entries until `start == −1`:
`{ptr start, ptr end, ptr linkA, ptr linkB, u32 0, u32 0}`. The last two words are 0 everywhere.

- `start` is the first real point; `end` is one past the last slot. Point count can be 0
  (one empty segment in several files).
- **Markers.** A *head marker* slot (flags 0xf000) may sit just before `start`; a *tail marker*
  (flags 0xe000) may be the last slot before `end`. In both, `z` = 0 and the segment index is stored
  twice: in the height field **and as a raw integer in the `x` field** (so `x` reads as a denormal
  float, e.g. 1e-45 for segment 1; segment 0 gives 0.0). **confirmed** (all 15 files; corrected in
  Phase 2 review: the first parser compared `x` with 0.0 and missed most tail markers, which then
  showed up as track points at the origin). Real track points can also carry 0xe000/0x8000 flags
  (turnouts etc.) and have ordinary coordinates.
- Segment 0 is the driven line. Its region is separate; segments 1.. are stored back to back in
  table order with no gaps (checked by the parser).
- **Links.** `linkA`/`linkB` point at a slot (0 = none). The walkers 0x4078c0 (previous) and
  0x407880 (next) step ±12 bytes; when the neighbour slot is a head (0xf…) / tail (0xe…) slot they
  take the segment index from its height field and jump to that segment's `linkA` / `linkB`
  (staying put if 0). Segment 0 has no links, so the player's walk stops at its ends. AI trains
  (§7) use these walkers. **confirmed** (code)
- The engine's main-line origin is `seg0.start + 12` (the *second* point, 0x41cb00); the end limit
  is `seg0.end − 0xb4` (15 slots before the end), and 0x41d160 clamps the camera between
  `origin + 0x90` and `end limit − 0x60`. **confirmed**

The parser keeps every 12-byte slot of both regions in `Route::slots` (markers included) and
resolves all pointers to slot indices, because draw runs (§6) walk slots linearly across segment
boundaries.

## 5. Scenery: placements (h2) and activation lists (h6/h7)

**Placements** (h2), 20 bytes: `u32 partId, f32 x, f32 y, f32 z, f32 yaw` until partId =
0xffffffff. Entry 0 is all zero (indices are 1-based in use). 1001–8689 per file. Part ids
32–11010 (R/RC parts, see loaders.md §5). **confirmed** (0x41cee0 reads partId at +0 and yaw at
+16 into sin/cos; placement *n* is at `h2 + n*20`).

**Activation** (h6 + h7). h6 has one u16 per main-line point (count = main-line point count
exactly), indexing an i16 list in h7 (0 = no change). When the camera steps **forward** onto
engine index *i* (= main-line point *i+1*), 0x41cfe0 walks list `h6[i]`: value *n* > 0 adds
placement *n* to the live object list (0x41cee0, deduplicated), *n* < 0 removes −*n* (0x41cf90).
Stepping **backward** (0x41d060) uses `h6[i+1]` with the signs swapped. The live list is drawn
by 0x41dd40 with a ±yaw matrix per object. **confirmed** (code), data: every list value is a
valid placement index in all 15 files.

So scenery streaming is precomputed per point: there is no runtime visibility test for placements
beyond the 10-unit radius check in `Scenery_DrawIfNear` (renderer.md §10).

## 6. Draw buckets (h4): track runs near the camera

h4 is an array of pointers, **one per 4 main-line points** (bucket = engine index / 4, 0x41cb00
and 0x41d160 store `h4[idx/4]` in the camera task at +0x44). Its length is not stored: the
relocation loop runs until the address of the first non-null entry, and that gives exactly
`main-line points / 4` (rounded down) in every file. A null entry (413 in total) means an empty
bucket. **confirmed**

Each bucket is a list of 20-byte **track runs**; the sign of `kind` marks the last one:

| off | type | meaning | conf. |
|---|---|---|---|
| 0 | i16 | ±kind; negative = last record of the bucket | confirmed (0x41dc90, relocation) |
| 2 | u16 | unknown; changes smoothly from bucket to bucket | unknown |
| 4 | 4 × u8 | LOD band counts (below) | confirmed (code) |
| 8 | ptr | first slot | confirmed |
| 12 | ptr | end slot (exclusive) | confirmed |
| 16 | f32 | ≈ squared XZ distance from the bucket's camera point to the run's nearest point (e.g. 72 for 8.5 units) | likely |

Kinds (0x41dc90): **1** = rails, drawn by 0x41dbf0 with the per-area rail table by point track
type; **3, 4, 7, 8** = fixed model sets in the exe (0x47c2cc, 0x47c2e0, 0x47c2f4, 0x47c308);
**≥ 9** = per-area table `0x486eb0[area]`, 5 part ids per kind starting at `(kind×5 − 45)`.
Kinds 2, 5, 6 are skipped. Values up to 416 occur.

LOD bands: for kind 1, byte 4 = number of single-point pieces, then byte 5 high/low nibble, byte 6
high/low nibble and byte 7 give piece counts whose pieces span 2, 4, 8, 16, 32 points (0x469cc0),
drawn at 1, ½, ¼, ⅛, 1/16, 1/32 detail scale (0x469d84). For other kinds (0x41d7d0) bytes 4–7
are piece counts spanning 1, 1, 2, 4 points (0x469cd4) and the remainder up to the end pointer is
drawn with the coarsest model. **confirmed** (code)

All runs lie in the **side-track region**: the main line is drawn separately by 0x41db80 with
fixed band counts (22 single points, then 4, 2, 1, 1, 2 pieces; 0x850120..). Kind-1 runs may
cross from one side segment into the next (the drawer walks memory), which is why the parser
works in slots. Farthest run ends are about 18 world units (180 m) from the bucket's point.

Implication for widescreen: track visibility is a **precomputed per-position list**, not a frustum
test. It is distance-based (the lists include track behind and beside the camera), so it should
not depend on aspect ratio, but this has to be checked visually at 21:9 in the viewer phase.

## 7. Traffic spawn points (h1) — likely

8 bytes: `u16 trainType, u16 flags, ptr slot` until trainType = 0xffff. 10–67 per file.
0x404f10 is called with an index into this table from the exe's traffic event lists (§9): it
takes `trainType − 1` as the row of the train-model table at 0x457358 (3 pointers + type, 12 B),
`flags & 1` as the direction (1 = forward via 0x407880, 0 = backward via 0x4078c0), and the
pointer as the spawn point. Types 1–72 occur; flags are 0 or 1 only. **likely** (code, data)

## 8. Exe stage tables — confirmed unless noted

A *stage* is one selectable run. There are 20; every per-stage pointer array below has 20 entries
and ends where the next begins. All are read at runtime from the user's exe by
`stages::StageTables::read`.

| VA | shape | content |
|---|---|---|
| 0x463c18 | u32[20] | area R-index (1 Yamanote, 2 Akita, 3 Tōkaidō/Kōbe, 4 Hokuhoku, 5 Keihin) |
| 0x463d14 | u32[20] | row of 0x463c74 |
| 0x463c74 | rows of 4 × i32 | TR train-part sets (−1 = none) |
| 0x47ca70 | char*[16] | COM index → lower-case filename; index 0 = none |
| 0x485610 | ptr[20] | segment list `{i32 com, i32 firstStation}` until com = −1 |
| 0x485660 | ptr[20] | 32 × i8: track type → sound class (§9) |
| 0x4856b0 | ptr[20×4] | timetable per stage × difficulty (`[stage*4 + difficulty]`, difficulty = 0x829d10) |
| 0x485800 | ptr[20] | course events, 6 × i16 (§9) |
| 0x485850 | ptr[20] | speed-limit events, 6 × i16 (§9) |
| 0x4858a0 | ptr[20] | traffic events, 8 × i16 (§9) |
| 0x485b30 | ptr[20] | char*[] romanised station names, space-padded (consumer not found; data matches) |

Segment lists (decoded): stages 0–2 akita1a@0, akita1b@8, akita1c@24; 3 akita2a; 4 akita2b;
5–6 hokuho1a; 7–9 hokuho2a; 10 keihin1a@0, 1b@4, 1c@8; 11 keihin1c; 12–13 toukai2a@0,
toukai1b@2; 14–15 toukai1a@0, toukai1b@2; 16 yamate1a@0, yamate1b@5; 17–19 akita1b@0,
akita1c@16. `stage_select_segment` (0x408010) picks the last segment with `firstStation ≤
station` and passes `station − firstStation` as the index into that `.COM`'s stop list. The last
station of one segment is the first of the next.

Correction to loaders.md §8: the "entries 20–23 of 0x485610" are the first entries of the next
array (0x485660, sound tables), not extra segment lists.

Station counts come from the timetable (rows until the −1 terminator): e.g. stage 0 has 9
stations (Akita → Ōmagari) even though its segment list continues; stages 1–2 have 26. The
station-name arrays have no terminator and some run on into the next stage's names, so the
parser takes exactly *timetable length* names.

## 9. Timetable, events and sound tables

**Timetable row** (36 B, 9 × i32; row *n* = arrival at station *n*; terminator time = −1):

| field | meaning | conf. |
|---|---|---|
| 0 | scheduled time in 1/30 s ticks since midnight (e.g. 1 620 900 = 15:00:30); the stage clock starts at `row[s].f0 + 0x2b2` | likely |
| 1 | cumulative position, m | confirmed |
| 2 | distance to the next station, m (0 on the last) | confirmed (matches `.COM` stops × 1.25 m) |
| 3 | 1 = stop, 0 = pass: with 0, reaching the stop point moves straight on to the next station (0x40dad0) | likely |
| 4 | read into 0x857084 at station start | unknown |
| 5 | distance threshold, m (0x40c4e0 compares the distance to stop with `f5 + 50` and `f5 − 10`) | guess |
| 6 | distance threshold, m (0x40dad0 latches a speed when the distance drops below `f6 + offset`) | guess |
| 7, 8 | 0–3 and −999/−15/−40 | unknown |

All four difficulty pointers are the same table for every stage in this exe.

**Event lists** are sorted by station, then by decreasing distance, and end with a record whose
station is 99 (distance 9999). The game keeps a cursor per list and fires a record once
`distance-to-stop ≤ record.distance` and `record.station == current station`.

- *Course events* (0x485800, consumed by 0x40acd0): `{station, distance_m, kind, a, b, c}`.
  kind 0 = signal: `a` = aspect for the block ahead, propagated backwards to the earlier blocks
  (aspect + 1 per block, capped at 4; `b == 0` turns 3 into 4), `c` → 0x856d70; 1 = sound `a`;
  2 = announcement `a`; 3 = …; 4/5/6 = speed limit start/change/end with `a` = limit in km/h
  (valid when `a/5 − 4` is 0..21). **likely** (structure) / guess (aspect encoding). The physics
  phase must pin this down.
- *Limit events* (0x485850, consumed by 0x40b4a0): same shape, kinds 4/5/6 only (checked in all
  stages). `a` = limit km/h, `b` = a distance in m passed to the HUD, `c` = extra trigger distance added
  to `distance_m` and `b` (−1 = none, 0 = the per-stage × difficulty default i16 at 0x45db58).
  Drives the limit announcement/HUD and sets the limit speed 0x8570bc. **likely**
- *Traffic events* (0x4858a0, consumed by 0x40b710): `{station, distance_m, mask, spawn, mode,
  p3, p4, model}`. Fires when bit `0x857216` of `mask` is set; `spawn` indexes the `.COM` traffic
  table (§7, checked in range for every event); `model` = −1 uses the spawn's own type.
  **likely**

**Sound tables** (0x485660): 32 × i8 per stage, indexed by point track type. 0x407c10 looks up the
points 16 behind, 4 ahead and 24 ahead of the train; class 1 → sound 0x2c (volume follows speed),
2 → 0x2e, 3 → 0x29, presumably bridges/tunnels. It also computes the gradient from the heights
of the points 4 and 6 ahead. **confirmed** (lookup) / guess (what the sounds are)

## 10. Open questions

- u16 at +2 of a track run; the f32 at +16 is only "likely" a squared distance.
- Bits 5–12 on 0x2000 / 0xc000 points; bit 12 on turnouts.
- Timetable fields 4–8; signal aspect encoding; course event kind 3.
- The per-area rail/model tables (0x486170, 0x486eb0, 0x47c2cc…) map track types and kinds to
  part ids; they belong to the renderer and are not read yet.
- Station-name array consumer (no code xref found).

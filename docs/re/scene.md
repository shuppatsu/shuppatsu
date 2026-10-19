# Scene assembly: track models, cab camera, visibility, background

Scope: what the driving view draws each frame and where. That covers the rail and track-side
models built from the `.COM` draw buckets, the in-cab camera, how scenery and track runs are
picked for drawing (and whether that depends on the 4:3 view), and the sky/ground background.
Addresses are VAs in `dengo3k.exe` (3000-bandai). Code: `crates/shuppatsu-formats/src/scene.rs`.
Data tests: `crates/shuppatsu-formats/tests/data_scene.rs`.

Confidence: **confirmed** = read from the code and checked against all data where that applies,
**likely** = one source or strong inference, **guess** = needs an experiment (oracle or Wine
screenshots).

## 1. Conventions

- **Units.** World units ≈ 10 m (routes.md §3). Part vertices are raw/256 (models.md §6).
- **Handedness.** The look-at builder 0x418420 maps the view direction to −z and the world
  vector `(−dz, dx)` to +x. The flip to +z is the separate VIEW transform, diag(1, 1, −1)
  (renderer.md §4). The camera matrix is a proper rotation, so **world space is right-handed**:
  looking along +z with y up, +x is on the left. In a port, `look_at_rh(eye, target, +y)` with a
  right-handed perspective reproduces the image without mirroring. **confirmed** (Phase 3: a
  screenshot of the original at Higashi-Kanagawa matched only with this view; the renderer's
  `Camera::view` builds the equivalent rows f × up, up, f; models.md §6 updated).
- **Model matrices** use row vectors: `world = l.x·X + l.y·Y + l.z·Z + T`. The helpers:
  - 0x4186a0 `(x, y, z, s, c)` sets X = (c, 0, −s), Y = (0, 1, 0), Z = (s, 0, c), T = (x, y, z).
    Called with sin/cos it is a yaw rotation. Track code passes an **unnormalised chord**
    instead, which also scales X and Z. **confirmed**
  - 0x418000 `(s, c)` sets Y' = c·Y + s·Z and Z' = c·Z − s·Y (pitch). Track code passes
    `c` = 1.1, 1.01 or 1.0 with a small `s`, so this is a shear plus stretch, not a rotation.
    **confirmed**
  - 0x418080 `(s, c)` sets X' = c·X − s·Z and Z' = c·Z + s·X (yaw on an existing matrix).
    0x417f90 adds a local-space translation. 0x418800 sets translation only. 0x432030 draws with
    the **rotation of the last WORLD matrix set** and a new translation; the draw loops use this
    to reuse rotations. **confirmed**
  - `scene::Affine` reproduces these matrices exactly, so the non-orthonormal axes come
    through as is.
- **Heights.** Track y = `height_raw / 4096`. Placement y is absolute world y. Nothing else is
  added to either. The per-COM float at 0x469dc4 (routes.md §3.1 called it a "base height")
  is the **eye height above the track**, used only by the camera (§3). **confirmed**
- **Engine index** *i* = camera slot − (seg0.start + 1), which is main-line point *i + 1*
  (0x41cb00, 0x41d160).

## 2. Track drawing

### 2.1 Frame order — confirmed

The drive task's draw callback 0x41e460 runs every rendered frame in this order:

1. Mask quad: `Draw2D_FlatQuad` with xy 0x486160 = (0,0)–(496,94), colours 0x469858 (palette
   entry 0) at **depth 0**. It fills the top 94 virtual rows of the 3D band with z = 0, so no
   3D or background pixel appears there (the cab roof covers it).
2. Main line, 0x41db80(current slot).
3. Bucket runs, 0x41dc90(`h4[i / 4]`): only **one** bucket, the camera's own.
4. Live scenery list, 0x41dd40 (§4).
5. Background, 0x41dfc0(current slot, area) (§6). It is drawn last at depth 0x7ffe/0x7ff0,
   just in front of the far plane, so the z test puts it behind everything.

### 2.2 Model tables — confirmed

| VA | shape | use |
|---|---|---|
| 0x486170 | ptr[5] → u32[8][32] | rail parts per area: group 0–2 single pieces, group 3–7 bands 1–5; index = track type |
| 0x486eb0 | ptr[5] → u32[n][5] | parts for run kinds ≥ 9: `table[kind·5 − 45 + m]`. Length not stored (areas 0–3: 439, 408, 517, 203 kinds by pointer distance) |
| 0x47c2cc / e0 / f4 / 308 | u32[5] each | fixed sets for kinds 3, 4, 7, 8 |
| 0x46a770 | i32[4] = 0,1,2,1 | rail group of single pieces, `(phase + k) & 3` |
| 0x469cbc | i32[6] = 1,2,4,8,16,32 | points per rail piece by band |
| 0x469d84 | f32[6] = 1 … 1/32 | 1/step |
| 0x469da4 | f32[8] | band *b* uses `[b+2]` = 0.0620, 0.248, 0.992, 3.97, 15.88 (= (0.125·step)²·cos²≈7°) |
| 0x469cd4 / 0x469ce4 / 0x469d9c | i32[4] = 1,1,2,4 / 2,0,3,4 / f32 1,1,½,¼ | non-rail band step / model index / scale |

The data test checks that every rail and kind part that a route can reach exists in
`partsRC` + `partsR<area>`. `SceneTables::read(exe, max_kind)` reads all of these at runtime.

### 2.3 Single-point rail pieces (0x41d890) — confirmed

For each of `count` consecutive slots *p* (next slot *q*), with `k` counting from 0:
- Track type `t = p.flags & 31`. If t = 31, nothing is drawn, but `k` still advances.
- Position: XZ midpoint of p and q, y = (h_p + h_q)/2/4096.
- Axes: 0x4186a0 with `(s, c) = (−8·dx, −8·dz)`, d = q − p. **Local +z points backwards
  along the track** and has length |d|·8 ≈ 1 (the piece is stretched to the chord).
- If h_q ≠ h_p: pitch `s = (h_q − h_p)/512`, `c = 1.1`. On a gradient the piece's local y and z
  come out 10 % longer.
- Cull: drawn only if the piece origin's camera-space z (camera forward = −z) is ≤ **0.625**
  (double 0x469740). That is a half-space test that drops pieces more than 0.625 units behind
  the eye plane. It does not depend on aspect ratio.
- Part = `rail[0x46a770[(phase + k) & 3]][t]`.

### 2.4 Banded rail pieces (0x41d9c0) — confirmed

Band *b* = 1..5 spans `step` = 2, 4, 8, 16, 32 points. For each piece starting at slot p, with q
= p + step and mid = p + step/2:
- Type from **p**. t = 31 skips the piece.
- Position: the mid slot itself (x, h/4096, z). No averaging.
- Axes: `(s, c) = (−dx·8/step, −dz·8/step)`. Pitch when dh ≠ 0: `s = dh/512/step`, `c = 1.01`.
- **Rotation reuse.** If an earlier piece in the same call was computed fresh, has the same dh,
  and `dot(prev_d, d) > 0x469da4[b+2]`, the piece reuses that rotation with its own position.
  This is roughly "within 7° of the last fresh piece". The original leaves the "previous"
  values uninitialised when the call's first piece is type 31. The port treats that case as
  "no previous". **confirmed** (code)
- Part = `rail[b + 2][t]`.

### 2.5 Main line (0x41db80) — confirmed

From **4 points ahead of the camera slot** (`+0x30`), the main line draws 22 single pieces, then
4×2, 2×4, 1×8, 1×16 and 2×32-point pieces. The counts are immediates stored to
0x850120..0x850134 at 0x41cc18..0x41cc3d. That makes 126 points, from 5 m to about 170 m
ahead. Nothing is drawn behind the camera or in the first 4 points. The single-piece phase is
`engine_index & 3`, the same as the first slot's own index, so the pattern stays fixed to the
track. **The band boundaries move with the camera.** Coarse pieces are re-cut every point,
which is visible as LOD shimmer in the original.

Near the end of a COM the main line can run past the region: COM 6 has only 58 points after
its last stop. The original then reads the h6 bytes that follow as points. `scene.rs` stops at
the segment end instead. **confirmed** (data) / the original's garbage there is **guess**.

### 2.6 Bucket runs (0x41dc90) — confirmed

0x41dc90 walks the 20-byte records of `h4[i/4]` until the negative kind:

- **kind 1** → 0x41dbf0. Counts are `byte4` single pieces, then nibbles `b5>>4, b5&15, b6>>4,
  b6&15` and `b7` for bands 1–5, starting at the run's first slot. Drawn as in §2.3/§2.4,
  with the single-piece phase taken from the **camera's** `engine_index & 3`, not from the run.
  The group-0/1/2 choice on side tracks therefore changes as the camera moves (on the original
  too). **Visible effect (Phase 3):** every side-track single piece swaps model (different
  ballast heaps) each time the camera passes a point, which is the flicker around the tracks
  at low speed (the user confirms the original flickers there). Across one point boundary this
  changes ~32,600 pixels of the lower half at 1280×800; anchoring the phase to the run's slot
  (`run.slots.start & 3`, enhancement `stable_track_variants`, `TrackOptions`) halves it to
  ~16,500, leaving ordinary motion and the far LOD re-cuts. Off by default.
- **kinds 3, 4, 7, 8** → the fixed sets. **kinds ≥ 9** → the area table. Both are drawn by
  0x41d7d0 (§2.7). **kinds 2, 5, 6** draw nothing.

### 2.7 Non-rail runs (0x41d7d0 / 0x41d5b0) — confirmed

The five parts are P0..P4. Bands 0–3 take their counts from bytes 4–7, step 1/1/2/4, parts
P2/P0/P3/P4 and scale 1/1/½/¼. The remainder up to the end pointer is drawn with P0 at step 1,
for `(end − ptr) − 1` pieces. Each piece:
- Position: XZ midpoint of p and q = p + step, y = (h_p + h_q)/8192.
- Axes: `(s, c) = ((p.x − q.x)·scale, (p.z − q.z)·scale)`. That is **without the ×8 of the
  rails**, so these models get about 0.125 scale in X/Z and 1 in Y. They are presumably
  authored at 1 point = 1 model unit. Pitch when dh ≠ 0: `s = dh/4096·scale`, `c = 1`.
- **No track-type check.**
- Hard-coded exceptions (station = 0x850214, the 1-based station within the COM):
  - Station 3, COM 1, engine 0xe4c..0xe56, chord > 3.5: skip.
  - Station 4, COM 2, engine 0x129b..0x129f, chord > 3.5: skip. These two hide "bridge" pieces
    whose chord jumps between side segments.
  - Part 0xe15 is skipped at station 6 of COM 7.
  - Part 0x2616 is skipped at station 6 of COM 3 when |dz| > 5.
  - Other bridge pieces exist (3 found over all routes, e.g. COM 1 engine ≈ 5211, a 339-unit
    chord into segment 110). They lie far beyond the far plane, so the original clips them.
    **confirmed** (data).

### 2.8 Buffer stops (0x41d400) — confirmed (code) / likely (meaning)

After a non-rail run: if `byte6 == 0` and the slot at the end pointer has flags `0xe000` in the
high nibble with `n = flags & 7 ≠ 0`, part P1 is drawn **n + 1 times**. The base is slot
`end − 1` (B), and E = slot `end`. With d = (E − B)/n (x, z and h/4096):
- It is drawn only if B is within XZ distance² ≤ 100 of the eye. This is the only radius cull.
  renderer.md's "Scenery_DrawIfNear" is this function.
- Axes: `(s, c) = (−8·d.x, −8·d.z)`. Pitch `s = d.y`, `c = 1` if the heights differ.
- Copies sit at `B − d/2 + k·d` for k = 0..n, in world space with one shared rotation.

### 2.9 Tunnel plug (0x41de20) — confirmed (code) / likely (purpose)

This is called from the background for area 1 (list 0x46a124) and area 3 (list 0x46a194). Track
types 28–30 mark tunnels. It probes the slot 110 points ahead and counts *n* (≤ 27) consecutive
tunnel-type points every 2 slots from there. If n > 0, it draws `list[n]` at that point: y = h,
axes `(−8dx, −8dz)` to the next slot, no pitch. The part ids count down with n, so the plug is
probably a dark tunnel interior sized to the tunnel length. Then it walks back from +110 in
6-point steps while the type is a tunnel. If it gets back past the camera, it reports
**inside**. In area 1 (COM ≠ 7/10) that skips the whole background (§6).

## 3. In-cab camera (0x41d160) — confirmed (code) / likely (that it is the cab view)

The drive task created in 0x41cb00 runs 0x41d160 every tick:

- **Position.** A slot pointer plus a fraction `t` (units). `t += speed_per_tick` (0x850208 =
  km/h × 0.000925). While `t > 0.125` the camera steps one slot forward and runs the
  activation list (§4). While `t < 0` it steps back. The comparison is strict, so a run-up that
  lands exactly on a point stops one slot short with t = 0.125 (`CabWalker` reproduces this).
  The slot is clamped to ≥ engine 12 when moving back and ≤ `seg0.end − 23` slots when moving
  forward.
- **Eye** = linear interpolation between the current slot and the next at `f = t × 8`, plus
  `eye_height` in y. **Target** = the same interpolation **one slot further on**, so the camera
  looks along the chord about 1.25 m ahead. Pitch follows the track gradient. There is no
  spline, no sway and no roll: `0x84c118` (roll) stays 0 in the drive mode, and the other
  writers belong to replay/demo cameras (0x419730, 0x41a7b0, 0x41b540).
- **Eye height** = `0x469dc4[com·8]` (f32): 0.26 for most COMs, 0.32 for COM 6/7, 0.328 for
  COM 8, 0.34 for COM 12. That is 2.6–3.4 m. The second f32 of each pair (0.02–0.04) has no
  reader. For COM < 1 the area table at 0x469e44 is used instead.
- **Jolt.** 0x40e370 creates a task with amplitude `a`: −0.004 at 0x409d0e, −0.0005 at
  0x409d74, −0.018 at 0x40a5ab. While |a| ≥ 1e-4 it lowers the eye by `a` and the target by
  `2a`, and multiplies `a` by −0.8 every other frame (parity of 0x84c494). The engine has to
  supply these events. **confirmed** (code) / guess (which gameplay events: brake/door sounds
  0x2a–0x2f are queued next to them).
- **Projection.** 0x41cb00 stores **FOV 450.0 tenths = 45° vertical** in 0x84c150 (0x41cb33).
  The aspect is the init value **1.42**, which 0x41c3ac restores after the 45°/1.28 viewer mode
  (0x41b540). Near 0.05, far 32, with the 4.12 quantisation of renderer.md §4. The 30° preset
  in renderer.md belongs to the init/menu state. **likely**: nothing else writes 0x84c150 on the
  drive path, but the oracle should confirm it.
- **Start of a segment** (0x41cb00): the camera is put up to 300 points (37.5 units) behind the
  station's stop with an empty live list and `t` = the distance back. The first tick walks to
  the stop and replays every activation list on the way. The data test checks, at every stop
  of every route, that this gives the same live set as a replay from the start of the route.
- **Interpolation for high refresh rates.** `cab_camera(route, pos, ..)` is a pure function of
  (slot, t). Evaluating it at a sub-tick position gives exactly the original's path, which
  is linear along the polyline chords.

## 4. Scenery

- **Live list** (0x8502a8, a singly linked list in insertion order). 0x41cee0 appends a
  placement unless it is already in the list. 0x41cf90 removes it. Lists `h6[i]` are applied when
  stepping forward onto engine index *i*, and `h6[i+1]` with the signs swapped when stepping
  back onto *i* (routes.md §5). **confirmed**
- **Draw** (0x41dd40): every live object, with 0x4186a0(x, y, z, sin yaw, cos yaw). There is
  **no distance or frustum test**; D3D clipping does the rest. Consecutive objects with equal
  `trunc(yaw × 65536)` reuse the previous rotation, which has no visible effect. The model
  LOD slots at +0x18 (depth < −6 or < 10 picks alternative models) are always 0 here
  (0x41cee0 clears them), so there is no scenery LOD. There is one hard-coded exception: part
  0x132f is hidden on COM 10, station 1, engine 0x579..1999. **confirmed**
- **Animated objects.** If the part id is in the 45-entry table 0x44e820, 0x401330 creates a
  task instead, and the object is skipped in 0x41dd40. The frames are `{n, ids[n]}` at
  `0x4841ec[k]`. Class by table position: k < 6 takes its frame from global 0x85744c (likely
  signal/lamp state); 6 ≤ k < 13 advances its frame every tick and moves by a velocity
  (0x401160); k ≥ 13 is triggered by eye proximity (0x4011b0, threshold at 0x44e8d8). Class 11
  and classes ≥ 23 have their own draw (0x401260, 0x4012d0). **likely** (structure). The exact
  triggers are open. For a viewer, frame 0 is a safe stand-in.

## 5. Visibility and widescreen

### 5.1 What depends on the view

| mechanism | test | aspect-dependent? |
|---|---|---|
| main line (§2.5) | fixed slot window ahead + half-space z ≤ 0.625 | no |
| buffer stops (§2.8) | XZ radius 10 | no |
| tunnel plug (§2.9) | fixed slot probe | no |
| scenery live list (§4) | **precomputed lists h6/h7** | **yes (data)** |
| bucket runs (§2.6) | **precomputed per 4 points** | **yes (data)** |
| everything | D3D clipping to the projection frustum, far 32 | follows the port's frustum |

At runtime the game has no yaw or frustum test. **But the precomputed lists were built for the
original's 4:3 view.** Measured with `widescreen_visibility_report` (all 15 routes, every point;
angle = horizontal angle of the object centre from the view direction):

- Placements leave the live list almost exclusively at **≥ 32°**. Per 1° bin from 20°:
  0, 4, 1, 0, 2, 1, 4, 3, 7, 7, 12, 15, **108, 952, 1983, 2316, …**. Only 33 of ~54 000
  removals happen below 30°. The edge matches the 4:3 3D band: atan(tan 22.5° × 1.53) =
  32.4°, where 1.53 is the on-screen aspect of the 640×418 viewport (renderer.md §5). Removal
  usually comes once the object's extent has also left (bulk at 33–45°). **confirmed** (data)
- Placements within 6 units that are *not* live: 609 / 106 / 29 in the 0–30° bins, then
  2492 / 8340 / 8272 … from 30°.
- Side-track slots within 8 units that are *not* in the current bucket: 9771 / 5475 / 3028 at
  0–30°, then 23 402 / 58 843 from 30–50°. The ones that are in it drop from 206 553 (20–30°) to
  31 857 (40–50°). Buckets are cut to the same 4:3 view, with a margin. **confirmed** (data)
- Only 425 of 59 146 placements are never activated at all.

Objects enter the view from far ahead, where their angle is small, and leave through the
sides. In a Hor+ view wider than 4:3, scenery and side tracks would therefore **vanish while
still visible near the left and right edges**. There is no pop-in at the far end, because
entry is unchanged.

### 5.2 Recommended widescreen approach (enhancement, own config option)

Keep everything the original draws, unchanged, and only add things outside its view:

1. **Scenery.** Use `live_intervals(route)` (equal to the walker's live set; data test). Then
   use `widen_intervals(route, &iv, WideView { tan_half_fov_x, original_tan_half_fov_x:
   ORIGINAL_TAN_HALF_FOV_X, max_extra_points }, radius)`. It postpones each removal that
   happened outside 90 % of the original half-angle until the object's bounding circle (radius
   from the part's vertices) leaves the wide wedge or falls behind the eye. Removals inside
   the original view are kept as they are, so the centre of the view stays faithful. At 21:9
   this cuts removals inside the wide view from 33 248 to 43 of 59 331 (data test).
2. **Side tracks.** `track_instances_wide(.., extra_buckets)` draws the current bucket exactly as
   the original does. It then adds pieces from the previous `extra_buckets` buckets, nearest
   first, but only at slots no nearer bucket covers. Pieces that left the 4:3 view through the
   side therefore stay until GPU clipping removes them. 16 buckets (64 points, 80 m) cover
   21:9 (a removal at lateral L is postponed by about 0.77·L units of travel). That adds about
   25–150 pieces per frame.
3. The main line, buffer stops and tunnel plug need no change. Keep the vertical FOV (45°) and
   widen the horizontal FOV. Extend the background (§6).

`tan_half_fov_x` for Hor+ is `ORIGINAL_TAN_HALF_FOV_X × (screen aspect)/(4/3)`. This assumes
the 4:3 screen's 3D band. Use the port's actual 3D viewport if it differs.

### 5.3 Pop-in, LOD and draw distance (port enhancements, each its own setting, off by default)

**What the original swaps or limits by distance** — confirmed (code and data; the reports are
the ignored data tests `activation_distance_report`, `lod_part_report`, `traffic_lod_report`,
`activation_swap_report` and `main_line_runs_report` in `data_scene.rs`):

| mechanism | distance rule | models |
|---|---|---|
| main line (§2.5) | 22 single pieces, then bands of 2/4/8/16/32 points, re-cut every point; ends **130 points (≈ 16 units, 160 m) ahead** although the far plane is 32 | singles: 10 triangles each (3 ballast variants); bands 1–2: 4, bands 3–5: 2 (flat strips) |
| side-track rail runs (§2.6) | the band counts are baked per bucket into the run record (+4..+7): the nearer the bucket's camera point, the more single pieces | as the main line |
| other kinds (§2.7) | bands 0–3: P2 / P0 / P3 (2 points) / P4 (4 points), then P0 | P2 has 8× the triangles of P0 (e.g. 32 vs 4); P3/P4 are P0 stretched |
| side tracks at all | a run is listed by a bucket only once it is near: the farthest slot of a newly listed run is ≤ 20 units for 95 % of runs | — |
| scenery (§4) | activation lists: an object enters at **4–20 units (40–200 m)** for 99 % of activations (0–4: 21, 4–8: 7489, 8–12: 19 856, 12–16: 11 861, 16–20: 18 955, 20–24: 250, beyond: 962 of 59 397) | no scenery LOD: the +0x18 slots are cleared (§4), and only 3 activations swap one placement for another at the same spot (data) |
| AI trains (traffic.md §4) | 5 bands of 3 units from 15 units ahead, plus 1–3 bands by lateral distance | e.g. 292 / 164 / 101 / 36 / 10 triangles; the farthest is a box, no flat sprite |
| result replay trains (§8.1) | the same bands on the car's z | as AI trains |

So in the original, buildings and trees appear 40–200 m ahead inside a view that reaches 320 m,
the main line's rails stop at 160 m, and the ballast changes model at band boundaries that move
with the camera. None of the far models is a deliberately different look (no impostor sprite):
each coarser model is the same surface with fewer triangles (flat ballast strips, boxes for far
cars), so `max_lod` uses the nearest model everywhere without judgement calls.

**`max_lod`** (`TrackOptions::max_lod`; the game's `traffic::add_cars`, `ResultView::max_lod`):
- Rail runs (main line and side tracks): single-point pieces over every slot the bands would
  cover, with the same single-piece variant pattern (`(phase + k) & 3`).
- Other kinds: P2 at one point per piece over the bands' and the remainder's slots; the buffer
  stop keeps the original's test (it reads the run's byte 6).
- AI trains and result-replay cars: LOD 0 wherever the original's bands draw them (the bands
  still decide *whether* a car is drawn: 25 m behind, the lateral cut-off, `far_traffic`).
- Scenery: nothing to change.
- Cost: 3–5× the track triangles (stage 10 Yokohama: 4 000 → 18 000 per frame).

**`early_activation`** (`scene::activate_early`, applied by `RouteScene::set_geometry` after
`wide_visibility`'s `widen_intervals`):
- Each live interval's start is moved back to the first point of the stretch before it from
  which the object's bounding circle is in front of the eye, inside the horizontal view (the
  wide view's tan when `wide_visibility` is on, else the original's 0.634) and within the far
  plane, evaluated with the cab camera at every point; at most 2 × far / 0.125 points earlier and
  never before the same placement's previous removal (no double drawing). Removals are not
  touched, so deactivation behind the camera stays the original's (or `wide_visibility`'s).
- Track: the main line runs on in 32-point pieces to the far plane, and side-track runs of
  the buckets up to `far / 0.5` ahead are drawn at slots the camera's bucket does not list
  (`TrackOptions::reach`, nearest bucket first, as `track_instances_wide` does behind).
- The engine is untouched (activation in the original is display-only; the replays are still
  0 mismatches).
- No fade-in of its own: with early activation objects appear at the far plane, where
  `distance_fade` dissolves them in.

**`draw_distance`** (`original` 32 units / `2x` / `3x` / `max` = 4×, 128 units = the panorama's
anchor distance, §6.4):
- `Projection::far`; activation and track building as `early_activation`, out to the new far
  plane (a further far plane with the original activation would show nothing new); AI trains
  with `far_traffic` to the new far plane.
- The 2D background keeps the same distance in front of the far plane as in the original
  (`camera::background_depth`): the original's depth words put the sky (0x7ffe) 1.2 units and the
  panorama (0x7ff0) 7.6 units in front of the far plane, so world geometry from 24.4 units on is
  hidden wherever a panorama sprite covers it (in the original too). Mapping the words
  proportionally would hide everything beyond 76 % of any far plane behind the mountains;
  keeping the gap keeps that strip as thin as the original's.
- The snow fog (its own start/end), the screen-space sky, ground, panorama and extra quads are
  unchanged; the station-result replay keeps its own projection (far 32).

**`distance_fade`** (`Renderer::distance_fade`, shader `faded_out`): a 4×4 ordered-dither
screen-door fade of the world from `end − far/4` to `end`, `end` = where the panorama starts
to hide the world (far − 7.6 units). Dither rather than blending to a colour: what lies behind a
distant object is the sky gradient, the flat or textured ground or a panorama sprite, so a
single fade colour would draw sky-coloured ghosts in front of the mountains; the dither shows
whatever is really behind and needs no sorting. With the original draw distance it fades
16.4–24.4 units, which removes objects the original shows between 24 and 30 units where no
panorama covers them; it is meant for the longer draw distances.

**Cost** (2560×1440, RDNA 3.5 iGPU, headless: compose + upload + draw + GPU wait, mean of 200–300
frames, the views of `out/explain/gfx/geometry/`; triangles / world draw calls / ms per frame):

| setting | stage 10 Yokohama | stage 9 Hokuhoku | stage 1 Akita |
|---|---|---|---|
| original | 3 988 / 85 / 0.59 | 4 192 / 49 / 0.52 | 4 024 / 25 / 0.51 |
| `max_lod` | 17 993 / 77 / 0.83 | 10 332 / 43 / 0.58 | 21 972 / 21 / 0.79 |
| `early_activation` | 4 446 / 105 / 0.97 | 5 542 / 53 / 1.02 | 4 661 / 56 / 0.84 |
| `draw_distance 2x` | 6 328 / 117 / 1.42 | 7 590 / 62 / 1.00 | 7 540 / 77 / 1.26 |
| `draw_distance 3x` | 7 643 / 125 / 1.81 | 9 160 / 73 / 1.15 | 9 107 / 86 / 1.86 |
| `draw_distance max` + fade | 9 404 / 136 / 2.39 | 9 232 / 75 / 1.44 | 10 680 / 89 / 1.64 |
| all four, max | 76 561 / 127 / 5.14 | 36 855 / 66 / 1.84 | 56 794 / 82 / 3.29 |

`distance_fade` itself costs nothing measurable. The time is mostly the CPU building the frame
(the buckets ahead: one bit per slot and run records deduplicated took the worst case from
13.5 to 3.6 ms); the GPU part stays under 1.5 ms. In the windowed game (stage 9 driven,
`--no-vsync`) every setting ran at the compositor's 55–60 fps, as the original does.

## 6. Background (0x41dfc0) — confirmed (code) unless noted

`mode` = area (0 Yamanote, 1 Akita, 2 Tōkaidō, 3 Hokuhoku, 4 Keihin). Everything is 2D, in
496×400 virtual units, drawn as 4-vertex fans of pre-transformed vertices (rhw 1, white
diffuse, `z = depth/32768` with 1/32768 at 0x47c968; screen = virtual × resolution/496 and
/400 via 0x47c964/0x47c960).

### 6.1 Layers per mode

- **Quads.** Sky gradient 0x469c68, corners (0,66) (0,235) (496,235) (496,66), colours
  0x469c60. Ground 0x469c90, (0,235) (0,415) (496,415) (496,235), with flat colours 0x469c78
  (gradient), 0x469c80 (all 0x380) or 0x469c88 (all 0x3ff). Textured ground in area 3 uses
  0x469ca0, (0,251)–(496,415). Depth 0x7ffe for all of them.
- **Per mode:**
  - 0: panorama + sky gradient + textured ground (0x76f). On COM 4 at engine index 1501–1589
    the ground is at depth 0x7fee instead of 0x7ffe, which only changes ordering.
  - 1 (COM ≠ 7/10): tunnel probe A. If inside, **nothing**. Otherwise panorama (s0/s1) + sky
    gradient + flat ground gradient.
  - 1 (COM 7/10) and 3: per-point table (0x41d0e0, `{from, bits}` entries: COM 7 0x46a5b4 ×30,
    10 0x46a69c ×25, 11 0x46a204 ×58, 12 0x46a3dc ×58). Bits 0–2 select the sprite (4 = s1 + the
    fourth). Bits 3–5 select the ground (area 1: any → flat 0x380; area 3: 1 flat 0x380,
    2 textured 0x2dd2, 3 flat 0x3ff). Bits 6–7 turn on the sky gradient. Bit 8 runs the tunnel
    probe, whose "inside" result is then ignored. Bits 9–10 select an extra world quad (§6.5).
  - 2: panorama (s0/s1) + sky gradient + flat ground gradient.
  - 4: panorama (s0/s1) + sky gradient + textured ground (0x283b).
- The clear colour stays black (0x000000). Fog colour is #BFBFBF (renderer.md §7).

### 6.2 Gradient palette and its variant

- Palette 3 is built by 0x40eed0(R-index, variant) from the 12-byte rows at 0x463b40
  (variant 0) / 0x463bac (variant 1): sky top RGB, sky bottom, ground top, ground bottom.
  Entries 0..127 are the sky and 128..255 the ground, `start + k·(end − start)/128`, truncated.
  Examples: R1 Yamanote sky (152,177,227)→(178,193,226), variant (91,106,136)→(106,115,136);
  R5 Keihin (145,145,145)→(240,80,30), variant (72,72,72)→(192,64,24). The ground rows are
  the same in both variants. `GradientColours::palette()` builds it. Colour words in the quads
  are `palette<<8 | index`: 0x301→0x37f is the sky, 0x380→0x3ff the ground. The flat-quad
  decoder (0x432b50) turns palette u32 bytes (1, 2, 0) into R, G, B.
- **What selects variant 1 — confirmed (code), meaning guess.** The variant is not a stage
  or time-of-day setting. It belongs to a timed per-stage *event* with state 0x84c3a0
  (active), 0x857230 (level) and 0x856fa8 (ticks active):
  - Stage start (0x40d040): level = 0, inactive. If `from_station[stage]` (i16[20] at
    0x459cf8) is 0, the event is active from the start (level 1).
  - Start conditions, all requiring level = 0, inactive and remaining time 0x856fb0 ≥
    `min[stage·4 + difficulty]` (i16[80] at 0x459d20; difficulty = 0x829d10):
    0x40d340 (on to the next segment after a stop): station 0x856d78 ≥ `from_station` and
    frame counter 0x84c494 divisible by 4, no fade; 0x40d620 (station reached):
    station ≥ `from_station` and counter divisible by 3, fades in; course event kind 0x15
    (0x40acd0): fades in. Kind 0x16 ends the active phase. Starting sets level 1, ticks 0.
  - Per tick (0x40c4e0): while active, level climbs to 254; the event stays active for
    `duration[stage]` ticks (i32[20] at 0x459dc0: 299970 for stages 0–2 and 17–19, 9000 for
    most, 10500 for stage 16). Afterwards level falls by one per tick; on reaching 1 the
    sound stops, level becomes −1 (never again this run) and the palette fades back.
  - Fades: 0x40f320 builds both palettes and starts task 0x436a60(palette 3, 150 ticks):
    `out = trunc(from + (to − from)·frame/150)` per channel (0x436ad0). Variant 1 is
    selected without fading when the drive (re)starts with level ≥ 1 (0x4080d0 passes the
    level as the variant). **likely** that the 0x40d340 start becomes visible only there.
  - Data: `from_station` is 0 for stages 1–2, 7 for 6–9, 5 for 12/14/16, 99 (course event
    only) elsewhere. Sound 0x28 loops at volume `level/2` while it runs, and 0x84c2e0 is set
    above level 199. 0x424740 builds the weather overlay: a wiper plus raindrops (rain
    stages) or scrolling snow with fog (Akita); 0x84c2e0 is the wiper flag (renderer.md §13).
    **confirmed**
  - `GradientEventTables::read`, `GradientEvent { stage_start, try_start(trigger), end, tick,
    variant, sound_volume }` and `palette_fade` model this for the game layer.

### 6.3 Panorama sprites (0x41c890 → 0x401420)

- 12 rows at 0x469e6c `{part, w, h, repeats, v_offset, 127, flags}`. Per COM ≥ 2, 0x469fa4
  gives `{count, s0, s1, s2, switch, switch2}`. COM < 2 uses sprite = area, and Keihin switches
  at 4419. Areas 1/2/4 draw `s0` before engine index `switch` and `s1` after. COM 15 copies s0
  over s1. With count 3, row 9 is also loaded as a fourth sprite.
- 0x41c890 turns a row into a 0x30-byte runtime record with 0x401420(part, flags & 0xf, w, h,
  128.0, repeats, depth 0x7ff0, v_offset, phase): +0x18 w, +0x1c h, +0x20 anchor distance
  128, +0x24 repeats, +0x28 `v_offset / h`, +0x2c phase = `(flags >> 8)/256` (arithmetic
  shift, 1/256 = double at 0x46a7b0), +0x14 depth. All floats except depth (u16).
- **Texture (0x432d30).** From the part's first group: `+2` = its rect id, then for the first
  4 vertices the page UV (already mapped through the rect, `TexRect::map_uv`) × 256,
  truncated, stored as (v, u). 0x432da0 divides by 256 and stores (u, v) back, so **there is
  no u/v swap**. Every sprite part's first group has exactly 4 vertices (data test).
- 0x401420 then pulls each coordinate one unit inwards (+1 if it equals the minimum of the
  4, else −1; a half-texel guard), and applies `flags`: bit 2 reverses the corner order,
  bits 0–1 rotate it (corner *i* takes the UV of corner *i + n mod 4*). Bit 3 is unused.
- The record's first u16 is set to `param_4` (0x8501e8) during the draw; nothing reads it.

### 6.4 Panorama placement (0x401600) — confirmed (disassembly; not yet compared with screenshots)

Arguments: the record, the **view** matrix 0x856be0 and the **inverse view** 0x856c20 (both
built by 0x418420; row-vector convention). From the inverse view it reads the camera's right
axis `R` (row 0, rolled), up axis `U` (row 1) and the eye (row 3). Let

```text
r = √(R.y² + U.y²);  a = R.y/r;  b = U.y/r          // screen direction of world up (roll)
p = a·U.z − b·R.z;   q = b·R.x − a·U.x              // horizontal right axis = (q, 0, −p)
heading = atan2(−p, q)                               // = π − yaw for a camera along (sin yaw, 0, cos yaw)
anchor = (eye.x − 128·p, 0, eye.z − 128·q)           // ground-plane point 128 ahead, y = 0
(vx, vy, vz) = view · anchor;  sx = −vx/vz;  sy = −vy/vz
t = (heading/2π + 0.5)·repeats + phase;  f = t − floor(t)     (0x457738, 0x457730, 0x437b57)
hx = halfW·Px;  hy = halfH·Py                         (0x849d68·0x856b80, 0x849d60·0x856b84)
ax = (w−1)/hy·hx·b;  ay = (w−1)·a;  bx = (h−1)/hy·hx·a;  by = (h−1)·b
v  = v_offset/h
X0 = cx + hx·sx − ax·f − bx·v + 0.5;   Y0 = cy − hy·sy − ay·f + by·v + 0.5
ax' = ax + 1.2·b;  ay' = ay + 1.2·a                   (1.2 = double at 0x457720)
for k in −1, 0, +1  (start −1, step 1.0 at 0x457718, while k < 1.1 at 0x457710):
    (x, y) = (X0 − k·ax, Y0 − k·ay)
    corners = (x, y), (x + bx, y − by), (x + bx + ax', y − by + ay'), (x + ax', y + ay')
```

Corners are truncated to integers (`__ftol`, so the +0.5 rounds positive values). With no roll
(`a = 0`, `b = 1`) and the original projection (hx ≈ hy) a tile is the rectangle from
(x, y − (h−1)) to (x + w − 1 + 1.2, y): bottom-left, top-left, top-right, bottom-right, w − 1
virtual units wide (plus 1.2 overlap), its bottom edge `(h − 1)·v_offset/h` below the projected
horizon point. The strip scrolls by one tile per `360°/repeats` of heading; turning left moves
it right. The middle tile starts at most one tile left of the anchor's x (the centre when not
rolled), so 3 tiles cover 0..496 when `w − 1 ≥ 248` (all data: checked along every route).
Depth 0x7ff0. Each tile uses the record's 4 UVs (§6.3), drawn with 0x432da0.

The anchor is at world y = 0, not at track height, so on raised track the horizon sits lower.

**Widescreen (enhancement):** keep hx/hy (same pixel scale), centre the wider viewport on the
same centre and draw tiles k = lo..hi with `lo ≤ ⌊(X0 + ax' − right)/ax⌋`, `hi ≥ ⌈(X0 −
left)/ax⌉`. `sky_quads` does this, plus one spare tile each side for roll; with the original
width it returns exactly the 3 original tiles.

### 6.5 Textured ground and extra quads — confirmed (code)

- **Areas 0 / 4** (0x76f / 0x283b, cached by 0x41cb00 into 0x850260 / 0x8500e0): the rect
  comes from the part's first group, but the UVs are replaced by the **sky-gradient quad's**
  corners shifted left by 4, stored as (x<<4, y<<4) in the (v, u) slots. So corner *i* of the
  ground quad (positions 0x469c90) gets `u = y_sky,i/16`, `v = x_sky,i/16` page units: a static
  screen-space tiling with the page repeated every 16 virtual units and turned 90°. The
  ground quad's own corners are not used for UVs. Drawn by 0x431ec0 → 0x432da0, depth 0x7ffe
  (0x7fee on COM 4, 1501–1589).
- **Area 3, ground 2** (0x2dd2 → 0x850220): the part's own first 4 UVs (no inset, no flags)
  on 0x469ca0, depth 0x7ffe.
- **Extra world quads** (0x41df00, area 3, bits 9–10 = 1 or 2): 4 world points at 0x469d24
  (y −0.2) or 0x469d54 (y 1.0). 0x432f40 transforms them with the view copy 0x856ba0 and
  returns "not drawn" if any has `vz ≥ 0`. Otherwise it projects `x = −Px·vx/vz`,
  `y = Py·vy/vz`, clips x: corner 0 to ≥ −1 along edge 0–1, corner 1 to ≤ 1 along 0–1,
  corner 2 to ≤ 1 along 3–2, corner 3 to ≥ −1 along 2–3, then maps `X = (x + 1)·248`,
  `Y = 174·y + 226` (floats at 0x47c978/74/70) and truncates. The clip helper 0x432f10
  returns `(a.x·b.y − a.y·b.x + (b.y − a.y)·x)/(a.x − b.x)`, which is the edge's y at **−x**
  (sign slip). `extra_quad` keeps it. Flat colours 0x469c80 (ground top), depth 0x7ffd
  (quad 1) / 0x7fe0 (quad 2). Likely water or a field surface.
- **What area 3's textured ground is — confirmed (data + experiment).** Part 0x2dd2's first
  group maps rect 2633 (texR4 page 19, u 0..127, v 0..63): a blue water texture with pale
  cloud reflections. Stretched over (0,251)–(496,415) with u running down the screen and v
  across it, one texel is ≈ 7.9 virtual units wide. Rivers and ponds are **holes in the
  terrain meshes**, not water polygons: with the layer switched off the river shows the black
  clear colour, and with the colour key turned into magenta (renderer.md §9) no keyed texel
  appears there. So the water is this static screen-space picture seen through the holes; it
  does not move with the camera, which makes the "reflections" look like something under the
  water. No z-fight is possible (depth 0x7ffe, behind every 3D polygon). The original does the
  same: 0x41dfc0 case 3 draws 0x469ca0 with the part's UVs (0x432da0), whatever is in front.
- **Gap at the horizon — confirmed (code + data + Wine capture).** The sky gradient
  ends at y 235 (0x469c68) and the area-3 textured ground starts at y 251 (0x469ca0), so the
  band 235..251 is covered only by the panorama, the 3D terrain and extra quad 1 (which the
  per-point table turns on together with ground 2, e.g. hokuho2a.com from engine index 3325,
  bits 0x251). Where none of them reaches, the black clear colour (0x4286b0) shows: a black
  strip under the hills, at 4:3 too (Hokuhoku 485 after Naoetsu, engine index ≈ 3350: x
  0..≈ 25 % of the view; ≈ 3460: the full width, the extra quad
  not being drawn there). The port keeps it; widescreen only extends it to the new edges.
  Wine check (scripts/capture-original.sh, stage 9 stopped at engine index 3346, 640×480): the
  original shows the same black band, rows 290–301 from x 0 to ≈ 160, and the port at the same
  index draws rows 289–300 to x ≈ 160 (1 px vertical offset, otherwise pixel-for-pixel the same).
- Widescreen: the flat and textured quads span x 0..496. `Quad2d::widened` and `ground_quad`
  move the left/right corners to the viewport edges and extrapolate the UVs linearly along each
  horizontal edge (v = x/16 for the tiled ground, whose page repeats anyway). The area-3 water
  (the part's own UVs) cannot be extrapolated: past 0 and 496 its UVs would leave rect 2633
  and show the neighbouring atlas textures as grey blocks (the bug seen at 16:10 and wider).
  `ground_quad` covers the extra width with horizontally mirrored copies of the quad instead,
  seamless at 0 and 496 and inside the rect. At the original width it is the single original
  quad.

`scene::background()` returns the per-frame plan. `BackgroundTables` holds the tables;
`sky_quads`, `ground_quad`, `extra_quad` turn them into `TexQuad2d` / `FlatQuad2d`.

## 7. API (`shuppatsu_formats::scene`)

- `SceneTables::read(&exe, max_run_kind)`: every table above (rails, kinds, bands, eye
  heights, tunnel plugs, animated parts, background).
- `CabWalker::at_segment_start(route, stop)` / `::at(route, pos)` / `.advance(route, units)` /
  `.live()`: the camera walk with clamps and activation lists.
- `cab_camera(route, pos, eye_height, jolt) -> CabCamera { eye, target }`. Also the
  `DRIVING_FOV_Y_DEG`, `DRIVING_ASPECT`, `NEAR` and `FAR` constants.
- `track_instances(route, tables, ctx, pos, &camera) -> Vec<TrackInstance { part_id,
  transform: Affine, slot, source }>`, and `track_instances_wide(.., extra_buckets)`.
- `scenery(route, tables, ctx, &walker) -> Vec<SceneryInstance>`.
- `live_intervals`, `widen_intervals`, `live_at`, `ORIGINAL_TAN_HALF_FOV_X` (§5);
  `activate_early` / `EarlyView` and `TrackOptions { max_lod, reach }` (§5.3).
- `tunnel_probe`, `background`, `sky_sprite_slots`, `GradientColours::palette`.
- Background drawing (§6.3–§6.5): `ViewBasis::{look_at, from_cab}` (camera axes as 0x856c20
  holds them), `Viewport2d::{original(tan½fov_y, aspect), hor_plus(half_width), x_range}`,
  `SpriteTexture::{from_group(group, rect), for_sky(flags)}`,
  `sky_quads(sprite, tex, view, viewport, snap) -> Vec<TexQuad2d>`,
  `ground_quad(bg, ground, tex, x_range) -> Option<TexQuad2d>`,
  `extra_quad(bg, which, view, viewport, snap) -> Option<FlatQuad2d>`, `Quad2d::widened`,
  `depth_z`. `TexQuad2d { xy (virtual units), uv (normalised page), rect_id, depth }`.
- Gradient event (§6.2): `GradientEventTables::read`, `GradientEvent`, `GradientTrigger`,
  `PaletteFade`, `palette_fade`, `PALETTE_FADE_TICKS`.
- `SceneContext { area, com, station }` feeds the hard-coded exceptions.

## 8. Station-result replay (0x419730 / 0x41a010 / 0x41a470)

At each station stop the result screen (menus.md §9) does not keep the cab view. It shows a
**replay of the last 4 s of the approach in a small platform set at the world origin**, in two
viewports. Code: `shuppatsu_engine::stop_replay`, `shuppatsu_formats::result_camera`,
`crates/shuppatsu/src/result_view.rs`.

### 8.1 What is drawn — confirmed (code) / confirmed (port screenshots look right; no Wine comparison yet)

- **The route is gone.** `Stage_Run` (0x4080d0, arrival branch) calls `TaskList_KillAll(0x84c1bc)`
  before the two `Task_Tick`s that precede `StationResult_Create` (0x413d90). List 5 holds the
  drive task (track, scenery, background, 0x41cb00), the AI-train tasks, the weather overlay
  (0x424740, 0x424f20) and the fog task (0x425070, whose destroy calls `Gfx_SetFog(0)`). So no
  track, scenery, sky, traffic, weather or fog is drawn until the next segment start.
- **Platform set.** The camera task's draw callback 0x41a470 draws, at the identity matrix,
  one part chosen by timetable field 7 of the arrival station (`row + 0x40`, 0x419a30):
  0 / other → part **0x1e**, 1 → **0x23**, 2 → **0x21**, 3 → **0x22** (all in `partsRC`). The
  parts are a short platform with passengers, the stop-position arrow and the distance marks.
- **The player's train, from outside.** The direction-1 car list of the train-model table
  (0x457358, traffic.md; the pointer read is `0x45735c[row·3]`) for `row = 0x4596e0[stage]`
  (i32[20]: 11 11 11 15 14 39 39 40 41 42 1 1 26 25 24 35 0 11 11 11). The lead car's origin
  is at `(0, 0, z0)` with `z0 = dist × −0.1 − 0x4696cc[train type]` (dist = replayed distance
  to the stop in m, positive = short; `0x4696cc` f32[20], e.g. 0.98 for type 2, 1.02/1.04/1.24…);
  each car adds its spacing (car record +24, ≤ 0) before it is drawn, so the train runs along
  +z and stops with its head near the arrow. LOD by the car origin's z (doubles 0x469780 …
  0x458c00): z < −24 or ≥ 2.5 not drawn; [−24,−18) LOD 4, [−18,−12) 3, [−12,−6) 2, [−6,−3) 1,
  [−3,2.5) 0; then |x| > 4/8/12 adds 1/2/3 and |x| > 16 skips (x is always 0 here).
- **Coupling stations** (stages 0–2 at segment-start station 23, stages 17–19 at station 15;
  0x419894): rows 12 (lead) / 13 (lead "after"), offset 1.07 (0x3f88f5c3). If course event 12
  (0x40acd0) set `0x855ff8 & 0x8000000` or `& 0x10000000`, a waiting train is drawn too (rows
  17/18 or 20/21: its last two cars at LOD 0, from `z = spacing/2` forwards) and the whole record
  is shifted by +40 m (0x4594b0), so the player's train comes in from further back. The "after"
  lists switch in when the record shows a standstill (sample < 0.01) for more than 29 / 39 draws,
  counted from `task+0x1cc` = −9999 (0 if `0x855ff8 & 0x40000`). After the coupling (station >
  23) the lead row is 8 (train type 14) or 7 (type 12); other types leave `+0x1d0` unset.
  0x419730 clears 0x40000 and the bit it used. Which bit is set and when: rules.md §7.5 (a
  stop that is not perfect clears both, so only a stop that started the minigame shows the
  waiting train). **confirmed** (code); port screenshots of stage 17 after a coupling in
  `out/explain/coupling/` (no Wine comparison yet: stage 17 is only reached through stage
  10's extension and stage 2's station 23 needs 23 perfect stops first).

### 8.2 The record — confirmed (code)

0x409fc0 (rules step 3, driving ticks only) writes `0x85708c` (distance to the stop) into the
400-entry f32 ring `0x850300` at index `0x850a80` while `dist ≤ 50` (0x4594c8) and the target
stops (`0x856d74 == 1`); during an obstacle stop (`0x856d80 ≥ 1`) it writes `0x856dac − odometer`
and advances only while that is ≤ 200 (0x4594b8); otherwise it rewinds (`0x850a80 = 0`,
wrapped flag `0x850a78 = 0`). The camera picks the last 120 samples (`(head + 280) % 400` when
wrapped), or `0..head` when fewer. **Overrun replacement:** when the last sample is ≤ −10 m
(0x459538) the camera overwrites samples 0..=120 with a made-up approach: from 20 m
(0x459558), step `2(|d|+20)/120` shrinking by `2(|d|+20)/14400` per sample (0x469768,
0x469760), ending about `|d|` m past the stop; it then plays 120 of them.

### 8.3 Timing — confirmed (code) / likely (task-list order)

| Tick (ending in) | 3D layer |
|---|---|
| `Arrived` 1–3 | cab view |
| `Arrived` 4–5, result tick 1 (countdown 95) | nothing (black under the HUD) |
| result tick 2 (countdown 94) | camera task created; rendered at once with sample 0 |
| each later tick | 0x41a010: one sample per tick (camera and train), holding the last |
| the tick the result closes | the result's destroy clears `0x855ff8 & 0x40`; the camera task, later in list 3, sees it in the same pass: `0x84c128 = 0`, task ends — black |
| after `finish_station` / 0x40d340 | next segment, cab view again |

Tasks are appended at the list tail (0x425260) and `TaskList_Run` (0x425320) reads `next`
before calling an update, so the camera created by the result's update first runs on the next
tick (likely: assumes no task was appended to list 3 after the result).

### 8.4 Viewports, cameras, projection — confirmed (code)

`0x84c128 = 1` selects render mode 1 of 0x425680 (renderer.md §12), two passes over all draw lists:

| View | Viewport (virtual 496×400: centre, half) | Pixels at 640×480 | FOV | Aspect | Eye / target |
|---|---|---|---|---|---|
| Orbit | (336,240), (160,156) — 0x425897 | (226.5,100.8)–(640,475.2) | 45.0° (0x43e10000) | 1.02 (0x3f828f5c) | `0x84c11c` / `0x84c10c`, roll 0x84c118 = 0 |
| Monitor | (90,240), (76,80) — 0x425934 | (18.1,192)–(214.2,384) | 75.5° (0x443cc000) | 1.0 | `0x84c100` / `0x84c13c`, roll 0x84c148 = 0 |

The monitor frame sprite (tbl 0x25e at (16,192), slot 0x57) surrounds the monitor view; the
result's text rows are sprites over the orbit view. Viewports are the 3D only; outside them the
frame stays the clear colour (black). The port positions both relative to the 640×480 canvas
rectangle and keeps the original's non-square projection (aspect after the 4.12 round trip,
stretched to the pixel shape) unless `square_pixels`.

**Monitor camera** (fixed per platform part, 0x419a4c..0x419b1a): part 0x1e / 0x21 eye
(0.6, 0.3, 0.732) → target (−0.643, −0.32, −1.607) (also the init pose of 0x4254b0); part 0x23
eye (−0.438, 0.3, 0.589) → (0.626, −0.52, −2.184); part 0x22 eye (0.470, 0.29, 0.610) →
(−0.834, −0.52, −2.103). A platform ITV view down the train's side.

**Orbit camera** (0x41a010, also once at creation): `v = sample × −0.1` (units past the stop).
Keys are 7 f32 (centre x, centre z, eye radius, target radius, eye y, target y, angle); sets at
0x469454 (parts 0x1e/0x21/0x22) and 0x4694c4 (part 0x23), three keys each: *short*, *at the stop*,
*past*. If `v ≥ 0.1`: `t = (min(v,1.3) − 0.1)/1.2`, lerp *stop → past*; else
`t = −clamp(v,−1.6,0) × 0.625`, lerp *stop → short*. Then
`eye = (sin(a+π)·r_eye + cx, y_eye, cos(a+π)·r_eye + cz)`,
`target = (sin a·r_target + cx, y_target, cos a·r_target + cz)`. Normal set:

| Key | cx | cz | r_eye | r_target | y_eye | y_target | angle |
|---|---|---|---|---|---|---|---|
| short (16 m) | −0.66 | 0.15 | 1.10 | 2.5 | 0.5 | −0.60 | 2.496 |
| stop | −0.66 | 0.15 | 0.51 | 2.5 | 0.5 | −0.52 | 1.920 |
| past (13 m) | −0.66 | 0.15 | 1.33 | 2.5 | 0.6 | −0.50 | 1.169 |

(The 0x4694a8 row between the sets has no reader here.) `0x41c750` copies the stop key and
the FOV to 0x8502c0.. for another reader (not traced).

### 8.5 Port

Faithful: everything above except the items below. **Approximated / differs:**
- The record is copied, not rewritten in place (the original's later results could see the
  shifted or made-up samples if nothing new was recorded; never in normal play).
- High refresh rates interpolate the replayed distance between ticks (standard render
  interpolation; the original steps at 30 Hz).
- The coupling flags come from `Rules::result_coupling_flags` (the engine keeps them past the
  result instead of clearing them; only the coupling station's result reads them).
- The "after" list counter counts ticks instead of drawn frames (equal at 30 fps).
- After a time-out at a station (continue screen) the port shows the cab view again; the
  original's 3D layer stays empty until a segment starts (not traced further).

## 9. Stage intro (0x408ec0 → 0x4091a0 / 0x408f40)

`Stage_Run` 0x4080d0 calls 0x408ec0 once per run, after the loading wait and `0x425490(15)` and
before 0x40d040 and the departure board (menus.md §7). It is skipped on a retry (始発駅に戻る
jumps past it) and when the debug byte `0x816840 & 0x18` is set. 0x408ec0 picks by a stack table
of 20 flags (code immediates): **1 → 0x4091a0** (stages 1, 5, 6, 7, 8, 11, 12, 13, 14, 15, 18),
**0 → 0x408f40** (stages 0, 2, 3, 4, 9, 10, 16, 17, 19). So every stage has an intro; both kinds
are ported (§9.3). **confirmed** (code; oracle traces of stages 0, 1 and 13)

### 9.1 The fly-by 0x4091a0 — confirmed (code, oracle trace, Wine captures)

Per stage, 9 × i32 at **0x458c80 + 0x24·stage**: caption tbl, station, jump (m), camera km/h,
spawn, train km/h, spawn parameter, deceleration (km/h per tick), sound count. The passing sound
is the i16 **0x459e88[stage]**; the caption position the i32 pairs **0x458c60 / 0x458c70**
[resolution mode] = (24, 408) at 640×480.

| stage | caption | station | jump m | camera km/h | spawn | train km/h | param | decel | sound @ count |
|---|---|---|---|---|---|---|---|---|---|
| 1, 18 | 0x3a6 E3系 | 24 / 16 | 1915 | 260 | 2 | 270 | 0 | 6 | 8 @ 110 |
| 5 | 0x3a7 | 0 | 1515 | 120 | 51 | 130 | 70 | 3 | 4 + 1 @ 110 |
| 6 | 0x3a7 | 1 | 375 | 99 | 25 | 100 | 750 | 3 | 4 + 1 @ 110 |
| 7 | 0x3a3 | 1 | 987 | 130 | 22 | 130 | 145 | 4 | 0x1a3 @ 70 |
| 8 | 0x3a1 | 0 | 1300 | 130 | 21 | 130 | 161 | 3 | 4 + 1 @ 70 |
| 11 | 0x39e | 0 | 395 | 100 | 7 | 100 | 540 | 3 | 6 @ 110 |
| 12 | 0x39b | 3 | 713 | 75 | 8 | 80 | 230 | 3 | 4 + 1 @ 110 |
| 13 | 0x39d 207系1000番代 | 9 | 903 | 80 | 30 | 80 | 0 | 3 | 4 + 1 @ 110 |
| 14 | 0x39f | 5 | 782 | 80 | 13 | 80 | 461 | 3 | 4 + 1 @ 110 |
| 15 | 0x3a0 | 9 | 770 | 80 | 28 | 80 | 0 | 3 | 4 + 1 @ 110 |

Sequence, one line per `Task_Tick` (tick numbers from the intro's first):

1. 0x40eed0(area, 0): the area palette, and its own `Task_Tick` (it ticks when area < 7). Black.
2. 0x4317d0 (VRAM preload, a no-op on PC), `0x855ff8 |= 0x20000`, `Task_Tick`. Black.
3. 0x408010(stage, station) → 0x41cb00: the camera task of that station's segment, at its stop
   (§3). `0x850208 = jump × 0.1` (f64 0x44e8e0), `Task_Tick`: the camera walks to the stop and
   on by `jump` metres in this one tick. The route is drawn from here.
4. `0x850208 = camera km/h × −0.000925` (f64 0x459438): the camera runs **backwards**, still
   looking down the line. `0x404f10(spawn, 5, train km/h, param, −1)` (traffic.md §2; mode 5,
   model from the spawn's type), BGM **0x197** (0x42ff20, ungated), the caption card
   `0x4316e0(0x1d, tbl, 24, 408, palette 4, layer 3)`. Then the loop, count i = 1, 2, …:
   - i > 0x6d: `0x850208 += decel × 0.000925` (f64 0x458af0), so the run back slows from
     count 110 (stage 1: −260 km/h + 6 a tick to −80 at the end);
   - i = sound count: `0x41fda0(sound)` (gated by the sound option), and sound 1 with sound 4;
   - i = 0x8c: leave the loop (no tick);
   - the pressed-edge word `0x48c42c` (active low) has the horn 0x100 or A 0x100000: release
     0x197 (0x420630) and leave the loop — **skip**;
   - `Task_Tick`.
5. Close: slot 0x1d hidden, 0x431eb0 (no-op), the passing sound released (0x420650; and 1 with
   4; around 0x1a3 the code sets and clears `0x855ffc & 0x80`, purpose unknown, **guess**: a
   release mode of the BGM voice), `0x855ff8 &= ~0x20000`, `0x84c0f8 = 0`, list 5 killed
   (camera and train), two `Task_Tick`s (black).

Unskipped that is 2 + 1 + 139 + 2 = **144 ticks**; the oracle trace (`0x855ff8`, `0x850208`
logged per tick; stages 1 and 13) shows 16 ticks with `0x855ff8 == 2` before the intro's first,
141 with the 0x20000 bit, `0x850208` = 0, 191.5, then −0.2405 until −0.19055 at the ninth
decelerated tick, and then 6 ticks of 2 (the two closing ticks and four of `Stage_Run`) before the board (`0x855ff8 = 0x1002`). A skip seen at
count i ends the intro after 3 + (i − 1) + 2 ticks. BGM 0x197 (bgm.rom entry 0, 5.50 s) is not
released at the natural end, so it runs on into the departure board.

**What it looks like** (Wine captures, stages 1 and 13): the driving view (the drive task's top
panel 0..94 virtual over a 3D band from y 52, 45°; no HUD exists yet), the line ahead with the
train coming towards the camera on the other track and passing it as the camera slows; the card
at the bottom left is a black box with the series name and two lines about the train.

### 9.2 The chase 0x408f40 — confirmed (code, oracle trace, Wine captures of stages 0 and 9)

The same frame (0x40eed0, `|= 0x20000`, caption, sound 0x197, closing) with: 0x408010(stage, 1)
(the camera at station 1's stop), `Task_Tick` (the camera moves by whatever `0x850208` held: 0 in
the trace), then the player's own train
`0x406960(engine index + 6 + k[stage], mode 1, 100 km/h, 0, 0x4596e0[stage] | 0x1000, 1)`,
BGM 0x197 and the card. k is a stack table built in 0x408f40's prologue (0x408f40..0x408fe0,
read with `shuppatsu_formats::code::stack_table`): 0x55 for stages 0–2 and 17–19, 0x20 for 3–4,
0x10 for 5–6, 0x90 / 0x86 / 0x96 for 7 / 8 / 9, 0x9a for 10–11, 0x6e, 0x6b for 12, 13, 0x7b for
14–15, 0xa7 for 16 (stage 0: 91 points ahead, stage 9: 156). The model row is the stage's
train (0x4596e0: 11 for stage 0, 42 for stage 9); 0x1000 runs it forwards with the row's second
car list. Loop, count i = 1, 2, …: i > 0x8c leaves (no tick); `0x850208 = 0.0925` (f32 immediate,
100 km/h) for i < 0x6f, else `−= (float)0.002775` (f64 0x459430); i = 0x78: the passing sound
(0x459e88[stage], and 1 with 4); the skip test (horn or A edge: 0x197 released); `Task_Tick`.
So 3 + 140 + 2 = **145 ticks**. The oracle trace of stage 0 (`out/captures/intro0.csv`) agrees
bit for bit: 142 ticks with 0x20000, `0x850208` = 0, 0, then 0.0925 ×110, 0.089725 … 0.00925003
(30 decrements), and the value left in `0x850208` over the board. **confirmed**

0x406960 (the train): `param_5 & 0x1000` → `+0x30` = 1 (forwards) and the list `0x457358[row·3 +
1]` (reversed from list 0 for kinds 3, 7, 8, as in the spawner). The head is the slot
`0x85020c + 12·param_1` (the engine index's slot). With `param_6 == 1` each car is walked back
from the previous car's point by `−spacing − |fraction|` in steps of 0.125 (f32 0x458b80); the
fraction is taken over only while the remainder is not 0 (so a whole-point spacing keeps the
previous step's −0.125); `+0x60 = 0`. Otherwise every car starts on the head's point with
`+0x60 = −offset`. Rear bogie 12 points back, gate `+0x3c` = 9000, `+0x54 = km/h · 0.000926`,
`+0x58 = km/h · 0.926`, `+0x5c` = `+0x58² / 2p` in mode 2 else `p · 0.000926 · 0.033333`; the
same update and draw tasks as the spawner's cars (0x405420 / 0x405640). **confirmed** (code;
the stage 0 and 9 captures match the port's placement to the pixel)

What it looks like: the driving view at the station, the train's tail standing just ahead under
the card, running away with the camera at its heels, then drawing off as the camera slows.

### 9.3 Port (`crates/shuppatsu/src/play/intro.rs`)

The front end runs the intro as its own `Play` (like the attract opening) between the 16 loading
ticks and the 4 before the board; `shuppatsu play <stage> --intro` plays it alone. The rows come
from the exe; the camera is `Drive::ending_place` / `ending_tick` / `ending_kill`, the fly-by's
train the normal spawner, the chase's `Drive::ending_place_train` (`Traffic::place_train`, the
0x406960 port). Compared frame by frame with the original (fly-by: stages 1 and 13; chase:
stages 0 and 9; ticks 3–142, `out/explain/stage_intro/`, `chase_*.png` for the chase; the Wine
frame of tick t is 2·(t − 3) after the first non-black one): camera, train and card match.
Decisions:

- **Widescreen**: Hor+ with the drive's top panel widened to the full width (as while driving,
  `Play::cab_mask`); no other masks exist to extend. The card stays in the centred 4:3 design in
  the original HUD layout and keeps to the left edge in the enhanced one (`wide_hud`), like the
  driving HUD's bottom-left group.
- **High refresh rates**: camera and train interpolate between ticks, except across the
  fly-by's jump and the placement tick.
- The intro's BGM runs on over the board (`Sound::leave_stage` instead of the stop-all a run
  ends with).
- **Translation**: the 13 caption cards (0x39b–0x3a7, the fly-by's and the chase's) are in
  `locales/en.toml`, one size per card, white as the title (the `ink` option, localisation.md).

## 10. Open questions

- The winding/handedness conflict with models.md §6: verify in the viewer (one quad of a
  known part, front face CCW with `look_at_rh`).
- 45° vs 30° driving FOV: confirm with the oracle (`0x84c150` while driving).
- The f32 at +16 and the u16 at +2 of a track run are still unknown. Buckets are cut to the
  4:3 view either way.
- Animated object triggers (global 0x85744c, the proximity threshold) and class-11/≥ 23 drawing.
- Resolved (renderer.md §13.6): the jolts come from the coupling minigame (0x409d0e, 0x409d74)
  and the buffer-stop crash (0x40a5ab).
- The gradient event: what 0x856fb0 measures (remaining time is a guess), whether the 0x40d340
  start changes the palette before the next drive restart. The overlay task 0x424740 is
  resolved in renderer.md §13.
- Panorama and ground: compare `sky_quads` / `ground_quad` with Wine screenshots (tile phase,
  vertical offset, corner/UV orientation against the bottom-up page rows).
- Station-result replay (§8): compare with a Wine screenshot of a result (viewport placement,
  monitor pose, the train's stop offset `0x4696cc`); whether the camera task really first
  updates on the tick after its creation; who reads the copy at 0x8502c0 (0x41c750). (Course
  event 12 and `0x855ff8 & 0x40000`: resolved, rules.md §7.5.)

# Scene assembly: track models, cab camera, visibility, background

Scope: what the driving view draws each frame and where. That covers the rail and track-side
models built from the `.COM` draw buckets, the in-cab camera, how scenery and track runs are
picked for drawing (and whether that depends on the 4:3 view), and the sky/ground background.
Addresses are VAs in `dengo3k.exe` (3000-bandai). Code: `crates/ddg-formats/src/scene.rs`.
Data tests: `crates/ddg-formats/tests/data_scene.rs`.

Confidence: **confirmed** = read from the code and checked against all data where that applies,
**likely** = one source or strong inference, **guess** = needs an experiment (oracle or Wine
screenshots).

## 1. Conventions

- **Units.** World units ≈ 10 m (routes.md §3). Part vertices are raw/256 (models.md §6).
- **Handedness.** The look-at builder 0x418420 maps the view direction to −z and the world
  vector `(−dz, dx)` to +x. The flip to +z is the separate VIEW transform, diag(1, 1, −1)
  (renderer.md §4). The camera matrix is a proper rotation, so **world space is right-handed**:
  looking along +z with y up, +x is on the left. In a port, `look_at_rh(eye, target, +y)` with a
  right-handed perspective reproduces the image without mirroring. **likely** (code). This
  conflicts with the "left-handed / negative volume" reading in models.md §6. Check the winding
  visually in the viewer (§8).
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
  too).
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

## 6. Background (0x41dfc0) — confirmed (code) unless noted

`mode` = area (0 Yamanote, 1 Akita, 2 Tōkaidō, 3 Hokuhoku, 4 Keihin). Everything is 2D, in
496×400 virtual units.

- **Gradient palette.** Palette 3 is built by 0x40eed0(R-index, variant) from the 12-byte rows at
  0x463b40 (variant 0) / 0x463bac (variant ≠ 0, from 0x857230; probably evening/night): sky
  top RGB, sky bottom, ground top, ground bottom. Entries 0..127 are the sky and 128..255 the
  ground, `start + k·(end − start)/128`, truncated. For example, R1 Yamanote has sky
  (152,177,227)→(178,193,226) and ground 40 grey; R2 Akita (213,213,238) with ground 200; R5
  Keihin (145,145,145)→(240,80,30), a sunset. `GradientColours::palette()` builds it. Colour
  words in the quads are `palette<<8 | index`: 0x301→0x37f is the sky, 0x380→0x3ff the ground.
  The flat-quad decoder (0x432b50) turns palette u32 bytes (1, 2, 0) into R, G, B.
- **Quads.** Sky gradient 0x469c68: (0,66)–(496,235), colours 0x469c60. Ground 0x469c90:
  (0,235)–(496,415), with flat colours 0x469c78 (gradient), 0x469c80 (all 0x380) or 0x469c88
  (all 0x3ff). Textured ground in area 3 uses 0x469ca0, (0,251)–(496,415).
- **Textured ground.** In areas 0 and 4, part 0x76f (area 0) or 0x283b (area 4) supplies the
  page. The UVs are replaced by the sky-quad corners × 16 (1/256 page units), so it is a static
  screen-space tiling. Whether u/v are swapped in 0x432d30 is a **guess**. Area 3 ground 2
  uses part 0x2dd2 with its own UVs.
- **Panorama sprites.** 12 rows at 0x469e6c `{part, w, h, repeats, v_offset, 127, flags}`. The
  texture is the first group of `part`, a 4-vertex quad: its rect and UVs (0x432d30). Per COM
  ≥ 2, 0x469fa4 gives `{count, s0, s1, s2, switch, switch2}`. COM < 2 uses sprite = area, and
  Keihin switches at 4419. Areas 1/2/4 draw `s0` before engine index `switch` and `s1` after.
  COM 15 copies s0 over s1. With count 3, row 9 is also loaded as a fourth sprite.
- **Panorama placement** (0x401600). The roll-free camera heading `atan2` is turned into
  `frac((heading/2π + 0.5)·repeats + phase)`, where phase = (flags>>8)/256. Tiles are drawn at
  offsets −1, 0 and +1 (loop to 1.1). The vertical anchor is the projection of the world point
  at y = 0, 128 units ahead horizontally (the horizon). Tile size on screen is (w − 1) ×
  (h − 1) virtual units, scaled by Px/Py to undo the anisotropic projection. So the strip
  scrolls by one tile width per 360°/repeats of heading. It is a slow parallax layer, not
  angle-true. **likely** (disassembly read, not run). For a wider view, draw enough tiles to
  cover the viewport.
- **Per mode:**
  - 0: panorama + sky gradient + textured ground (0x76f). On COM 4 at 1501–1589 the ground is
    at depth 0x7fee, which only changes ordering.
  - 1 (COM ≠ 7/10): tunnel probe A. If inside, **nothing**. Otherwise panorama (s0/s1) + sky
    gradient + flat ground gradient.
  - 1 (COM 7/10) and 3: per-point table (0x41d0e0, `{from, bits}` entries: COM 7 0x46a5b4 ×30,
    10 0x46a69c ×25, 11 0x46a204 ×58, 12 0x46a3dc ×58). Bits 0–2 select the sprite (4 = s1 + the
    fourth). Bits 3–5 select the ground (area 1: any → flat 0x380; area 3: 1 flat 0x380,
    2 textured 0x2dd2, 3 flat 0x3ff). Bits 6–7 turn on the sky gradient. Bit 8 runs the tunnel
    probe, whose "inside" result is then ignored. Bits 9–10 select an extra world quad
    (0x41df00, area 3): 4 world points at 0x469d24 / 0x469d54, CPU-projected and drawn flat in
    ground colour (likely a water surface).
  - 2: panorama (s0/s1) + sky gradient + flat ground gradient.
  - 4: panorama (s0/s1) + sky gradient + textured ground (0x283b).
- The clear colour stays black (0x000000). Fog colour is #BFBFBF (renderer.md §7).

`scene::background()` returns this plan per frame. `BackgroundTables` holds the tables.

## 7. API (`ddg_formats::scene`)

- `SceneTables::read(&exe, max_run_kind)`: every table above (rails, kinds, bands, eye
  heights, tunnel plugs, animated parts, background).
- `CabWalker::at_segment_start(route, stop)` / `::at(route, pos)` / `.advance(route, units)` /
  `.live()`: the camera walk with clamps and activation lists.
- `cab_camera(route, pos, eye_height, jolt) -> CabCamera { eye, target }`. Also the
  `DRIVING_FOV_Y_DEG`, `DRIVING_ASPECT`, `NEAR` and `FAR` constants.
- `track_instances(route, tables, ctx, pos, &camera) -> Vec<TrackInstance { part_id,
  transform: Affine, slot, source }>`, and `track_instances_wide(.., extra_buckets)`.
- `scenery(route, tables, ctx, &walker) -> Vec<SceneryInstance>`.
- `live_intervals`, `widen_intervals`, `live_at`, `ORIGINAL_TAN_HALF_FOV_X` (§5).
- `tunnel_probe`, `background`, `sky_sprite_slots`, `GradientColours::palette`.
- `SceneContext { area, com, station }` feeds the hard-coded exceptions.

## 8. Open questions

- The winding/handedness conflict with models.md §6: verify in the viewer (one quad of a
  known part, front face CCW with `look_at_rh`).
- 45° vs 30° driving FOV: confirm with the oracle (`0x84c150` while driving).
- The f32 at +16 and the u16 at +2 of a track run are still unknown. Buckets are cut to the
  4:3 view either way.
- Animated object triggers (global 0x85744c, the proximity threshold) and class-11/≥ 23 drawing.
- What fires the camera jolts (0x409d0e, 0x409d74, 0x40a5ab), and variant 1 of the gradient
  palette (0x857230).
- 0x432d30 u/v order for the textured ground, and the exact panorama vertex formula
  (0x401600; compare with Wine screenshots).

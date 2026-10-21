# Performance

How a frame is made, where its time goes, and how to measure it. Numbers are from an AMD laptop
with an RDNA 3.5 integrated GPU (RADV), a 3840×2160 120 Hz display, release builds, full screen.

## 1. Measuring

**Frame trace.** `SHUPPATSU_FRAME_TRACE=<file>` writes one tab-separated line per presented frame
(`shuppatsu_render::perf`): the frame's wall time, the interval since the previous frame, and the
CPU time of each part of it:

| Column | What |
|---|---|
| `tick` | the 33 ms ticks run in the frame (simulation, screens), without the compose inside them |
| `compose` | the 2D layer drawn on the CPU (`Composer::compose`); in GPU mode (§3) its draw list built |
| `upload2d` | 2D pixels copied to the GPU (`Overlay::sync`); in GPU mode the frame's data uploaded and the compose dispatched |
| `build3d` | the 3D frame's geometry built (scene compose, effects) |
| `upload3d` | that geometry copied to the GPU (`Renderer::set_geometry`) |
| `render` | render passes encoded and submitted |
| `acquire`, `present` | waiting for the surface and presenting |
| `movie` | the movie's decoding |

then the counters `ticks`, `composes`, `upload2d_kb`, `full_composes` (in GPU mode: frames that
changed, so were composed again on the GPU), the screen's name and the
2D canvas size. Sections are exclusive: a compose inside a tick counts as compose only. Work on
helper threads is inside the section that waits for it.

**Report.** `shuppatsu-tools frame-report [--hz 120] <trace>...` prints, per screen, the frame
intervals (average, p50, p99, max), the refreshes missed at the given rate (an interval of n
periods, rounded, misses n − 1), a histogram of intervals in periods, the CPU work per frame
(all sections but acquire and present) and each section's average / p99 / max.

**Composer check.** `SHUPPATSU_VERIFY_COMPOSE=1` composes every partial 2D update a second time in
full and panics at the first differing pixel (a debug aid: it costs a full compose per tick); in
GPU mode it checks every GPU-composed frame against a CPU compose (§3). `SHUPPATSU_COMPOSE=cpu`
or `gpu` picks where the layer is composed (default `gpu`). `SHUPPATSU_IDLE_WAIT=<ms>|poll` sets
how the event loop waits between redraws (§4; default 1 ms).

A measurement run, muted, in throwaway config and data directories, stopped after a time:

```sh
XDG_CONFIG_HOME=$(mktemp -d) XDG_DATA_HOME=$(mktemp -d) SHUPPATSU_FRAME_TRACE=out/t.tsv \
  timeout 30 shuppatsu --inputs <script> --fullscreen --mute
shuppatsu-tools frame-report out/t.tsv
```

On KDE Wayland a new window opens on the active output; make the display under test active
first (its refresh rate is what the report should be read against).

**What counts.** A frame whose CPU work stays under about 5 ms at 120 Hz is shown on time; above
that the compositor's deadline is often missed even though the period is 8.3 ms.

## 2. The 2D layer

The HUD and menus are the original's software sprite layer (docs/re/hud.md §1.1), composed on the
CPU into an RGBA canvas of 640×480 × the canvas multiple (up to 4), widened to the window's shape
(3413×1920 for a 16:9 4K window, 26 MB), and drawn over the 3D view as one texture
(`overlay.rs`).

**Retained compose** (`sprites/retained.rs`, `Composer`). The canvas is kept between composes.
Each compose lists the layer's draws in order — slots (layer 3 → 0, high → low), the widgets of
each layer after its slots, then the fans — with what decides their pixels: the slot or widget
itself, its anchor shift, the palettes it reads, and the bank's cached pictures for it (paper
fill, xBRZ image). Draws are matched by their place in the order; a draw that changed, appeared or
went marks its old and new boxes dirty. The dirty boxes are merged, cleared and redrawn with every
draw that reaches them, clipped to them, and the overlay uploads only those rectangles
(`Overlay::sync`; a canvas it does not hold, after a resize or another screen's canvas, goes up
whole). Outside the boxes the same draws cover each pixel in the same order, and each draw reads
only the pixel it writes, so the result equals a full compose. A change of the bank (its
`generation`), the canvas size, the shake offset or the backdrop colour redraws everything; so
does a dirty area over half the canvas (palette fades).

The bank's caches have state that depends on the calls (a paper fill rebuilt exactly when the
same palette comes twice; an xBRZ image built once a map's colours have been seen twice), so
`SpriteLayer::prepare` still asks them once per slot and compose, in draw order, as a full compose
did; drawing then reads the prepared results.

**Drawing.** All draws write through a clip rectangle (`Canvas::clip`) and a row offset
(`Canvas::row0`): a rectangle of more than 128 K pixels is drawn in up to 8 bands of whole rows,
each on its own thread. Tiles outside the clip are skipped whole, each art pixel's block is filled
by rows, the paper backdrop by runs, and text outlines are a separable dilation (they tested
(2s + 1)² neighbours per pixel: 7 ms for one station banner at 4K). Sprite widgets draw from
their map's art kept per bank generation.

**Caches built on first use.** Paper fills (the wide menu backdrops) are built on all cores and
kept for up to 32 palettes, so fades that come back are lookups; a fill scaled for a fade goes
through a 256-entry table. A fade-in still builds an exact fill whenever the palette passes
twice the brightness of the last one built (about six times in the title fade): what does not
depend on the palette — each fill pixel's three lattice taps and weights, the ramp and light
shares per column (a side's *shape*) — is kept per picture size and reach, built ahead on a
thread when the map is marked as paper (for the reaches of 16:10 and 16:9 canvases), so a build
only colours the exemplars and mixes them (4K: about 0.9 ms, 2 ms before; the first one 1.3 ms,
3.3 ms before). The arithmetic and its order are the same, so the fill is byte-identical. xBRZ
images (`hud_upscale`) are built on all cores.

**Translated text** is rasterised on all cores (`TextOverlay::build_with`), at start-up and
whenever the canvas multiple changes; the lettering analysis of the original art behind it (a
bitmap and summed-area-table shadow search) is kept for the process, so a window going full screen
re-rasterises in about 0.1 s (3 s before).

## 3. GPU 2D compositing

The retained CPU compose (§2) still redraws and uploads the whole canvas whenever a palette
changes, since every pixel's colour then changes: the attract fades composed 26 MB in about
3.7 ms and uploaded it in about 0.9 ms, and missed about 1.5 % of the refreshes. The default is
now to compose the layer on the GPU; `SHUPPATSU_COMPOSE=cpu` keeps the CPU compose, which stays
the reference (tests, `SHUPPATSU_VERIFY_COMPOSE`, and a fallback for a GPU without compute
shaders).

**What the CPU does** (`sprites/gpu.rs`, `Composer::compose` in GPU mode). The same
`SpriteLayer::prepare` as a CPU compose (so the bank's paper-fill and xBRZ caches see the same
calls), then a list of draw records in the CPU's draw order: slots layer 3 → 0, high → low, the
widgets of each layer after its slots, then the fans. A record is 32 words: its kind, the canvas
box it may change, the columns a framed slot keeps out of on its rows, and its parameters.
Nothing is drawn on the CPU per frame; the frame is compared with the previous one, and only a
different frame is composed again on the GPU.

| Record | Per-pixel data on the GPU | Kept |
|---|---|---|
| Slot (one per run of columns) | tiles and tilemaps as indices; xBRZ index images (`hud_upscale`); the regions of the translated text over it (art under them is not drawn) | bank art per art id; images while used |
| Paper fill | the fill's two sides, RGBA | per fill |
| Translated text part | base index, coverage, text colour index per pixel | per bank generation |
| Fill | — (one colour) | — |
| Triangle / fan | a byte mask, rasterised by the CPU's edge test | per frame |
| Label text | coverage, outline and backdrop bits (the CPU's mask and dilation) | by text, style and size |
| Legend | one coverage raster per group of blends (shape, rim, letters), recorded from the CPU's legend drawing | by legend, colours and place |
| Picture (logo board) | the scaled picture, RGBA | by picture and size |
| Resampled sprite | per pixel the id of its list of (index, weight) contributions; per palette each list's colour, coverage and its complement as the CPU's `f32` values | lists by map, rectangle and scale; colours by palette |

Palette-indexed art is coloured on the GPU from the frame's 128 palettes (rewritten only where
they changed), so a fade uploads palettes, not pixels.

**What the GPU does** (`gpu2d.rs`, `gpu2d.wgsl`). The canvas is split into 64 × 64 bins, each
with the list of records whose box reaches it and a hash of what its pixels are made of (its
records in order, the data they read, the palettes they colour with, the canvas size and
backdrop); only bins whose hash changed since the texture's last frame are composed again (the
needle moving recomposes a few bins, a palette fade the bins that use the palette). One compute
dispatch runs a thread per pixel of those bins: it starts from transparent, applies every
record of its bin that holds the pixel, in order, puts the backdrop colour under the design
rectangle (`put_backdrop`) and stores the result into the overlay's canvas texture (an
`Rgba8Unorm` storage texture, sampled through an sRGB view as before). Each record reproduces
the CPU's arithmetic exactly: whole canvas pixels per art pixel by integer division (no
sampling, so no filtering or texel-centre questions), index 0 transparent, the integer blends
of text (`(u·(255 − t) + f·t) / 255`), legends and pictures, and for resampled sprites the
CPU's `f32` products, kept from being fused into a multiply-add by an opaque bit operation,
with Rust's round-half-away. The presentation (§2, `overlay.rs`) is unchanged.

Data a record reads lives in a pool buffer, uploaded once and kept while it is used (by the
identity of the data); a full pool is emptied and filled with the current frame's data again.

**Checking.** `SHUPPATSU_VERIFY_COMPOSE=1` in GPU mode also composes each frame on the CPU from
the same prepared slots and compares the GPU's canvas, read back, with it byte for byte
(panicking at the first difference). A randomized test (`sprites/gpu_tests.rs`) composes random
layers with every kind of record on both and compares them; it is skipped without a GPU.

**Results** (CPU compose → GPU compose, same binary; 4K full screen on a 3840×2160 60 Hz monitor
and the 2560×1600 180 Hz panel, as the 120 Hz TV was not connected; CPU work does not depend on
the display):

| Screen (4K) | 2D upload per frame | `compose` p99 | Work p99 |
|---|---|---|---|
| Attract (palette fades) | 3.9 MB → 88 KB | 4.3–4.9 → 2.8–3.4 ms | 5.7–6.7 → 3.3–3.9 ms |
| Title, route select, board | 0.2–1.7 MB → 30–50 KB | 0.9–4.3 → 0.1 ms | 1.4–6.0 → 0.6–0.8 ms |
| Driving | 200–270 → 18 KB | 2.6 → 0.2 ms | 5.4 → 4.0 ms |
| Ending (staff roll) | 372 → 11 KB | 2.2 → 0.06 ms | 5.7 → 4.1 ms |

At 180 Hz the attract fades miss 0.5–1 % of the refreshes instead of 2–3 %, driving 0.1–0.3 %
instead of 0.6–0.7 %. The 68 reference shots are byte-identical in both modes, and the verify
mode found no difference in about 26 000 GPU frames over every reference session (with
`hud_upscale` and the first-start language screen).

On the 120 Hz TV (measured later, round 2): the attract misses 1.9–2.6 % with the CPU compose and
0.4–0.6 % with the GPU compose (work p99 4.4–5.1 → 1.7–2.2 ms), the title 0.4–1.2 % → 0–0.2 %,
driving 0.4 % → 0.1 %. What is left in the attract is not compose work (§6).

## 4. Frame pacing

With vsync the runs present with Fifo. A run's simulation advances by whole refresh periods of
the display's reported rate, phase-locked to the wall clock (`pacing.rs`, `FrameClock`): redraws
run ±0.5 ms (up to ±2 ms) off their refresh, and the 33 ms ticks and the interpolation between
them followed that jitter. An offset of more than half a period (a stall, a variable rate) jumps
to the wall clock; without vsync the clock is the redraws' own.

**Waiting between redraws** (`pacing.rs`, `IdleWait`). Polling (`ControlFlow::Poll`) kept the
main thread at 100 % of a core. Sleeping until the next event (`ControlFlow::Wait`) brought that
to about 6 %, but the core clocked down between frames: every part of the frame got slower and
driving missed 1.2 % of the refreshes instead of 0.1 %. The loop now sleeps
(`ControlFlow::WaitUntil`) until 1 ms before the next redraw is due — the previous redraw plus
the shortest of the recent intervals — and polls from there; a redraw long overdue (a hidden
window gets no frame callbacks) sleeps until an event. On the 120 Hz TV, 2 × 120 s of driving:
missed refreshes 0.1 % as with polling, frame work the same (1.15 ms on average), the process's
CPU time halved (68 against 130 s per 120 s), the main thread at 39 % of a core. Margins of 2 and
4 ms used more CPU for nothing; 0 (sleeping up to the redraw) made the frame work about 40 %
slower.

The cost: work that comes in one burst after idle frames runs on a core that has clocked down.
The stage-start tick takes about 40 % longer (the run after the board: a 35 ms interval instead
of 25 ms; the intro's start 50 instead of 38 ms) and the attract fade's paper builds about 70 %
longer (still well inside the period). `SHUPPATSU_IDLE_WAIT=poll` polls as before;
`SHUPPATSU_IDLE_WAIT=<ms>` sets another margin.

## 5. Run start

The tick that starts a run builds the run (`Play::new`): exe tables and the drive (2 ms), the
cab's translated captions (landmark names 3 ms, one-line captions 2 ms, board names 10 ms),
screen tables (1.6 ms), subtitles (3 ms), the route scene (14–22 ms, on its own thread) and the
GPU setup (4 ms).

- The departure boards' diagrams are analysed on all cores (`board_en.rs`), and the analysis is
  kept for the next start with the same bank state (its art id and generation) and names: a
  stage intro and the run after its board build the same. `SpriteBank::set_upscale`, called at
  every start, changes the generation only when the setting changes (it reset every cache keyed
  by the generation at each start).
- The route scene of the run after the departure board loads on a thread while the board shows
  (`play::load_scene_ahead`); `Play::new` takes it if its key (COM file, visibility, settings,
  geometry, stage, texture pack) matches, and loads it on its own thread otherwise. Texture
  pages convert to RGBA on all cores.
- The movie (`Komachi.DAT`, 33 MB: 13 ms to read, 13 ms to demultiplex) is read on a thread at
  start-up and again after each showing (`movie::MovieAhead`), so it starts without a stall; the
  demultiplexed movie stays in memory.

The tick that starts the run after the board went from about 28 to 16 ms (polling); the intro's
start (no board before it) is unchanged at about 27–32 ms.

## 6. Results

Missed refreshes at 120 Hz on the TV, 4K full screen (windowed 2560×1440 where marked), default
settings (GPU compose, idle wait), English; the first measurement, the first round's end, and
round 2's end (out/perf/round2.md):

| Screen | Before | Round 1 (CPU compose, polling) | Round 2 |
|---|---|---|---|
| Title menu | 40 % | 0.1 % | 0–0.2 % |
| Options / port settings | 37 / 41 % | under 0.5 % | 0.2 / 0.2 % |
| Route select | 20 % | 0.4 % | 0 % |
| Driving, enhanced HUD / original HUD | 22 / 23 % | 0.1 % / 0.1 % | 0.1 / 0.1 % |
| Coupling minigame | 22 % | 0.1 % | 0.1 % |
| Pause | 25 % | 0.1 % | 0 % |
| Driving report | 20 % | 0.1 % | 0.1 % |
| Ending (staff roll) | 20 % | 0.1 % | 0.2 % |
| Attract (palette fades) | 8 % | about 1.5 % | 0.4–0.6 % |
| Title menu, windowed | 20 % | 0.2 % | 0 % |
| Driving, windowed | 12 % | 0.1 % | 0.1 % |

Most of what remains comes from outside the frame's work, in every run: the window's switch to
full screen (two intervals of 16–25 ms about 0.25 s after it opens, the wait in `acquire`), and
an interval of 16–18 ms every 5.0 s (600 frames) with no CPU work in the frame, also with
MangoHud off — the compositor or the system. Within the work: the stage-start tick (§5), the
first frame of the 3D view after a start (4–7 ms of `build3d`), and the first frame of the title
menu (3–4 ms of GPU draw-list building for the translated menu items, first-touch page faults of
their packed text).

The reference shots (every screen, English and Japanese, 1440p and 4K) are byte-identical before
and after each change, in both compose modes.

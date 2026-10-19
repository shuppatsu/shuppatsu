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
| `compose` | the 2D layer drawn on the CPU (`Composer::compose`) |
| `upload2d` | 2D pixels copied to the GPU (`Overlay::sync`) |
| `build3d` | the 3D frame's geometry built (scene compose, effects) |
| `upload3d` | that geometry copied to the GPU (`Renderer::set_geometry`) |
| `render` | render passes encoded and submitted |
| `acquire`, `present` | waiting for the surface and presenting |
| `movie` | the movie's decoding |

then the counters `ticks`, `composes`, `upload2d_kb`, `full_composes`, the screen's name and the
2D canvas size. Sections are exclusive: a compose inside a tick counts as compose only. Work on
helper threads is inside the section that waits for it.

**Report.** `shuppatsu-tools frame-report [--hz 120] <trace>...` prints, per screen, the frame
intervals (average, p50, p99, max), the refreshes missed at the given rate (an interval of n
periods, rounded, misses n − 1), a histogram of intervals in periods, the CPU work per frame
(all sections but acquire and present) and each section's average / p99 / max.

**Composer check.** `SHUPPATSU_VERIFY_COMPOSE=1` composes every partial 2D update a second time in
full and panics at the first differing pixel (a debug aid: it costs a full compose per tick).

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
through a 256-entry table. xBRZ images (`hud_upscale`) are built on all cores.

**Translated text** is rasterised on all cores (`TextOverlay::build_with`), at start-up and
whenever the canvas multiple changes; the lettering analysis of the original art behind it (a
bitmap and summed-area-table shadow search) is kept for the process, so a window going full screen
re-rasterises in about 0.1 s (3 s before).

## 3. Frame pacing

With vsync the runs present with Fifo and the event loop polls (`ControlFlow::Poll`). A run's
simulation advances by whole refresh periods of the display's reported rate, phase-locked to the
wall clock (`pacing.rs`, `FrameClock`): redraws run ±0.5 ms (up to ±2 ms) off their refresh,
and the 33 ms ticks and the interpolation between them followed that jitter. An offset of more
than half a period (a stall, a variable rate) jumps to the wall clock; without vsync the clock is
the redraws' own.

Polling keeps one core busy between frames. Waiting instead (`ControlFlow::Wait`) brings that to
about 6 % but made every part of the frame slower (the core clocks down between frames) and
driving missed 1.2 % of the refreshes instead of 0.1 %, so the loop polls.

## 4. Run start

A run's start reads the departure boards' diagrams on all cores (`board_en.rs`); the frame that
starts a stage takes about 45 ms (95 ms before), most of it the route scene and the cab's
translated captions.

## 5. Results

Missed refreshes at 120 Hz, 4K full screen (windowed 2560×1440 where marked), default settings,
English, before the work and after it:

| Screen | Before | After |
|---|---|---|
| Title menu | 40 % | 0.1 % |
| Options / port settings | 37 / 41 % | under 0.5 % |
| Route select | 20 % | 0.4 % |
| Driving, enhanced HUD / original HUD | 22 / 23 % | 0.1 % / 0.1 % |
| Coupling minigame | 22 % | 0.1 % |
| Pause | 25 % | 0.1 % |
| Driving report | 20 % | 0.1 % |
| Ending (staff roll) | 20 % | 0.1 % |
| Attract (palette fades) | 8 % | about 1.5 % |
| Title menu, windowed | 20 % | 0.2 % |
| Driving, windowed | 12 % | 0.1 % |

The reference shots (every screen, English and Japanese, 1440p and 4K) are byte-identical before
and after each change.

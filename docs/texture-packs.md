# Texture packs

Enhancement `texture_pack` (docs/settings.md): replacement images for the 3D texture pages,
loaded instead of the pages of `tex*.rom`. Unset (the default) draws the original textures.
A pack is derived from the user's own game data (or drawn by hand): it lives outside the
repository (`out/texpacks/`, `~/.cache/ddg`, …) and is never committed.

## Using a pack

```sh
ddg-game --texture-pack out/texpacks/xbrz                # or DDG_TEXTURE_PACK=…
ddg-game --texture-pack out/texpacks/xbrz --texture-filtering
```

or `texture_pack = "/path/to/pack"` in `config.toml`. The settings screen shows it under
3D & effects; like the font lists it is changed only in the file. It applies from the next
stage: the pages are read when a stage (route textures `texRC.rom`, `texR<n>.rom`) and its
trains (`texTR<n>.rom`, also the attract opening's and the bonus stage's sets) load.

Sampling stays the original's nearest neighbour unless `texture_filtering` is on; with an
upscaled pack, filtering is the usual choice (CPU-built mipmaps also make the larger pages
shimmer less in the distance).

## Format

Code: `crates/ddg-formats/src/texpack.rs` (format, QOI codec), `crates/ddg-game/src/texpack.rs`
(loader), `GameData::add_textures` in `crates/ddg-game/src/scene.rs`.

```text
<pack>/<tex file stem, lowercase>/page<NN>.qoi     texrc/page00.qoi, texr1/page17.qoi, textr2/page03.qoi
<pack>/pack.toml                                   optional, informational (how it was made)
```

* [QOI](https://qoiformat.org/qoi-specification.pdf), 3 or 4 channels, non-premultiplied
  RGBA. Chosen because it is lossless, needs no new dependency (a 200-line codec) and most
  image tools read it (GIMP, ImageMagick, ffmpeg).
* **Alpha is the transparency.** The original's colour key (RGB565 `0x0000`, renderer.md §9;
  the only transparent value, every other colour including near-black `0x0001` is opaque)
  becomes alpha 0; anything with alpha ≥ 128 is drawn, so a pack can have opaque pure black.
  Transparent texels may keep a colour: smooth filtering blends towards it instead of black.
* **Stored row order**: the first row of the image is the first row of the page in
  `tex*.rom`, which is the bottom of the picture (tex.rs; the game uploads rows unchanged and
  its UVs address that layout). A pack page therefore looks upside down in a viewer
  (`ddg-tools export textures` writes upright PNGs for looking at).
* **Size**: the original page's times a whole factor, the same on both axes (×1, ×2, ×3, ×4,
  …; `scale_of`). Pages of one pack may use different factors. UVs are normalised to the page
  (`TexRect::map_uv`), so nothing else changes: rects, wrapping and the 2D panorama and ground
  sprites (which use the same pages) map onto the larger page as they did onto the small one.
* A page without a file keeps the original; a file that does not decode, or whose size is not
  a whole multiple, is reported on stderr and the original is kept.

## Making a pack: `ddg-tools upscale-textures`

```sh
ddg-tools upscale-textures --method xbrz --scale 4                      # out/texpacks/xbrz
nix shell nixpkgs#realesrgan-ncnn-vulkan -c \
  ddg-tools upscale-textures --method esrgan --model realesrgan-x4plus    # out/texpacks/esrgan-realesrgan-x4plus
nix shell nixpkgs#waifu2x-converter-cpp -c ddg-tools upscale-textures --method waifu2x
```

Options: `--scale N` (default 4), `--out DIR` (default `out/texpacks/<method>`),
`--model` (esrgan: `realesrgan-x4plus`, `realesrgan-x4plus-anime`, `realesr-animevideov3`),
`--noise 0..3` (waifu2x denoise, default 1), `--mask smooth|nearest`, `--pad N` (default 2 for
xbrz/nearest, 8 for the AI methods), `--only texr1,textr2`, `--tool PATH`, `--keep-work`.
Code: `crates/ddg-tools/src/upscale.rs`, `crates/ddg-tools/src/xbrz.rs`.

Methods:

* `nearest`: plain enlargement (a pack identical in look to the original; for testing).
* `xbrz`: an xBRZ-style edge-directed pixel-art scaler written for this tool (xbrz.rs): the
  xBRZ corner classification on a 4×4 neighbourhood, with blend weights computed as the exact
  sub-pixel areas beyond each edge shape, so any factor works. CPU, all cores.
* `esrgan`: `realesrgan-ncnn-vulkan` (Vulkan, GPU). The x4plus models scale ×4 only; a
  smaller `--scale` is box-filtered down from ×4.
* `waifu2x`: `waifu2x-converter-cpp` (OpenCL; falls back to the CPU). On a Mesa system the
  GPU is reached through rusticl: `OCL_ICD_VENDORS=$(nix build --print-out-paths
  nixpkgs#mesa.opencl)/etc/OpenCL/vendors RUSTICL_ENABLE=radeonsi` (radeonsi for AMD).

Per page the tool:

1. **Regions**: the whole page, padded by wrapping (tiled textures repeat across the page),
   then every distinct texture rect (`rect_pixels`: 1/256-page units rounded outwards),
   padded with **its own edge pixels**, so atlas neighbours do not bleed across rect borders.
   Rects are pasted over the whole-page result, largest first.
2. **Colour fill**: inside each padded region the colour-keyed texels get the mean colour of
   their already filled 8-neighbours, repeated outwards from the opaque texels (a dilation),
   so no upscaler sees black next to a cut-out edge (no dark halos).
3. **Upscale** the colour by the method (the AI tools run once over a directory of all tiles).
4. **Alpha** is rebuilt from the original key mask only: enlarged by the xBRZ scaler on a
   black/white image and thresholded at one half (`--mask smooth`, rounded stair steps), or by
   nearest neighbour (`--mask nearest`). Transparency stays binary and crisp; the AI output
   never decides it.
5. The padding is cut off and the region pasted at its scaled place; transparent texels keep
   their filled colour with alpha 0.

Tests: `ddg_formats::texpack::tests` (QOI round trip, bad input, paths and factors),
`texpack::tests` in ddg-game (a synthetic pack replaces pages, keeps UVs, rejects bad
sizes), `upscale::tests` (rect pixels, regions, padding without neighbour bleed, colour fill
and alpha, mask smoothing) and `xbrz::tests` in ddg-tools.

## HUD sprites: comparison only (`ddg-tools upscale-sprites`)

There is no runtime pack for the 2D sprites (`cg.rom` tiles, `tbl.rom` tilemaps, the exe's
palettes; re/hud.md) yet. This tool writes contact sheets that compare upscalers on the HUD
art, plus whole frames recomposed with each method, so a method can be chosen first:

```sh
DDG_SHOT_LAYER=1 ddg-game play 0 --lang ja --hud-layout enhanced --size 2560x1440 \
    --shot 420:$PWD/out/explain/upscale_hud/frame/wide_t420.ppm   # also .world.ppm, .layer.txt
nix shell nixpkgs#realesrgan-ncnn-vulkan nixpkgs#waifu2x-converter-cpp -c \
  ddg-tools upscale-sprites --frame out/explain/upscale_hud/frame/wide_t420.layer.txt
```

Output (default `out/explain/upscale_hud/`): `sheet_x2.png`, `sheet_x4.png` (a row per sprite
group, a column per method, 1:1), `rows/<group>_x{2,4}.png` (the same rows one file each),
`zoom_x4.png` (crops magnified twice more), `frames/<shot>_<method>.png` (window size) and
`<shot>_crops.png` (HUD corners at 1:1, the first row being the game's current output).

* `DDG_SHOT_LAYER=1` (debug aid, `shot.rs`): a `play` shot also writes the frame without the
  2D layer (`.world.ppm`) and the sprite layer as text (`.layer.txt`: canvas, visible slots in
  draw order with their anchor shifts, the palettes they use as stored, the rect-layer fans).
  The tool recomposes the canvas from it and presents it like the overlay (sharp bilinear,
  alpha test at one half, premultiplied blending in linear light). Checked: with nearest
  neighbour at the game's own multiple the recomposition differs from the game's shot in
  38 of 3.7 M pixels (the needle's edge). Translated text widgets are not recomposed (shoot
  with `--lang ja`).
* Methods: `nearest` ×2/×4; sharp bilinear (the overlay with the original art: canvas ×4
  shown at 1600 px height, ×3.33); `xbrz` (colour, as for textures); **`xbrz-indexed`**
  (new: the xBRZ edge shapes, but each output pixel keeps the palette index of its own
  source pixel or of the neighbour beyond the edge when more than half of it lies there:
  no new colours); **`scale2x`/`scale4x`** (AdvMAME2x on indices, twice for ×4);
  Real-ESRGAN `realesrgan-x4plus`, `realesrgan-x4plus-anime`, `realesr-animevideov3` (×2, ×4);
  waifu2x-converter-cpp (its bundled anime-style-art models) with noise 0 and 1, ×2 and ×4.
* Transparency: index 0 is the only transparent value. Index methods keep it exactly. Colour
  methods dilate the opaque colours into the transparent pixels before upscaling and rebuild
  the alpha from the index-0 mask alone (xBRZ on black/white, threshold one half), so no halo
  and binary alpha, as for textures.

### Palette effects: a constraint for any HUD pack

The sprite layer is palettised at run time (`SpriteLayer::compose` looks every pixel up in the
slot's palette each frame), and the HUD relies on it:

* **Palette swaps**: the same art in palettes 4/5/6 (clock, remaining-time and distance
  digits change colour when late, at ≤ 14 s, at an overrun, for passing stations; re/hud.md
  §3.2–3.4), vehicle pictures in 9/10.
* **Animated entries**: the seven-segment speed readout (tbl 0x53) is lit by rewriting
  palette 4 entries 0xdb..0xef every tick (§3.5); the route strip's arrows are entries
  0x50..0x5f (green progress, the current segment blinking every 10 ticks, §3.7).
* **Fades**: none on the driving HUD; the title and attract screens fade palettes 0x15–0x17
  in from black (menus.md), and the sky's palette 3 fades are 3D only.

So a pack cannot simply be one RGBA image per tilemap. Options: (1) an **index** pack (the
indices at ×N, `xbrz-indexed` or `scale4x`): compose stays a palette lookup and every swap,
animation and fade keeps working unchanged; (2) RGBA per (tilemap, palette) for art drawn
with fixed palettes only, with a fade applied as a multiply (fades from black are linear in
colour, exact for linear scalers, close for AI output) and the animated maps (0x53, the route
strips) kept indexed. AI output blends lit and unlit segments and neighbouring palette
entries, so it cannot be recoloured per entry.

### Runtime integration (not done)

* Packs per tilemap at a whole factor N: slots address 16-px tile rects (`col, row, w, h`),
  which map to 16·N-px rects; translated text (`text.rs`), the `wide_menus` repeat and paper
  fills work on art pixels and would need the same factor (or keep the original art).
* The canvas multiple (`ui_scale`, `overlay::auto_multiple`) must match N or the art be
  resampled: smooth gives ×3 at 1440 and ×4 at 1600/2160, integer ×3 at 1440 and ×4 at
  2160. A ×4 pack on a ×3 canvas has to be shrunk (or the canvas raised to ×4 and the
  overlay shrinks it by 0.75 with plain bilinear, as the recomposed frames show: softer than
  the native ×3). Best: build packs at ×3 and ×4 (index methods are instant) and pick by
  the multiple; nearest blocks remain for multiples above the pack.
* Colour methods would make the canvas compose RGBA sprites with binary alpha; index
  methods need only larger index tiles (`SpriteBank` per tilemap instead of per 16×16 tile).

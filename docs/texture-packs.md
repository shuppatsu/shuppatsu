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

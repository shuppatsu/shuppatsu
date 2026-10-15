# Model parts (parts*.rom): mesh format

Scope: the byte layout of an unpacked model part, how the game turns it into Direct3D vertices,
how UVs combine with `tex*.rom` rectangles, and what a port needs to draw a part correctly.
Addresses are VAs in `dengo3k.exe` (3000-bandai; file offset = VA − 0x400000).

Confidence tags follow loaders.md: **confirmed** = read from code *and* checked against all the
data, **likely** = one source only, **guess** = inference.

Parser: `crates/ddg-formats/src/part.rs` (`Part::parse`, `Part::parse_entry`, `TexRect::map_uv`).
Data test: `every_part_parses_exactly` in `crates/ddg-formats/tests/data.rs`.

## 1. Container recap

- `parts*.rom` is a ROM2 container (loaders.md §2). Each entry is `u16 partId` followed by a
  packed block (loaders.md §4a): `u32 unpackedSize, u8 0x73, u8 passes, u16 0`, then data at +8.
  With `passes = 0` the data is stored raw. `part_get_unpacked` @0x432840 tests the byte at
  block+5 to choose between using the raw data at block+8 and running `lz_unpack_multipass`
  @0x425af0. **confirmed**
- The stream described below is the unpacked block. **confirmed**

## 2. Unpacked layout — confirmed

All values little-endian. There is no header besides the group count, and nothing after the last group.

```
off  size            field
0    u16             nGroups
2    groups[nGroups] (variable size, packed back to back, no padding or alignment)

group:
+0   u16             rectId      global texture-rectangle id (tex*.rom record, loaders.md §3)
+2   u16             nVerts
+4   nVerts × 10 B   vertex:  i16 x, i16 y, i16 z, i16 u, i16 v
     u16             nIdx
     nIdx × index    u8 when nVerts ≤ 255, u16 when nVerts > 255
```

Evidence:
- Code: 0x432917–0x432957 reads rectId/nVerts and locates nIdx at `verts + nVerts·10`.
  0x432aa1 (`cmp nVerts, 0xff; jg`) selects the index width. The byte path widens each u8 to a u16.
  The u16 path copies `2·nIdx` bytes. The source pointer moves by exactly the bytes consumed,
  so there is no padding between groups, even after an odd number of u8 indices.
- Data: all **13 087** parts in all 15 `parts*.rom` files parse and consume their unpacked
  length exactly. That is 37 736 groups, 497 336 vertices and 314 511 triangles. Every index is
  less than nVerts, every nIdx is a multiple of 3, and every vertex is referenced. Seven groups
  have more than 255 vertices (the largest has 287) and use u16 indices. 21 triangles are
  degenerate (repeated index), which is harmless. Groups per part range from 1 to 28: 4 873 parts
  have 1 group and 2 998 have 2.
- Every rectId used by a part exists in some `tex*.rom`.

## 3. Runtime expansion (for reference only) — likely

`part_get_unpacked` caches its result in the model heap 0x796480..0x816480. When the heap is full
it calls `unpacked_parts_flush` @0x4327e0 and retries. Records grow upward from the cursor
0x8248fc and vertices are allocated downward from 0x82491c:

- part header: `u32 sourcePtr` (to restore the 0x817c7c slot on flush), `u32 nGroups`;
- group record: `u16 rectId, u16 nVerts, u32 vertexPtr, u16 nIdx, u16 idx[nIdx]`. The record is
  `10 + 2·(nIdx | 1)` bytes long, which keeps every record 4-byte aligned. The renderer's
  "padded to even" sub-mesh records (renderer.md §8) are these records.
- vertex: 32 bytes, `D3DFVF_LVERTEX` (FVF 0x1e2).

A port doesn't need this layout. It only explains how the drawn values are derived (§4, §5).

## 4. Vertex values — confirmed (code)

From the per-vertex loop 0x43298c–0x432a93:

| LVERTEX field | Value |
|---|---|
| x, y, z | `i16 / 256.0` (float 0x3b800000 at 0x47c95c) |
| reserved | 0 |
| diffuse | **0xFFFFFFFF**, a constant (opaque white). The store is at 0x4329d5. |
| specular | 0 (SPECULARENABLE is off anyway) |
| tu, tv | rect-mapped UV, §5 |

- **No per-vertex colour and no lighting.** Parts have no normals, and the draw call passes
  D3DDP_DONOTLIGHT (renderer.md §8). With TEXTUREMAPBLEND = MODULATE and a white diffuse, the
  pixel colour is the texel colour (shading is baked into the textures), followed by fog.
  **confirmed**
- Texel 0x0000 (pure black) is transparent through the colour key, with COLORKEYENABLE = 1 and
  no alpha blending (renderer.md §6/§9). In a port this is an alpha test or discard on
  RGB565 == 0. **likely** (from renderer.md)

## 5. UVs and texture rectangles — confirmed (code + data)

A part stores rect-relative UVs, with 256 meaning one rect width or height. The rect record
(`tex*.rom`: `u16 rectId, u16 page, u8 u, u8 v, u8 w, u8 h`) sits in the global table 0x825144
(6 B per id: page + page base, u, v, w, h; written at 0x432200, loaders.md §3).

The code at 0x432a41–0x432a90 computes D3D texture coordinates as

```
tu = ((w + 1) · (u_part / 256) + u_rect) / 256
tv = ( h      · (v_part / 256) + v_rect) / 256
```

- **Asymmetry:** the U axis uses `w + 1` (it loads byte +4 and runs `inc`), but the V axis uses
  `h` (it loads byte +5 without `inc`). Read literally, V spans one 1/256 step less than the rect.
  This is the original's behaviour, and `TexRect::map_uv` reproduces it. **confirmed** (code).
  It is visually negligible: under 0.4 % of a full page.
- **The rect fields are in 1/256-of-page units, not texels.** D3D texcoords are normalised, so a
  rect covers the same fraction of the page whatever the page's pixel size. Data: every page
  smaller than 256 px (16, 32, 64 or 128) has only full-page rects (0, 0, 255, 255). 512×512 pages
  use the same 0..255 rect grid, for example (0, 48, 63, 63) on a 512 page = 128×128 px.
  loaders.md §3 describes w/h as "127 = 128 px". That is only true for 256-px pages. **confirmed** (data)
- The most common (w, h) values among the groups are (255, 255) ×19 577, (127, 127) ×7 466,
  (63, 63) ×5 982, (127, 63) ×1 301 and (31, 31) ×1 235. These are power-of-two sizes stored as
  size − 1. **confirmed**
- **Tiling relies on WRAP addressing.** Part UVs range from −256 to 12 800. 78 409 of the
  497 336 vertices lie outside 0..256. Most of them (15 914 groups) map onto full-page rects of
  small pages, which repeat under D3D's default `D3DTADDRESS_WRAP`. The game never sets an address
  mode (renderer.md §6), so WRAP is in effect. 190 groups have out-of-range UVs on a *sub*-rect of
  a 256-px page, such as rect (192, 64, 63, 63) with u up to 2048. Under WRAP these sample the
  neighbouring parts of the page, and a port must do the same (repeat sampler) to match. **confirmed**
  (code formula + data). Whether that wrap is intentional is a **guess**.
- The rect's page selects the texture: handle = `0x824920[page + pageBase]` (renderer.md §8).
  Pages are uploaded at their native size. A port should do the same and keep normalised UVs.
- Id resolution order: parts use "first loaded wins" per part id, while rect records use "last
  loaded wins" per rect id (loaders.md §3/§5). Rect ids such as 9, 1366 and 2651 occur in several
  area tex files. Only one area set is loaded at a time, so a port should resolve a part's rects
  against RC + the current area + the current TR sets, in the game's load order. **likely**

## 6. Geometry conventions

- **Scale:** raw coordinates are 1/256 of a world unit (positions use the same 1/256 float as
  UVs). **confirmed** (code)
- **Axes:** x is lateral, y is up and z is longitudinal (along the track or car). All
  `partsTR2..8` parts have y ≥ 0 (the car stands on y = 0), x is symmetric (±36..±51 raw) and z
  spans about ±256..±325 raw. **likely** (data)
- **Size of a unit:** a train car spans about 650 raw in z. A 20 m car then gives 32 raw ≈ 1 m,
  so 1 world unit ≈ 8 m. That fits fog 1→10 units (≈8–80 m), far plane 32 (≈256 m) and the
  scenery cull radius 10 (renderer.md). **guess**: the route/placement scale (stages/route docs) should confirm it.
- **Handedness and winding:** the projection is left-handed (renderer.md §4) and CULLMODE =
  D3DCULL_CW. Clockwise triangles are culled, so faces drawn as visible wind **counter-clockwise
  on screen**. Data check: of the parts whose edges form a closed, consistently oriented
  manifold (12), all 12 have a negative signed volume Σ a·(b×c) in the raw coordinates. That is
  exactly what CCW-from-outside in a left-handed frame produces. **likely**
  - For a port: keep the index order. Draw with front face = CCW and back-face culling, either in
    the game's left-handed frame or after converting to right-handed by negating z. The mirror
    in that conversion keeps the on-screen orientation, so the same rule applies.
- Parts carry no normals, bounding volumes or LOD data. Culling uses object placement
  (renderer.md §10).

## 7. Built-in replacement part (exe 0x48c39c) — confirmed (bytes) / likely (purpose)

- When the current COM index is 6, 8 or 9 (akita1a, akita1c, akita2a), `part_get_unpacked`
  substitutes the block at 0x48c39e for part 0x1153 (0x432888–0x4328b6). The bytes at 0x48c39c are a
  normal parts entry: `u16 0x1153` and then a packed block (unpacked size 0x91, marker 0x73,
  1 pass). It decodes with the same parser to 3 groups (rects 228, 2526, 1732; 11 vertices;
  5 triangles). The part 0x1153 in `partsR2.rom` has 5 groups and is a different, larger
  piece. The replacement is a hard-coded data fix for those three Akita segments.
- A port should read this entry from the user's exe at runtime (rule 4) and apply the same
  substitution. It must not be committed.

## 8. Rendering checklist for a port

1. Unpack (lz), then `Part::parse`.
2. Per group: positions = raw / 256. Colour = white. UVs = `TexRect::map_uv(rect[rectId])`. Bind
   the texture page `rect.page`. Use a repeat sampler (nearest filtering for arcade fidelity,
   renderer.md §6).
3. Discard texels with RGB565 == 0. Depth test and write on. No blending.
4. Front face CCW, cull back. Apply linear fog per renderer.md §7.

## Open questions

- The world-unit-to-metre scale (§6) needs confirming from the route data or the speed/odometer code.
- Whether the 190 out-of-range sub-rect groups look intentional in the original. Compare
  screenshots under Wine once the oracle exists.
- Whether any driver-level texture filtering (bilinear) was common on period hardware. That is
  an enhancement choice, not a format question.

## Texture orientation (added in Phase 2 review)

Texture pages in `tex*.rom` are stored bottom-up. `0x432200` copies them row for row into the
surface, so in GPU memory the picture is upside down, and the model UVs from `TexRect::map_uv`
address that layout directly. **confirmed** (loader code, plus exported pages are upright only
after a vertical flip). For rendering, upload the rows in stored order and use the UVs as mapped,
with no flip. `TexPage::to_rgba8_upright` is for viewing only.

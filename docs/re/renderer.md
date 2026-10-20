# dengo3k.exe: renderer setup, projection/camera, drawing

Scope: DirectDraw/Direct3D device setup, display modes, projection and camera, viewport, fog,
render states, draw-call style and vertex formats, texture upload, culling and draw distance,
and the 2D/HUD path. Sources: the Ghidra decomp in `re/decomp/dengo3k/`, plus `objdump -d -M intel`
of `BIN/dengo3k.exe` for the constant tables that Ghidra folded away. VA = file offset + 0x400000.
All constants were read from the exe bytes.

Confidence: **confirmed** means read directly from the code or bytes. **likely** means a strong
inference. **guess** means plausible but needing an experiment (an oracle hook or a Wine run).

The CD check (`FUN_00425ca0`/`00425bf0`/`00425bc0`) was not looked at.

## Function table

| Address | Suggested name | Role | Conf. |
|---|---|---|---|
| 00426fe0 | `CFWDriverList::CFWDriverList` | `DirectDrawEnumerateExA(cb=0x427160, ctx, flags=7)` builds the driver list | confirmed |
| 00427160 | `CFWDriverList::EnumDriverCb` | Per DDraw driver: `DirectDrawCreate(guid)`, QI `IDirectDraw4`, QI `IDirect3D3`, `IDirectDraw4::GetCaps` (HAL+HEL, 0x17c bytes each), `IDirect3D3::EnumDevices(cb=0x427420)` | confirmed |
| 00429950 | `ComPtr<IDirectDraw4>::QueryFrom` | QI with IID at 0x47e228 = `IID_IDirectDraw4` {9c59509a-39bd-11d1-8c4a-00c04fd930c5} | confirmed |
| 004299d0 | `ComPtr<IDirect3D3>::QueryFrom` | QI with IID at 0x47e548 = `IID_IDirect3D3` {bb223240-e72b-11d0-a9b4-00aa00c0993e} | confirmed |
| 00429fb0 | `Gfx_CheckDirectX6` | `DirectDrawCreate(NULL)` + QI `IDirectDraw4`. If this fails, WinMain shows "DirectX6 or later is not installed" | confirmed |
| 00428810 | `Gfx_Get` | Returns the renderer singleton (`0x816618`) | confirmed |
| 00427be0 | `Gfx_CreateDevice` | Picks the driver and device, sets the cooperative level and mode, creates primary/back/Z surfaces, then the device and viewport | confirmed |
| 00428820 | `Gfx_InitDevice` | Calls `Gfx_CreateDevice`, sets 8 default render states, sets VIEW = diag(1,1,-1,1) | confirmed |
| 004285a0 | `Gfx_Release` | Error/cleanup path of `Gfx_CreateDevice` | likely |
| 00427b90 | `Gfx_RestoreSurfaces` | `IDirectDraw4::RestoreAllSurfaces` + `IDirect3D3::EvictManagedTextures` (after device loss) | confirmed |
| 004286b0 | `Gfx_Present` | Windowed: `Blt` back to primary client rect. Fullscreen: `Flip(0, DDFLIP_WAIT)`. Then `IDirect3DViewport3::Clear2(TARGET\|ZBUFFER, colour=+0x138, z=1.0)` | confirmed |
| 00428990 | `Gfx_SetClearColour(r, gb)` | `+0x138 = (r&0xff)<<16 \| (gb&0xffff)`. Seen values: 0x000000 and 0x707070 | confirmed |
| 004287c0 | `Gfx_SetTexture(tex2)` | Cached `IDirect3DDevice3::SetTexture(0, tex)` (`+0x134` = current texture) | confirmed |
| 00428ab0 / 00428b20 | `ScopedRenderState` ctor / dtor | `GetRenderState`, save, `SetRenderState`; the dtor restores | confirmed |
| 004289c0 / 00428a50 | `ScopedSurfaceLock` ctor / dtor | `IDirectDrawSurface4::Lock(NULL, desc, flags, 0)` / `Unlock(NULL)` | confirmed |
| 00428b80 | `Gfx_SetFog(mode)` | mode 0: defaults + FOGENABLE=0. mode 1: linear table fog from the registry values (only if FOGENABLE) | confirmed |
| 00425070 | `FogTask_Create` | Creates a task and calls `Gfx_SetFog(1)`. Its destroy callback (0x425220) calls `Gfx_SetFog(0)` | confirmed |
| 004254b0 | `RenderTask_Create` | Initialises the camera block `0x84c100..0x84c15c` (near/far/FOV/aspect) and creates the render task | confirmed |
| 00425680 | `RenderTask_Update` | Per frame: BeginScene, per-view-mode viewport + projection + look-at, run every task's draw callback, `Frame_End` | confirmed |
| 004183b0 | `Camera_SetProjection(fovTenthsDeg, aspect, near, far)` | Converts to 4.12 fixed point, calls `Gfx_SetProjection`, stores CPU projection scales | confirmed |
| 00431ee0 | `Gfx_SetProjection(s16 invAspect_4_12, s16 tanHalfFov_4_12)` | Builds the matrix (via 0x4262a0/0x4261e0), `SetTransform(PROJECTION=3)` | confirmed |
| 004262a0 | `Mat_PerspectiveFovLH(fov, aspect, n, f)` | h = 2n·tan(fov/2), w = h·aspect, then 0x4261e0 | confirmed |
| 004261e0 | `Mat_PerspectiveLH(w, h, n, f)` | Same layout as D3DX `PerspectiveLH` | confirmed |
| 00418420 | `Camera_LookAt(eye xyz, target xyz, roll)` | Builds the view matrix `0x856be0`, its inverse `0x856c20`, and copies view to the "current model-view" `0x856ba0` | confirmed |
| 004185f0 | `memcpy` (inlined 0x40-byte matrix copy helper) | | likely |
| 004186a0 | `ModelView_PushTRS(x, y, z, rotA, rotB)` | Multiplies a model transform into `0x856ba0` | likely |
| 00417f90 / 00418000 / 00418080 / 00418100 | `Mat_Translate` / `Mat_RotX` / `Mat_RotY` / `Mat_RotZ` (in place, sin/cos args) | | likely (axis names: guess) |
| 00417f20 | `Camera_Reset` | Identity view matrices, projection scales = 1.0, matrix-stack depth 0x20 | confirmed |
| 00431f70 | `Model_Draw(modelId)` | `SetTransform(WORLD=1, 0x856ba0)`, then per textured sub-mesh `SetTexture` + `DrawIndexedPrimitive(TRIANGLELIST, D3DFVF_LVERTEX, …, D3DDP_DONOTLIGHT)` | confirmed |
| 00432030 | `Model_DrawInstanceAtViewOrigin(modelId)` | `GetTransform(WORLD)`, replaces the translation row with `0x856bd0..d8`, then draws as above | confirmed (purpose: guess) |
| 00432840 | `Model_GetConverted(modelId)` | Converts or caches a model from `parts*.rom` into the vertex/index cache `0x796480..0x816480` | likely |
| 00432200 | `Tex_LoadSet(index, isTrainSet)` | Loads `tex*.rom` pages into managed RGB565 textures and `parts*.rom` models | confirmed |
| 00432170 | `Tex_ReleaseAll` | Releases every `IDirect3DTexture2` in `0x824920[0..0x1ff]` and clears the model caches | confirmed |
| 00432b50 | `Draw2D_FlatQuad(xy[], colours[], n, depth)` | Untextured TLVERTEX fan in 496×400 virtual space (gradients, fills) | confirmed |
| 00432da0 | `Draw2D_TexQuad(xy[], uvtex[], n, depth)` | Textured TLVERTEX fan in 496×400 virtual space | confirmed |
| 00431ec0 | `Draw2D_*` (third variant) | Same family, not analysed | guess |
| 00432f40 | `Project4ToScreen(out, pts)` | CPU projection of 4 world points into virtual-screen coordinates, using `0x856b80/84` | confirmed |
| 00401600 | `Sky_DrawPanorama(sprite, view, invView, scroll)` | Background strip scrolled by camera yaw; 3 tiles at −1, 0, +1 | confirmed |
| 0041dfc0 | `Background_Draw(task, mode)` | Sky panorama + sky gradient quad + ground quad per scene type | confirmed |
| 0041d400 | `Track_DrawBufferStop(slot, model)` | Horizontal-distance cull (r² ≤ 100), then draws a buffer-stop model n + 1 times along the last chord of a run (scene.md §2.8; earlier named `Scenery_DrawIfNear`) | confirmed |
| 00436fd0 | `FramePacer_BeginFrame` | Covered in `main-loop.md`. Device-lost handling + `BeginScene` | confirmed |
| 004370a0 | `Frame_End` | Full-screen viewport, sprite layer, rect layer, `EndScene`, `Gfx_Present` | confirmed |
| 00436c80 | `Viewport_SetDefault3D` | Virtual viewport centre (248,226), half-size (248,174) | confirmed |
| 00436eb0 | `Viewport_Set(cx, cy, hw, hh)` | Same, explicit values | confirmed |
| 00436cb0 | `Viewport_Apply` | Maps the 496×400 virtual rectangle to pixels, `IDirect3DViewport3::SetViewport2` | confirmed |
| 00430ec0 | `Sprite_Init` | Loads `tbl.rom` (sprite maps) and `cg.rom` (tiles), builds a 16×16 pitch-offset table | confirmed |
| 00430fb0 | `Sprite_RenderLayer` | Locks the back buffer and software-blits all sprites (4 priority layers) | confirmed |
| 004316e0 | `Sprite_Set(slot, image, x, y, palette, layer, cb)` | Sprite slot table `0x816f88`, 16 bytes per slot | confirmed |
| 00431790 / 004318c0 | `Sprite_Clear(slot)` / `Sprite_SetImage(slot, image)` | | confirmed |
| 004319b0 | `RectLayer_Render` | Untextured TLVERTEX rect fills (Z off, cull off) + black borders when 640×480 2D is centred at a higher resolution | confirmed |
| 004376b4 / 004376d9 | MFC `CDC::ExtTextOut` / `CDC::TextOut` | Not called by game code (no direct xrefs) | likely |

## 1. Interfaces: DirectDraw 4 + Direct3D 3 (the DirectX 6 API) — confirmed

- `IID_IDirectDraw4` at 0x47e228, `IID_IDirect3D3` at 0x47e548, `IID_IDirect3DHALDevice`
  {84e63de0-46aa-11cf-816f-0000c020156e} at 0x47e578, `IID_IDirect3DTexture2`
  {93281502-8cf8-11d0-89ab-00a0c9054129} at 0x47e5f8. Only `DDRAW.dll` is imported; Direct3D is
  reached through QueryInterface.
- The vtable offsets match DX6 exactly. Examples: `IDirect3D3::FindDevice` +0x1c with a 0x5c-byte
  `D3DFINDDEVICESEARCH` and a 0x20c-byte result; `IDirect3D3::CreateDevice` +0x20 with 4 args (the
  DX7 version has 3); `IDirectDraw4::GetDeviceIdentifier` +0x6c with a 0x428-byte buffer;
  `IDirect3DViewport3::SetViewport2` +0x44 with a 0x2c-byte `D3DVIEWPORT2`; `Clear2` +0x50.
- Renderer singleton at `0x816618`. Field offsets: +0x11c `IDirectDraw4`, +0x120 primary surface,
  +0x124 back buffer, +0x128 `IDirect3D3`, +0x12c `IDirect3DDevice3`, +0x130 `IDirect3DViewport3`,
  +0x134 current texture, +0x138 clear colour.
- The framework class names `CFWDriverInfo`/`CFWDeviceInfo`/`CFWDisplayModeInfo` (RTTI strings)
  belong to the enumeration objects built by 0x426fe0/0x427160 (likely).

## 2. Driver/device selection (0x427be0) — confirmed unless noted

1. Driver: the first enumerated DDraw driver. Option `-secondryv` (bit 10, 0x400 of the option
   word `DAT_00816840`) uses the second one, for Voodoo-style add-on cards (likely).
2. It walks that driver's D3D device list looking for the HAL GUID. On the way it records the
   device's texture size limits: `DAT_00816758` = min width, `0x816610` = min height,
   `0x816760` = max width, `0x81675c` = max height. These are words 0x32..0x35 of the device record.
   That they are `dwMin/MaxTextureWidth/Height` is likely.
3. `FindDevice(D3DFDS_GUID, HAL)`. If the hardware description has no flags (no HAL), surfaces
   are created in system memory (`DDSCAPS_SYSTEMMEMORY`, 0x800). Otherwise they go in video memory
   (0x4000). The flag is kept in `DAT_00816754`.
4. `GetDeviceIdentifier` is called, but the result is not used for anything visible here.
   Option `-squareonly` (0x800) sets bit 0x20 at renderer+0xa4, which forces square textures.
5. If FindDevice fails: message "デバイスが見つかりません。" ("device not found", 0x487118),
   followed by an exception.

## 3. Display mode, windowing, surfaces — confirmed

- Resolution table, indexed by `DAT_00816af4`: widths at 0x48c1e0 = {512, 640, 800, 1024},
  heights at 0x48c1f0 = {384, 480, 600, 768}. The default index is 1 (640×480, set in WinMain at
  0x429bf0). Options `-vga_640x480`/`-vga_800x600`/`-vga_1024x768` (bits 21/22/23) select 1/2/3.
  Index 0 (512×384) is not reachable through options. All four modes are 4:3.
- Options (0x42b700): the registry string value `Option` of
  `HKLM\Software\TAITO\電車でGO！２ 高速編 ３０００番台` is tokenised, and each `-name`/`/name`
  token sets its bit. After that, `..\bin\dengo3k.ini` section `[default]` (key = option name,
  nonzero = on) is ORed in. Name table at 0x47c7a0 (24 names; bit = index). DengoSetting.exe
  writes `-squareonly`, `-texmax128`, `-secondryv`, `-vga_800x600` into `Option`.
- **Fullscreen** (default, `DAT_00816d80 = 1`): `SetCooperativeLevel(hwnd, FULLSCREEN|EXCLUSIVE|FPUSETUP)`
  (0x811), then `SetDisplayMode(w, h, 16, 0, 0)`. Flipping primary
  (`PRIMARYSURFACE|FLIP|COMPLEX|3DDEVICE`, `BACKBUFFERCOUNT=1`); the back buffer comes from
  `GetAttachedSurface(BACKBUFFER)`.
- **Windowed** only with `-debug` (0x4000): `SetCooperativeLevel(0, NORMAL|FPUSETUP)` (0x808). The
  desktop must be 16 bpp (`GetDeviceCaps(BITSPIXEL)==16`), otherwise "Display mode ERROR" (0x487104)
  with the Japanese text at 0x487060: set High Color 16-bit, needs ≥16 MB VRAM, Voodoo2 does not
  work. Primary + clipper, offscreen `3DDEVICE` back buffer of the mode size, and the window is
  resized to fit the client area.
- **Colour depth: always 16-bit.**
- **Z-buffer**: separate surface, `DDSCAPS_ZBUFFER`, `DDPF_ZBUFFER` 16-bit (mask 0xffff),
  attached to the back buffer.
- Present (0x4286b0): `Flip(NULL, DDFLIP_WAIT)` or `Blt` to the client rect. The clear for the
  next frame happens right after the present. It is `Clear2` with rect {0,0,W,**W**}: the bottom
  edge is set to the width, a harmless bug.

## 4. Projection — confirmed

- Camera parameter block (globals): `0x84c150` FOV in **tenths of a degree, vertical**;
  `0x84c138` aspect (width/height of the 3D viewport, in virtual units); `0x84c134` near;
  `0x84c14c` far. Eye at `0x84c11c/120/124`, target at `0x84c10c/110/114`, roll at `0x84c118`.
  A second eye/target pair for the split view is at `0x84c100..108` / `0x84c13c..144`.
- Initial values (`RenderTask_Create` 0x4254b0): FOV = int at 0x47c3e0 = **300 → 30.0°**;
  aspect = int at 0x47c3e4 (142) × 0.01 (double at 0x469750) = **1.42**; near = **0.05**
  (0x3d4ccccd, at 0x4255e7); far = **32.0** (0x42000000, at 0x4255f1). Near and far are written
  nowhere else, so they are constant.
- `Camera_SetProjection` (0x4183b0): t = tan(fov · π/1800 · 0.5) (doubles at 0x467740 = π/1800 and
  0x457730 = 0.5). The two args are converted with `__ftol` (truncating) to **4.12 fixed point**:
  `tanHalfFov = trunc(t·4096)` and `invAspect = trunc(4096/aspect)` (4096.0 at 0x467738).
  `Gfx_SetProjection` turns them back into floats (1/4096 at 0x47c958), so the projection
  carries that quantisation. For example, 30° gives tan 0.267822 instead of 0.267949, and
  aspect 1.42 gives 4096/2884 = 1.42025.
- Matrix (0x4261e0, row-vector D3D convention): m00 = 2n/w, m11 = 2n/h, m22 = f/(f−n),
  m23 = 1, m32 = −n·f/(f−n), everything else 0. With h = 2n·tanHalf, w = h·aspect, this gives
  m11 = 1/tanHalf (**fixed vertical FOV**) and m00 = 1/(tanHalf·aspect). It is a left-handed,
  w-friendly perspective matrix (m23 = 1), so table fog works in eye-space depth (see §7).
- CPU copies for 2D projection: `0x856b80` = 1/(aspect·t) and `0x856b84` = 1/t, both unquantised.
- VIEW transform = diag(1, 1, −1, 1), a Z flip (0x428820, also re-set in 0x428b80). The real
  camera matrix is folded into WORLD: `Model_Draw` sets WORLD = `0x856ba0` = model × view.
  PROJECTION is set only by `Gfx_SetProjection`.
- FOV/aspect values written by game code (immediates):

| Site | FOV | Aspect | Context |
|---|---|---|---|
| 0x4254b0 (init) | 30.0° | 1.42 | default |
| 0x41c3a2 / 0x41c3ac | 30.0° | 1.42 | in `FUN_0041c3c0` (camera preset reset; likely the driving view) |
| 0x41b7dc / 0x41b7e6 | 45.0° | 1.28 | end of `FUN_0041b540` |
| 0x402f82, 0x41cb33 | 45.0° | – | with view mode 2 (fixed camera) / `FUN_0041cb00` |
| 0x4033cf, 0x41b534 | 30.0° | – | |
| 0x403540 | 16.0°, 12.0°, 22.0° | – | attract/demo cameras (likely) |
| 0x41ac29 | 40.0° | – | `FUN_0041a7b0` |
| 0x425680 mode 1 | 45.0° / 75.5° | 1.02 / 1.00 | split view: two viewports |
| 0x425680 mode 2 | 45.0° | 1.25 | fixed look-at |

  The drive path stores 45.0° at 0x41cb33 (in 0x41cb00, segment start), and nothing else on that
  path writes 0x84c150. The aspect stays 1.42. So the in-cab view is most likely **45° vertical,
  aspect 1.42** (scene.md §3). Confirm with the oracle hook on `0x84c150`.

## 5. Viewport and the 496×400 virtual screen — confirmed

- All 3D viewports and all TLVERTEX 2D quads use a **496×400 virtual screen** that is
  stretched non-uniformly to the full mode: `px = x·W/496` (0x1f0, and 1/496 at 0x47c964),
  `py = y·H/400` (1/400 at 0x47c960).
- Viewport descriptor: centre `0x849d64,0x849d5c`, half-size `0x849d68,0x849d60`.
  Default (`0x436c80`): centre (248, 226), half (248, 174), so the 3D view is
  **x 0..496, y 52..400**: 496×348 virtual = aspect 1.425 ≈ the 1.42 projection aspect. The top
  52 virtual rows (13 %) are outside the 3D viewport. At 640×480 that is pixels (0,62)–(640,480),
  i.e. 640×418.
- **Non-square result:** the projection aspect (1.42) is in virtual units. On a 4:3 screen the
  viewport is 1.532 wide in pixels, so the original draws the world **≈7.9 % wider than it is
  tall** (1.532/1.42). A faithful port has to reproduce this; a "square pixels" mode would be
  an enhancement. This is a fidelity-vs-enhancement decision.
- Special case (0x436e14): if `DAT_00816afc != 0` and the viewport is exactly the full virtual
  frame (centre (248,200), half (248,200); 248.0 at 0x47c978, 200.0 at 0x4676c4), the viewport is
  instead 496×400 **unscaled pixels** at x = (W−480)/2 + 16, y = (H−400)/2 + 16 (16.0 at
  0x47cbec). This is a 1:1 mode used at higher resolutions. Flag `0x855ff8 & 0x2000` forces
  a full-screen viewport (0,0,W,H).
- `D3DVIEWPORT2` clip volume is always X −1, Y 1, W 2, H 2, Z 0..1.
- `Frame_End` resets the viewport to the full mode size before drawing the 2D layers.

## 6. Render states — confirmed

Set once after device creation (pair table built on the stack at 0x42884a), and set again by
`Gfx_SetFog(0)`:

| State (DX6 number) | Value |
|---|---|
| TEXTUREMAPBLEND (21) | 2 = MODULATE |
| SHADEMODE (9) | 2 = GOURAUD |
| DITHERENABLE (26) | 1 |
| CULLMODE (22) | 2 = D3DCULL_CW |
| SPECULARENABLE (29) | 0 |
| ZENABLE (7) | 1 |
| ZWRITEENABLE (14) | 1 |
| COLORKEYENABLE (41) | 1 |

- I scanned every `call [reg+0x58]` (SetRenderState) and `[reg+0xa0]` (SetTextureStageState) in
  game code. There are none besides the ones listed here. **ALPHABLENDENABLE, SRC/DESTBLEND,
  TEXTUREMAG/MIN and mip filtering are never set** (confirmed absence). Filtering is the device
  default (DX6 default: nearest, likely), there are no mipmaps, and transparency is colour key
  only (§9).
- 2D passes use `ScopedRenderState`: CULLMODE = 1 (NONE) for the 2D quads; ZENABLE = 0 and
  ZWRITEENABLE = 0 for the rect layer.

### 6.1 Depth: 16-bit Z, LESSEQUAL, draw order, and coplanar track layers

User report: shimmer at crossovers and where grass meets the ballast, with every enhancement
on or off. Shots, crops and counts: `out/explain/track_zfight/` (gitignored).

**What overlaps — confirmed (data, keihin1a.com engine index 4964).** Every rail part
(e.g. 0x2d6b–0x2d6d) has vertices only at file y = −5, 0 and 3 (ballast foot, the bed with the
sleepers painted on, the rails), and the track-side kind parts (e.g. 0x24b3, 0x24b8) lie at
y = −5. On level track every piece is placed at y = h/4096 with no pitch (scene.md §2.3–§2.7),
so at a crossover the two tracks' beds are exactly coplanar, and so are the grass strips and
the ballast foot they overlap. Their triangles differ, so the interpolated depths differ only
by rounding.

**Cause in the port — confirmed (experiment).** The port stored that depth in a reversed
32-bit float buffer, fine enough to resolve the rounding: which surface won changed from pixel
to pixel and with every camera move (jagged interleaved patches). It is independent of
`max_lod`, `early_activation`, `draw_distance` and `distance_fade` (each off, and all off:
the same patches; skipping the main line or the kind runs in a debug build removes the
crossover or the grass-edge patches respectively), and of `msaa`, `ground_detiling`,
`ground_variation`; those only shade or sample the surfaces.

**What the original does — confirmed (code) / likely (behaviour).** It never sets ZFUNC
(§6: the only render states are the eight of `Gfx_InitDevice`), so the DX6 default
D3DCMP_LESSEQUAL applies; its Z buffer is 16-bit (§3); it draws part by part, sub-mesh by
sub-mesh, in the scene's order (`Model_Draw` 0x431f70; main line, bucket runs, scenery,
scene.md §2.1). Coplanar surfaces then land on the same 16-bit step almost everywhere and
the later one drawn wins; only a pixel where the rounding straddles a step boundary can
differ. Experiment: storing D3D z rounded to 16 bits in the port's shader removed the patches
at the Keihin crossover, at a Hokuhoku crossover (world coordinates ≈ 2200), at the Shinagawa
start of stage 11 and on the grass edges. A Wine capture of stage 11 standing at tick 300
(`scripts/capture-original.sh zfight11 2 0`, wined3d on llvmpipe with its D16 surface) agrees
better with the 16-bit rendering on the overlap pixels (mean difference 0.154 against 0.205
for the float one), which is the "likely": llvmpipe is not 1999 hardware, and 640×480 is too
coarse to see the pattern directly. The rounding of 16-bit z (nearest or truncated) is a
**guess** (nearest); it does not change which surface wins.

**Port (`DepthSteps`, `stepped_depth` in shader.wgsl).** The world pass writes its own depth:
- the step of the fragment's depth: `D16` = the original's 16-bit D3D z (`fine_depth` off), or
  `Fine` = the eye depth in steps of 1/1024 octave (0.068 % of the distance: 22 cm at 320 m,
  87 cm at 1280 m) with `fine_depth` on (default). 16-bit steps grow with the square of the
  distance (3 m at 320 m, 12 m at 640 m), which matters once `draw_distance` reaches further;
- inside the step, a place set by the instance's draw order (`Vertex::order`: the frame's
  instances in the order the scene adds them, the original's order): the later instance
  nearer. That gives LESSEQUAL-with-draw-order results although the batches are drawn per
  texture page (drawing in the exact order instead took ~6 500 draw calls a frame, +2.3 ms of
  CPU at 4K; this costs nothing extra). Float precision leaves at least ~250 places per step
  from the cab's eye height outwards, so neighbouring instances may share a place and then
  fall back to page order; overlapping layers are far apart in the order;
- `Fine` also chooses the later instance's step as if it were up to 0.01 % nearer
  (`FINE_ORDER_BIAS`), above the rounding between coplanar surfaces, so the one-pixel slivers
  along step boundaries go too (1e-5 left a few). `D16` keeps the original's slivers.
- Vertices are stored relative to the cab eye (`FrameGeometry::origin`, transformed in f64),
  which makes the GPU's rounding small even far from the world origin.

Result (1280×800, two frames 1 mm apart, pixels changed by > 10 %, Keihin crossover):
before 5 194, after 1 991 (the rest is nearest-sampling texture swim; with `anisotropic 16`
to remove the swim: 2 702 before, 101 after). Original mode (`--no-max-lod
--no-early-activation --draw-distance original --no-distance-fade --no-fine-depth`): 5 607
before, 1 744 after. Tests: `camera::tests::depth_steps_merge_coplanar_and_keep_layers`,
`fine_depth_lets_the_later_coplanar_instance_win`, `d16_steps_are_the_originals`,
`world::tests::vertices_carry_their_instance_order`, `positions_are_relative_to_the_origin`.

## 7. Fog — confirmed

- Registry values (strings, parsed with `atof`): `FOGENABLE` → `DAT_00816830`, `FOGTABLESTART` →
  0x48754c, `FOGTABLEEND` → 0x487550, `FOGTABLEDENSITY` → 0x487554. The exe's built-in initial
  values are 1.0 / 10.0 / 0.5, but they are always overwritten by the registry read (a missing
  value parses as 0, likely).
- `DengoSetting.exe` writes `FOGENABLE` = "1"/"0" (checkbox 0x3ef) and always the fixed strings
  `FOGTABLESTART`="1.0", `FOGTABLEEND`="10.0", `FOGTABLEDENSITY`="0.5" (its .data 0x41a118 /
  0x41a104 / 0x41a0f0; code at 0x401944–0x4019c9). Effective values: **linear fog 1.0 → 10.0**.
- `Gfx_SetFog(1)` (only if FOGENABLE): FOGENABLE(28)=1, FOGCOLOR(34)=**0xBFBFBF**,
  FOGTABLESTART(36), FOGTABLEEND(37), FOGTABLEDENSITY(38), FOGTABLEMODE(35)=3 (**LINEAR**).
  The density is unused in linear mode. Fog is enabled by a task created in `FUN_00424740` via
  0x425070 and disabled when that task dies.
- Because the projection is w-friendly, D3D6 HALs that support w-fog (`D3DPRASTERCAPS_WFOG`)
  interpret start/end as eye-space depth (**1..10 world units**). Others use device z, where
  start/end 1.0/10.0 would mean no visible fog (guess). The port should use eye-space linear fog.
- Port: FOGENABLE defaults to on (decided enhancement `snow_fog`, §13.9); `--no-fog` or
  `fog = false` in `config.toml` gives the fresh-install behaviour (no fog).

## 8. Draw calls and vertex formats — confirmed

- 3D models: `DrawIndexedPrimitive(D3DPT_TRIANGLELIST=4, FVF 0x1e2, verts, nVerts, idx, nIdx,
  D3DDP_DONOTLIGHT=8)`. FVF 0x1e2 = `D3DFVF_LVERTEX` (XYZ, reserved dword, diffuse, specular,
  one UV set; 32 bytes). The vertices are pre-lit and D3D transforms them; D3D lighting is
  never used. One call per textured sub-mesh, preceded by `SetTexture`. Texture handle =
  `0x824920[ 0x825144[page·3] ]`. Sub-mesh records: `u16 tex, u16 nVerts, u32 vertsPtr,
  u16 nIdx, u16 idx[nIdx]` padded to even.
- 2D: `DrawPrimitive(D3DPT_TRIANGLEFAN=6, FVF 0x1c4, …, 8)`. 0x1c4 = `D3DFVF_TLVERTEX`
  (XYZRHW, diffuse, specular, UV). rhw = 1.0, z = depth·(1/32768) (3.05e-5 at 0x47c968).
  Backgrounds use depth 0x7ffe (≈1.0, far). UVs are texel/256 (0.00390625 at 0x47c95c), so
  texture pages are addressed as 256-texel units. Colours come from a 32-bit palette at
  `0x829d5c` (high byte of the 16-bit colour < 0x80), or from an inline ARGB4444 expanded to
  8888 (≥ 0x80).
- No vertex buffers, no `DrawPrimitiveVB`, no strided calls.
- Model-view matrix stack: `0x856380`, depth counter `0x85637c` (starts at 0x20, 64-byte entries).

## 9. Textures (0x432200) — confirmed

- Pairs: `partsRC.rom`+`texRC.rom`, `partsR1..R5` + `texR1..R5` (route sets), and
  `partsTR1..TR9` + `texTR1..TR9` (train sets). Name tables at 0x47c8e0 / 0x47c8f8 / 0x47c910 /
  0x47c934.
- `tex*.rom` (after the generic loader 0x42fe40): `u16 nPageMap, u16 nPages`, then nPageMap
  8-byte records (`u16 id, u16 page, u8,u8,u8,u8` = placement inside the page, stored at
  `0x825144 + id·6`), then per page `s16 width, s16 height, u16 pixels[w·h]`.
- **Pixel format: raw 16-bit RGB565** (`DDPF_RGB`, 16 bpp, masks F800/07E0/001F), copied row
  by row with no conversion. Surface caps: `DDSCAPS_TEXTURE` + `DDSCAPS2_TEXTUREMANAGE`, flags
  include `DDSD_CKSRCBLT` with a zeroed key, so **colour 0x0000 (pure black) is transparent**
  (with COLORKEYENABLE=1).
- Size: rounded up to a power of two between the device min and max. `-texmax128` caps at 128
  (256 if the source is ≥1024 wide / ≥512 tall); `-texmax256` caps at 256; `-squareonly` forces
  square. If the rounded size differs from the source, the page is uploaded to a temporary
  surface and **stretch-blitted** (`Blt`, DDBLT_WAIT) into the texture, so the hardware samples
  a resampled image. A port should upload pages at their native size and keep UVs in 1/256
  units.
- The texture interface is `IDirect3DTexture2` (QI at 0x47e5f8). Up to 512 handles in
  `0x824920`.

## 10. Culling and draw distance

- There is **no CPU frustum culling in the paths traced here**. D3D clips against the
  projection frustum, which follows the projection aspect. A wider aspect therefore widens the
  clipped frustum automatically (confirmed for the model path).
- `Track_DrawBufferStop` (0x41d400, buffer stops only, scene.md §2.8): draws only if the horizontal (XZ) distance² from the eye
  (`0x84c11c`, `0x84c124`) is ≤ **100.0** (double at 0x458b38), i.e. radius **10 units**. That
  equals the fog end (10.0). A radial test like this does not depend on aspect (confirmed).
- Far plane 32.0, near 0.05 (world units; the metre scale is unknown, guess ≈ train-length units).
- Track and scenery selection is resolved in scene.md: precomputed per-point lists, with no
  runtime yaw test, **but cut to the original 4:3 view**. Hor+ needs the widening of scene.md §5.
- **Sky panorama** (0x401600) is a 2D strip, not geometry. Its horizontal scroll = yaw/2π
  (1/2π at 0x457738) plus a per-sprite offset. Its on-screen scale uses the viewport half-size
  × `0x856b80/84`. It draws exactly 3 tiles (offset −1, 0, +1; loop bound 1.1 at 0x457710). With
  a much wider FOV, 3 tiles may not cover the screen; the port should tile until the viewport
  is covered.
- Sky/ground fill quads (0x469c68 / 0x469c90 / 0x469ca0) span virtual x 0..496 (sky y 66..235,
  ground y 235..415). In the port they must be stretched to the widened viewport.

## 11. 2D / HUD / text — confirmed

There are three 2D layers, drawn in this order by `Frame_End` (0x4370a0) after the 3D tasks and
before `EndScene`:

1. **TLVERTEX quads in 496×400 virtual space** (`Draw2D_*`, called from task draw callbacks):
   background gradients, textured panels.
2. **Software sprite layer** (`Sprite_RenderLayer` 0x430fb0): locks the back buffer and blits
   sprites in 4 priority passes (layer byte 3 → 0), up to 0xcf slots. Sprite image =
   `tbl.rom` entry (`u8 wTiles, u8 hTiles, u16 tile[]`); tiles are **16×16 8-bit indexed,
   compressed** in `cg.rom` (type 1: RLE runs/literals; type 2: LZ-style back-reference). Index 0
   is transparent. Palettes are 256 × 32-bit at `0x829d60 + pal·0x400`, converted per frame to
   555 or 565 depending on the back-buffer green mask. A result of 0 is bumped to 1 so it stays
   opaque. Positions are in **native pixels**. Some positions are per-resolution tables, e.g.
   x at 0x47ca48 = {80, 80, 112, 890} and y at 0x47ca58 = {144, 144, 179, 45}. Screen-shake
   offsets come from the bytes at `0x856358/59` unless `0x855ffc & 0x800`.
3. **Rect layer** (`RectLayer_Render` 0x4319b0): untextured fans from a list at `0x816b00`
   (count `DAT_00816af8`), in 640×480 pixel units.

- **640×480 centring**: when `DAT_00816afc != 1` and `0x855ffc & 0x1000`, the sprite and rect
  layers are offset by ((W−640)/2, (H−480)/2), and 4 black border quads are drawn. This is a
  built-in pillarbox/letterbox for the 2D screens at 800×600/1024×768. It is a natural model
  for the port's HUD anchoring.
- **Text is not GDI.** `TextOutA`/`ExtTextOutA` appear only inside MFC `CDC` wrappers
  (0x4376b4/0x4376d9) with no direct xrefs from game code, and `CreateFont*` is not imported.
  On-screen text, including Japanese, is pre-rendered tile art in `cg.rom`, placed through
  `tbl.rom` sprite maps (confirmed for the render path; that every visible string is a sprite
  is likely). Consequence for translation: English text has to be supplied as replacement
  sprite art or drawn by the port's own font renderer over the sprite IDs.

## 12. Frame flow summary (render side)

`RenderTask_Update` (0x425680): `FramePacer_BeginFrame` decides whether this tick is drawn
(33 ms cadence, see `main-loop.md`). It handles device loss (`TestCooperativeLevel`, then
`Gfx_RestoreSurfaces`) and calls `BeginScene`. Then, by view mode `DAT_0084c128`:

- 0: default viewport, projection, look-at, all draw lists.
- 1: two viewports. Used only by the station-result replay (scene.md §8).
- 2: fixed camera.
- 3: debug free camera, moved with inputs `0x48c42c` bits.

Then `Frame_End`: full viewport, sprite layer, rect layer, `EndScene`, `Gfx_Present` + clear.
Draw lists are the 9 task lists at `0x84c180..0x84c1ec`; each task's draw callback is at +0x10
(`TaskList_Draw` 0x425ac0).

## 13. Weather and cab effects in the driving view

Source: disassembly of 0x424740–0x425232 (the Ghidra output loses the x87 operands of these
functions), 0x40e370 / 0x40e3d0, and the callers named below. Port: `shuppatsu/src/weather.rs`
(state and quads), `shuppatsu-render/src/effects.rs` + `Renderer::effects` (the pass).

### 13.1 Who creates what — confirmed (code)

`0x424740(type)` builds the weather overlay on task list 5 (0x84c1bc). `type` is `0x857210`, set
per stage by 0x40d040: 5 for stages 0–2 and 17–19, 4 for 3–4, 2 for 7, 1 for 8–11, 0 otherwise.
It sets `0x84c3a0 = type + 1` (the event's "active" word), clears the wiper flag `0x84c2e0`,
caches the four drop textures (parts 0x299e–0x29a1 from 0x47c3a8 into 0x84c240) and four parts
from 0x47c3b8 (0x2ae3–0x2ae0 into 0x84c300; **nothing reads that cache**, confirmed by scanning
every absolute reference). Then:

| type | wiper shape (draw fn) | swing table | extra |
|---|---|---|---|
| 0 | 0 (0x424950) | 0x47c340 / 0x47c344 / 0x47c350 | 10 drops |
| 1 | 1 (0x424aa0) | 0x47c358 / 0x47c35c / 0x47c360 | 10 drops |
| 2 | 2 (0x424cb0) | 0x47c380 / 0x47c384 / 0x47c388 | 10 drops |
| 3 | 1 | as 1 | none (no stage uses it) |
| 4 | 0 | as 0 | snow layers + fog (0x425070) |
| 5 | 2 | as 2 | snow layers + fog |

So rain (types 0–2, with the rain sound 0x28) is Hokuhoku, Keihin, Tōkaidō and Yamanote; snow
(4–5, silent) is Akita. Callers of 0x424740: the drive (re)start 0x4080d0 when the level
`0x857230 > 0` (it then sets `0x84c2e0 = 1` if the level is above 199), course event 0x15
(0x40acd0), the station-reached start 0x40d620, and the obstacle-replay exit of 0x40a0a0 (level
> 1). List 5 is cleared (0x425380) when the station result starts and when the driving loop is
left (0x4080d0), so the overlay is gone during result screens and comes back at the next
segment start while the level is above 0. Creating it at a segment start also sets 0x84c3a0
again; if the event had already timed out, the next driving tick of 0x40c4e0 clears it again.

### 13.2 Wiper task (update 0x4248d0, kill 0x424940, draw 0x424ee0) — confirmed

State: counter +0x30, angle +0x3c (f32, starts at the swing minimum), step +0x40, direction
+0x44 (starts 0). Per tick: `angle += dir·step` (stored as f32); if `dir > 0` and
`angle > max`: `dir = −dir`; else if `dir == 0` or `angle < min`: `dir = 0x84c2e0` (0 or 1).
So the wiper only starts while `0x84c2e0` is set (level > 199, cleared in tunnels by 0x40c4e0)
and parks just below the minimum when it is clear. The task ends when `0x84c3a0 == 0`; its kill
callback clears `0x84c3a0` and `0x84c2e0` (**0x84c2e0 has no other reader**: it is the wiper
flag, not a physics input). Swings (radians, from the table): shape 0 86°–112° by 1.5°/tick,
shape 1 15°–90° by 5°, shape 2 40°–140° by 2°.

Shapes, in 496×400 virtual units, every value truncated by `__ftol` and stored as a 16-bit word
(`T()` below). Colours: 0x47c31c = four words 0x0039 ("blade"), 0x47c324 = four words 0x003c
("arm"); depth word 0 (nearest).

- **Shape 0** (0x424950): pivot P = (399, 1167) (0x47c33c), radius 952 (0x47c348). Tip
  `x = T(952·cos a) + 399`, `y = 1167 − T(952·sin a)`. Blade: rectangle x ± 3 (0x47c34a), from
  y − 112 to y + 80 (0x47c34c / 0x47c34e). Arm: from (x ± 3, y) to (X ± 3, 399) with
  `X = T(399 − 767·tan(a − π/2))` (π/2 = f64 0x4697b0; 767 = 1167 − 0x190).
- **Shape 1** (0x424aa0): pivot (100, 480) (0x47c354). Blade fan: root (r 128, 0x47c36e) at
  `a + 32° + 1°`, tip (r 384, 0x47c36c) at `a + 0.3°`, tip at `a − 0.3°`, root at
  `a − 1° + 32°` (32° f32 0x47c364, 1° f64 0x47c3d0, 0.3° f64 0x47c3d8); each point
  `(T(r·cos) + 100, 480 − T(r·sin))`. Arm: from the point at r 200 (0x47c370), angle
  `a + 12° + 1°` (12° f32 0x47c368), ± 3 in x, down to (X ± 3, 399) with
  `X = T(48 + 81 / tan(a + 12°))` (48 = 0x47c372, 81 = dword 0x47c374 (480) − 399).
- **Shape 2** (0x424cb0): nothing while `a < 1.6465` (0x47c39c, ≈ 94.3°). Centre (550, 800)
  (f32 0x47c378 / 0x47c37c), radius 740 (0x47c394), half-angle 0.0027027 (0x47c38c). Points
  `p0 = (T(740·cos(a+δ) + 550), T(800 − 740·sin(a+δ)))`, `p3` likewise at `a − δ`. The quad
  `p0, p1, p2, p3` runs from the tip circle along the arm to the screen edge: `p1`/`p2` are the
  crossings of the two edge lines with x = 496 (`y − (496 − x)·tan a`) while `a ≤ 1.699712`
  (0x47c3a0); with p1 on y = 399 (`x − (399 − y)/tan a`) and p2 on x = 496 up to 1.709598
  (0x47c3a4); both on y = 399 above. One quad, blade colour.

**Colour — likely.** 0x432b50 reads palette entry `0x829d5c + 4·(hi·256 + lo)`; palette 0 is
never written (no writer among the palette loaders 0x436720 / 0x436770 / 0x436bf0 / 0x436c20 and
their callers, and no other absolute reference to the area), so both words are black and
opaque.

### 13.3 Drops (create 0x424f20, update 0x425000, draw 0x425040) — confirmed

0x424740 creates 10 drops with counters `i & 7`; each update does `counter += 1` and at 8
creates one successor (counter 0) if `0x84c3a0 ≠ 0`, then ends. The successor is appended to
list 5 and is updated in the same pass (0x425320 re-reads the next pointer after a task ends),
so it is drawn first with counter 1. A drop is a 63×63 textured quad at
`x = rand·432 >> 15` (0..431), `y = (rand·284 >> 15) + 52`, corners TL, TR, BR, BL; frame
`(counter >> 1) & 3` picks the cached texture of part 0x299e + frame; depth word 2. Visibility
+0x5c is fixed at creation: hidden when the camera point's track type `0x850114` (byte 10 & 0x1f
of the camera's point, 0x41d160) is 0x1b–0x1e (tunnels), and also type 10 on stage 16
(0x424720 / 0x424fd3). `rand` is the CRT LCG (0x437d10). **Its seed is not reproducible**: the
only other user, the traffic spawner 0x404f10, reseeds it with `rand()·0x84c494`.

### 13.4 Snow layers and fog (0x425070, update 0x425100, draw 0x425120, kill 0x425220) — confirmed

0x425070 caches part 0x8de and overrides its corner UVs (page units ×256, `(u, v)`):
corner 0 (0x7c0, 0), corner 1 (0x7c0, 0x570), corner 2 (part u, 0x570), corner 3 (part u, 0)
— the texture repeats 7.75 × 5.4 times with v across the screen. It sets `0x8502ac = 0` and
calls `Gfx_SetFog(1)` (fog on, if FOGENABLE; §7). Update: counter += 8 while `0x84c3a0 ≠ 0`,
else the task ends and 0x425220 sets `0x8502ac = 1` and `Gfx_SetFog(0)`. Draw (skipped at tunnel
points, 0x4246f0): three textured quads over the band rect 0x47c32c ((0,52)–(496,400)), each
with every u and v shifted by an offset: `(−3n) & 0xff` at depth 0x10, `(n + 2) & 0xff` at
0x100, `(n/2 + 4) & 0xff` at 0x200. With the Z test the nearest layer wins and colour-keyed
texels write nothing, so the result is the three layers back to front.
`0x8502ac == 0` makes 0x41dfc0 skip the panorama sprites in area 1 (Akita, COM ≠ 7/10), so the
mountains vanish into the fog while it snows. **The fog therefore only exists during snow**: the
only `Gfx_SetFog(1)` call is here.

### 13.5 Sky gradient fades — confirmed (code), see scene.md §6.2

Course event 0x15 and 0x40d620 fade palette 3 to the darker set over 150 ticks (0x40f320 →
0x436a60 on list 7, which list clears do not touch); the drive restart rebuilds it without a
fade for `level ≥ 1`; the level reaching 1 fades back.

### 13.6 Camera jolt (0x40e370 / 0x40e3d0) — confirmed

Created on list 2 with an amplitude `a`: −0.018 by 0x40a0a0 when the train stops with
`0x8570b4 == 1` (ran past the forced-stop point / buffer stop), −0.004 and −0.0005 by the coupling
minigame 0x4093c0. Each tick while `|a| ≥ 1e-4`: eye height `0x850140 = h − a`,
`0x850294 = +0x34 + a` (scene.md §5 gives the net camera effect: eye lowered by `a`, target by
`2a`), then `a ·= −0.8` on even `0x84c494`. The kill callback restores both. The buffer-stop
jolt lasts ≈ 48 ticks.

### 13.7 Other screen effects while driving — confirmed (search)

There is no screen-fade task on the driving path. List 0 (0x84c180) holds sound tasks (0x420470
timed sound, 0x4205c0 turnout clatter; audio-text.md), not fades as main-loop.md guessed. List 7
holds only palette fades (0x4367c0 / 0x436920 for palettes 0x15–0x17 in the title/attract code,
0x436a60 for palette 3 above). The callers of the 2D quad functions (0x432b50 / 0x432da0 /
0x431ec0) are the background (0x401600, 0x41df00, 0x41dfc0; scene.md §6), this overlay, the
render task's border quads (0x425680), the viewer camera preset (0x41c3c0) and the title picture
(0x4037f0).

### 13.8 Port notes

- `Weather` mirrors the tasks from the rules' state each tick: created on the rules' weather-on
  edge or at a segment start with level > 0; cleared at the station result; wiper flag =
  `Rules::wet_rails` while the overlay lives. Headless shots draw the latest tick's state; the
  window interpolates the wiper and the snow scroll (§13.9).
- **Approximations.** The RNG starts at seed 1 (§13.3). The jolt's parity uses the drive tick
  count, not 0x84c494. The jolt is applied to the window's camera but not to headless shots or
  the 2D panorama placement. A fade already running when a segment restarts keeps running
  (as in the original), but its frame count starts at 0 on the tick it is created (the exact
  first frame of 0x436ad0 is not checked).
- **4:3 placement (the original).** Everything is in virtual x 0..496, the 4:3 area: at 16:10
  the snow would end at the 4:3 edges. The port widens it by default (§13.9); shape 2's arm
  still stops at x = 496, as it is part of the cab.
- **Station-based starts.** Besides the stage start and course event 0x15, the rules start the
  weather in 0x40d620 (passing a station; global tick counter `0x84c494 % 3 == 0`, dry rails if
  the track class ahead is a tunnel) and 0x40d340 (next segment; `% 4 == 0`), at or past
  `0x459cf8[stage]` with enough remaining time (`0x459d20`). `Drive` models the counter (set
  from the app's tick count at the stage start, +1 per tick; exact against all three traces).
  **confirmed** (code); not yet seen in a trace (the traces are of stage 0, whose first weather
  station is 99).

### 13.9 Port enhancements (decided, on by default)

Each has its setting in docs/settings.md (`config.toml` key, `SHUPPATSU_<KEY>`, `--<key>` /
`--no-<key>`).

Presentation only: the weather state, its RNG for the original area, the rules and every trace
are unchanged (the three drive replays stay at 0 mismatches).

- **`wide_weather`** (off: `--original-weather`). The drops and the snow layers cover the
  whole visible Hor+ width. The snow band is repeated in 496-unit tiles to both sides, tile *k*
  moved by *k*·496 in x with its v moved by *k*·0x570, so the layers continue seamlessly
  (corners 2 and 3 share the part's u, §13.4, so the mapping is affine in x). Drops get two
  extra tiles per side, each with ten drops of the same law, life and visibility as §13.3,
  drawn from their own CRT generators (seeds 2–5) so the original area keeps seed 1's
  sequence. Only tiles that overlap the visible range are drawn, so at 4:3 the output is the
  original's pixel for pixel (checked by a headless pixel compare). The wiper shapes stay
  where they are: they belong to the cab.
- **`interpolate`** (off: `--no-interpolate`; with the camera's interpolation; `crate::smooth`).
  Off, every frame shows the last tick (the camera, AI trains, wiper, snow and needle step at
  30 Hz).
  The wiper angle, the snow counter (the three offsets are taken from the interpolated counter,
  modulo 256 = one texture repeat) and the analog speedometer needle are placed between the
  last two ticks with the render alpha. At alpha 0 and 1 they are exactly the ticks' values
  (unit tests `wiper_and_snow_interpolate_between_ticks`, `needle_interpolates_between_the_ticks`);
  headless shots, pauses and position jumps use alpha 1. The needle stays a rect-layer fan in
  whole 640×480 pixels (hud.md §3.5): the HUD draws it once per tick, and the window moves that
  fan and composes the canvas again only on frames where a needle pixel changes (on the GPU,
  or on the CPU redrawing and uploading only the needle's rectangle: docs/performance.md §2–3).
- **`far_traffic`** (off: `--original-traffic`). AI trains are drawn up to the far plane (32
  units, 320 m, §10), as far as the scenery reaches, instead of 150 m; beyond 150 m they use
  the farthest distance band, LOD 4 (traffic.md §4). The lateral bands and the 25 m behind are
  unchanged, and spawning, movement and the passing sounds (which use the original bands in
  the engine) are untouched.
- **`snow_fog`** (key `fog`): the fog of §13.4 is on by default, as with DengoSetting's FOGENABLE checked.
  A fresh install of the original never ran DengoSetting, so FOGENABLE parses as 0 and it
  never fogs (§7). Off: `--no-fog` or `fog = false` in `config.toml` (localisation.md,
  "Settings"); `--fog` forces it on. It still only exists while it snows.

## 14. Port enhancements: shading (named settings, off by default)

The original's world pass is nearest sampling without mipmaps, colour-key transparency and no
antialiasing (§6, §9). Four settings change only how the 3D pass shades; each is off by
default, and with all four off the frame is the same as before they existed (headless shots of
the three comparison views below are byte-identical to the build before them). Settings in
docs/settings.md; code in `crates/shuppatsu-render/src/shader.wgsl`, `gpu.rs` and `ground.rs`.
Comparison sheet: `out/explain/gfx/shading/` (not committed: derived from game data).

### 14.1 Which surfaces are ground (`shuppatsu_render::ground`)

The original has no ground flag: terrain is ordinary part groups. The port marks it per vertex
(`Vertex::surface`), from what the scene composer knows and from the geometry:

- **Source**: only the non-rail runs (kinds 3/4/7/8/≥ 9, scene.md §2.7) and the live scenery
  (§4) are candidates (`FrameGeometry::add_terrain_instance`, called from
  `RouteScene::compose`). Rail pieces (main line and side tracks), buffer stops, the tunnel
  plug, AI trains, the cab/result/opening props never are.
- **Facing up**: ≥ 60 % of the group's world-space triangle area within 50° of horizontal
  (either winding).
- **Not raised on walls**: if the whole part has ≥ 12 % of its area steeper than 70° (a
  platform, canopy, building, viaduct), only upward groups within 0.04 units (≈ 0.4 m) of
  the part's lowest point count. This drops platform tops and canopies (Akita, stage 1) and
  roofs; verges at the foot of a wall stay.
- **Repeating axes** (for de-tiling): an axis counts if the group's UVs span at least one
  whole rect along it and the rect's texture wraps without a seam along it (first vs last
  texel column/row compared with the mean neighbouring difference inside the rect, × 2 + 6).
  Transition textures (grass into gravel, the asphalt with a kerb line of Keihin's verge,
  rect 952) fail along the axis that would move their edge.
- Seen on the three views: Hokuhoku's rice and grass fields (scenery slabs 8180/8181, rects
  1619/1620; runs 8723–8725, 8885–8889) and the viaduct's cable trough; Akita's snow strips
  between the tracks (runs 4520–4524, rect 1956), not the snowy rail beds of the yard, which
  are rail pieces; Keihin's ballast verges (runs 9826–9830, rect 1337), the grass bank and
  the asphalt strip (9551–9553). **likely** (visual check with a debug tint of the bits).

### 14.2 `ground_variation`

On ground fragments the texel (linear colour) is multiplied by a procedural world-space value
noise in x/z: brightness `1 + 0.3·(0.7·n(0.4·xz) + 0.3·n(1.5·xz) − 0.5)` (mostly ±6 %, at most
±15 %, features of about 25 m and 7 m) and a warm/cool tint `(1 + 0.06t, 1, 1 − 0.08t)` with
`t = n(0.25·xz) − 0.5`. No texture asset; fog applies after it. Keyed on world position, so it
is continuous across parts and LOD pieces and stays put as the camera moves.

### 14.3 `msaa` (1 / 2 / 4 / 8)

The world pass (2D background quads, world, debug lines, weather effects: everything in the
3D band) renders into a multisampled colour and depth target and resolves into the frame; the
HUD/overlay pass is separate and unchanged. A view that does not clear (the station result's
second view, scene.md §8) loads the multisampled target the first view left. Counts other
than 1 and 4 need the adapter's format features: the game requests
`TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES` when the adapter has it, and the renderer probes a
count once (validation error scope) and falls back to the next lower power of two.

Colour-keyed cut-outs (portal braces, fences, trees, signal masts; the original draws no
overhead wires, scene.md §4.1) would get no edge antialiasing from
MSAA with `discard`. With MSAA on, the world pipeline uses **alpha to coverage**: the coverage
is the bilinear colour-key alpha sharpened to about one pixel (`(a − 0.5)/fwidth(a) + 0.5`),
and where the nearest texel is the key but the coverage is not zero, the colour comes from the
bilinear colour divided by its alpha (keyed texels are RGB 0, so that is the opaque
neighbours' colour, no dark fringe). Magnified cut-outs therefore follow the bilinear 0.5
contour (rounded corners instead of texel stair-steps). Checked on Keihin's fence rails and the
Hokuhoku trees (sheet crops).

### 14.4 Sampling: `texture_filtering` × `anisotropic`

| `texture_filtering` | `anisotropic` | world pass |
|---|---|---|
| off | 1 | nearest, no mipmaps: the original |
| on | 1 | mipmaps, trilinear (magnified texels blurred) |
| off | N > 1 | mipmaps; along each axis where a texel covers more than a pixel the coordinate is pulled to the texel centre except within one pixel of its edge ("sharp texels", antialiased edges), elsewhere trilinear + N× anisotropic. Close-ups keep the original's square texels, distant ground stops shimmering |
| on | N > 1 | mipmaps, trilinear + N× anisotropic |

`texture_filtering` alone used to mean trilinear + 16× anisotropic; anisotropy is now its own
setting (`texture_filtering = true` + `anisotropic = 16` is the old look). The 2D background
and effect quads keep the page bind group's sampler (nearest, or with `texture_filtering`
trilinear + 16×), unchanged.

**Atlas bleeding.** Mipmaps are built per page (`mip.rs`, 2×2 box, alpha-weighted). Rects sit
at power-of-two-aligned positions of their own size, so a level mixes neighbouring rects only
once the rect is smaller than a texel there. The shader caps the derivatives of a sub-rect
lookup at a quarter of the rect, so filtering never picks a level where the rect is under
4×4 texels; whole-page rects wrap through the sampler and need no cap. Bilinear taps at a
sub-rect edge can still take half a texel of the neighbour, as any filtered atlas does.

### 14.5 `ground_detiling`

Quilez's "texture repetition" technique 3: a smooth value noise of world x/z (0.6 per unit,
regions of ≈ 15 m) picks one of 8 variants per region, and the two neighbouring variants are
cross-faded (`smoothstep(0.2, 0.8, f − 0.1·Σ(a − b))`, so the seam follows the texels'
contrast). A variant is a random offset and mirror applied **only along the repeating axes**
of §14.1, to the part-local tile coordinate (`Vertex::tile`, 1.0 = one repeat), which is then
wrapped inside the group's rect (`Vertex::rect`, origin and repeat size in page UV, exactly
`TexRect::map_uv`'s mapping) — never into the neighbouring atlas texels. The lookups use the
true screen derivatives (`textureSampleGrad`), so the wrap does not disturb mip selection. Keyed
on world position: the pattern continues across part and LOD-band seams (whose tile coordinates
restart). Non-repeating axes keep their coordinate. Cost: two lookups instead of one on ground
fragments (four with MSAA's coverage lookup).

### 14.6 The 2D textured ground background (scene.md §6.5)

Not treated. Areas 0/4 draw it as a static screen-space tiling behind everything (page
repeated every 16 virtual units), and area 3's water is a fixed picture seen through holes in
the terrain. Both sit at the horizon, mostly covered by the 3D ground; a world-space variation
has no world position to key on there, and a screen-space one would look like a stain fixed to
the screen as the camera turns. **decided** (port).

## Implications for the port (Hor+)

- Keep the vertical FOV from `0x84c150` and replace the aspect with the actual viewport aspect.
  Decide whether arcade mode reproduces the 7.9 % horizontal stretch (§5); this needs the
  user's decision.
- The 3D viewport is a band (13 % top strip, virtual y 52..400), not the whole screen.
- Fog: linear, eye-space 1→10, colour #BFBFBF. The XZ radius-10 cull applies only to buffer stops.
  Both are aspect-independent, but the scenery and side-track lists are 4:3-dependent (scene.md §5). The sky panorama tile count and the background fill quads need
  widening.
- Textures: RGB565 with black = transparent, nearest filtering, no blending; UV unit = 1/256.

## Open questions

1. Which camera function and FOV is the in-cab driving view (trace `0x84c150`/`0x84c138` with the hook).
2. Track/scenery section visibility selection (§10 candidates).
3. World unit scale (near 0.05 / far 32 / fog 10).
4. Whether real DX6 drivers applied w-fog or z-fog to this projection; compare screenshots under Wine.
5. Formats of `tbl.rom`/`cg.rom`/`parts*.rom` beyond what the render path shows (formats phase).

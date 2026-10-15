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
- 1: two viewports.
- 2: fixed camera.
- 3: debug free camera, moved with inputs `0x48c42c` bits.

Then `Frame_End`: full viewport, sprite layer, rect layer, `EndScene`, `Gfx_Present` + clear.
Draw lists are the 9 task lists at `0x84c180..0x84c1ec`; each task's draw callback is at +0x10
(`TaskList_Draw` 0x425ac0).

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

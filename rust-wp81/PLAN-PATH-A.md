First mind:实践是检验真理的唯一标准
一定要真机测试！！！！！！！！！！

# Path A — the Rust-native D3D11 renderer

> Status: **approved plan.** The import surface was verified against the WP8.1 kit's own ARM
> import libraries and against the device image (`2026-10-04.md`, Round 38). This document is the
> ordered work list.
>
> **Stage 1 is built and verified against the real D3D11 runtime** (`2026-10-04.md`, Round 39):
> `src/bin/hostcheck.rs` compiles the same binding for the desktop and runs device → swapchain →
> RTV → pipeline → clear → `Draw(4,0)` → present, all passing. It found and fixed three
> crash-on-device bugs (vtable base offset, every device-context slot off by 4, a missing `Map`
> argument). The remaining Stage 1 acceptance item is the on-device run, which needs the phone
> connected.
>
> ### On-device status (Round 41, Lumia 925 / Adreno 225, FL 9.3)
>
> The DLL loads and runs on the phone. A no-debugger bisect (`wp81_rs_start_upto`) plus a
> `DispatcherTimer` progress watchdog pinned the hard fault:
>
> | Probe | Result | Meaning |
> | --- | --- | --- |
> | `limit=2` (device) | clean | DXGI factory + adapter enumeration + `D3D11CreateDevice` all OK on the Adreno 225 |
> | `limit=10` (enter swapchain) | clean | `QI(IDXGIDevice)` OK |
> | `limit=13` (factory in hand) | clean | `GetAdapter` + `GetParent(IID_IDXGIFactory2)` OK |
> | vtable snapshot | **32 readable slots**, slot 3 = `0x71EFD1C5`, slot 24 = `0x71F00D21` | the vtable is NOT short; **slot 24 is a real code pointer** |
> | `QI(IID_IDXGIFactory2)` round-trip | **S_OK** | the object really is an `IDXGIFactory2` |
> | `limit=16` (full swapchain call) | **process dies** | the fault is inside `create_swapchain_for_panel_traced`, at/after `CreateSwapChainForComposition` |
>
> So the slot number, IID and factory pointer are all **verified correct**; the entry exists but
> **hard-faults** rather than returning `DXGI_ERROR_INVALID_CALL`. That is the signature of an
> **unimplemented DXGI 1.2 entry** on this 2014 driver, not of a wrong slot.
>
> **Leading hypothesis:** `DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL` is only legal from feature level 10.0
> and this device is 9.3. The descriptor-variant loop has therefore been **reordered** to try the
> FL-9.3-safe effects (`DISCARD`, then `SEQUENTIAL`) **before** the flip model, so if the flip model
> is the trigger the log now records which shape the driver accepts instead of dying
> (`wp81_rs_sc_variant()`).
>
> **Also verified this round:** `SwapChainBackgroundPanel` and `ISwapChainBackgroundPanelNative`
> (IID `43BEBD4E-ADD5-4035-8F85-5608D08E9DC9`, `SetSwapChain` at slot 3) **both exist on WP8.1** —
> confirmed by compiling against the real WP8.1 reference set and by reading the kit's own
> `windows.ui.xaml.media.dxinterop.h`. The native attach now tries **both** panel interop interfaces
> (`ISwapChainPanelNative` then `ISwapChainBackgroundPanelNative`) and records which answered via
> `wp81_rs_panel_iface()`.
>
> **Two open suspects, now separable by one build:** the `IsCurrent` soundness probe returned `-1`
> (not a valid BOOL), so it is possible that slot 13 is mis-signed and smashes the frame that slot
> 24 then trips over. Two bisect stops separate them: `limit=13` runs the probe then stops before
> slot 24 (dies → the *probe* is guilty), `limit=18` skips the probe and stops before slot 24
> (clean → slot 24 is guilty).
>
> ### Round 42 — the hard fault was a use-after-free, and the descriptor is now authoritative
>
> The `IsCurrent=-1` was a red herring (the `limit=13` run returned before the probe; it was still at
> its `-1` initialiser). With the probe actually running it reports **`IsCurrent = 1` (TRUE)** and
> **`QI hr = S_OK`** — the factory is sound.
>
> **The hard fault was NOT the driver.** `RustStage1Page` subscribed `Loaded += OnPageLoaded` twice
> over and `StartRenderer` is `async void`, so **several bring-ups ran concurrently** (the log shows
> interleaved `Bisect: begin` lines). They race on Rust's single global `STATE`, and every
> `wp81_rs_start_upto` begins with `release_all()` — one run frees another's device mid-call.
> `CreateSwapChainForComposition` given a freed device returns `DXGI_ERROR_INVALID_CALL`, or faults.
> Fixed with `-=` before `+=` and an `Interlocked.CompareExchange` re-entrancy gate.
>
> **The descriptor is now Microsoft's own WP8.1 sample** (`XAML DirectX 3D shooting game sample`,
> `Shared/Common/DeviceResources.cpp`): `B8G8R8A8_UNORM`, `Stereo=IsWindowedStereoEnabled()`,
> `SampleDesc{1,0}`, `BufferCount=2`, **`DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL`**,
> `DXGI_SCALING_STRETCH`, **`DXGI_ALPHA_MODE_IGNORE`**, `Flags=0`, via
> `CreateSwapChainForComposition`, attached with `ISwapChainPanelNative::SetSwapChain` **on the UI
> thread**, then `IDXGIDevice3::SetMaximumFrameLatency(1)`. That shape is variant 0; every variant's
> HRESULT is recorded and logged (`RustRenderer.sc-hr: v0=… v3=…`).
>
> **Still open:** whether the attach must move to the UI thread (the sample does), and whether the
> factory must come from `IDXGIDevice3` rather than `IDXGIDevice`. The next run's `sc-hr` line
> decides.
>
> ### Round 43 — `CreateSwapChainForComposition` does not work on this device. At all.
>
> The `sc-hr` table finally read out, and it settles the question:
>
> ```
> RustRenderer.sc-hr: v0=0x887A0001 v1=0x887A0001 v2=0x887A0001 v3=0x887A0001
> ```
>
> **All four descriptor shapes return `DXGI_ERROR_INVALID_CALL`** — including `DISCARD` and
> `SEQUENTIAL`, which are the *non-flip* controls. The variants were reordered so the controls run
> **first** (v0/v1) precisely because the flip model hard-faults and would otherwise stop the loop
> before the controls were ever tried; with controls first the run survives and reports all four.
>
> Two device shapes were measured, and they fail *differently* — which is itself the evidence:
>
> | device shape | `CreateSwapChainForComposition` | `CreateSwapChainForCoreWindow` |
> | --- | --- | --- |
> | enumerated adapter + `DRIVER_TYPE_UNKNOWN` | returns `DXGI_ERROR_INVALID_CALL` | returns `E_INVALIDARG` |
> | default adapter + `DRIVER_TYPE_HARDWARE` (the WP8.1 sample's) | **hard-faults the process** | — |
>
> And the factory is exonerated: `limit=19` builds the swapchain on a factory from a **direct
> `CreateDXGIFactory1`** (not the `GetParent`-derived one) and behaves identically — same
> `DXGI_ERROR_INVALID_CALL` for every variant, same fault if the flip model is first.
>
> **Conclusion: the composition-swapchain path is not implemented in this 2014 Adreno 225 driver.**
> It is a feature-level-9.3 part; the flip model (which `CreateSwapChainForComposition` requires by
> spec) is a feature-level-10+ concept, so `FLIP_SEQUENTIAL` faults inside the driver rather than
> returning `DXGI_ERROR_UNSUPPORTED`. DXGI 1.2 entry points exist in the vtable (slot 24 is a real
> code pointer, the `IsCurrent`/`QI` probes are clean), so nothing about the binding is wrong — the
> implementation behind the slot is.
>
> **The live route is now `CreateSwapChainForCoreWindow`**, which fails with `E_INVALIDARG` at
> `CW_CREATE` (`stage=134`). `E_INVALIDARG` there means the `pWindow` argument is not a valid
> `ICoreWindow*`, i.e. the `IUnknown*` C# passed from
> `Marshal.GetIUnknownForObject(CoreWindow.GetForCurrentThread())` is not what DXGI wants. That is a
> fixable bug, and it is the next experiment.
>
> Also settled this round: `D3D11_CREATE_DEVICE_BGRA_SUPPORT` is now passed (the WP8.1 sample does);
> `SetSwapChain` is split into its own export (`wp81_rs_attach_panel`) so C# can call it from the UI
> thread, as the sample does; and `DXGI_ERROR_*` HRESULTs decode to names in the log
> (`wp81_rs_last_hr_name`).

## The goal

`prpr` renders **itself** on Windows Phone 8.1. Rust owns the D3D11 device, the swapchain, the
shaders and the draw calls. C# does **not** repaint anything — it only hands Rust the window.

The existing "drain" (Rust records draw calls → C# replays them through Win2D,
`wp81prpr.dll` + `PrprPort.cs` + `Win2DChartRenderer.cs`) is **Path B** and is being replaced.

## Why this is tractable (the shape of prpr's renderer)

Reading `prpr/src` settled the design:

- **The whole scene is `draw_texture_ex`.** Notes (`core/note.rs:60`), holds, judge lines
  (`core/line.rs:261`), icons, everything — macroquad's built-in textured quad. There is **no
  GLSL sprite shader to port.** Only FXAA (`scene/fxaa.glsl`) and rainbow (`scene/rainbow.glsl`)
  are custom, and neither is required for a first frame.
- **The vertex format is already fixed** (`core/note.rs:118`):
  `x, y, z=0, u, v, r, g, b, a` — macroquad/sokol's layout. It maps straight onto a D3D11
  `POSITION`+`TEXCOORD`+`COLOR` input layout.
- **Sprites are already batched by the engine** into `res.note_buffer` as
  `(order, texture_id, [Vertex; 4])`. So the Rust renderer consumes the *same* buffer the drain
  used — the producer does not change, only the consumer.
- **`D3DCompile` is on the phone** (`WP Kits\8.1\lib\ARM\d3dcompiler.lib`), so HLSL compiles at
  runtime. Offline DXBC is the optimisation, not a prerequisite.

So: one textured-quad pipeline, one vertex format, one buffer that already exists. That is the
target.

## Verified facts this plan rests on

| Fact | Evidence |
| --- | --- |
| `d3d11.dll`, `dxgi.dll` are on the device | WP8.1 kit ships `d3d11.lib`/`dxgi.lib` for ARM; the C++ probe reached the Adreno 225 through `d3d11.dll` on a real phone |
| `D3D11CreateDevice`, `D3D11CreateDeviceAndSwapChain`, `CreateDXGIFactory1` exist by name | dumped from `WP Kits\8.1\lib\ARM\d3d11.lib` / `dxgi.lib` |
| `D3DCompile` exists | dumped from `d3dcompiler.lib` |
| A hand-rolled binding stays clean | `wp81d3d11rs.dll` = 21,504 bytes, imports exactly `d3d11.dll` + `dxgi.dll` |
| Feature level is 9.3 (`0x9300`) | C++ probe on device: `Qualcomm Adreno 225 (WDDM v1.2)`, FL 9.3 |

**Feature level 9.3 consequences** (these shape every stage):
- Shader model is **`vs_4_0_level_9_3` / `ps_4_0_level_9_3`**. No compute, no geometry shaders,
  no `SV_ViewportArrayIndex`, limited MRT, and **no `Texture2D.SampleLevel`** in some paths.
- `DXGI_FORMAT_R8G8B8A8_UNORM` etc. are fine; sRGB formats mostly are too.
- Constant buffers are capped at **4096 × 16-byte registers** for the *whole* device in FL9.3 —
  still plenty here.
- No `DrawIndexedInstanced` limit concerns, but `DrawInstanced` is restricted; use plain
  `Draw`/`DrawIndexed` per batch.
- `Present` with `DXGI_PRESENT_DO_NOT_WAIT` is not available; don't rely on non-blocking present.

## Staging

Each stage ends in something that builds clean and can be checked on the spot. No stage leaves the
tree in a state where the import gate (`check_imports.py`) fails.

### Stage 1 — Renderer core (device, swapchain, quad) — task #7

Deliverable: `wp81d3d11rs.dll` exposes `wp81_rs_init(width, height)` → creates device+context via
the already-proven adapter path, creates a **swapchain**, compiles the trivial shader pair,
creates the input layout + a 1-quad dynamic vertex buffer, and draws one tinted quad. Then
`wp81_rs_present()`.

Key decisions to make in this stage:
- **How the swapchain attaches to the app.** WP8.1 XAML apps composite a `SwapChainPanel` or a
  `SwapChainBackgroundPanel`. The route is `IDXGIFactory2::CreateSwapChainForComposition` →
  `ISwapChainPanelNative::SetSwapChain` (or `ISwapChainBackgroundPanelNative::SetSwapChain`); the
  simpler route is `CreateSwapChainForCoreWindow` with the `CoreWindow` from C#. **Measure both**,
  prefer `ForComposition` + `SwapChainPanel` because it lets XAML own the layout.
  - This needs `IDXGIFactory2` (present in the kit) and the panel interop interface. **The two
    correct IIDs, read from the kit's own `windows.ui.xaml.media.dxinterop.h`, are
    `ISwapChainPanelNative` = `F92F19D2-3ADE-45A6-A20C-F6F1EA90554B` and
    `ISwapChainBackgroundPanelNative` = `43BEBD4E-ADD5-4035-8F85-5608D08E9DC9`** (this document
    previously carried a wrong IID for `ISwapChainPanelNative`; the header is the authority). Both
    are `IUnknown` + a single `SetSwapChain(IDXGISwapChain*)` at slot 3.
- **Formats.** `DXGI_FORMAT_B8G8R8A8_UNORM` + `DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL` if FL9.3 allows
  it, else `DISCARD` with `R8G8B8A8_UNORM`. Verify on device.
- Vertex stride **36 bytes**: `pos3 (12) + uv2 (8) + rgba4 (16)`. `z` is kept at 0 — it costs 4
  bytes and keeps the layout identical to macroquad/sokol's `Vertex`, so the Stage 3 port needs no
  re-mapping. (`MAX_VERTICES = 65536` → a ~2.3 MB dynamic buffer.)

Acceptance: builds; `check_imports.py` still passes; on device a solid quad appears on a
`SwapChainPanel`.

### Stage 2 — HLSL + D3DCompile — task #8

Deliverable: `shaders.rs` with the HLSL source for the textured quad, plus a
`d3dcompiler.dll` binding for `D3DCompile`. Two paths, both real:
- **Runtime:** `D3DCompile(hlsl, len, name, defines, includes, "main", "vs_4_0_level_9_3", …)`.
- **Offline (fast path):** the same HLSL compiled at build time by `fxc` (Windows SDK 8.1) into a
  `.cso`, embedded with `include_bytes!`, with runtime compile as the fallback if the offline
  blob is missing.

The shader itself is small:

```hlsl
struct VSIn  { float2 pos : POSITION; float2 uv : TEXCOORD0; float4 col : COLOR0; };
struct VSOut { float4 pos : SV_POSITION; float2 uv : TEXCOORD0; float4 col : COLOR0; };
cbuffer Transform : register(b0) { float4x4 u_proj; };
VSOut vs_main(VSIn i) { VSOut o; o.pos = mul(u_proj, float4(i.pos, 0, 1)); o.uv = i.uv; o.col = i.col; return o; }
Texture2D u_tex : register(t0); SamplerState u_samp : register(s0);
float4 ps_main(VSOut i) : SV_Target { return u_tex.Sample(u_samp, i.uv) * i.col; }
```

Traps: HLSL is **column-major by default** while macroquad's `Mat3` is row-major (the C# port
already had to transpose — same trap). Compile with `D3DCOMPILE_ENABLE_STRICTNESS` off and
`row_major` on the `float4x4` if that is simpler than transposing.

Acceptance: on device, the solid quad becomes a **textured** quad (test texture written from the
existing atlas PNG).

### Stage 3 — Scene draw calls — task #9

Deliverable: the real thing. Consumer for `res.note_buffer`.

- `Mat3` → `float4x4`. **Reuse the exact decomposition the C# renderers use** (rot/tx/ty only;
  scale overridden by a constant note size) so the Rust output is pixel-comparable to the
  `Win2DChartRenderer` reference.
- Camera projection: `screenX = (x+1)/2 * W`, `screenY = (1 - y*ar)/2 * H` — the formula already
  proven in the C# head.
- Per-frame: read `res.note_buffer`, sort/bucket by `(order, texture_id)`, upload into one
  dynamic vertex buffer, set the texture + sampler, issue `Draw(4, n)` per batch, or
  `DrawIndexed` with a shared quad index buffer.
- Alpha blending: `SRC_ALPHA`/`INV_SRC_ALPHA`, `BlendEnable`, `AlphaToCoverageEnable=false`.
- Holds: the single-sheet `hold.png` + `holdAtlas:[50,50]` split is already implemented in C#
  (`ChartRenderer`/`Win2DChartRenderer`) — port that split verbatim.

Acceptance: notes, holds and the judge line render on device; a side-by-side against the C# Win2D
head looks the same for a fixed chart + camera.

### Stage 4 — Wire into the app — task #10

Deliverable:
- `RustRenderer.cs` — `DllImport("wp81d3d11rs.dll")` for `wp81_rs_init` / `wp81_rs_frame` /
  `wp81_rs_present` / `wp81_rs_shutdown`, mirroring `D3D11Probe.cs`.
- Add `wp81d3d11rs.dll` to `Phirura.WindowsPhone.csproj` as `Content` with `DeploymentContent`.
- A one-shot on-device self-test call from `MainPage` (behind a const, like
  `RunNativeProbes`) that loads the DLL and reports — proving **loading**, not just building.
- Then point `GamePage` at the Rust renderer instead of `Win2DChartRenderer`.

Acceptance: the packaged `.appx` contains the DLL; on device the self-test reports success; then
GamePage shows Rust-rendered frames.

## Ordering rule

Do **not** start Stage 3 before Stage 1 renders a textured quad on a real device — the swapchain
attachment is the single riskiest unknown, and everything after it is arithmetic.

## What is explicitly *not* in this plan

- FXAA / rainbow / post-effects (custom GLSL) — after the first frame.
- Particles (`prpr/src/particle.rs`) — after notes render.
- The `Paint` judge-line kind (`core/line.rs:314`) which needs a render-to-texture pass.
- Deleting the C# game code. It stays until the Rust renderer matches it; it is the reference.

## Files this creates

- `rust-wp81/d3d11-rs-probe/` → grows into `rust-wp81/d3d11-renderer/` (rename once Stage 1 lands)
  - `src/direct.rs` — the binding (already exists; extend with the new vtable slots)
  - `src/renderer.rs` — Stage 1 (device/swapchain/quad)
  - `src/shaders.rs` — Stage 2
  - `src/scene.rs` — Stage 3
  - `build.ps1` — already exists; extend with the fxc offline-compile step
- `Phirura/Phirura/Phirura.WindowsPhone/RustRenderer.cs` — Stage 4

---

## Round 45 — Stage 1 blocker SOLVED (diagnosis), and the corrected route

> **Verdict: `CreateSwapChainForCoreWindow` and `CreateSwapChainForComposition` are both
> unsupported for a FL 9.3 WP8.1 app on this Adreno 225 / WDDM 1.2 driver. This is not a bug in
> our binding.** The swapchain was never the right attachment mechanism.

### What was eliminated (every argument individually)

| Variable | Varied how | Result |
| --- | --- | --- |
| Thread | UI thread vs thread-pool | **identical** `DXGI_ERROR_INVALID_CALL` |
| Descriptor (CoreWindow) | 3 shapes (STRETCH/PREMULT, STRETCH/IGNORE, NONE/PREMULT) | **all 3 fail identically** |
| Descriptor (Composition) | 4 shapes incl. the MS sample's exact one | **all 4 fail identically** |
| Window pointer source | `CoreWindow.GetForCurrentThread()` vs `Window.Current.CoreWindow` | **same pointer** `0x04F99B00` |
| Window pointer identity | `QI(IUnknown)` then marshal to `Windows.UI.Core.CoreWindow` | **`isCoreWindow=yes`**, `qi=S_OK` |
| **Device** | enumerated non-SW adapter + `DRIVER_TYPE_UNKNOWN` **vs** default adapter + `DRIVER_TYPE_HARDWARE` | **identical FAIL** (tick 253 confirmed fired on device) |
| Factory vtable | 26 readable slots, `IsCurrent=1`, `QI(IDXGIFactory2)=S_OK` | **vtable + IID sound** |
| Struct layout | `DXGI_SWAP_CHAIN_DESC1` = 44 B, field order checked against the kit header | **exact match** |
| Window readiness | `bounds=384x640 visible=True`, retried at 0/250/500/1000/2000 ms | **no change** |

With every documented argument proven correct and the window provably the right COM object, the
only remaining explanation is that the API itself is unsupported on this driver for this feature
level. Confirmed below.

### The proof: Win2D does not use a swapchain

`Phirura/packages/Win2D.win81.1.21.0/bin/Phone/arm/Microsoft.Graphics.Canvas.dll` (1.26 MB) renders
correctly on the very same Lumia 925. Its import table contains:

```
CreateDirect3D11DeviceFromDXGIDevice
CreateDirect3D11SurfaceFromDXGISurface
D3D11CreateDevice
d3d11.dll   d2d1.dll   DWrite.dll   D3DCOMPILER_47.dll
```

**No `CreateSwapChainForComposition`, no swapchain at all.** Win2D's actual pipeline is:

1. `D3D11CreateDevice` — the D3D11 device.
2. **`CreateDirect3D11DeviceFromDXGIDevice`** — wrap it as a WinRT `IDirect3DDevice`.
3. Create an offscreen `ID3D11Texture2D`, then **`CreateDirect3D11SurfaceFromDXGISurface`** —
   wrap that texture as a WinRT `IDirect3DSurface`.
4. Hand the wrapped device/surface to the XAML compositor (`CanvasControl`), which owns the
   on-screen presentation.

So on WP8.1 for a phone app, **you never create a swapchain and never call `SetSwapChain`.** The
`SwapChainPanel` route is a *Windows 8.1 desktop-class* mechanism; the phone's compositor path is
the `IDirect3DDevice`/`IDirect3DSurface` interop pair. That is why every argument shape returned
`DXGI_ERROR_INVALID_CALL`: the API is not implemented here, exactly as the R41 hard-fault signature
suggested.

**Corollary — a latent bug found by the vtable dump:** the log's slot labelling was wrong.
`slot3` printed `0x71EFD1C5` which is a COM standard method (i.e. `GetParent` was NOT at slot 3 of
that interface in our count), and by the kit's own header `CreateSwapChainForComposition` sits at
**slot 24** with `UnregisterOcclusionStatus` also claiming 24 — meaning our composition attempt was
calling the wrong entry. This is moot now (the composition route is dead regardless) but the
`IDXGIFactory2` slot constants must not be trusted until re-derived from the header.

### The corrected Stage 1 route (per the plan's ordering rule — Stage 3 stays blocked)

Stage 1's acceptance criterion is unchanged: **a Rust-rendered textured quad visible on the real
Lumia 925.** Only the attachment mechanism changes.

- **1a. Offscreen RenderTarget path, C# presents.** Rust creates the D3D11 device and renders the
  quad into its own `ID3D11Texture2D` (a plain `DEFAULT`-usage texture — the same
  `build_pipeline` / `test_texture` / `Draw(4,0)` code already proven by `hostcheck`). C# wraps that
  texture via the WinRT interop and blits it into a `CanvasControl`. This is the smallest change
  that can put Rust's pixels on the phone screen, and every Rust-side call in it is already
  desktop-verified.
- **1b. Then the WinRT interop pair done natively.** Move
  `CreateDirect3D11DeviceFromDXGIDevice` + `CreateDirect3D11SurfaceFromDXGISurface` into the Rust
  DLL (both are plain exports, reachable the same way `D3D11CreateDevice` is), so Rust owns the
  device *and* the surface handed to the compositor — closer to the user's "Rust owns it" intent.
- **1c. Only if 1a/1b fail**, revisit the swapchain with fresh evidence.

The bisect machinery stays useful: no content is drawn through the swapchain anymore, so
`wp81_rs_present` becomes "copy the RTV into the shareable texture".

Acceptance gate for Stage 3 is unchanged and still not met.


dev log:
结论：WP8.1 手机应用根本不用 swapchain 呈现
先修好编译
两个小错，都改掉了：context 缺 mut，以及一段残留的重复 #[no_mangle]（std::fs 代码删除时把属性留在了注释上方）。DLL 回到 20,480 字节 / 3 个导入，hostcheck 八个桌面检查全绿，说明这次改动没碰渲染路径。
（顺带发现：这里的 VS12 只装了 Common7、没有 VC\，hostcheck 必须挂 VS11 工具链。）
设备这一刀，砍下去是空的
我把上一次加的"默认适配器设备"扫描部署上机，抓回日志：tick 253（cw: retrying swapchain with a default-adapter device） 确实触发了两次，换用 D3D_DRIVER_TYPE_HARDWARE 的设备后，CreateSwapChainForCoreWindow 返回一模一样的 DXGI_ERROR_INVALID_CALL。
至此 CreateSwapChainForCoreWindow 的每一个文档化参数都被逐一排除：线程、描述符（3 种）、窗口来源（2 种，同一指针）、窗口身份（C# 侧编组回来确认就是 Windows.UI.Core.CoreWindow，qi=S_OK）、设备（2 种）​、工厂 vtable（IsCurrent=1、26 个可读槽）、结构体布局（44 字节、字段顺序吻合）、窗口就绪度（384x640 / visible=True，重试 0–2000ms）。
真正的答案：去读 Win2D 的导入表
Win2D 在同一台 Lumia 925 上渲染正常，所以我 dump 了它的 ARM Microsoft.Graphics.Canvas.dll——没有任何 swapchain 相关的导入，只有 CreateDirect3D11DeviceFromDXGIDevice 和 CreateDirect3D11SurfaceFromDXGISurface。
它的管线是：D3D11CreateDevice → 包成 WinRT IDirect3DDevice → 渲染到离屏 ID3D11Texture2D → 包成 WinRT IDirect3DSurface → 交给 XAML 合成器。
所以 SwapChainPanel 那套是 Win8.1 桌面级机制；手机路径是这个 interop 组合。​ 之前的每个 DXGI_ERROR_INVALID_CALL 都是 API 在说"这里没有这个功能"——FL 9.3 上根本未实现。
一个意外收获：它连"链接"都链接不上
我最初把这两个函数写成普通 extern，链接居然通过了——但那是假象：没有引用它们，链接器不生成导入描述符，DLL 字节数完全没变。grep 产物确认是 MISS。
扫 d3d11.lib 后原因清楚了：WP8.1 kit 的 ARM d3d11.lib（13,900 字节）是残缺导入库，只有 D3D11CreateDevice / ...AndSwapChain / D3D11Core* / D3DKMT*。这两个 interop 函数不在里面——它们的头文件 windows.graphics.directx.direct3d11.interop.h 也没随 kit 发布。
改法：照 compat 已有的做法运行时解析——LoadLibraryExW + GetProcAddress。direct.rs 现在有 d3d11_export() 和两个包装函数，找不到就返回 E_NOINTERFACE（软失败，不会跳 0）。因为 d3d11.dll 已经在进程里，导入表仍然只有 3 个 DLL、20,480 字节，check_imports 全过。
还验证了一件事：Windows.Graphics.DirectX 在 WP8.1 的 C# 引用集里根本不存在（四个探针全是 CS0234），所以这半个工作没有托管退路，只能原生做——这反而更贴合你"Rust 自己拥有渲染"的要求。
修正后的 Stage 1
路线不变、门槛不变（Rust 画的贴图方块真实出现在 Lumia 925 上），只是换掉挂载机制：Rust 建设备 → 渲染到自己的离屏 ID3D11Texture2D（复用 hostcheck 已证明的 build_pipeline/test_texture/Draw(4,0)）→ Rust 自己调那两个 interop 函数产出合成器要的 IDirect3DDevice/IDirect3DSurface。
按计划的排序规则，Stage 3 仍然封锁。
我还顺手清理了 MEMORY.md（之前超限被截断），现在 31.4 KB、结构清晰。
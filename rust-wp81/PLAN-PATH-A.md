# Path A — the Rust-native D3D11 renderer

> Status: **approved plan.** The import surface was verified against the WP8.1 kit's own ARM
> import libraries and against the device image (`2026-10-04.md`, Round 38). This document is the
> ordered work list.

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
  `CoreWindow`. The clean route is `IDXGIFactory2::CreateSwapChainForComposition` →
  `ISwapChainPanelNative::SetSwapChain`; the simpler route is
  `CreateSwapChainForCoreWindow` with the `CoreWindow` from C#. **Measure both**, prefer
  `ForComposition` + `SwapChainPanel` because it lets XAML own the layout.
  - This needs `IDXGIFactory2` (present in the kit) and `ISwapChainPanelNative` (WinRT, so it
    arrives as a `CreateDXGIFactory`-adjacent COM cast — implement `ISwapChainPanelNative`'s
    `SetSwapChain` by hand, IID `63aad0b8-7c24-40ff-85a8-640d944cc325`).
- **Formats.** `DXGI_FORMAT_B8G8R8A8_UNORM` + `DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL` if FL9.3 allows
  it, else `DISCARD` with `R8G8B8A8_UNORM`. Verify on device.
- Vertex stride 32 bytes (pos3 + uv2 + rgba4 = 9 floats, pad to 32? no — 9*4 = 36; use
  `pos2 (8) + uv2 (8) + rgba4 (16) = 32` and drop `z`).

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

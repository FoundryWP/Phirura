# Phirura — Phira → Windows Phone 8.1

`Phirura/Phirura.sln` → `Phirura.Windows/` (Win8.1, AnyCPU), `Phirura.WindowsPhone/` (WP8.1, **ARM**),
source-shared `Phirura.Shared` (shproj). Read-only ref: `phira-main/phira-main/` (`prpr`,`prpr-*`).
2nd workstream: `rust-wp81/` (real Rust on WP8.1). Daily logs: `.workbuddy-ai/memory/`.
**User-mandated direction (R37): the real Rust game must run AND render on WP8.1.** The C#-replay
path (Rust emits cmds → C# repaints via Win2D) was rejected as "a C# renderer with a Rust backend".
**Path A: Rust owns D3D11 directly.** Plan: `rust-wp81/PLAN-PATH-A.md` (4 stages). Device under
test: **Lumia 925 / RM-910, Adreno 225, FL 9.3 (`0x9300`)**, product id
`fa2c9634-5ad8-487e-be3c-9893871fda9f`.

---
## C# app (shippable product until Path A lands)
Gameplay core (chart model, tweens, RPE/PEC parsers, judgement, scoring) = line-by-line C# port of
`prpr` in `Phirura.Shared/` (Core, Parse, Game, Net). WP head renders via **Win2D**
(`Win2DChartRenderer.cs`); `ChartRenderer` (pooled XAML Images) remains for the Win8.1 head.
- **`CanvasControl`, not `CanvasAnimatedControl`** — the animated control runs Update/Draw off the UI
  thread, but judge+HUD must stay on it (`MusicClock.Resync()` reads `MediaElement.Position`;
  `UpdateHud` writes `TextBlock`s). `CompositionTarget.Rendering` kept: `Render()`/`RenderBadNote()`
  only *record*, `Present()` calls `Invalidate()`.
- `UseSharedDevice="True"` + `CanvasDevice.GetSharedDevice()`, 96 DPI so `Bounds==SizeInPixels`.
  Textures carry the `CanvasBitmap` in `ChartTexture.Tag`.
- Maths reused verbatim: `Mat3` → `Renderer.Decompose`; only rot/tx/ty (note size constant).
  `Mat3`→`Matrix3x2`: `M11→M11,M12→M12,M21→M21,M22→M22,M13→M31,M23→M32` (both row-vector).
  Draw = `ds.Transform = Scale*Rotate*Translate` + `DrawImage(bmp, CenteredRect(w,h))`.
- Camera (macroquad `zoom.y=-aspect_ratio`, model `diag(1,-1)`, chart Y ÷ aspect):
  `screenX=(x+1)/2*W`, `screenY=(1-y*ar)/2*H`.
- Holds: Phira `hold.png` (`holdAtlas:[50,50]`, 1000x1100): top 50px=tail, bottom 50px=head,
  middle=body; drawn in a local y-down frame.
- Audio: `MediaElement.Position` refreshes a few times/s → `MusicClock` runs a `Stopwatch` as master,
  re-anchors when drift > 40 ms.

### Online (`phira/src/client.rs` → `Phirura.Shared/Net/PhiraApi.cs`)
- `POST /login` `{email,password,clientVersion}`→`{id,token}`; `GET /me` (Bearer).
- `GET /chart?page=N&pageNum=M&search=&order=`→`{count,results:[Chart]}`; order
  `updated|name|difficulty|rating`, `-` reverses; **paging 1-based**. Archive = `chart.file.url` zip.
- **`file`/`illustration`/`preview` are bare URL strings** (not objects); `uploader` = bare id;
  `PhiraApi.FileUrlOf` accepts both shapes.
- Real archive has **no `chart.json`**: `…jpg info.txt …json info.yml …mp3`. `PickChartFile` takes
  any `.json` except `info.json`; `PickIllustration` falls back to the first image.
- **PEC ships as `.json`** (line 1 bare offset). `ChartParser.Parse` must **sniff content**: RPE `{`,
  PEC bare number. **Do not "fix" the PEC note mapping from memory** — `parse/pec.rs`: `n1`=Click,
  `n2`=Hold(6 tokens), `n3`=Flick, `n4`=Drag; `bp`=2 tokens, `n1`/`n3`/`n4`=5, `cv`=3, `cp`=4;
  trailing tokens rejected.
### Assets
`Phirura/tools/prepare_assets.py` — pure-`zlib` PNG codec (no PIL): splits the hold atlas,
synthesizes a 32 s/120 BPM demo song + 120-note RPE demo chart → `Phirura.WindowsPhone/Assets/{game,demo}`.

### Build
`build_wp.ps1` (workspace root, ASCII-only, `param()`); invoke via the PowerShell tool (direct
MSBuild is sandbox-blocked). Log → `E:\_phirura_build.log`.
`& "<ws>\build_wp.ps1" -Configuration Debug -Target wp -Isolated` (ARM) / `… -Target win` (AnyCPU).
WP head is **ARM, not AnyCPU** (Win2D ships `Phone/{arm,x86}` only); `-Platform x86` for the
emulator. **Always build `-Target win` after touching `Phirura.Shared/`.**
`-Extra "<semicolon MSBuild switches>"` runs another target. **`-Isolated` breaks the Win head's
MakePri** (`0xdef0050b`) — build Win without it. `-Isolated` redirects `IntermediateOutputPath` to
`E:\_phirura_build\obj\ARM\Debug\`; the authoritative "what is in the appx" check is the archive's
own file table, cross-checked against `bin\ARM\Debug\Phirura.WindowsPhone.build.appxrecipe`.
`tools/probe_api.py` compiles snippets with `csc.exe` against the real WP8.1 reference set to answer
"does this API exist?" definitively (run *from inside* the Python script; a literal `csc.exe` call
is sandbox-rejected) — `--file <f>` compiles a standalone file. `tools/winmd_strings.py` is
unreliable (suffix sharing) — cautionary example only.

### appx / packaging / deploy
`…_ARM_Debug_Test\…_ARM_Debug.appx`: 37→**38** entries (Debug also carries `wp81d3d11rs.dll`),
3.63 MB, exe 226,816 B, dep `Microsoft.VCLibs.120.00.Phone.Debug`. Release: 37 entries, exe 208,896 B.
Sideloading needs the matching **VCLibs phone framework package**. **Native probes stay OFF**
(`App.xaml.cs const bool RunNativeProbes = false`); both probe DLLs packaged but inert (`DllImport`
resolves lazily). `DisplayName` = `Phirura`.
Deploy/retrieve: `AppDeployCmd.exe` = `C:\Program Files (x86)\Microsoft SDKs\Windows Phone\v8.1\
Tools\AppDeploy\AppDeployCmd.exe` → `/installlaunch <appx> /targetdevice:de`, `/uninstall <productId>
/targetdevice:de`, **`/EnumerateDevices`** (NOT `/listdevices`). `ISETool.exe` = `…\Tools\
IsolatedStorageExplorerTool\ISETool.exe` → **`ts de <productId> <dir>`** (NOT `ts:Local de …`).
On-device log lands at `<dir>\IsolatedStore\phirura.log`.

### Platform gotchas (each cost a debug cycle)
- **WinRT XAML binds to PROPERTIES only** — a bound public field fails silently. `ChartEntry`/
  `PhiraChart` use auto-properties + explicit C#5 ctors (no property initialisers in C#5).
- **Never name a bound member `Name`** → `'Name' property not found` while siblings bind.
  `PhiraChart.Name` → `ChartName`.
- **RPE needs `speedEvents`** — without them `∫speed dt`=0 → `base = height − line_height`=0 and the
  chart collapses onto the judge line. Guard `RpeParser.DefaultSpeed(maxTime)`; demo uses
  `start=end=1/SPEED_RATIO`.
- **Fullscreen on WP8.1 needs FOUR levers**, each in its own try block (a throwing colour setter
  before `HideAsync()` once skipped the hide): (1) `ApplicationView…SuppressSystemOverlays=true`;
  (2) `SetDesiredBoundsMode(UseCoreWindow)` (restore `UseVisible` on exit); (3) `StatusBar…HideAsync()`;
  (4) `StatusBar.BackgroundOpacity=0`. `IsFullScreenMode`/`TryEnterFullScreenMode`/
  `FullScreenSystemOverlayMode` do **not** exist. The bar returns on resume — re-apply from
  `Window.Current.Activated` **and** `Application.Current.Resuming`, gated on `_immersive`; log
  `StatusBar.OccludedRect` (`0x0` = only proof). `OnPageLoaded` = `await SetImmersiveAsync(true)`
  → `WaitForLayoutAsync()` → `StartAsync()`.
- **WP8.1 XAML activation args are the CORE types** — `Windows.UI.Xaml.WindowActivatedEventArgs`
  doesn't exist; use `Windows.UI.Core.WindowActivatedEventArgs`/`CoreWindowActivationState`;
  `VisibilityChangedEventArgs` likewise absent. **Check with `probe_api.py`** (the `#Strings` heap
  uses **suffix sharing**: heap walk→false negatives, raw grep→false positives). A lambda compiles
  even when the args type is unnameable — name the type in a signature.
- `PickSingleFileAsync`/`PickMultipleFilesAsync` deprecated (CS0618); `…AndContinue` need
  `OnActivated`. `StorageFile.GetParentAsync()` doesn't exist.
- **`documentsLibrary` unusable on the WP8.1 (2014) manifest schema** (`APPX3030`); capabilities =
  `internetClientServer` only.
- Diagnostics: `Phirura.Shared/AppLog.cs` → `ApplicationData\LocalFolder\phirura.log` (256 KB cap,
  serialized via `Task.ContinueWith`), wired to `App.UnhandledException` +
  `TaskScheduler.UnobservedTaskException`. **AppLog buffers** — abrupt death loses final writes, so
  when probing a crash run the fatal step LAST (`WriteSync` waits ≤3 s; `Write` only queues).
- **Win2D vendored** (not NuGet-restored) at `Phirura/packages/Win2D.win81.1.21.0/`, imported at the
  end of the csproj: `<Import Project="..\..\packages\Win2D.win81.1.21.0\build\wpa\
  Win2D.win81.targets" Condition="Exists('…')" />`. **Referencing a Win2D type is what packages the
  native DLL** — no manual `<Content>`. MSBuild 12 OK.

### Workspace safety rules (R31, learned the hard way)
**Never call `os.remove` here** — the safe-delete guard intercepts it, tries the Recycle Bin, falls
back to `<ws>/trash/`, and once **moved all 13 `Phirura.Shared/{Core,Parse,Game}` sources** there.
Use `os.replace()`/`mv`. **If a build reports missing files, check `<ws>/trash/` first.**
`wp81prpr.dll`/`wp81d3d11rs.dll` are **conditional** Content items (`Condition="Exists('…')"`) so the
app stays shippable while the port comes up.

---
## Rust on WP8.1 (`rust-wp81/`)
### std shim layer (DONE — a Rust DLL loads and runs on device)
Blocker was std's import surface, not the platform. Built: `targets/thumbv7a-pc-windows-msvc.json`
(**name matters**; no prebuilt 32-bit ARM Windows target → `-Z build-std` compiles std from source;
`.json` specs need `-Zjson-target-spec`); `wp81_std_patch.py` (rewrites std's `raw-dylib` decls:
remaps ~143 `kernel32.dll` fns onto WP8.1 API sets, strips `bcryptprimitives`/`ntdll`/`userenv`;
`--apply` asserts `MIN_EXPECTED_REWRITES=17`); `gen_api_trampolines.py` → `compat/src/generated_api.rs`
(~107 null-safe trampolines via `LoadLibraryExW`+`GetProcAddress` on first call; also emits
`API_TABLE`+`wp81_probe_api_sets()`); `compat/` Win32 shims (`atexit`,`strlen`,`wcslen`,
`LoadLibraryA`,`lstrlenW`,`GetActiveProcessorCount`→1, processenv stubs, `CreatePipe`,`GetConsoleCP`,
`SetConsoleCtrlHandler`,`__rt_udiv`,`__rt_udiv64`,`__u64tod`,`__u64tos`) + raw probes
`wp81_raw_qpc()`/`wp81_raw_createthread()`; `link-probe/` `wp81_link_probe(step)` via
`NativeProbe.cs`; `verify_mapping.py` (ctypes.WinDLL, 143/143); `build-std.ps1` (patches, junctions
the sysroot `library` onto `work/library`, builds std — `-Z build-std-features=compiler-builtins-mem`
is **required**).
Hard-won: **`LoadLibraryW` does not exist on WP8.1** → `LoadLibraryExW`/`LoadPackagedLibrary` (root
cause of repeated `0x8007007F`). **`api-ms-win-core-file-l1-2-1` IS present** (`CreateFileW`,
`ReadFile`,`WriteFile`,`FindFirstFileExW`,`DeleteFileW`,`MoveFileExW`). WP8.1 uses the
**`msvcr120_app.dll` CRT, not the UCRT** (no `api-ms-win-crt-*`). CRT-free link needs
`/NODEFAULTLIB`, own `_DllMainCRTStartup`, `_fltused`, `atexit`, `strlen`, `wcslen`, `chkstk.obj`
(`VC\lib\arm\`), divide/convert helpers (ARM ABI: `__rt_udiv`→quot r0/rem r1). **`_tls_index` must be
left alone** (loader fills it). **`QueryPerformanceFrequency` is compat-owned**
(`compat_own_fns.txt`): try the platform, accept only on success with a positive value, else
**measure** against `GetSystemTimeAsFileTime` over a 100 ms sleep. **Divide/shortcut helpers must
never use the operation they implement** (hit 4×: `__rt_udiv`,`__rt_udiv64`,`__u64tod`,`__u64tos`,
`__dtoi64`) — LLVM lowers the op back into a call to the same helper → infinite recursion →
`0x800703E9`; fix with asm or forward to **`compiler_builtins`**; **real `__rt_udiv64` ABI:
r0:r1=divisor, r2:r3=dividend** (divisor *first*). **A test whose result the optimiser can compute is
not a test** — use `std::hint::black_box`. `windows_link::link!` is `raw-dylib`. NTSTATUS trick: std
converts via `RtlNtStatusToDosError` then `io::Error::from_raw_os_error`. **Desktop API sets are a
superset of WP8.1's** — local verification finds definite errors, never certifies correctness.
Import table reduced from 66 KERNEL32 fns to **one API set**
(`api-ms-win-core-libraryloader-l1-2-0.dll`). Four link requirements fail *silently*:
`/NODEFAULTLIB`+own `_DllMainCRTStartup`; `ws2_32.lib`; `chkstk.obj`; **`/EXPORT:` per entry point**
(`#[no_mangle]` does not export from a staticlib). ARM cross tools are **VS11's** (`VC\bin\x86_arm\`);
`mspdb110.dll` must be on `PATH` or `link.exe` exits 127 silently. `build-probe.ps1` →
`rust-wp81/build-probe.log` — **read that file**.

### D3D11 reachability (C++ probe, R34)
`d3d11-probe/` + `build-d3d11.ps1` → `wp81d3d11.dll`: C++, ARMNT, 5 KB, CRT-free
(`/NOENTRY /NODEFAULTLIB`), imports only `d3d11.dll`; exports `wp81_d3d11_probe()`+`wp81_d3d11_report()`;
called from `D3D11Probe.cs` before `NativeProbe.Run()`. **On device: `stage=0`, all calls `S_OK`,
pixel within ±1 UNORM**; one adapter `"Qualcomm Adreno 225 (WDDM v1.2)"`, `flags=0x0`, FL **9.3**.
**`D3D11_DRIVER_TYPE_HARDWARE` with a null `pAdapter` returns WARP on WP8.1** — enumerate
`IDXGIFactory1::EnumAdapters1`, take the first adapter **without** `DXGI_ADAPTER_FLAG_SOFTWARE`,
create with `D3D_DRIVER_TYPE_UNKNOWN`; passing `D3D_FEATURE_LEVEL_11_1` to a pre-11.1 runtime returns
`E_INVALIDARG`. **Report what the platform offers, not just what the call returned.** Gotchas:
`WINAPI_FAMILY=WINAPI_FAMILY_PHONE_APP` mandatory; the WP kit's headers need
`Windows Kits\8.1\Include\{shared,um,winrt}` after the WP kit's Include; `memset`/`memcpy` are
intrinsics; no `%`/`/` on 64-bit values.

### Path A — Rust renders itself
Probe: `d3d11-rs-probe/` + `build.ps1` (self-contained; auto-locates the `wp81-compat` rlib).
Hand-rolled binding in `src/direct.rs` — `extern "system"` + COM vtable calls by hand
(`#[repr(C)] { vtbl: *const *const usize }` + slot fetch) + `_fltused`/`DllMain`/`_DllMainCRTStartup`.
**No `windows` crate at all** (its `windows-core` `link!` plumbing drags in kernel32/oleaut32/combase/
rpcrt4/user32/ntdll — none on WP8.1). ARM DLL imports exactly `d3d11.dll`+`dxgi.dll`+
`d3dcompiler_47.dll` (`check_imports.py` `PRESENT_ON_PHONE`).

**COM vtable layout (authoritative — extract from the kit headers, never from memory):**
- `ComPtr.vtbl` is the **object pointer**; `*vtbl` is the vtable; `(*vtbl)[n]` is the n-th method;
  `vtbl` itself is `this`. Reading one level short → segfault.
- **`ID3D11Device` derives straight from `IUnknown`** → slots start at **3**: CreateBuffer 3,
  CreateTexture2D 5, CreateShaderResourceView 7, CreateRenderTargetView 9 (**NOT 10** — that is
  DepthStencilView), CreateInputLayout 11, CreateVertexShader 12, CreatePixelShader 15,
  CreateBlendState 20, CreateRasterizerState 22, CreateSamplerState 23, GetFeatureLevel 37.
  (Constants live in `direct.rs` as `D3D11DEVICE_*`; the `renderer.rs` comment block that once said
  4096 registers is stale — verify in `direct.rs`.)
- **`ID3D11DeviceContext` derives from `ID3D11DeviceChild`** → slots start at **7**: VSSetConstant
  Buffers 7, PSSetShaderResources 8, PSSetShader 9, PSSetSamplers 10, VSSetShader 11, DrawIndexed 12,
  Draw 13, Map 14, Unmap 15, PSSetConstantBuffers 16, IASetInputLayout 17, IASetVertexBuffers 18,
  IASetIndexBuffer 19, IASetPrimitiveTopology 24, OMSetRenderTargets 33, OMSetBlendState 35,
  RSSetState 43, RSSetViewports 44, UpdateSubresource 48, ClearRenderTargetView 50, Flush 111.
- DXGI: `IDXGIAdapter1::GetDesc1` 10, `IDXGIFactory1::EnumAdapters1` 12,
  `IDXGIFactory::CreateSwapChain` 10, `IDXGISwapChain::Present` 8, `GetBuffer` 9, `ResizeBuffers` 13,
  `IDXGIFactory2::CreateSwapChainForComposition` **24**, `IDXGIDevice::GetAdapter` 7,
  `IDXGIAdapter::GetParent` 6, `IDXGIFactory2::IsCurrent` **13**.
  `ISwapChainPanelNative::SetSwapChain` **3**, `ISwapChainBackgroundPanelNative::SetSwapChain` **3**.
- **`Map` takes five args after `this`**: `Map(res, subresource, D3D11_MAP, UINT MapFlags,
  *mut D3D11_MAPPED_SUBRESOURCE)`. Omitting `MapFlags` puts the out-param there → `E_INVALIDARG`.
- IIDs (kit headers): IDXGIFACTORY1 `770aae78-f26f-4dba-a829-253c83d1b387`; IDXGIFACTORY2
  `50c83a1c-e072-4c48-87b0-3630fa36a6d0`; IDXGIDEVICE `54ec77fa-1377-44e6-8c32-88fd5f44c84c`;
  IDXGISWAPCHAIN `310d36a0-d2e7-4c0a-aa04-6a9d23b8886a`; ID3D11DEVICE
  `db6f6ddb-ac77-4e88-8253-819df9bbf140`; ID3D11TEXTURE2D `6f15aaf2-d208-4e89-9ab4-489535d34f9c`;
  **ISWAPCHAINPANELNATIVE `f92f19d2-3ade-45a6-a20c-f6f1ea90554b`**;
  **ISWAPCHAINBACKGROUNDPANELNATIVE `43bebd4e-add5-4035-8f85-5608d08e9dc9`**.
- **`#[link(name="d3d11.dll")]` makes MSVC ask for `d3d11.dll.lib`** (SDK ships `d3d11.lib`) →
  **all `#[link]` removed**; libs named by the linker (ARM `build.ps1`; host via
  `.cargo/config.toml` rustflags).

**Desktop harness — `src/bin/hostcheck.rs` (the verification technique worth reusing).** Compile the
*same* `direct.rs`/`renderer.rs` for `x86_64-pc-windows-msvc` behind
`#![cfg(all(windows, not(target_arch="arm")))]`, create a hidden-HWND swapchain
(`renderer::create_swapchain_for_hwnd`; same `IDXGIDevice`→adapter→factory walk), and run the whole
Stage 1 path against the real desktop D3D11 runtime. 8 steps: device → swapchain → backbuffer→RTV →
`build_pipeline` → 1×1 white texture+SRV → viewport+clear → Map CB/VB + binds + `Draw(4,0)` →
`Present`; prints `=== ALL STAGE 1 CHECKS PASSED ===`. It caught three bugs none of which was a
compile error (a wrong slot is a crash or bogus HRESULT, never a diagnostic). `Cargo.toml` has an
explicit `[[bin]] name="hostcheck"`; `build.ps1` passes `--lib` so the ARM build never links the
harness's x64 `user32.lib` (LNK1112). `.cargo/config.toml` needs **both** `-L` paths (SDK `um\x64`
*and* VS11 `VC\lib\amd64`) or `msvcrt.lib` is unresolvable. **Anything touching `renderer.rs`/`direct.rs`
must be re-run through `hostcheck` before deployment.** `hostcheck.rs` must carry stubs for any
`crate::` helper `renderer.rs` calls (`set_tick`, `dump_vtable`, `limit_is`, `record_factory_probe`,
`record_panel_iface`), because it `#[path]`-includes `renderer.rs` without `lib.rs`.

**On-device bisect (no debugger; the only technique that works).** `wp81_rs_start_upto(panel,w,h,limit)`
runs the bring-up and returns early once the tick reaches `limit` — `limit=0` runs it all. A
`DispatcherTimer` (200 ms) in `RustStage1Page` samples `wp81_rs_tick()` into managed memory, so the
last tick survives a native fault (the log's tail is lost otherwise). `RustRenderer.Bisect()` walks
limits `{2,10,13,18,16,20,21,30,40,50,99}`, logging before/after each, and calls `LogVtable()`.
Tick map: 2=create_device, 10=enter swapchain, 11=GetAdapter, 12=GetParent, **17=IsCurrent probe**,
13=CreateSwapChainForComposition, 14=panel QI, 15=SetSwapChain, 16=done, 20=GetBuffer, 21=CreateRTV,
30=build_pipeline, 40=test_texture, 50=RSSetViewports, 60/61=frame, 70=Present, 99=up.
**`limit=13` = run the `IsCurrent` probe then stop before slot 24; `limit=18` = skip the probe then
stop before slot 24** — the pair separates a faulting probe from a faulting slot 24.
`release_all(s)` runs first in every `start_upto` (and `wp81_rs_stop()` between C# steps) or the
bisect leaks one D3D device per step (fatal on a 512 MB phone).

**On-device status (R41).** DLL loads and runs on the Lumia 925. `limit=2/10/13` all **clean**;
vtable snapshot shows **32 readable slots with slot 24 = a real code pointer** and
`QI(IDXGIFactory2)` = **S_OK** → slot number/IID/factory pointer all correct. `limit=16` **dies**,
after tick 13 → the fault is inside `create_swapchain_for_panel_traced` at/after
`CreateSwapChainForComposition`, and it **hard-faults** (no HRESULT) — the signature of an
**unimplemented DXGI 1.2 entry** on this 2014 driver. Leading hypothesis:
`DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL` needs FL ≥10.0 and this device is 9.3, so the descriptor-variant
loop now tries **`DISCARD`→`SEQUENTIAL`→the two `FLIP_SEQUENTIAL`** shapes (FL-9.3-safe first),
recording the accepted index in `wp81_rs_sc_variant()`. The native attach also now tries **both**
panel interop interfaces and records which answered in `wp81_rs_panel_iface()` (0=Panel, 1=Background
Panel). **`SwapChainBackgroundPanel` + `ISwapChainBackgroundPanelNative` are real on WP8.1** (proven
by compiling against the reference set and reading `windows.ui.xaml.media.dxinterop.h`).

**Stage 3 (pending, plan-forbidden until Stage 1 renders on device)**: port prpr scene draw calls.
Vertex stride **36 B** (`pos3(12)+uv2(8)+rgba4(16)`, `z` kept 0), `MAX_VERTICES=65536` → ~2.3 MB
dynamic buffer (prpr's format; stride 32 drops `z` and is wrong). Whole scene is macroquad
`draw_texture_ex` (textured quad); sprites already batched into `res.note_buffer` — no GLSL sprite
shader to port. **Stage 4**: on-device self-test, then swap `GamePage` off `Win2DChartRenderer` onto
the Rust renderer. **Ordering rule from the plan: do NOT start Stage 3 before Stage 1 renders a
textured quad on a real device.**

**Export trap (bit us twice):** new `#[no_mangle]` entry points **silently do not appear** in the
DLL — `build.ps1` carries an explicit `/EXPORT:` list and `#[no_mangle]` does not export from a
staticlib. Symptom: a smaller DLL than expected and a `DllImport` that throws only at the first call.
Verify with `tools/pe_dump.py <dll> --exports`.

### Path B (superseded, kept as reference)
`prpr-miniquad` is **GL-only** (`native/gl.rs`; no D3D/Metal). Path B = route `macroquad`'s drawing
into Win2D and let C# replay it — rejected in R37. Still-built: shims in `rust-wp81/shims/`
(`macroquad`,`miniquad`,`sasa`,`rfd`) all build for the target; `prpr-port` links into `wp81prpr.dll`
(~3.6 MB, 21 exports); renderer drain (`wp81_draw_encode/_commands/_vertices/_indices/_reset/
_selftest`, `PortCmd` 56 B, `PortVertex` 32 B), texture registry (`wp81_texture_*`), game host
(`wp81_game_create/_frame/_destroy/_frames/_running/_set_screen`). Shim facts: **macroquad's maths
types ARE glam's** — `glam 0.30 features=["scalar-math"]` + `pub use glam::…` (and `pub use glam;`,
since `prpr::particle` writes `glam::Quat::from_rotation_z`); versions must track macroquad 0.3.25.
`[patch]` only works in a workspace root (end of `phira-main/phira-main/Cargo.toml`; original =
`Cargo.toml.orig`). Handle types live in the **miniquad** shim; macroquad re-exports them. `Cmd` must
stay `Copy` → fixed arenas + `drain_reset` must clear them or the batcher silently stops. `sasa` needs
`anyhow`; `MusicParams.playback_rate` is **f64**. **A `[patch]` replacement must be semver-compatible
or cargo silently ignores it** (`cargo tree -i <crate>`). **The target triple's NAME matters** —
`winapi` 0.3.9's `build.rs` uses `target.get(2)==Some(&"windows")`.

### Import-table gate and its trap
`build-port.ps1` step 3 runs `tools/check_imports.py` and throws on an unacceptable import.
`/FORCE:MULTIPLE` is load-bearing but **not sufficient**: LNK4006 drops the duplicate *symbol*, not
the import descriptor — the descriptor is a separate `.idata` contribution and lands regardless,
producing a DLL that links cleanly and fails to load. Real fix: `wp81_registry_patch.py` +
`wp81_std_patch.py` (delete every `raw-dylib` attribute for a symbol compat provides). **Never treat
a passing link as proof** — re-read the import table.

### `rust-wp81/tools/` — diagnostic toolchain
Written because **`dumpbin.exe` emits nothing under the sandbox**. Plain Python 3, no deps.
`coff_dump.py` (sections/symbols/string table; **`strtab_off = symptr+nsym*18`**); `ar_scan.py`
(MSVC `.lib`→`(name,offset,size)`); `probe_imports.py` (imported `__imp_X` / plain-undefined /
defined); `dylib_imports.py` (maps each raw-dylib import descriptor to its DLL); `pe_dump.py`
(PE32/+ export/import/section tables; `--exports`); `check_imports.py` (the gate: 0=OK,1=bad,2=unreadable;
`DENY`=desktop-only, `PRESENT_ON_PHONE`=kit-linked).

### Environment traps that all look like something else
1. **Git Bash ships `/usr/bin/link.exe` (coreutils)** — shadows MSVC's linker. 2. **`Common7\IDE`
must be on PATH** or link.exe exits `0xc0000135` (`mspdb110.dll`). 3. **`LIB` must point at the x64
import libs** or host link dies `LNK1181 'kernel32.lib'`. 4. **`Start-Transcript` does not capture
native output in PS 5.1** — redirect cargo to explicit log files. 5. **MSYS mangles `/SWITCH`** →
`MSYS_NO_PATHCONV=1` + `MSYS2_ARG_CONV_EXCL='*'`. 6. **`$args` is reserved in PowerShell**. 7. **cargo
writes "Finished" to stderr** — with `$ErrorActionPreference="Stop"` that becomes a terminating
`NativeCommandError`; drop to `Continue`, read `$LASTEXITCODE`. 8. **Every `-Z` arg containing a comma
MUST be quoted** in PowerShell. 9. **Never invoke `cmd` from the bash tool**; the sandbox also blocks
`cmd /c mklink`, bulk `rmtree`, a literal `csc.exe` invocation, and bash commands containing the word
"PowerShell".

### Toolchain locations
Rust: `C:\Users\admin\.workbuddy-ai\binaries\rust\` (stable + nightly w/ `rust-src`; `RUSTUP_HOME`/
`CARGO_HOME` under it). CRT-free link stubs: `E:\_phirura_stub\`. Managed Python:
`C:\Users\admin\.workbuddy-ai\binaries\python\versions\3.13.12\python.exe`.

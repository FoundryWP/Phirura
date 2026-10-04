# Phirura — Phira → Windows Phone 8.1

Deploy: `phirura.log`; **deep reference (vtable slots, IIDs, constants, full eliminated-hypothesis
log, tool/env catalogues) lives in `REFERENCE.md` — read it before touching any slot number or GUID.**

`Phirura/Phirura.sln` → `Phirura.Windows/` (Win8.1, AnyCPU), `Phirura.WindowsPhone/` (WP8.1, **ARM**;
real dir `Phirura/Phirura/Phirura.WindowsPhone/`), source-shared `Phirura.Shared` (shproj). Read-only
ref: `phira-main/phira-main/` (`prpr`, `prpr-*`). 2nd workstream: `rust-wp81/`. Daily logs:
`.workbuddy-ai/memory/`.

**User-mandated direction: the real Rust game must run AND render on WP8.1.** The C#-replay path
(Rust emits cmds → C# repaints via Win2D) was rejected as "a C# renderer with a Rust backend".
**Path A: Rust owns D3D11 directly.** Plan: `rust-wp81/PLAN-PATH-A.md` (4 stages). Device: **Lumia 925
/ RM-910, Adreno 225, WDDM 1.2, FL 9.3 (`0x9300`)**, product id
`fa2c9634-5ad8-487e-be3c-9893871fda9f`. **Ordering rule: do NOT start Stage 3 before Stage 1 renders a
textured quad on a real device.**

---
## Current state (R48)

**Stage 1 = XAML `SurfaceImageSource` route (route 3). Deployed and running on device without
crashing.** Bring-up reached tick 286; the first bug was `SURFACE_IMAGE_SOURCE_RUNTIME_CLASS` declared
`[u16; 56]` for a **48**-char name, so `with_hstring` passed `length = 56` and WinRT looked up
`"…SurfaceImageSource\0\0\0\0\0\0\0\0"` → activation failed with `hr = S_OK` but a **null factory**.
Fixed (array is now `[u16; 48]`); also fixed the diagnostic that hid it (`SurfaceChain::create` now
returns `Err((step, hresult))` and the caller records both — it used to hard-set `sis_trace[1] = 0`).
**Never over-declare a UTF-16 literal array: `literal.len()` is what gets used.**

**Present path, solved.** Two mechanisms were exhaustively eliminated — do not revisit:
1. **A WP8.1 *phone* app has no swapchain.** `CreateSwapChainForCoreWindow` (16) and
   `CreateSwapChainForComposition` answer `DXGI_ERROR_INVALID_CALL` for every documented shape on this
   Adreno 225 / FL 9.3 driver. Win2D's ARM DLL renders here and imports **no swapchain anything**.
2. **The D3D↔WinRT interop helpers are unreachable.** On-device probe:
   `interop-trace=winrt{h=0,exp=0} d3d11{h=1,exp=0}` — `winrt.dll` will not load from an app container
   and `d3d11.dll` does not export the helper. The kit's ARM `d3d11.lib` is a *limited* import lib; no
   `winrt.lib` exists.

**What works:** `runtimeobject.lib` (ships in the kit; linked in `build.ps1`) imports
`RoGetActivationFactory`/`WindowsCreateString` from the API set an app container *may* load — the same
one Win2D uses. So an app **can activate WinRT classes**; it just cannot ask for the D3D helpers. That
suffices, because `SurfaceImageSource` is a XAML type: `SetDevice(IDXGIDevice*)` once →
`BeginDraw(RECT, IDXGISurface**, POINT*)` per frame → `EndDraw()`. The returned `IDXGISurface` lives
**inside the compositor's own render target**, so drawing into it *is* presenting — no swapchain, no
interop helper, no offscreen texture, no per-frame blit. `ISurfaceImageSourceNative` comes from a
**plain `QueryInterface`** (no D3D helper). `SurfaceImageSource` **is** a
`Windows.UI.Xaml.Media.ImageSource`, so C# sets `Image.Source` from `wp81_rs_sis_source()` via
`Marshal.GetObjectForIUnknown` — the entire managed involvement, which is what keeps this Path A.

**Route wiring:** `route` 0=swapchain, 1=offscreen+interop, 2=CoreWindow, **3=XAML SurfaceImageSource**,
-1=none. Files: `d3d11-rs-probe/src/{direct.rs,surface_source.rs,lib.rs}`, `build.ps1`;
`Phirura.WindowsPhone/{RustRenderer.cs,RustStage1Page.xaml(.cs)}`.
`wp81_rs_start_surface(w,h)` ticks: 2 create_device → 285 QI `IDXGIDevice` → 286 activate+create+QI
native → 287 `SetDevice` → 30 `build_pipeline` → 40 `create_test_texture` → 99 done. `SIS_BASE=500`+n
step codes (1 activation factory, 2 create instance, 3 QI native, 4 SetDevice, 5 BeginDraw, 6 EndDraw,
7 QI surface, 8 CreateRTV, 9 QI DXGIDevice) — `wp81_rs_sis_step_name()` names the failing call.
**`wp81_rs_draw_test_quad` is the whole frame on route 3**: `BeginDraw` → RTV over `frame_surface` →
`OMSetRenderTargets` → `RSSetViewports` offset by `frame_offset` → normal binds → `Draw(4,0)` →
`EndDraw` (that *is* the present; `wp81_rs_present` only reports `frame_err`). C# uses no Win2D at all
on this route.

## C# app (shippable product until Path A lands)
Gameplay core (chart model, tweens, RPE/PEC parsers, judgement, scoring) = line-by-line C# port of
`prpr` in `Phirura.Shared/` (Core, Parse, Game, Net). WP head renders via **Win2D**
(`Win2DChartRenderer.cs`); `ChartRenderer` (pooled XAML Images) remains for the Win8.1 head.
- **`CanvasControl`, not `CanvasAnimatedControl`** — the animated control runs Update/Draw off the UI
  thread, but judge + HUD must stay on it (`MusicClock.Resync()` reads `MediaElement.Position`;
  `UpdateHud` writes `TextBlock`s). `CompositionTarget.Rendering` kept: `Render()`/`RenderBadNote()`
  only *record*, `Present()` calls `Invalidate()`.
- `UseSharedDevice="True"` + `CanvasDevice.GetSharedDevice()`, 96 DPI so `Bounds == SizeInPixels`.
- Camera (macroquad `zoom.y=-aspect_ratio`, model `diag(1,-1)`, chart Y ÷ aspect):
  `screenX=(x+1)/2*W`, `screenY=(1-y*ar)/2*H`. `Mat3`→`Matrix3x2`:
  `M11→M11,M12→M12,M21→M21,M22→M22,M13→M31,M23→M32` (both row-vector).
- Holds: Phira `hold.png` (`holdAtlas:[50,50]`, 1000x1100): top 50px = tail, bottom 50px = head,
  middle = body; drawn in a local y-down frame.
- Audio: `MediaElement.Position` refreshes a few times/s → `MusicClock` runs a `Stopwatch` as master,
  re-anchors when drift > 40 ms.

### Online (`phira/src/client.rs` → `Phirura.Shared/Net/PhiraApi.cs`)
- `POST /login` `{email,password,clientVersion}` → `{id,token}`; `GET /me` (Bearer).
- `GET /chart?page=N&pageNum=M&search=&order=` → `{count,results:[Chart]}`; order
  `updated|name|difficulty|rating`, `-` reverses; **paging 1-based**. Archive = `chart.file.url` zip.
- **`file`/`illustration`/`preview` are bare URL strings**; `uploader` = bare id;
  `PhiraApi.FileUrlOf` accepts both shapes.
- Real archive has **no `chart.json`** (`…jpg info.txt …json info.yml …mp3`). `PickChartFile` takes any
  `.json` except `info.json`; `PickIllustration` falls back to the first image.
- **PEC ships as `.json`** (line 1 = bare offset). `ChartParser.Parse` must **sniff content**: RPE `{`,
  PEC bare number. **Do not "fix" the PEC note mapping from memory** — `parse/pec.rs`: `n1`=Click,
  `n2`=Hold (6 tokens), `n3`=Flick, `n4`=Drag; `bp`=2 tokens, `n1`/`n3`/`n4`=5, `cv`=3, `cp`=4;
  trailing tokens rejected.

### Build / deploy
`build_wp.ps1` (workspace root, ASCII-only, `param()`); invoke **via the PowerShell tool** (direct
MSBuild and `Add-Type` are sandbox-blocked). `& "<ws>\build_wp.ps1" -Configuration Debug -Target wp
-Isolated` (ARM) / `… -Target win` (AnyCPU). WP head is **ARM, not AnyCPU** (Win2D ships
`Phone/{arm,x86}` only). **Always build `-Target win` after touching `Phirura.Shared/`.** `-Isolated`
breaks the Win head's MakePri (`0xdef0050b`) — build Win without it.
Rust ARM DLL → copy `rust-wp81/d3d11-rs-probe/out/wp81d3d11rs.dll` → `Phirura.WindowsPhone/`, then
rebuild the WP head. Deploy: `AppDeployCmd.exe /installlaunch <appx> /targetdevice:de`
(`/EnumerateDevices`, NOT `/list`; invoke **via PowerShell with variables**). Retrieve log:
`ISETool.exe ts de <productId> <dir>` — **works iff `<dir>` does not already exist**; delete it first.
`tools/probe_api.py --file <bare-name>` compiles a snippet against the real WP8.1 reference set
(`MSYS_NO_PATHCONV=1`; errors are GBK → `iconv -f gbk -t utf-8`).

### Platform gotchas (each cost a debug cycle)
- **WinRT XAML binds to PROPERTIES only** — a bound public field fails silently. **Never name a bound
  member `Name`** (`'Name' property not found` while siblings bind) — `PhiraChart.Name` → `ChartName`.
- **RPE needs `speedEvents`** — without them `∫speed dt`=0 → chart collapses onto the judge line.
  Guard `RpeParser.DefaultSpeed(maxTime)`; demo uses `start=end=1/SPEED_RATIO`.
- **Fullscreen on WP8.1 needs FOUR levers**, each in its own try block: `SuppressSystemOverlays=true`;
  `SetDesiredBoundsMode(UseCoreWindow)`; `StatusBar.HideAsync()`; `StatusBar.BackgroundOpacity=0`.
  The bar returns on resume — re-apply from `Window.Current.Activated` **and**
  `Application.Current.Resuming`, gated on `_immersive`. `OnPageLoaded` = `await
  SetImmersiveAsync(true)` → `WaitForLayoutAsync()` → `StartAsync()`.
- **WP8.1 XAML activation args are the CORE types** (`Windows.UI.Core.WindowActivatedEventArgs`);
  `Windows.UI.Xaml.WindowActivatedEventArgs` does not exist. **Check with `probe_api.py`** (the
  `#Strings` heap uses suffix sharing → both false negatives and false positives).
- **`System.IO.File` does NOT exist** in the WP8.1 shared-project profile (CS0234).
  `System.IO.Path`/`MemoryStream`/`Compression` do.
- **`documentsLibrary` unusable on the WP8.1 (2014) manifest schema** (`APPX3030`); capabilities =
  `internetClientServer` only.
- Diagnostics: `Phirura.Shared/AppLog.cs` → `ApplicationData\LocalFolder\phirura.log` (256 KB cap).
  **AppLog buffers — run the fatal step LAST.** A `DispatcherTimer` cannot sample while the UI thread
  is blocked in a synchronous native call, so a `System.Threading.Timer` bg watchdog exists — **but
  native bring-up ON the UI thread blocks WinRT `FileIO` too, so keep native calls on a pool thread
  (`Task.Run`).** That split is what finally made the log survive a fault.
- **Win2D is vendored** (not NuGet-restored) at `Phirura/packages/Win2D.win81.1.21.0/`, imported at the
  end of the csproj. **Referencing a Win2D type is what packages the native DLL** — no manual
  `<Content>`.

### Workspace safety rules (learned the hard way)
**Never call `os.remove` here** — the safe-delete guard intercepts it and once **moved all 13
`Phirura.Shared/{Core,Parse,Game}` sources into `<ws>/trash/`**. Use `os.replace()`/`mv`. **If a build
reports missing files, check `<ws>/trash/` first.**

---
## Rust on WP8.1 (`rust-wp81/`)

**std shim layer DONE** — a Rust DLL loads and runs on device. `targets/thumbv7a-pc-windows-msvc.json`
(**name matters**; no prebuilt 32-bit ARM Windows target → `-Z build-std`) + the `wp81-compat` shim
crate supplying ~31 missing std symbols. **`std::fs`/`PathBuf`/`format!` in the native DLL are
harmful** — grew it 19,968 → 116,736 B, added an import, and the DLL then failed to load (instant crash
at first native call). Keep it small; the ARM DLL is now 26,624 B / 6 imports, all shippable.

**Path A probe** = `d3d11-rs-probe/` + `build.ps1`. Hand-rolled binding in `src/direct.rs`
(`extern "system"` + COM vtable calls by hand; `ComPtr.vtbl` is the **object** pointer, `*vtbl` the
vtable, `vtbl` itself is `this`). **No `windows` crate** — its `link!` plumbing drags in
kernel32/oleaut32/combase/rpcrt4/user32/ntdll, none on WP8.1. **All `#[link]` removed** (MSVC would ask
for `d3d11.dll.lib`); libs are named by the linker in `build.ps1`. **Slot numbers, IIDs and DXGI
constants: see `REFERENCE.md` — never from memory (a wrong slot is silent corruption).**

**Export trap (bit us twice):** new `#[no_mangle]` entry points **silently do not appear** in the DLL —
`build.ps1` carries an explicit `/EXPORT:` list. Verify by parsing the DLL's export table (currently 45
names). Likewise **link ≠ emitted**: an unreferenced `extern` produces no import descriptor, so grep
the built binary for the symbol name.

**`hostcheck.rs` = the verification technique worth reusing.** Compiles the *same*
`direct.rs`/`renderer.rs` for `x86_64-pc-windows-msvc` and runs the whole Stage 1 path against the real
desktop D3D11 runtime (`=== ALL STAGE 1 CHECKS PASSED ===`). It caught three bugs, **none of which was
a compile error**. **Anything touching `renderer.rs`/`direct.rs` must go through `hostcheck` before
deployment.** `build.ps1` passes `--lib` so ARM never links the harness's x64 `user32.lib` (LNK1112).

**On-device bisect (no debugger; the only technique that works).** `wp81_rs_start_upto(panel,w,h,limit)`
returns early once the tick reaches `limit` (0 = run all); a 200 ms timer samples `wp81_rs_tick()` into
managed memory so the last tick survives a fault. **`release_all(s)` runs first in every `start_upto`**
(or the bisect leaks one D3D device per step — fatal on 512 MB) and must release `surface` +
`dxgi_device`.

**A hard fault at `limit=16` was a USE-AFTER-FREE, not a driver bug.** `this.Loaded += OnPageLoaded`
with no `-=` plus `async void StartRenderer` with no gate → several bring-ups ran concurrently, racing
on Rust's single global `STATE`, each starting with `release_all()` — one freed another's device
mid-call. **Fixed** with `-=` before `+=` and an `Interlocked.CompareExchange` gate. **Lesson: any
native bridge with one global STATE must have exactly one C# caller in flight** — and a bug that "goes
away" when you move a probe is a timing/race bug.

**Stage 3 (plan-forbidden until Stage 1 renders):** port prpr scene draw calls. Vertex stride **36 B**
(`pos3(12)+uv2(8)+rgba4(16)`, `z` kept 0), `MAX_VERTICES=65536` → ~2.3 MB dynamic buffer (stride 32
drops `z` and is wrong). The whole scene is macroquad `draw_texture_ex`; sprites are already batched
into `res.note_buffer` — no GLSL sprite shader to port. **Stage 4:** on-device self-test, then swap
`GamePage` off `Win2DChartRenderer` onto the Rust renderer.

**Path B is superseded** (kept as reference in `REFERENCE.md`): `prpr-miniquad` is GL-only, so Path B
meant replaying macroquad's draws through Win2D — rejected. `wp81prpr.dll` (~3.6 MB) still builds.

**Diagnostics:** `dumpbin.exe` emits nothing under the sandbox → `rust-wp81/tools/*.py` (plain Python,
no deps): `check_imports.py` is the gate (0=OK); the rest are listed in `REFERENCE.md`. **Never treat a
passing link as proof** — re-read the import table.

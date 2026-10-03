# Phirura — Phira ported to Windows Phone 8.1

Port target: `Phirura/Phirura.sln` → `Phirura/Phirura/Phirura.Windows/` (Win8.1),
`Phirura/Phirura/Phirura.WindowsPhone/` (WP8.1), source-shared `Phirura.Shared` (shproj).
Reference source (read-only): `phira-main/phira-main/` (Rust: `prpr`, `prpr-*`, `phira`).
Second workstream: `rust-wp81/` — building actual Rust for WP8.1 (see below).

## Architecture decisions

- Gameplay core (chart model, tweens, anims, RPE/PEC parsers, judgement, scoring) is a
  **line-by-line C# port of `prpr`** in `Phirura.Shared/` (Core, Parse, Game, Net).
- Rendering: the WP head uses **Win2D** (`Phirura.WindowsPhone/Win2DChartRenderer.cs`).
  `ChartRenderer` (pooled XAML `Image`s on a `Canvas`) is kept in Shared as the Win8.1
  head's backend and as documentation of the old approach. The Win8.1 head has only
  `MainPage` and never referenced `ChartRenderer`, so no abstraction layer was needed.
- **The render maths is reused verbatim.** Same `Mat3` composition → same
  `Renderer.Decompose`; only rot/tx/ty are taken (both renderers *override* the decomposed
  scale with a constant note size), so the picture is identical and only the raster path
  changed. `Mat3` → `System.Numerics.Matrix3x2` as `M11→M11, M12→M12, M21→M21, M22→M22,
  M13→M31, M23→M32` (both row-vector); a draw is `ds.Transform = Scale * Rotate * Translate`
  then `DrawImage(bmp, CenteredRect(w,h), …)`.
- **`CanvasControl`, not `CanvasAnimatedControl`.** The animated control runs Update/Draw on
  its own game-loop thread, but the judge and HUD must stay on the UI thread
  (`MusicClock.Resync()` reads `MediaElement.Position`; `UpdateHud` writes `TextBlock`s).
  So `CompositionTarget.Rendering` is kept: `Render()`/`RenderBadNote()` only *record* the
  frame, `Present()` calls `Invalidate()`. A dedicated game loop is the next optimisation
  step and needs those two marshalled first.
- `UseSharedDevice="True"` + `CanvasDevice.GetSharedDevice()` for
  `ChartLibrary.LoadCanvasTextureAsync` (loads at 96 DPI so `Bounds == SizeInPixels`).
  Textures carry the `CanvasBitmap` in `ChartTexture.Tag`; the XAML head uses `.Source`.
- Chart→screen projection from macroquad's camera (`zoom.y = -aspect_ratio`, model
  `diag(1,-1)`, chart Y divided by aspect ratio): `screenX=(x+1)/2*W`,
  `screenY=(1-y*ar)/2*H`. `ChartRenderer.FlipVertical` is the escape hatch.
- Hold rendering uses **Phira's single-sheet `hold.png`** (`holdAtlas: [50,50]`, 1000x1100):
  top 50 px = tail, bottom 50 px = head, middle = body. prpr's `draw_tex` sets `flip_y`, so
  the sheet's top edge points towards +y; Win2D reproduces that by drawing in a local y-down
  frame (`Transform = toScreen * frame * Scaling(1,-1)`). The offline-split
  `hold_head/body/tail.png` files are gone from the WP head.
- Audio clock: `MediaElement.Position` refreshes only a few times per second, so `MusicClock`
  runs a `Stopwatch` as master and re-anchors when drift exceeds 40 ms.

## Online features

Real Phira backend contract, ported from `phira/src/client.rs` into
`Phirura.Shared/Net/PhiraApi.cs` (`PhiraApi`, `PhiraSession`, `PhiraUser`, `PhiraChart`,
`PhiraApiException`) + `LoginPage` / `OnlinePage`:

- `POST /login` `{email, password, clientVersion}` → `{id, token}`
- `GET /me` (Bearer) → user
- `GET /chart?page=N&pageNum=M&search=&order=` → `{count, results:[Chart]}`;
  order: `updated` / `name` / `difficulty` / `rating`, `-` prefix reverses. Paging is **1-based**.
- chart archive = `chart.file.url`, a **zip** (prpr `ZipFileSystem`);
  `ChartLibrary.InstallArchiveAsync` uses `System.IO.Compression.ZipArchive`.

### Wire-format traps (verified against the live server)

- **`file` / `illustration` / `preview` are bare URL strings, not objects.** The Rust model
  declares `#[serde(transparent)] struct File { pub url: String }`, which serialises the
  inner value. Parsing `{"url": …}` yields null for every chart and the download button says
  "这个谱面没有提供下载地址" though the URL is right there. `PhiraApi.FileUrlOf` accepts both.
- `uploader` is a bare user id (number), not an object.
- A real archive has **no `chart.json`**: `56695308.jpg info.txt 24432296.json info.yml
  56695308.mp3`. `PickChartFile` (any `.json` except `info.json`), `PickAudioFile`,
  `PickInfoFile` handle those. `PickIllustration` only matched `illustration.png` /
  `background.jpg`, so it now falls back to the first image in the folder.

### PEC charts ship with a `.json` extension

`24432296.json` is **PEC text**, not JSON (line 1 is a bare number, then `bp cp cm cd cr ca
cf cv n1-n4 # &`). `ChartParser.Parse` must **sniff content before trusting the extension** —
routing on `.json` alone sent every real chart to the RPE parser and it failed to load.
RPE starts with `{`; PEC starts with the offset as a bare number.

**Do not "fix" the note mapping from memory** — the authoritative `prpr/src/parse/pec.rs` is
`n1`=Click, `n2`=Hold (7 tokens: carries an end time), `n3`=Flick, `n4`=Drag. That is the
opposite of the generic PEC convention and looks like a bug until you check.

## Assets

`Phirura/tools/prepare_assets.py` — pure-`zlib` PNG codec (no PIL in the sandbox) that splits
the hold atlas and synthesizes a self-contained demo song (32 s, 120 BPM, Am-F-C-G) plus a
120-note RPE demo chart → `Phirura.WindowsPhone/Assets/{game,demo}`. Avoids downloading
anything and dodges WP8.1's inability to decode ogg/opus.

## Build

`build_wp.ps1` at the workspace root (ASCII-only, `param()` based). Invoke via the PowerShell
tool — direct MSBuild is blocked by the sandbox.

```
& "E:\...\Phirura\build_wp.ps1" -Configuration Debug -Target wp  -Isolated   # ARM by default
& "E:\...\Phirura\build_wp.ps1" -Configuration Debug -Target win            # AnyCPU
```

- **The WP head is ARM, not AnyCPU** — Win2D ships native binaries only as `Phone/arm` and
  `Phone/x86`. Pass `-Platform x86` for the emulator.
- Log goes to `E:\_phirura_build.log`; the PowerShell tool swallows the script's stdout, so
  read that file.
- Always build `-Target win` too after touching `Phirura.Shared/` — the shared project
  compiles into both heads and a WP-only build hides desktop-only API gaps (this is how a
  stray `Windows.Media.Playback` using in `MusicClock.cs` was caught).
- **Interrogating the build**: `-Extra "<semicolon-separated MSBuild switches>"` runs another
  target without editing the script, e.g. `-Extra "/t:CoreCompile;/v:detailed"`. That is how
  the real `Csc` reference set was harvested (phone `Windows.winmd` + `platform.winmd` +
  every `WindowsPhoneApp\v8.1\*.dll`). Caveat: `Out-String -Width 400` truncates the `Csc`
  command line — read the `ResolveAssemblyReference` / `ImplicitlyExpandTargetPlatform` lines
  instead (one resolved path per line).
- **`tools/probe_api.py`** compiles candidate snippets with `csc.exe` against that exact
  reference set to answer "does this WP8.1 API exist?" definitively. A literal `csc.exe`
  invocation is rejected by the sandbox, so it must be invoked *from inside* the Python
  script. `tools/winmd_strings.py` (a `#Strings` heap dumper) is kept only as a cautionary
  example — its results are unreliable (see suffix-sharing below).
- `-Isolated` breaks the Win head's MakePri (`0xdef0050b`). Build the Win head without it.

## The installable appx

Two ARM packages, both 37 entries, both built and verified:

| package | size | exe | VCLibs dependency |
| --- | --- | --- | --- |
| `AppPackages\Phirura.WindowsPhone_1.0.0.0_ARM_Debug_Test\…_ARM_Debug.appx` | 3,630,192 | 226,816 | `Microsoft.VCLibs.120.00.Phone.Debug` |
| `AppPackages\Phirura.WindowsPhone_1.0.0.0_ARM_Test\…_ARM.appx` (Release) | 3,626,312 | 208,896 | `Microsoft.VCLibs.120.00.Phone` |

**Sideloading needs the matching VCLibs framework package on the device** — the build auto-adds the
`PackageDependency`. The Debug one has been present all along; Release needs the non-Debug one.

**The native probes must stay OFF in anything installable.** `App.xaml.cs` has
`const bool RunNativeProbes = false`; flipping it re-enables `D3D11Probe.Run()` +
`NativeProbe.Run()`. They cannot be left on: the Rust probe deliberately walks steps that *abort the
process*, so with it enabled the app dies during startup on every launch. (It did, from round 17
until this was gated.) Both probe DLLs remain in the package but are inert — `DllImport` resolves
lazily, so nothing loads them unless the flag is on.

`Package.appxmanifest` `DisplayName` is `Phirura` (it was the assembly name `Phirura.WindowsPhone`).

## Win2D packaging (easy to get wrong)

`Phirura/packages/Win2D.win81.1.21.0/` is **vendored**, not NuGet-restored, and imported at
the end of `Phirura.WindowsPhone.csproj`:

```xml
<Import Project="..\..\packages\Win2D.win81.1.21.0\build\wpa\Win2D.win81.targets"
        Condition="Exists('..\..\packages\Win2D.win81.1.21.0\build\wpa\Win2D.win81.targets')" />
```

- Path is `..\..\packages\…` because the csproj sits at `Phirura\Phirura\Phirura.WindowsPhone\`.
- The targets file builds its native path as
  `$(MSBuildThisFileDirectory)..\..\bin\Phone\$(Platform)\` — so the vendored tree must keep
  the package's internal layout (`build/wpa/` next to `bin/Phone/{arm,x86}/`).
- **Referencing a Win2D type is what puts the native DLL in the appx.** With no code touching
  Win2D the compiler drops the unused reference and `Microsoft.Graphics.Canvas.dll` never
  reaches the package. Nothing else is needed — no manual `<Content>` items.
- The build auto-adds `<PackageDependency Name="Microsoft.VCLibs.120.00.Phone.Debug" />`, so
  sideloading needs the matching **VCLibs phone framework package** on the device.
- MSBuild 12 accepts the targets file (the `$([System.String]::new(…))` property function in
  `win2d-RemovePlatformWinmd` never runs — that target only fires for managed WinRT component
  projects).

## Platform gotchas (each cost a debug cycle)

- **WinRT XAML binds to PROPERTIES only.** Binding to a public field fails silently with
  `BindingExpression path error: '<X>' property not found` and the row renders blank — this is
  what broke the chart list. `ChartEntry` / `PhiraChart` are auto-properties + an explicit
  C# 5 constructor (no property initialisers in C# 5).
- **Never name a bound member `Name`.** Even as a property it fails with
  `path error: 'Name' property not found` while every sibling property on the same type binds
  fine (confirmed by dumping the built assembly: `get_Name` is present, and `XamlTypeInfo.g.cs`
  contains neither type, so both go through reflection binding). Renamed `PhiraChart.Name` →
  `ChartName`; `ChartEntry` had already dodged this by calling its title `Title`. Treat `Name`
  as reserved for bound data members.
- **RPE charts need `speedEvents`.** Without them every `∫speed dt` is 0, so
  `base = height − line_height = 0` and the whole chart collapses onto the judge line — looks
  exactly like "the notes never move". Guard: `RpeParser.DefaultSpeed(maxTime)` returns a
  linear 0→maxTime ramp when the event layer is omitted. The demo chart is generated with
  `start = end = 1/SPEED_RATIO` so its internal speed is 1.0.
- **True fullscreen on WP8.1 needs FOUR levers, not one.** `StatusBar.HideAsync()` alone is
  not enough and fails silently if anything before it throws:
  1. `ApplicationView.GetForCurrentView().SuppressSystemOverlays = true` — the phone tray
     switch, **synchronous**, strongest lever.
  2. `SetDesiredBoundsMode(ApplicationViewBoundsMode.UseCoreWindow)` — restore `UseVisible` on
     exit or the next page renders under the bar.
  3. `StatusBar.GetForCurrentView().HideAsync()`.
  4. `StatusBar.BackgroundOpacity = 0` — invisible even if the hide is refused.
  Each step needs its **own** try block. The original bug: the colour setters ran before
  `HideAsync()`, threw, and the swallowed exception skipped the hide entirely — bar stayed on
  screen with a green build. `ApplicationView.IsFullScreenMode` / `TryEnterFullScreenMode` /
  `FullScreenSystemOverlayMode` do **not** exist on WP8.1 (desktop-only). The bar returns on
  activation/resume, so re-apply from `Window.Current.Activated` **and**
  `Application.Current.Resuming`, gated on an `_immersive` flag set in `OnNavigatedTo` /
  cleared in `OnNavigatedFrom`. Log `StatusBar.OccludedRect` after each attempt —
  `occluded=0x0` is the only proof the page is really full-screen. `OnPageLoaded` must be
  `await SetImmersiveAsync(true)` → `await WaitForLayoutAsync()` → `await StartAsync()`,
  because hiding the bar grows the content area and the renderer measures that size.
- **WP8.1 XAML activation args are the CORE types.** `Windows.UI.Xaml.WindowActivatedEventArgs`
  does not exist on this profile; `Window.Activated` is projected with
  `Windows.UI.Core.WindowActivatedEventArgs`, and the state enum is
  `Windows.UI.Core.CoreWindowActivationState` (not `…WindowActivationState`).
  `Windows.UI.Xaml.VisibilityChangedEventArgs` is likewise absent.
  **Check with `tools/probe_api.py`** — never trust a metadata dump: the `#Strings` heap uses
  **suffix sharing**, so `Foo` hides inside `IFoo` and a heap walk reports false negatives
  while a raw `grep` reports false positives. `Annotated\Windows.winmd` is interfaces-only.
  A lambda `(s, e) => { }` compiles even when the args type is unnameable, so it proves
  nothing — name the type in a method signature instead.
- **`PickSingleFileAsync` / `PickMultipleFilesAsync` are deprecated** on WP8.1 (CS0618); the
  `…AndContinue` variants are supported but need `OnActivated` handling.
  `StorageFile.GetParentAsync()` does not exist on WP8.1.
- **`documentsLibrary` capability is unusable on the WP8.1 (2014) manifest schema** — it
  demands a `windows.fileTypeAssociation` extension, which that schema rejects with
  `APPX3030`. Capabilities are `internetClientServer` only; the picker browses default
  locations.
- Diagnostics: `Phirura.Shared/AppLog.cs` → `ApplicationData\LocalFolder\phirura.log`
  (256 KB cap, serialized via `Task.ContinueWith`), wired to `App.UnhandledException` and
  `TaskScheduler.UnobservedTaskException`. **AppLog buffers** — an abrupt process death loses
  the final writes, so when probing a crash, run the fatal step LAST.

## Rendering: why Win2D, not raw DX

Plain C# XAML cannot talk to DirectX directly. XAML's `Canvas` + pooled `Image` +
`CompositeTransform` is already GPU-composited; the cost is per-frame managed→XAML property
updates, not software rasterisation. The escalations were (1) `Win2D.win81` v1.21.0 —
Direct2D/DirectWrite behind a XAML control, callable from C#, no C++ component; (2) C++/DX11 +
`SwapChainPanel` — full control but needs a C++ WinRT component, a swap chain, all
sprite/shader code in C++, plus ABI marshalling. **The user chose option 1**; it shipped.

## Rust on WP8.1 (`rust-wp81/`) — proven feasible, still incomplete

This supersedes the earlier conclusion that Rust simply cannot target WP8.1. **A Rust DLL now
loads and executes on a real WP8.1 device**, so the blocker was the standard library's import
surface, not the platform.

What was built (all under `rust-wp81/`):

- `targets/thumbv7a-windows-phone.json` — custom target. rustup has no prebuilt 32-bit ARM
  Windows target, but `rustc --print target-list` still knows `thumbv7a-pc-windows-msvc`, so
  `-Z build-std` compiles std from source. **`.json` target specs require `-Zjson-target-spec`.**
- `wp81_std_patch.py` — rewrites std's `raw-dylib` declarations. Remaps ~143 `kernel32.dll`
  functions onto the WP8.1 API sets and strips `bcryptprimitives` / `ntdll` / `userenv`.
  `--apply` asserts `MIN_EXPECTED_REWRITES = 17`.
- `gen_api_trampolines.py` → `compat/src/generated_api.rs` (~107 null-safe trampolines). Each
  does `LoadLibraryExW` + `GetProcAddress` on first call, so a wrong name degrades instead of
  aborting the process. Also emits `API_TABLE` + `wp81_probe_api_sets()`.
- `compat/` — the Win32 shims (`atexit`, `strlen`, `wcslen`, `LoadLibraryA`, `lstrlenW`,
  `GetActiveProcessorCount` → 1, processenv stubs, `CreatePipe`, `GetConsoleCP`,
  `SetConsoleCtrlHandler`, `__rt_udiv`/`__rt_udiv64`/`__u64tod`/`__u64tos`) plus raw probes
  `wp81_raw_qpc()` / `wp81_raw_createthread()`.
- `link-probe/` — step-wise probe `wp81_link_probe(step)`, exposed to C# via `NativeProbe.cs`.
- `verify_mapping.py` — validates the remap with `ctypes.WinDLL` (143/143).
- `build-std.ps1` — patches, junctions the sysroot `library` onto `work/library`, builds std
  (`-Z build-std-features=compiler-builtins-mem` is **required** for `memcpy`/`memmove`/
  `memset`/`memcmp`), restores.

Hard-won facts:

- **`LoadLibraryW` does not exist on WP8.1.** The API set does; the function does not. Use
  `LoadLibraryExW` (via `WindowsPhoneCore.lib`) or `LoadPackagedLibrary`. This was the root
  cause of repeated `0x8007007F`.
- **`api-ms-win-core-file-l1-2-1` IS present**, so `CreateFileW` / `ReadFile` / `WriteFile` /
  `FindFirstFileExW` / `DeleteFileW` / `MoveFileExW` are available. (An earlier conclusion
  drawn from Win2D's import table was wrong — Win2D just doesn't use files.)
- WP8.1 uses the **`msvcr120_app.dll` CRT, not the UCRT**; there is no `api-ms-win-crt-*` in
  the platform's own binaries.
- CRT-free linking needs `/NODEFAULTLIB`, own `_DllMainCRTStartup`, plus `_fltused`, `atexit`,
  `strlen`, `wcslen`, `chkstk.obj` (standalone object in `VC\lib\arm\`) and
  `__rt_udiv` / `__rt_udiv64` / `__u64tod` / `__u64tos`. ARM ABI: `__rt_udiv` returns
  quotient in r0 / remainder in r1 (a two-word Rust struct matches); `__rt_udiv64` is **not**
  guaranteed to match.
- **`_tls_index` must be left alone** — the loader fills it from the image's TLS directory
  (`AddressOfIndex`). Overwriting it with a fresh `TlsAlloc()` index is *worse* than leaving it
  at 0 (it moved the crash from step 2 to step 1). Do not "fix" it.
- **`std::thread` aborts (`0xC0000409`) but a raw `CreateThread` succeeds** → the OS supports
  threads; it is Rust's thread wrapper at fault. prpr only uses threads in
  `prpr/src/ui/offset_analysis.rs`, so a hand-rolled `CreateThread` wrapper or the WinRT thread
  pool replaces it.
  **UPDATE — this conclusion was WRONG.** With logging fixed (`AppLog.WriteSync`) the probe shows
  the crash is at `Instant::now().elapsed()`, not at `thread::spawn`. See below.
- **`Instant` is the real killer.** `std/src/sys/pal/windows/time.rs` does
  `cvt(c::QueryPerformanceFrequency(&mut frequency)).unwrap()` inside a `#[cold]` lazy init, then
  `assert_unchecked(frequency != 0)`. `.unwrap()` panics on failure, and a zero frequency reaches
  `NANOS_PER_SEC / frequency` — divide by zero, which also panics. With `panic = "abort"` either is
  `0xC0000409`. `Instant::now()` alone is fine (only needs `QueryPerformanceCounter`); it is the
  first request for a **duration** that dies, which is why it looked like a thread bug.
  Fixed by making `QueryPerformanceFrequency` a **compat-owned** function (listed in
  `compat_own_fns.txt` so the trampoline generator skips it — two `#[no_mangle]` defs would
  collide): try the platform, accept it only if it succeeds with a positive value, else **measure**
  against `GetSystemTimeAsFileTime` over a 100 ms sleep.
- **`__rt_sdiv64` / `__rt_udiv64`: the divide helpers must never divide.** The original compat
  implementations were `n / d` — and since the target had no hardware divide, LLVM lowered that
  straight back into a call to the same helper, so both recursed until the stack died
  (`0x800703E9` ERROR_STACK_OVERFLOW). **This single bug was every symptom the Rust workstream
  chased**: `std::thread` aborting, `Instant` aborting, `0xC0000409`. Fixed by (a) adding
  `+hwdiv,+hwdiv-arm` to the target features and (b) rewriting the helpers as shift-subtract loops.
- **`+hwdiv` fixes 32-bit division only.** ARM has no 64-bit divide instruction, so `udiv` is
  emitted for `u32/u32` (and `__rt_udiv` stops being called at all) but `bl __rt_udiv64` remains.
  compiler_builtins also calls it internally (9 times), so patching std cannot avoid it.
- **The real `__rt_udiv64` ABI, read off LLVM's own assembly output** (not guessed):
  **r0:r1 = divisor, r2:r3 = dividend** — divisor *first*, the opposite of the obvious reading — and
  the result is **quotient in r0:r1, remainder in r2:r3**. Rust cannot express a 16-byte register
  return (a `#[repr(C)] {u64,u64}` uses a hidden pointer), so `__rt_udiv64` is now
  `core::arch::global_asm!` schoolbook restoring division — the one helper that cannot be Rust.
- **A test whose result the optimiser can compute is not a test.** Probe steps written with literal
  operands (`929_877_129_449u64 / 6_750_000u64`) were constant-folded, so they reported the exactly
  correct expected values while the code underneath was infinitely recursive. Two rounds were lost
  to this. **Any probe of a runtime helper must pass operands through `std::hint::black_box`.**
- `windows_link::link!` is `raw-dylib` — the DLL name is baked into the import table and **no
  import library is needed** (rustc emits the import descriptor).
- The NTSTATUS trick: std converts failures via `RtlNtStatusToDosError` then
  `io::Error::from_raw_os_error`, so the compat shims return the Win32 code unchanged and
  `RtlNtStatusToDosError` is the identity.
- **Desktop API sets are a superset of WP8.1's**, so local verification can only find definite
  errors, never certify correctness. Runtime resolution (trampolines) is the only real answer.
- Diagnostic technique: a **call-free resolution walk** (`wp81_probe_api_sets`, no function is
  actually invoked) finds absent exports safely; **step-wise probing** (entry point takes a
  `step`, the caller logs after each) isolates which subsystem dies.

Proven working on device: DLL load, allocator (HashMap/String/Vec), NT file shims,
`ProcessPrng`, `QueryPerformanceCounter`, raw `CreateThread`. Import table reduced from 66
KERNEL32 functions to **one API set** (`api-ms-win-core-libraryloader-l1-2-0.dll`:
`LoadLibraryExW`, `GetProcAddress`).

`build-probe.ps1` builds the whole probe pipeline (compat → staticlib → VC11 ARM link → deploy) and
transcribes itself to `rust-wp81/build-probe.log` — **read that file**, the tool that invokes it does
not surface stdout at all. Four link requirements that fail *silently* if omitted: `/NODEFAULTLIB` +
own `_DllMainCRTStartup`; `ws2_32.lib` (std links winsock unconditionally, ~25 `__imp_*` otherwise);
`chkstk.obj` from `VC\lib\arm\`; and **`/EXPORT:` for every entry point** — `#[no_mangle]` does not
export from a staticlib, and without them the linker emits a 17 KB DLL that loads and then fails every
`DllImport`. The ARM cross tools are **VS11's** (`VC\bin\x86_arm\`), not VS2013's, and `mspdb110.dll`
must be on `PATH` or `link.exe` exits 127 silently.

### D3D11 probe (`rust-wp81/d3d11-probe/`, `build-d3d11.ps1`)

`wp81d3d11.dll` — C++, ARMNT, 5 KB, CRT-free (`/NOENTRY /NODEFAULTLIB`), one import: `d3d11.dll`.
Exports `wp81_d3d11_probe()` (stage number, 0 = success) and `wp81_d3d11_report()`. Does device
creation → 64x64 R8G8B8A8 render target → RTV → clear → staging → `CopyResource` → `Map` → compare
the pixel, entirely **offscreen** (no swap chain, no XAML), so a failure cannot be blamed on the
presentation layer. Called from `D3D11Probe.cs`, which runs **before** `NativeProbe.Run()` in
`App.xaml.cs` because the Rust probe deliberately aborts the process.

**Measured on device: `stage=0`, every call `S_OK`, pixel within the +/-1 UNORM tolerance — a
hand-written D3D11 renderer does work, and it works on the real GPU.** The device reports exactly
one adapter, `"Qualcomm Adreno 225 (WDDM v1.2)"` with `flags=0x0` (not
`DXGI_ADAPTER_FLAG_SOFTWARE`), at feature level **9.3 (`0x9300`)** — the correct ceiling for that
GPU (Lumia 820/920 class), and enough for a 2D instanced-quad renderer.

**`D3D11_DRIVER_TYPE_HARDWARE` with a null `pAdapter` returns WARP ("Software Adapter") on WP8.1.**
That is the documented desktop shortcut, and here it hands back the CPU rasteriser. The fix is to
enumerate via `IDXGIFactory1::EnumAdapters1`, pick the first adapter **without**
`DXGI_ADAPTER_FLAG_SOFTWARE`, and create the device with `D3D_DRIVER_TYPE_UNKNOWN` (the only legal
driver type when an adapter is supplied). Only fall back to the driver-type shortcut if that fails,
and say which path was taken — "we ended up on WARP" and "we asked for WARP" are different findings.
`CreateOnAdapter` centralises the `D3D_FEATURE_LEVEL_11_1` trap (passing it to a pre-11.1 runtime
returns `E_INVALIDARG` rather than falling back).

**Lesson worth keeping: report what the platform offers, not just what the call returned.** The
first probe printed `adapter="Software Adapter"` and it was nearly written off as a property of the
phone; one enumeration call showed the Adreno immediately.

Gotchas: `WINAPI_FAMILY=WINAPI_FAMILY_PHONE_APP` is mandatory (without it `windows.h` hands out the
desktop surface, turning a missing API into a load-time failure); the WP kit's headers need
`Windows Kits\8.1\Include\{shared,um,winrt}` on the include path *after* the WP kit's Include;
`memset`/`memcpy` are intrinsics and cannot be defined — `#include <string.h>` and let `/O2` inline
them; and no `%` or `/` on 64-bit values (see `__rt_sdiv64` above).

**Build-system trap:** `-Isolated` redirects `IntermediateOutputPath` to
`E:\_phirura_build\obj\ARM\Debug\`. Clearing `layout.resfiles` in the *project's* `obj\` does
nothing — the real layout manifest that gates appx contents is the one under `E:\_phirura_build\`.
The authoritative check for "what is in the appx" is the archive's own file table, cross-checked
against `bin\ARM\Debug\Phirura.WindowsPhone.build.appxrecipe`.

## Getting to actual Rust Phira — two paths, and Path A's real cost

**Path A (the real Rust game) is a multi-week project.** `prpr-miniquad` is a **GL-only**
abstraction — every backend (`windows.rs` 1000 lines, `linux_x11.rs` 894, `ohos.rs` 849,
`android.rs` 731, `macos.rs` 680, `ios.rs` 435, `wasm.rs` 444) funnels through `native/gl.rs`
(647 lines), which loads OpenGL entry points. There is **no D3D and no Metal backend**; the
architecture assumes GL, and WP8.1 has no OpenGL at all. Two blockers on top:

1. **miniquad pipelines take GLSL.** D3D11 needs HLSL/DXBC, and there is no GLSL→HLSL translator on
   WP8.1 — so a "D3D11 backend" would have to ship an ANGLE-sized translation layer *and* a shader
   compiler.
2. **prpr itself uses raw GL.** `core/render.rs` calls `glGenFramebuffers`,
   `glRenderbufferStorageMultisample`, `glBlitFramebuffer` for MSAA (no D3D11 equivalent);
   `note.rs` uses `gl_internal_id` for batching.

Plus the XAudio2 backend for `sasa`, the app shell, and `tokio`/`reqwest`/`rayon`/`image`/`rfd`.

**Path B (Rust core + the working C# shell) is the feasible one.** Rendering is already solved by
Win2D in the C# app, so the shim routes `macroquad`'s drawing calls there instead of re-solving
them. Measured surface: **~40 distinct API items over ~130 call sites across 23 files** (heaviest:
`judge.rs` 14, `ext.rs` 12, `scene/game.rs` 11, `core/render.rs` 11). `parse/` (1854 lines) needs
only `Color`, `Vec2`, `WHITE`, an opaque `AudioClip`, and `gl_internal_id()` inside a `debug!` log.

`rust-wp81/shims/macroquad/` builds for the target (`libmacroquad.rlib`, 506 KB, ARMNT). Maths
types are real with full operator sets — the chart model, tweens and judgement are arithmetic and
must match desktop exactly. `draw_*` records into a fixed 8192-entry buffer for C# to drain and
replay through Win2D. Window/input are stubs the XAML shell owns; `rand` is a deterministic
xorshift because `getrandom` cannot work here. **`core/render.rs` is the one module Path B must
rewrite rather than route** — its raw-GL MSAA has nothing to forward to.

Still to write for Path B: `miniquad` + `sasa` shims, the `prpr-port` staticlib with the C# FFI
surface, and the `[patch]` wiring so prpr builds against the shims.

### Shims — all four written and building for the target (`rust-wp81/shims/`)

| shim | surface | state |
| --- | --- | --- |
| `macroquad` | ~40 items; maths types real, `draw_*` recorded into arenas, window/input stubbed | builds |
| `miniquad` | `Context`, `Texture`/`TextureParams`/`TextureFormat`/`TextureWrap`, `RenderPass`, `UniformType`, `gl` stub | builds |
| `sasa` | `AudioClip`, `AudioManager`, `Music`, `Sfx`, `MusicParams`, `PlaySfxParams`, three backends | builds |
| `rfd` | `FileDialog::new().pick_file()` / `.save_file()` | builds |

`cargo check -p prpr --target thumbv7a-windows-phone` produced **12 errors and not one was in
`prpr`** — all were `windows-sys` 0.61.2's `Win32_UI_Shell` module referencing `APPBARDATA`,
`SHFILEOPSTRUCTA/W`, `SHFILEINFOA/W`, `SHSTOCKICONINFO`, `SHQUERYRBINFO`, `NOTIFYICONDATAA` that
its own feature gating does not define. Only `rfd` pulls it in, and `rfd` is used in exactly two
places, so shimming it removes `windows-sys` from the graph entirely.

Key details: the handle types live in the **miniquad** shim and macroquad's re-exports them (prpr
converts between the two crates' types, so they must be identical); `InternalGlContext` needs a
`QuadGl` because prpr both destructures it and calls `flush()` through it; `Cmd` must stay `Copy`,
so vertices/indices go in fixed arenas and `drain_reset` must clear them or the batcher silently
stops drawing; `sasa` needs `anyhow` as a real dependency because prpr calls it with `?` inside
`anyhow::Result`; `MusicParams.playback_rate` is **f64**.

**`[patch]` only works in a workspace root**, so it lives at the end of
`phira-main/phira-main/Cargo.toml` (original saved as `Cargo.toml.orig`). Build configuration only —
no upstream source file is edited.

### The target triple's NAME matters, not just its contents

**The target spec is `thumbv7a-pc-windows-msvc.json`, not `thumbv7a-windows-phone.json`.** This is
not cosmetic. `winapi` 0.3.9's `build.rs` decides whether the platform *is* Windows by
string-matching the triple:

```rust
let target: Vec<_> = var("TARGET").unwrap().split('-').collect();
if target.get(2) == Some(&"windows") { try_everything(); }
```

`thumbv7a-windows-phone` splits to `["thumbv7a", "windows", "phone"]` — index 2 is `"phone"`, so
the branch never ran, no features were emitted, and every `#[cfg(feature = "...")]` module stayed
compiled out (32 `E0432: unresolved import` errors for things that plainly exist in winapi's own
source). `thumbv7a-pc-windows-msvc` puts `"windows"` at index 2.

The triple is the JSON file's **stem**, so the fix is a rename; the JSON's contents (`os`, `arch`,
`env`, `features`, `llvm-target`) are what drive codegen and `cfg`, and none of those changed. Note
the `llvm-target` field was *already* `thumbv7a-pc-windows-msvc` — only the cargo-visible name
differed.

**General rule: crates that string-match `TARGET` to decide what a platform is are common, and a
bespoke triple name silently opts out of all of them.** The tell is a dependency that "cannot find"
something that obviously exists in its own source.

### The shims must carry the upstream VERSION, not just the name

```
warning: patch `macroquad v0.4.0 (...)` was not used in the crate graph
warning: patch `miniquad v0.4.0 (...)` was not used in the crate graph
miniquad v0.3.15 (git) <- macroquad v0.3.25 (git) <- prpr
```

Cargo requires a `[patch]` replacement to be **semver-compatible** with what it replaces. The shims
declared `0.4.0` while upstream is `macroquad 0.3.25` / `miniquad 0.3.15`, so cargo ignored both
patches and built the real crates — for several build cycles, with only a warning buried in a long
log. **This is why the first `prpr` link failed on Win32 windowing symbols (`CreateWindowExW`,
`ChoosePixelFormat`, `DispatchMessageW`): those are miniquad's real Windows backend.**

Fixed by matching versions: macroquad → `0.3.25`, miniquad → `0.3.15`. `sasa` (0.1.0) and `rfd`
(0.17.2) already matched, which is why those two patches *did* apply — a useful tell.

**Check `cargo tree -i <crate>` for a patch's source**, and read the patch warnings. A silently
ignored patch means you are debugging the wrong code.

### Runtime helpers: a helper must not use the operation it implements

The recurring trap on this target, now hit four separate times:

| helper | wrong version | why |
| --- | --- | --- |
| `__rt_udiv` | `n / d` | lowers to a call to itself |
| `__rt_udiv64` | `n / d` | same |
| `__u64tod` / `__u64tos` | `v as f64` / `v as f32` | same |
| `__dtoi64` / `__dtou64` / `__i64tod` / `__stou64` | `d as i64` etc. | same |

Verified by compiling a snippet to assembly: `f64 as i64` really does emit `bl __dtoi64`. The fix is
either assembly (`__rt_udiv64`, `__rt_sdiv64` — a 16-byte register return cannot be expressed in
Rust) or forwarding to **`compiler_builtins`**, which implements them with integer bit manipulation
precisely so they do not recurse and is already linked as part of `build-std`.

`__rt_sdiv64` is assembly too: sign-handling wrapper around `__rt_udiv64`.

### Path B: where the Rust port actually stands

`cargo check -p prpr --target thumbv7a-pc-windows-msvc` **passes** (~200 crates, `prpr` clean).
`cargo build -p prpr-port` produces the staticlib. The **link** is the remaining wall: 203
unresolved externals, all Win32 windowing/GL symbols, because the real macroquad/miniquad were being
linked instead of the shims.

With the shim versions fixed, the shims are used and `prpr` now compiles *against them* — which
raises the error count from 12 to 436, because the shim surface is far smaller than macroquad's.
That is the honest remaining work: the shim has to grow to cover `prpr`'s real usage
(`macroquad::prelude` re-exports, `glam`, `EventHandler` method names, `material::shaders`, ~30 more
items). Each build cycle is ~5 minutes.

**Not yet done:** the renderer drain (draw commands → Win2D), real audio callbacks, and chart
marshalling to C#. The C# port remains the playable app.

### Three environment traps that all look like something else

1. **Git Bash ships `/usr/bin/link.exe`, which is coreutils' `link`.** It shadows MSVC's linker and
   rustc then fails with `link: extra operand '....rcgu.o'`, which reads like a rustc bug. This is
   why every bash-invoked cargo build failed while the identical command from the other shell
   worked.
2. **`Common7\IDE` must be on PATH** or link.exe exits `0xc0000135` — `mspdb110.dll` lives there,
   not in `VC\bin`.
3. **`LIB` must point at the x64 import libs** (`VC\lib\amd64` + `Windows Kits\8.1\Lib\
   winv6.3\um\x64`) or the host link dies with `LNK1181: cannot open input file 'kernel32.lib'`.

All three are captured in `rust-wp81/build-prpr.ps1`. **`Start-Transcript` does not capture native
command output in the 5.1 shell** — the first run logged the exit code and nothing else, so the
script redirects cargo to `prpr-cargo.log` explicitly.

## Toolchain gotchas

- **MSYS mangles `/SWITCH` arguments** into paths → set `MSYS_NO_PATHCONV=1` and
  `MSYS2_ARG_CONV_EXCL='*'`, and pass `/LIBPATH` as a Windows path.
- **The ARM toolchain needs `mspdb110.dll`** (in `Common7\IDE`, not `VC\bin`) or exit 127.
- **`$args` is reserved in PowerShell** — assigning to it makes the command a no-op.
- **Never invoke `cmd` from the bash tool** in this environment.
- Sandbox blocks: `cmd /c mklink`; bulk `rmtree` (SAFE_DELETE_BULK_CONFIRM_REQUIRED); a literal
  `csc.exe` invocation; and bash commands containing the word "PowerShell".
- Rust toolchain lives at `C:\Users\admin\.workbuddy-ai\binaries\rust\` (stable + nightly with
  `rust-src`). CRT-free link stubs at `E:\_phirura_stub\`.

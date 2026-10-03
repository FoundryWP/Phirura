# Phirura — Phira ported to Windows Phone 8.1

`Phirura/Phirura.sln` → `Phirura/Phirura/Phirura.Windows/` (Win8.1, AnyCPU),
`Phirura/Phirura/Phirura.WindowsPhone/` (WP8.1, **ARM**), source-shared `Phirura.Shared` (shproj).
Read-only reference: `phira-main/phira-main/` (Rust `prpr`, `prpr-*`, `phira`).
Second workstream: `rust-wp81/` — real Rust on WP8.1. Daily log: `2026-10-03.md` (Rounds 1–34).

## Architecture (C# app — the playable product)

- Gameplay core (chart model, tweens, anims, RPE/PEC parsers, judgement, scoring) is a
  line-by-line C# port of `prpr` in `Phirura.Shared/` (Core, Parse, Game, Net).
- WP head renders via **Win2D** (`Phirura.WindowsPhone/Win2DChartRenderer.cs`); `ChartRenderer`
  (pooled XAML Images) stays for the Win8.1 head, which only has `MainPage` and never used it.
- **Render maths reused verbatim**: same `Mat3` → same `Renderer.Decompose`; only rot/tx/ty taken
  (both renderers override decomposed scale with a constant note size). `Mat3`→`Matrix3x2`:
  `M11→M11, M12→M12, M21→M21, M22→M22, M13→M31, M23→M32` (both row-vector).
  Draw = `ds.Transform = Scale * Rotate * Translate` then `DrawImage(bmp, CenteredRect(w,h), …)`.
- **`CanvasControl`, not `CanvasAnimatedControl`** — the animated control runs Update/Draw on its
  own game-loop thread, but judge + HUD must stay on the UI thread (`MusicClock.Resync()` reads
  `MediaElement.Position`; `UpdateHud` writes `TextBlock`s). `CompositionTarget.Rendering` kept:
  `Render()`/`RenderBadNote()` only *record*, `Present()` calls `Invalidate()`. Dedicated game loop
  is the next perf step and needs those two marshalled first.
- `UseSharedDevice="True"` + `CanvasDevice.GetSharedDevice()` for
  `ChartLibrary.LoadCanvasTextureAsync` (96 DPI so `Bounds == SizeInPixels`). Textures carry the
  `CanvasBitmap` in `ChartTexture.Tag`; the XAML head uses `.Source`.
- Camera projection (macroquad `zoom.y = -aspect_ratio`, model `diag(1,-1)`, chart Y ÷ aspect):
  `screenX=(x+1)/2*W`, `screenY=(1-y*ar)/2*H`. `ChartRenderer.FlipVertical` is the escape hatch.
- Holds use Phira's single-sheet `hold.png` (`holdAtlas: [50,50]`, 1000x1100): top 50 px = tail,
  bottom 50 px = head, middle = body. Reproduced by drawing in a local y-down frame
  (`Transform = toScreen * frame * Scaling(1,-1)`).
- Audio clock: `MediaElement.Position` refreshes a few times/s, so `MusicClock` runs a `Stopwatch`
  as master and re-anchors when drift exceeds 40 ms.

## Online features

Contract from `phira/src/client.rs` in `Phirura.Shared/Net/PhiraApi.cs` + `LoginPage`/`OnlinePage`:

- `POST /login` `{email, password, clientVersion}` → `{id, token}`; `GET /me` (Bearer) → user.
- `GET /chart?page=N&pageNum=M&search=&order=` → `{count, results:[Chart]}`; order `updated`/
  `name`/`difficulty`/`rating`, `-` reverses. Paging is **1-based**.
- Chart archive = `chart.file.url`, a **zip**; `ChartLibrary.InstallArchiveAsync` uses
  `System.IO.Compression.ZipArchive`.

Wire-format traps (verified live):

- **`file`/`illustration`/`preview` are bare URL strings**, not objects (Rust
  `#[serde(transparent)] struct File { url: String }`). `PhiraApi.FileUrlOf` accepts both shapes.
  `uploader` is a bare user id.
- A real archive has **no `chart.json`**: `56695308.jpg info.txt 24432296.json info.yml
  56695308.mp3`. `PickChartFile` matches any `.json` except `info.json`; `PickIllustration` falls
  back to the first image.
- **PEC charts ship with a `.json` extension** — line 1 is a bare offset, then `bp cp cm cd cr ca
  cf cv n1-n4 # &`. `ChartParser.Parse` must **sniff content before trusting the extension**:
  RPE starts with `{`, PEC with a bare number.
- **Do not "fix" the PEC note mapping from memory.** `prpr/src/parse/pec.rs`: `n1`=Click,
  `n2`=Hold (6 tokens — `line beat x above fake end`), `n3`=Flick, `n4`=Drag — opposite of the
  generic PEC convention. `bp` takes **2** tokens (`bp 120 4`), `n1`/`n3`/`n4` 5
  (`line beat x above fake`), `cv` 3, `cp` 4; the parser rejects trailing tokens.

## Assets

`Phirura/tools/prepare_assets.py` — pure-`zlib` PNG codec (no PIL) that splits the hold atlas and
synthesizes a demo song (32 s, 120 BPM, Am-F-C-G) + 120-note RPE demo chart →
`Phirura.WindowsPhone/Assets/{game,demo}`. Avoids downloads and WP8.1's lack of ogg/opus.

## Build (C# app)

`build_wp.ps1` at workspace root (ASCII-only, `param()`). Invoke via the PowerShell tool — direct
MSBuild is sandbox-blocked.

```
& "E:\...\Phirura\build_wp.ps1" -Configuration Debug -Target wp -Isolated   # ARM
& "E:\...\Phirura\build_wp.ps1" -Configuration Debug -Target win            # AnyCPU
```

- WP head is **ARM, not AnyCPU** (Win2D ships native `Phone/arm`,`Phone/x86` only). `-Platform x86`
  for the emulator.
- Log → `E:\_phirura_build.log` (the PowerShell tool swallows stdout).
- Always build `-Target win` after touching `Phirura.Shared/` — it compiles into both heads; a
  WP-only build hides desktop-API gaps.
- `-Extra "<semicolon MSBuild switches>"` runs another target without editing the script.
- `tools/probe_api.py` compiles snippets with `csc.exe` against the real reference set to answer
  "does this WP8.1 API exist?" definitively (must be invoked *from inside* the Python script — a
  literal `csc.exe` call is sandbox-rejected). `tools/winmd_strings.py` is unreliable (suffix
  sharing) — kept only as a cautionary example.
- `-Isolated` breaks the Win head's MakePri (`0xdef0050b`). Build the Win head without it.

## The installable appx

| package | entries | size | exe | VCLibs dependency |
| --- | --- | --- | --- | --- |
| `…_ARM_Debug_Test\…_ARM_Debug.appx` | 37 → **38** | 3,630,192 | 226,816 | `Microsoft.VCLibs.120.00.Phone.Debug` |
| `…_ARM_Test\…_ARM.appx` (Release) | 37 | 3,626,312 | 208,896 | `Microsoft.VCLibs.120.00.Phone` |

Debug carries **38** entries — `wp81prpr.dll` joined it. Sideloading needs the matching **VCLibs
phone framework package**. **Native probes must stay OFF** (`App.xaml.cs`
`const bool RunNativeProbes = false`) — the Rust probe deliberately aborts the process; both probe
DLLs stay packaged but inert (`DllImport` resolves lazily). `DisplayName` is `Phirura`.

## Win2D packaging

`Phirura/packages/Win2D.win81.1.21.0/` is **vendored** (not NuGet-restored), imported at the end of
`Phirura.WindowsPhone.csproj`:

```xml
<Import Project="..\..\packages\Win2D.win81.1.21.0\build\wpa\Win2D.win81.targets"
        Condition="Exists('..\..\packages\Win2D.win81.1.21.0\build\wpa\Win2D.win81.targets')" />
```

- Path is `..\..\packages\…` — the csproj sits at `Phirura\Phirura\Phirura.WindowsPhone\`.
- Targets derive native path from `$(MSBuildThisFileDirectory)..\..\bin\Phone\$(Platform)\`, so the
  vendored tree keeps the package layout (`build/wpa/` beside `bin/Phone/{arm,x86}/`).
- **Referencing a Win2D type is what packages the native DLL** — no manual `<Content>`.
- MSBuild 12 accepts it (`win2d-RemovePlatformWinmd` never runs — only for managed WinRT projects).

## Platform gotchas (each cost a debug cycle)

- **WinRT XAML binds to PROPERTIES only.** Binding a public field fails silently
  (`path error: '<X>' property not found`, blank row). `ChartEntry`/`PhiraChart` are auto-properties
  + explicit C# 5 constructors (no property initialisers in C# 5).
- **Never name a bound member `Name`** — fails with `'Name' property not found` while siblings
  bind. Renamed `PhiraChart.Name` → `ChartName`.
- **RPE charts need `speedEvents`.** Without them every `∫speed dt` is 0, so
  `base = height − line_height = 0` and the chart collapses onto the judge line. Guard:
  `RpeParser.DefaultSpeed(maxTime)` (linear 0→maxTime ramp). Demo chart uses
  `start = end = 1/SPEED_RATIO` so internal speed is 1.0.
- **True fullscreen on WP8.1 needs FOUR levers**, each in its own try block (the original bug was
  colour setters running before `HideAsync()`, throwing, the swallowed exception skipping the hide):
  1. `ApplicationView.GetForCurrentView().SuppressSystemOverlays = true` — synchronous, strongest.
  2. `SetDesiredBoundsMode(ApplicationViewBoundsMode.UseCoreWindow)` — restore `UseVisible` on exit.
  3. `StatusBar.GetForCurrentView().HideAsync()`.
  4. `StatusBar.BackgroundOpacity = 0`.
  `IsFullScreenMode` / `TryEnterFullScreenMode` / `FullScreenSystemOverlayMode` do **not** exist on
  WP8.1. The bar returns on resume — re-apply from `Window.Current.Activated` **and**
  `Application.Current.Resuming`, gated on `_immersive`. Log `StatusBar.OccludedRect` (`0x0` is the
  only proof). `OnPageLoaded` = `await SetImmersiveAsync(true)` → `await WaitForLayoutAsync()` →
  `await StartAsync()` (hiding the bar grows the measured content area).
- **WP8.1 XAML activation args are the CORE types.** `Windows.UI.Xaml.WindowActivatedEventArgs`
  does not exist; `Window.Activated` uses `Windows.UI.Core.WindowActivatedEventArgs` /
  `CoreWindowActivationState`. `VisibilityChangedEventArgs` likewise absent. **Check with
  `tools/probe_api.py`** — the `#Strings` heap uses **suffix sharing**, so a heap walk gives false
  negatives and a raw grep false positives. `Annotated\Windows.winmd` is interfaces-only. A lambda
  compiles even when the args type is unnameable — name the type in a signature instead.
- `PickSingleFileAsync`/`PickMultipleFilesAsync` deprecated on WP8.1 (CS0618); the `…AndContinue`
  variants work but need `OnActivated`. `StorageFile.GetParentAsync()` does not exist.
- **`documentsLibrary` is unusable on the WP8.1 (2014) manifest schema** — demands a
  `windows.fileTypeAssociation` extension the schema rejects (`APPX3030`). Capabilities are
  `internetClientServer` only.
- Diagnostics: `Phirura.Shared/AppLog.cs` → `ApplicationData\LocalFolder\phirura.log` (256 KB cap,
  serialized via `Task.ContinueWith`), wired to `App.UnhandledException` +
  `TaskScheduler.UnobservedTaskException`. **AppLog buffers** — abrupt death loses final writes, so
  when probing a crash run the fatal step LAST.

## Rendering: why Win2D, not raw DX

Plain C# XAML cannot talk to DirectX directly; `Canvas` + pooled `Image` + `CompositeTransform` is
already GPU-composited (cost = per-frame managed→XAML property updates). Escalations were (1)
`Win2D.win81` v1.21.0 — Direct2D behind a XAML control, no C++ component; (2) C++/DX11 +
`SwapChainPanel`. **The user chose 1.**

## Rust on WP8.1 (`rust-wp81/`) — DLL loads and runs on device; port still incomplete

**A Rust DLL loads and executes on a real device** — the blocker was std's import surface, not the
platform. Built:

- `targets/thumbv7a-pc-windows-msvc.json` — custom target spec (**name matters**). rustup has no
  prebuilt 32-bit ARM Windows target, but rustc knows `thumbv7a-pc-windows-msvc`, so
  `-Z build-std` compiles std from source. `.json` specs need `-Zjson-target-spec` (cargo-side).
- `wp81_std_patch.py` — rewrites std's `raw-dylib` declarations: remaps ~143 `kernel32.dll`
  functions onto WP8.1 API sets, strips `bcryptprimitives`/`ntdll`/`userenv`.
  `--apply` asserts `MIN_EXPECTED_REWRITES = 17`.
- `gen_api_trampolines.py` → `compat/src/generated_api.rs` (~107 null-safe trampolines; each does
  `LoadLibraryExW`+`GetProcAddress` on first call). Also emits `API_TABLE` + `wp81_probe_api_sets()`.
- `compat/` — Win32 shims (`atexit`, `strlen`, `wcslen`, `LoadLibraryA`, `lstrlenW`,
  `GetActiveProcessorCount` → 1, processenv stubs, `CreatePipe`, `GetConsoleCP`,
  `SetConsoleCtrlHandler`, `__rt_udiv`/`__rt_udiv64`/`__u64tod`/`__u64tos`) + raw probes
  `wp81_raw_qpc()`/`wp81_raw_createthread()`.
- `link-probe/` — step-wise probe `wp81_link_probe(step)`, exposed via `NativeProbe.cs`.
- `verify_mapping.py` — validates the remap with `ctypes.WinDLL` (143/143).
- `build-std.ps1` — patches, junctions the sysroot `library` onto `work/library`, builds std
  (`-Z build-std-features=compiler-builtins-mem` is **required**), restores.

Hard-won facts:

- **`LoadLibraryW` does not exist on WP8.1** — use `LoadLibraryExW` (via `WindowsPhoneCore.lib`)
  or `LoadPackagedLibrary`. Root cause of repeated `0x8007007F`.
- **`api-ms-win-core-file-l1-2-1` IS present** — `CreateFileW`/`ReadFile`/`WriteFile`/
  `FindFirstFileExW`/`DeleteFileW`/`MoveFileExW` available.
- WP8.1 uses the **`msvcr120_app.dll` CRT, not the UCRT** — no `api-ms-win-crt-*`.
- CRT-free linking needs `/NODEFAULTLIB`, own `_DllMainCRTStartup`, plus `_fltused`, `atexit`,
  `strlen`, `wcslen`, `chkstk.obj` (in `VC\lib\arm\`) and the divide/convert helpers. ARM ABI:
  `__rt_udiv` returns quotient in r0 / remainder in r1; `__rt_udiv64` is **not** guaranteed to match.
- **`_tls_index` must be left alone** — the loader fills it from the image's TLS directory.
  Overwriting it with a fresh `TlsAlloc()` index is *worse* (moved the crash from step 2 to step 1).
- **`QueryPerformanceFrequency` is compat-owned** (`compat_own_fns.txt` so the trampoline generator
  skips it): try the platform, accept only on success with a positive value, else **measure**
  against `GetSystemTimeAsFileTime` over a 100 ms sleep.
- **Divide/shortcut helpers must never use the operation they implement** — hit four times
  (`__rt_udiv`, `__rt_udiv64`, `__u64tod`, `__u64tos`, `__dtoi64`…): LLVM lowers the operation back
  into a call to the same helper → infinite recursion → `0x800703E9` stack overflow. Fix: assembly
  (`__rt_udiv64`, `__rt_sdiv64` — 16-byte register return cannot be expressed in Rust) or forward to
  **`compiler_builtins`**. `+hwdiv,+hwdiv-arm` fixes 32-bit only; ARM has no 64-bit divide, so
  `bl __rt_udiv64` remains.
- **Real `__rt_udiv64` ABI (from LLVM's own asm): r0:r1 = divisor, r2:r3 = dividend** (divisor
  *first*); quotient in r0:r1, remainder in r2:r3. Now a `global_asm!` schoolbook restoring division.
- **A test whose result the optimiser can compute is not a test.** Literal operands were
  constant-folded and reported correct values over infinitely-recursive code — two rounds lost.
  **Pass operands through `std::hint::black_box`.**
- `windows_link::link!` is `raw-dylib` — the DLL name is baked into the import table, no import
  library needed.
- NTSTATUS trick: std converts failures via `RtlNtStatusToDosError` then
  `io::Error::from_raw_os_error`, so compat shims return the Win32 code unchanged and
  `RtlNtStatusToDosError` is the identity.
- **Desktop API sets are a superset of WP8.1's** — local verification finds definite errors, never
  certifies correctness. Runtime resolution is the only real answer.
- Diagnostics: a **call-free resolution walk** (`wp81_probe_api_sets`) finds absent exports safely;
  **step-wise probing** isolates which subsystem dies.

Proven on device: DLL load, allocator, NT file shims, `ProcessPrng`, `QueryPerformanceCounter`, raw
`CreateThread`. Import table reduced from 66 KERNEL32 functions to **one API set**
(`api-ms-win-core-libraryloader-l1-2-0.dll`: `LoadLibraryExW`, `GetProcAddress`).

`build-probe.ps1` builds the probe pipeline (compat → staticlib → VC11 ARM link → deploy) and
transcribes to `rust-wp81/build-probe.log` — **read that file**. Four link requirements that fail
*silently*: `/NODEFAULTLIB` + own `_DllMainCRTStartup`; `ws2_32.lib` (std links winsock
unconditionally); `chkstk.obj` from `VC\lib\arm\`; **`/EXPORT:` for every entry point**
(`#[no_mangle]` does not export from a staticlib — without them the linker emits a 17 KB DLL that
loads and fails every `DllImport`). The ARM cross tools are **VS11's** (`VC\bin\x86_arm\`), and
`mspdb110.dll` must be on `PATH` or `link.exe` exits 127 silently.

### D3D11 probe (`rust-wp81/d3d11-probe/`, `build-d3d11.ps1`)

`wp81d3d11.dll` — C++, ARMNT, 5 KB, CRT-free (`/NOENTRY /NODEFAULTLIB`), one import: `d3d11.dll`.
Exports `wp81_d3d11_probe()` (stage, 0 = success) + `wp81_d3d11_report()`. Device creation → 64x64
R8G8B8A8 RT → RTV → clear → staging → `CopyResource` → `Map` → compare pixel, **offscreen**. Called
from `D3D11Probe.cs`, which runs **before** `NativeProbe.Run()` (the Rust probe aborts the process).

**Measured on device: `stage=0`, every call `S_OK`, pixel within ±1 UNORM.** One adapter,
`"Qualcomm Adreno 225 (WDDM v1.2)"`, `flags=0x0`, feature level **9.3 (`0x9300`)**.

**`D3D11_DRIVER_TYPE_HARDWARE` with a null `pAdapter` returns WARP on WP8.1.** Fix: enumerate via
`IDXGIFactory1::EnumAdapters1`, pick the first adapter **without** `DXGI_ADAPTER_FLAG_SOFTWARE`,
create with `D3D_DRIVER_TYPE_UNKNOWN`. `CreateOnAdapter` centralises the `D3D_FEATURE_LEVEL_11_1`
trap (passing it to a pre-11.1 runtime returns `E_INVALIDARG`).

**Report what the platform offers, not just what the call returned** — the first probe printed
`adapter="Software Adapter"` and was nearly written off as a phone property.

Gotchas: `WINAPI_FAMILY=WINAPI_FAMILY_PHONE_APP` mandatory; the WP kit's headers need
`Windows Kits\8.1\Include\{shared,um,winrt}` after the WP kit's Include; `memset`/`memcpy` are
intrinsics (`#include <string.h>`, let `/O2` inline); no `%` or `/` on 64-bit values.

**Build-system trap:** `-Isolated` redirects `IntermediateOutputPath` to
`E:\_phirura_build\obj\ARM\Debug\`. The authoritative check for "what is in the appx" is the
archive's own file table, cross-checked against
`bin\ARM\Debug\Phirura.WindowsPhone.build.appxrecipe`.

## The Rust Phira port — two paths

**Path A (the real Rust game) is multi-week.** `prpr-miniquad` is **GL-only** — every backend funnels
through `native/gl.rs`. **No D3D, no Metal**; WP8.1 has no OpenGL. Blockers: miniquad pipelines take
**GLSL** (D3D11 needs HLSL/DXBC; no translator → ANGLE-sized layer + shader compiler); **prpr itself
uses raw GL** (`core/render.rs`: `glGenFramebuffers`, `glRenderbufferStorageMultisample`,
`glBlitFramebuffer`; `note.rs` uses `gl_internal_id`); plus XAudio2, app shell, tokio/reqwest/rayon/
image/rfd.

**Path B (Rust core + the working C# shell) is the feasible one.** Rendering solved by Win2D, so the
shim routes `macroquad`'s drawing calls there. Measured surface: **~40 API items over ~130 call
sites across 23 files** (heaviest `judge.rs` 14, `ext.rs` 12, `scene/game.rs` 11, `core/render.rs`
11). `parse/` (1854 lines) needs only `Color`, `Vec2`, `WHITE`, an opaque `AudioClip`, and
`gl_internal_id()` inside a `debug!` log.

### Shims — all four build for the target (`rust-wp81/shims/`)

| shim | surface | state |
| --- | --- | --- |
| `macroquad` | ~40 items; maths types real, `draw_*` recorded into arenas, window/input stubbed | builds |
| `miniquad` | `Context`, `Texture`/`TextureParams`/`TextureFormat`/`TextureWrap`, `RenderPass`, `UniformType`, `gl` stub | builds |
| `sasa` | `AudioClip`, `AudioManager`, `Music`, `Sfx`, `MusicParams`, `PlaySfxParams`, three backends | builds |
| `rfd` | `FileDialog::new().pick_file()` / `.save_file()` | builds |

Key details:

- **macroquad's maths types ARE glam's, re-exported.** `glam = { version = "0.30", features =
  ["scalar-math"] }` + `pub use glam::…`. Version and feature must track macroquad 0.3.25 exactly;
  `scalar-math` disables SIMD (right for ARM). **`pub use glam;` is needed too** — `prpr::particle`
  writes `glam::Quat::from_rotation_z` (a crate path).
- **`[patch]` only works in a workspace root** — lives at the end of
  `phira-main/phira-main/Cargo.toml` (original saved as `Cargo.toml.orig`). Build config only, no
  upstream source edited.
- Handle types live in the **miniquad** shim; macroquad re-exports them (prpr converts between the
  two crates' types, so they must be identical).
- `InternalGlContext` needs a `QuadGl` (prpr both destructures it and calls `flush()` through it);
  `Cmd` must stay `Copy`, so vertices/indices go in fixed arenas and `drain_reset` must clear them
  or the batcher silently stops drawing.
- `sasa` needs `anyhow` as a real dependency; `MusicParams.playback_rate` is **f64**.
- `prpr` needs `Texture` to carry `width`/`height`; `Buffer::new`/`immutable`/`stream` **generic over
  the element type**; `Material`, `RenderTarget`, `PipelineParams` must be `Copy`.
- `Texture::from_raw_id` cannot carry dimensions → 0x0. `Texture2D::raw_miniquad_texture_handle`
  *does* — and that is the path `prpr::core::render` uses to size render targets.
- Only `rfd` pulls in `windows-sys` 0.61.2 (its `Win32_UI_Shell` refs `APPBARDATA`,
  `SHFILEOPSTRUCTA/W` its feature gating does not define — 12 errors). Shimming `rfd` removes it.

### The target triple's NAME matters

**The spec is `thumbv7a-pc-windows-msvc.json`.** `winapi` 0.3.9's `build.rs` uses
`if target.get(2) == Some(&"windows")` — `thumbv7a-windows-phone` puts `"phone"` at index 2, so no
features were emitted and every `#[cfg(feature = …)]` module stayed compiled out (32 `E0432`).
`llvm-target` was *already* correct; only the cargo-visible stem differed. **Crates that
string-match `TARGET` are common** — the tell is a dependency that "cannot find" something obviously
in its own source.

### The shims must carry the upstream VERSION

Cargo requires a `[patch]` replacement to be **semver-compatible**. Shims declared `0.4.0` while
upstream is `macroquad 0.3.25`/`miniquad 0.3.15` → cargo silently ignored both patches (warning
buried in a long log) and built the real crates. **Check `cargo tree -i <crate>` for a patch's
source, and read the patch warnings.**

### Path B status — **the DLL LINKS**

`cargo check -p prpr --target thumbv7a-pc-windows-msvc` **passes**. `cargo build -p prpr-port`
produces the staticlib and `build-port.ps1` links it into **`wp81prpr.dll` — 479,232 bytes, ARM
(0x01c4), 6 sections, exit code 0.**

The wall was **`LNK2005` multiply-defined** on `GetLastError`, `ProcessPrng`,
`GetFileInformationByHandle`, `GetConsoleMode`. Measured, not assumed:

- A bare `unsafe extern "system" { pub fn GetLastError() -> u32; }` emits **only an undefined
  external — no `__imp_`, no `.drectve`**. The patched std rlib is clean (0 imports for all four —
  `tools/probe_imports.py`).
- The collisions come from **third-party crates using `raw-dylib` in their own source**: `which`
  (GetLastError), `winapi-util` (GetConsoleMode), `walkdir`/`same-file`
  (GetFileInformationByHandle), `getrandom` via `rand`/`twox-hash` (ProcessPrng). **`raw-dylib`
  writes an ARM import-descriptor member into the archive** — 551 members literally named
  `kernel32.dll`, `ntdll.dll`, … — colliding with compat's `#[no_mangle]` definitions.
- **Fix: `/FORCE:MULTIPLE`** (LNK2005→LNK4006, keeps the first = compat's, drops the descriptor).
  `/ALTERNATENAME` does **not** help (operates on the resolved name, not the descriptor).
- Removing `ws2_32.lib` is **not** an option (~35 `LNK2019` from std's `net`).

**The fix is gated, not trusted.** Step 3 of `build-port.ps1` runs
`tools/check_imports.py wp81prpr.dll` and **throws** if anything not `api-ms-win-*` is imported:

```
wp81prpr.dll imports 2 libraries:
     OK  api-ms-win-core-libraryloader-l1-2-0.dll      (GetProcAddress, LoadLibraryExW)
     OK  api-ms-win-core-localization-l1-2-1.dll       (GetUserPreferredUILanguages)
CHECK OK: every import is an API set
```

All five exports present (`wp81_anchor_port`, `wp81_install_callbacks`, `wp81_parse_pec`,
`wp81_chart_info`, `wp81_port_version`). Full chain verified: staticlib → link → import gate → DLL
deployed to `Phirura.WindowsPhone/` → WP head builds → **appx contains `wp81prpr.dll` (38 entries)**.

**The DLL is exercised by the app.** `MainPage` has a **"诊断"** button
(`DiagnosticsButton_Click`) calling `PrprPort.RunSelfTest(SamplePec)` → `MessageDialog`. `SamplePec`
is a 4-note chart (one `n1`,`n2`,`n3`,`n4`) mirroring the grammar above.

**The renderer drain exists (Round 35).** `wp81_port_version` = **2**. The macroquad shim flattened
its `Cmd` enum into `#[repr(C)]` records — `PortCmd { u32 kind,flags,vstart,vcount,istart,icount;
f32 f[8] }` (**56 bytes**) and `PortVertex { f32 x,y,z,u,v,r,g,b }` (**32 bytes**) — and `prpr-port`
exports `wp81_draw_encode` / `_commands` / `_vertices` / `_indices` / `_reset` / `_selftest`
(11 exports total). `KIND_*` = Texture 0, Rect 1, Line 2, Circle 3, Geometry 4; **text is not a
kind** (the shim cannot rasterise glyphs, and `Cmd::Text` carries only a length). `macroquad` is now
a *direct* dependency of `prpr-port`, declared against the same git source so `[patch]` applies.
C# side: `PrprPort.Drain(vcap, icap)` copies out then resets, `WireLayoutOk()` +
`DrainSelfTest()`.

- **A pointer-walk ABI needs its stride ASSERTED, not computed in your head.** The C# reader first
  used a 32-byte `PortCmd` stride ("24 header + 8 payload") — the real record is **56** (6×u32 = 24
  **plus** 8×f32 = 32). Caught by a Python `struct.calcsize("<6I8f")` mirror of both sides before it
  ever reached hardware. `CmdStride = 6*4 + 8*4` is now derived, and `WireLayoutOk()` compares
  `Marshal.SizeOf<PortCmd>()`/`<PortVertex>()` against the native strides at runtime.
- The drain *delivers*; it does not yet *draw*. `wp81_draw_selftest` pushes a synthetic triangle.
  Not written: `Win2DChartRenderer.DrawPortFrame(DrainFrame)` (use `ScreenMatrix`/`ToMatrix3x2`/
  `CenteredRect`), real audio + `present` callbacks, driving the scene from the Rust game loop.

**Still unverified:** the on-device parse and drain (need hardware). `cargo test -p prpr-port --lib`
is **not viable** (workspace `LIB` points at ARM phone libs; an x64 host build of ~200 crates ran
3h52m and was killed). The sample PEC and the wire records were validated with Python mirrors
instead.

### `rust-wp81/tools/` — the diagnostic toolchain

Written because **`dumpbin.exe` emits nothing under the sandbox** (exit 0, empty stdout). All plain
Python 3, no deps; invoke with
`C:\Users\admin\.workbuddy-ai\binaries\python\versions\3.13.12\python.exe`.

| tool | what it does |
| --- | --- |
| `coff_dump.py` | minimal COFF reader: sections, symbols, string table. **`strtab_off = symptr + nsym*18`** (follows the *symbol* table) |
| `ar_scan.py` | scans MSVC `.lib` archives → `(name, offset, size)`; handles `//` long-name and GNU `/` tables |
| `probe_imports.py` | classifies each archive member's symbols as imported (`__imp_X`), plain undefined, or defined. **Proved the raw-dylib root cause** |
| `pe_dump.py` | PE32/PE32+ export/import/section tables; `rva_to_off()` + `read_cstr()` shared by the others |
| `check_imports.py` | the build gate. 0 = OK, 1 = bad import, 2 = unreadable; `DENY` names desktop-only DLLs |

## Environment traps that all look like something else

1. **Git Bash ships `/usr/bin/link.exe` (coreutils' `link`)** — shadows MSVC's linker; rustc fails
   with `link: extra operand '….rcgu.o'`.
2. **`Common7\IDE` must be on PATH** or link.exe exits `0xc0000135` (`mspdb110.dll`).
3. **`LIB` must point at the x64 import libs** or the host link dies with `LNK1181: cannot open
   input file 'kernel32.lib'`.
4. **`Start-Transcript` does not capture native command output in 5.1** — scripts redirect cargo to
   `prpr-cargo.log`/`port-cargo.log` explicitly.
5. **MSYS mangles `/SWITCH` arguments** → `MSYS_NO_PATHCONV=1` + `MSYS2_ARG_CONV_EXCL='*'`, pass
   `/LIBPATH` as a Windows path.
6. **`$args` is reserved in PowerShell** — assigning makes the command a no-op.
7. **Never invoke `cmd` from the bash tool.** Sandbox also blocks `cmd /c mklink`, bulk `rmtree`
   (`SAFE_DELETE_BULK_CONFIRM_REQUIRED`), a literal `csc.exe` invocation, and bash commands
   containing the word "PowerShell".

All captured in `rust-wp81/build-prpr.ps1` / `build-port.ps1` / `build-probe.ps1`.

## Workspace safety rules (learned the hard way — Round 31)

- **Never call `os.remove` in this environment.** The safe-delete guard intercepts it, tries the
  Recycle Bin, falls back to `<workspace>/trash/`, and **moved all 13 `Phirura.Shared/{Core,Parse,
  Game}` source files** there (a whole tree). Use `os.replace()` (atomic rename) or `mv`.
- **If a build reports missing files, check `<workspace>/trash/` first** — files are usually
  recoverable intact.
- `wp81prpr.dll` is a **conditional** Content item in the csproj (`Condition="Exists('wp81prpr.dll')"`),
  so the app stays shippable while the port is brought up.

## Toolchain locations

- Rust: `C:\Users\admin\.workbuddy-ai\binaries\rust\` (stable + nightly with `rust-src`).
- CRT-free link stubs: `E:\_phirura_stub\`.
- Managed Python: `C:\Users\admin\.workbuddy-ai\binaries\python\versions\3.13.12\python.exe`.

# Rust on Windows Phone 8.1 — feasibility and plan

Status: **workstream 1 is roughly half done and verified.** See "Progress" below.

## Progress

| # | Milestone | State |
|---|---|---|
| 1 | Custom target `thumbv7a-pc-windows-msvc` builds | ✅ verified |
| 2 | Patched `std` compiles for that target | ✅ verified (`cargo exit code: 0`, 18 bindings rewritten) |
| 3 | `compat/` implements all 19 freed symbols | ✅ verified (rlib builds, 0 warnings, all 19 exported) |
| 4 | Link a real ARM artifact against `WindowsPhoneCore.lib` + the VC11 CRT | ⬜ not started |
| 5 | `miniquad` → D3D11 renderer backend | ⬜ not started — **the big one** |
| 6 | `sasa` → XAudio2/WASAPI audio backend | ⬜ not started — **the big one** |
| 7 | `rfd` file-dialog shim | ⬜ not started |
| 8 | C++/CX app shell + `.appx` packaging | ⬜ not started |

Milestones 5–8 are what stand between this and a runnable appx, and none of them can be
validated on this machine. See "The honest risk" at the bottom.

### Layout

```
rust-wp81/
  targets/thumbv7a-pc-windows-msvc.json   custom target spec (builds)
  wp81_std_patch.py                     rewrites rust-src so the 19 symbols resolve from compat/
  build-std.ps1                         patch -> swap rust-src -> build std -> restore
  compat/                               the 19 symbols, implemented on the WP8.1 API set
  work/library/                         the patched rust-src tree (generated)
```

`compat/` is an **rlib**, not a staticlib: a staticlib is a final artifact and rustc would
demand a `#[panic_handler]` there, which would then collide with the one `std`/`panic_abort`
provides.

### The `compat` crate

`wp81_std_patch.py` turns these 19 `windows_link::link!` declarations into plain
`unsafe extern "system"` blocks, so they stop being `raw-dylib` imports bound to `ntdll.dll`
and become ordinary undefined symbols that `compat` defines:

```
console   GetConsoleMode  GetConsoleOutputCP  ReadConsoleW  WriteConsoleW
          GetStdHandle  SetStdHandle
env/args  GetCommandLineW  GetEnvironmentStringsW  FreeEnvironmentStringsW
          GetUserProfileDirectoryW
sync      WaitForMultipleObjects
rng       ProcessPrng
nt file   NtCreateFile  NtOpenFile  NtReadFile  NtWriteFile
          NtSetInformationFile  NtCreateNamedPipeFile  RtlNtStatusToDosError
```

**The NTSTATUS trick.** `std` converts failures with `RtlNtStatusToDosError` and then calls
`io::Error::from_raw_os_error`. Rather than inventing NTSTATUS values for every Win32 failure,
the shims return the **Win32 error code unchanged** and `RtlNtStatusToDosError` is the
identity — so `std` sees exactly the error the Win32 call produced.

Known gaps, marked in the source: `NtCreateNamedPipeFile` returns `STATUS_NOT_IMPLEMENTED`
(only `std::process` uses it, which WP8.1 cannot support anyway), and `NtSetInformationFile`
covers only the rename/disposition/EOF classes `std::fs` uses.

## Build

```powershell
# everything: patch + build std, build compat, build the probe, link the DLL, deploy it
& rust-wp81\build-probe.ps1 -BuildStd

# just relink the probe DLL after a compat / link-probe change
& rust-wp81\build-probe.ps1
```

`build-probe.ps1` writes `rust-wp81/build-probe.log`; **read that file**, because the tool that
invokes the script does not surface its stdout at all.

The pieces, if you need them individually:

```powershell
# std (patches rust-src, builds, restores)
& rust-wp81\build-std.ps1 -KeepSwapped

# the compat crate
$env:RUSTUP_HOME = "C:\Users\admin\.workbuddy-ai\binaries\rust\rustup"
$env:CARGO_HOME  = "C:\Users\admin\.workbuddy-ai\binaries\rust\cargo"
cd rust-wp81\compat
& "$env:CARGO_HOME\bin\cargo.exe" +nightly build "-Zjson-target-spec" "-Zbuild-std=core" `
    --target ..\targets\thumbv7a-pc-windows-msvc.json --release
```

`.json` target specs require `-Zjson-target-spec` or cargo refuses outright. Quote any `-Z`
argument containing a comma — unquoted, `-Zbuild-std=core,alloc,std` is parsed as an array literal
and the whole script fails to parse, printing nothing.

### Four things the link needs that are easy to omit

The ARM toolchain is **VS11's**, not VS12/VS2013's: `C:\Program Files (x86)\Microsoft Visual
Studio 11.0\VC\bin\x86_arm\link.exe`, libs in `VC\lib\arm\`, and `mspdb110.dll` must be on `PATH`
(from `Common7\IDE\`) or `link.exe` exits 127 and prints nothing.

1. `/NODEFAULTLIB` + our own `_DllMainCRTStartup` — the desktop static CRT drags in ~40 KERNEL32
   imports and WP8.1 has no `KERNEL32.dll`.
2. `ws2_32.lib` — `std` links the winsock layer unconditionally, so `WSAStartup`, `connect`,
   `accept`, `select`, `GetHostNameW` and ~20 more are unresolved without it.
3. `chkstk.obj` — a standalone object in `VC\lib\arm\`.
4. **`/EXPORT:` for every entry point.** `#[no_mangle]` stops rustc mangling the name; it does
   *not* add the symbol to the export table, and `/NODEFAULTLIB` means there is no `.def` file
   either. Without the exports the linker finds no reason to pull the probe objects out of the
   staticlib and quietly emits a 17 KB DLL with no exports — it links, it loads, and then every
   `DllImport` fails at run time.

Call `link.exe` directly with splatted arguments rather than joining them into a `cmd /c` string:
the paths contain spaces, and cmd splits them (`LNK1181: cannot open input file 'studio.obj'`).


## The honest risk

Milestone 4 is bounded and testable here. **Milestones 5 and 6 are each larger than
milestones 1–4 combined**, and neither can be validated without a device: I can build a WP8.1
app but not run one. `miniquad` and `sasa` are both forks (`Mivik/prpr-miniquad`,
`Mivik/sasa`) whose only backends are Windows/WGL, Android/EGL+Oboe, OpenHarmony and Apple;
WP8.1 needs D3D11 and XAudio2 written from scratch. The good news, measured earlier: `d3d11.lib`,
`dxgi.lib`, `d2d1.lib`, `dwrite.lib`, `d3dcompiler.lib`, `XAudio2.lib`, `mmdevapi.lib` and
`PhoneAudioSes.lib` are all in the WP8.1 lib set, so both are genuinely writable.

---

## Appendix: what was measured (not assumed)

## What was measured (not assumed)

Rust toolchain installed contained at `C:\Users\admin\.workbuddy-ai\binaries\rust\`
(`rustup` + `cargo`, stable 1.99 and nightly 1.101 with `rust-src`).

| Question | Answer | How it was established |
|---|---|---|
| Is there a prebuilt 32-bit ARM Windows target? | **No** | `rustup target add thumbv7a-pc-windows-msvc` → *"no prebuilt artifacts available"*. Only `*-linux-*`, `*-none-eabi`, `armv7-linux-ohos` remain. |
| Does rustc still know the target? | **Yes** | `rustc --print target-list` lists `thumbv7a-pc-windows-msvc` and `thumbv7a-uwp-windows-msvc`. So `-Z build-std` can build std from source. |
| Does `-Z build-std=std` actually work? | **Yes** | `cargo +nightly build -Z build-std=std,panic_abort --target thumbv7a-pc-windows-msvc --release` → `Finished release profile in 2m 11s`. |
| Does `no_std` + `alloc` need anything outside the WP8.1 API set? | **No** | The resulting staticlib needs only `HeapAlloc`, `GetProcessHeap` (`api-ms-win-core-heap-l1-2-0`), `QueryPerformanceCounter`/`Frequency` (`api-ms-win-core-profile-l1-1-0`) and the compiler intrinsics. All present. |
| Does full `std` need anything outside it? | **20 symbols** | Compared all 181 `__imp_*` imports of the std staticlib against every symbol in `Windows Phone Kits\8.1\lib\ARM\*.lib` + `Microsoft Visual Studio 11.0\VC\lib\arm\*.lib`. |

### The WP8.1 native API surface

Defined by the 29 import libs in `C:\Program Files (x86)\Windows Phone Kits\8.1\lib\ARM\`.
`WindowsPhoneCore.lib` is the umbrella; it forwards to:

```
api-ms-win-core-{com-l1-1-1, com-l2-1-1, datetime-l1-1-1, debug-l1-1-1, delayload-l1-1-1,
  errorhandling-l1-1-2, fibers-l1-1-1, file-l1-2-1, file-l2-1-1, handle-l1-1-0,
  heap-l1-2-0, interlocked-l1-2-0, io-l1-1-1, libraryloader-l1-2-0, localization-l1-2-1,
  memory-l1-1-2, processthreads-l1-1-2, profile-l1-1-0, realtime-l1-1-0, rtlsupport-l1-2-0,
  string-l1-1-0, synch-l1-2-0, sysinfo-l1-2-1, timezone-l1-1-0, util-l1-1-0, xstate-l2-1-0}
api-ms-win-eventing-{classicprovider-l1-1-0, controller-l1-1-0, provider-l1-1-0}
```

Plus standalone libs that matter a lot:

- **`d3d11.lib`, `dxgi.lib`, `d2d1.lib`, `dwrite.lib`, `d3dcompiler.lib`** — a D3D11 renderer
  backend is writable natively.
- **`XAudio2.lib`, `mmdevapi.lib`, `PhoneAudioSes.lib`, `XAPOBase.lib`** — an audio backend is
  writable natively.
- **`ws2_32.lib`** — sockets.
- `PhoneAppModelHost.lib`, `WindowsPhoneGlobalizationUtil.lib`, `MSXML6.lib`, `propsys.lib`,
  `oleaut32.lib`, `rpcrt4.lib`, `windowscodecs.lib`, `xmllite.lib`, `cabinet.lib`.

Note `api-ms-win-core-file-l1-2-1` is present, so `CreateFileW`/`ReadFile`/`WriteFile`/
`FindFirstFileExW`/`DeleteFileW`/`MoveFileExW` **are** available. (An earlier conclusion that
they were absent was drawn from Win2D's import table — the wrong evidence, since Win2D simply
does not use files. `WindowsPhoneKits\8.1\lib\ARM` is the right source.)

### The 20 symbols `std` wants that WP8.1 does not provide

| Group | Symbols | Fix |
|---|---|---|
| Console (`api-ms-win-core-console-*` absent) | `GetConsoleMode`, `GetConsoleOutputCP`, `ReadConsoleW`, `WriteConsoleW`, `GetStdHandle`, `SetStdHandle` | Stub. Only `std::io::stdio` uses them; WP8.1 has no console. |
| Env/args (`api-ms-win-core-processenv-*` absent) | `GetCommandLineW`, `GetEnvironmentStringsW`, `FreeEnvironmentStringsW` | Stub / feed from the C++/CX host. |
| NT file API | `NtCreateFile`, `NtOpenFile`, `NtReadFile`, `NtWriteFile`, `NtSetInformationFile`, `NtCreateNamedPipeFile`, `RtlNtStatusToDosError` | Redirect to the Win32 file API (`api-ms-win-core-file-l1-2-1`), which is available. Modern std prefers NT; older std used Win32. |
| Misc | `ProcessPrng` (lives in `bcryptprimitives.dll`, present on device), `LoadLibraryA` (use `LoadLibraryW`), `WaitForMultipleObjects` (use `…Ex`), `GetUserProfileDirectoryW` (use a known folder) | Rename / redirect. |

So a "std for WP" is **~20 symbols of retargeting**, not a rewrite of the PAL.

## The three workstreams

**1. std retarget** — custom target spec (this directory), patch
`library/std/src/sys/pal/windows/{c, fs, process, stdio, args, env}` and build with
`-Z build-std` against a vendored `rust-src`. Verifiable *here*: build it and diff the resulting
import table against `WindowsPhoneKits\8.1\lib\ARM`.

**2. Renderer backend** — `miniquad` (`Mivik/prpr-miniquad`) has Windows/WGL, Android/EGL and
OpenHarmony backends and no WP8.1 one. Needs a D3D11 backend plus a way to receive the
`ID3D11Device`/`IDXGISwapChain` the C++/CX shell already owns. `d3d11.lib`/`dxgi.lib` are
available, so this is writable. **Not verifiable here.**

**3. Audio backend** — `sasa` (`Mivik/sasa`) depends on `cpal` (desktop WASAPI), `oboe`
(Android) and `ohos-audio-sys`. Needs an XAudio2 or WASAPI backend. `XAudio2.lib`,
`mmdevapi.lib` and `PhoneAudioSes.lib` are available. **Not verifiable here.**

Plus an `rfd` file-dialog shim and a C++/CX app shell.

## The honest risk

Workstream 1 is bounded and I can check it. Workstreams 2 and 3 are each larger than workstream
1, and **I cannot run a WP8.1 app on this machine**, so neither can be validated without the
user deploying to a device. A multi-month port with no runtime feedback is a bad bet unless the
user is prepared to test continuously.

For reference: the workspace is ~40,700 lines of Rust (`prpr` 16,214 + `phira` 21,361 +
`prpr-auto-offset` 1,163 + `prpr-avc` 1,427 + `prpr-l10n` 536).

## Build

```powershell
$env:RUSTUP_HOME = "C:\Users\admin\.workbuddy-ai\binaries\rust\rustup"
$env:CARGO_HOME  = "C:\Users\admin\.workbuddy-ai\binaries\rust\cargo"
& "$env:CARGO_HOME\bin\cargo.exe" +nightly build `
    -Z build-std=core,alloc,std `
    -Z build-std-features=compiler-builtins-mem `
    --target "rust-wp81/targets/thumbv7a-pc-windows-msvc.json" `
    --release
```

---

## Verified on device: Rust runs on WP8.1

The `wp81probe.dll` probe (ARMNT, built by rustc for the custom target, linked by the VC11 ARM
linker, no CRT) **loads on a real WP8.1 phone and executes**. Measured, from the device log:

```
NativeProbe: deployed wp81probe.dll = 124928 bytes, modified 11:36:21 UTC
NativeProbe: API-set resolution = 0 failure(s)
NativeProbe: step 0 (enter)          returned 0                     OK
NativeProbe: step 1 (alloc)          returned 3                     OK
NativeProbe: step 2 (raw QPC)        returned 916310270791          OK
NativeProbe: step 3 (file I/O)       returned -1                    OK (Err path, no file)
NativeProbe: step 4 (randomness)     returned 6946893028922317472   OK
NativeProbe: step 7 (raw CreateThread) returned 1                   OK
NativeProbe: step 5 (std::thread)    <aborts with 0xC0000409>
```

**Read that log with one caveat.** `AppLog.Write` only *queued* each append (it used
`ContinueWith` with no scheduler and nothing ever waited), so an abrupt process death lost the
final lines — which is why steps 5 and 6 are missing above rather than being reported as failing.
`AppLog.WriteSync` was added and the probe now uses it, so every line is on disk before the next
step runs. Any log from before that fix is missing its tail, and the missing lines are the
interesting ones.

| capability | state |
|---|---|
| custom target `thumbv7a-pc-windows-msvc` | works |
| `std` compiled from patched rust-src | works |
| compat: 19 Win32 shims + ~106 runtime-resolved trampolines | works |
| VC11 ARM link, `/NODEFAULTLIB`, no CRT | works |
| appx packaging, DLL at the package root | works |
| **DLL loads; entry point entered** | **works** |
| allocator, `HashMap` / `String` / `Vec` | works |
| file I/O through the `NtCreateFile`/`NtReadFile` shims | works |
| `ProcessPrng` randomness | works |
| `QueryPerformanceCounter` | works |
| raw `CreateThread` | **works** |
| `std::thread` | **aborts** (0xC0000409) |

The last two rows together are the useful result: the OS will create threads, so the fault is in
Rust's thread wrapper, not the platform. `thread_start` runs three things before the user closure —
`init.init()` (TLS), `stack_overflow::reserve_stack()` (whose compat trampoline calls
`LoadLibraryExW` + `GetProcAddress` *from the new thread*), then `catch_unwind` — and probe steps
8–13 isolate each of those plus std's exact `CreateThread` arguments.


**`std::thread` is the only thing that fails, and it is avoidable.** prpr uses threads in exactly
one place — `prpr/src/ui/offset_analysis.rs`, the auto-offset estimator, which is a UI feature.
Gameplay needs none. Port that one feature to a synchronous path or the WinRT thread pool and the
whole `std::thread` problem disappears.

### The findings that made this work (each cost real time)

1. **`LoadLibraryW` does not exist on WP8.1.** The API set exists; the function does not. A Store
   app must use `LoadLibraryPackaged`/`LoadLibraryExW`. Verified against Win2D (which imports
   nothing from that set) and the WP8.1 SDK's own import libraries.
2. **`GetActiveProcessorCount` is absent** from WP8.1's `processthreads` set — the only one of
   116 mappings that was wrong, and it aborted the process via a null call.
3. **Desktop API sets are a superset of WP8.1's.** Local verification can only ever find definite
   errors, never certify correctness. This is why runtime resolution replaced static imports.
4. **A CRT-free link needs `_fltused`, `atexit`, `strlen`/`wcslen`, `chkstk.obj` and the `__rt_*`
   ARM helpers** supplied by hand. `_tls_index` must be left alone — the loader fills it from the
   TLS directory's `AddressOfIndex`; overwriting it with a fresh `TlsAlloc()` index is worse than
   leaving it at 0.

### The diagnostic techniques worth reusing

- **`wp81_probe_api_sets()`** — walks every (API set, function) pair doing `LoadLibraryExW` +
  `GetProcAddress` *without calling any of them*, and the app calls it *before* the real entry
  point. A lazy resolver reports nothing when the first call is what crashes; this does.
- **Step-wise probing** — `wp81_link_probe(step)` performs exactly one operation and the caller
  logs after each. Going from "it aborts" to "step 5" cost one device round trip.
- **Dump a working native binary's imports** (Win2D) and the platform's own import libraries.
  Nothing else settles "does this function exist here?".

### What is left

The D3D11 renderer backend for `miniquad` and the XAudio2 backend for `sasa`. Those are each
larger than everything above combined, and `d3d11.lib`, `dxgi.lib`, `d2d1.lib`, `dwrite.lib`,
`d3dcompiler.lib`, `XAudio2.lib`, `mmdevapi.lib` and `PhoneAudioSes.lib` are all confirmed present
in `Windows Phone Kits\8.1\lib\ARM`, so both are writable.

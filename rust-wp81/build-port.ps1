# Build prpr-port into wp81prpr.dll for Windows Phone 8.1.
#
# Same shape as build-probe.ps1: rustc produces a staticlib, and the VC11 ARM linker turns it into
# an ARMNT DLL with no CRT.
#
# The crate is built from the *phira workspace*, not from rust-wp81. It has to be: `[patch]` is only
# honoured in a workspace root, so a standalone crate would resolve macroquad/miniquad/sasa/rfd to
# the real ones - the crates that cannot build for WP8.1. Being a member also means it reuses the
# dependency artifacts already compiled for that target instead of rebuilding ~200 crates.
#
# The CRT-free requirement is the same one that shaped wp81probe.dll: WP8.1 has no KERNEL32.dll, so
# the desktop ARM CRT's startup objects cannot be linked. /NODEFAULTLIB plus an entry point supplied
# by the crate is the whole story.
#
# Two things that are easy to get wrong and fail silently:
#   * /EXPORT: for every entry point. `#[no_mangle]` does not export from a staticlib, and with
#     /NODEFAULTLIB there is no .def file either. Without them the linker emits a tiny DLL with no
#     exports that loads fine and then fails every DllImport.
#   * ws2_32.lib on the link line. std links the winsock layer unconditionally.
#
# Usage:
#   & build-port.ps1
#   & build-port.ps1 -NoDeploy

param(
    [switch]$NoDeploy
)

$ErrorActionPreference = "Stop"

$here = Split-Path -Parent $MyInvocation.MyCommand.Path

$vs11   = "C:/Program Files (x86)/Microsoft Visual Studio 11.0"
$wpKit  = "C:/Program Files (x86)/Windows Phone Kits/8.1"
$sdkLib = "C:/Program Files (x86)/Windows Kits/8.1/Lib/winv6.3"
# The base Windows Kits tree, not the phone kit's overlay: see step 1c for why the overlay cannot
# be put on the include path at all. The phone kit is still checked for, because a machine without
# it has no XAudio2 SDK to build against in any configuration.
$sdkInc       = "C:/Program Files (x86)/Windows Kits/8.1/Include"
$sdkIncUm     = Join-Path $sdkInc "um"
$sdkIncShared = Join-Path $sdkInc "shared"
$wpIncArm     = Join-Path $wpKit "Include"

$env:RUSTUP_HOME = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { "C:/Users/admin/.workbuddy-ai/binaries/rust/rustup" }
$env:CARGO_HOME  = if ($env:CARGO_HOME)  { $env:CARGO_HOME }  else { "C:/Users/admin/.workbuddy-ai/binaries/rust/cargo" }
$env:RUSTUP_TOOLCHAIN = "nightly"

# Host linker first (Git Bash's /usr/bin/link.exe is coreutils' `link` and would shadow it), and
# Common7\IDE for mspdb110.dll. See build-prpr.ps1 for the full explanation.
$env:PATH = "$vs11\VC\bin;$vs11\Common7\IDE;" + $env:PATH
$env:LIB  = "$vs11\VC\lib\amd64;$sdkLib\um\x64"

$cargo     = Join-Path $env:CARGO_HOME "bin\cargo.exe"
$armLink   = Join-Path $vs11 "VC\bin\x86_arm\link.exe"
$vcLibArm  = Join-Path $vs11 "VC\lib\arm"
$wpLibArm  = Join-Path $wpKit "lib\ARM"
$chkstk    = Join-Path $vcLibArm "chkstk.obj"

$ws      = "E:/Users/admin/Documents/visual studio 2013/Phirura/phira-main/phira-main"
$target  = Join-Path $here "targets\thumbv7a-pc-windows-msvc.json"
$outDir  = Join-Path $here "port-out"
$dll     = Join-Path $outDir "wp81prpr.dll"
# The TLS template markers, compiled separately and linked first/last - see step 1b.
$tlsStartSrc = Join-Path $here "tlsdir_start.rs"
$tlsEndSrc   = Join-Path $here "tlsdir_end.rs"
$deploy  = "E:/Users/admin/Documents/visual studio 2013/Phirura/Phirura/Phirura/Phirura.WindowsPhone/wp81prpr.dll"

# The XAudio2 mixer is C++ rather than Rust, so it needs the VC11 ARM cross compiler and the WP8.1
# kit's Include directory - xaudio2.h exists nowhere else on this machine. It compiles to a single
# object that goes straight onto the link line as a direct object: no /EXPORT: for the internal
# half, no second DLL to deploy, and no import table entry for xaudio2_8.dll either, because that
# module is resolved from inside the code instead of being linked. Keeping the import table at
# seven API sets is the whole point - a DLL that names a library the phone does not have fails to
# load at all, and the failure looks like an app that never starts.
$armCl    = Join-Path $vs11 "VC\bin\x86_arm\cl.exe"
$mixSrc   = Join-Path $here "wp81mix.cpp"
$mixObj   = Join-Path $outDir "wp81mix.obj"

foreach ($p in @($cargo, $armLink, $vcLibArm, $wpLibArm, $chkstk, $target, $ws, $tlsStartSrc, $tlsEndSrc, $armCl, $wpIncArm, $sdkIncUm, $sdkIncShared, $mixSrc)) {
    if (-not (Test-Path $p)) { throw "missing: $p" }
}

Set-Location $ws

Write-Host "=== 0. patch the registry crates that bind Win32 with raw-dylib ==="
# `which` and `getrandom` each declare their Win32 imports with `#[link(kind = "raw-dylib")]` in
# their own source. That attribute makes rustc emit an ARM *import descriptor*, which the linker
# puts in `.idata` no matter what `/FORCE:MULTIPLE` decides about the duplicate symbol - so the
# DLL ends up importing kernel32.dll and bcryptprimitives.dll, and WP8.1's loader refuses it.
#
# `wp81_std_patch.py` fixes the same mistake in std, but it only walks rust-src; cargo compiles
# dependencies out of $CARGO_HOME, where nothing patches them. This is that missing pass. It is
# idempotent - a second run reports 0 changes - and `check_imports.py` in step 3 is what proves it
# worked.
$py = "C:\Users\admin\.workbuddy-ai\binaries\python\versions\3.13.12\python.exe"
# The wp81_*.py helpers live in the repo-level tools\ directory, not next to this script.
$pyTools = Join-Path (Split-Path -Parent $here) "tools"
& $py (Join-Path $pyTools "wp81_registry_patch.py") --apply
if ($LASTEXITCODE -ne 0) { throw "registry patch failed ($LASTEXITCODE)" }

# Second half of the same pass, and the one that has to run before cargo rather than after:
# `phira-mp` is the only crate asking tokio for `net`, and that single feature line is what
# enables `mio` - whose Windows IOCP/AFD path links ws2_32.dll, kernel32.dll and ntdll.dll into
# the image whether or not any of it is called. The registry patch above cannot reach a git
# checkout, so this is its counterpart: drop `net` from the tokio table and stand a failing
# `TcpStream` in for the type the two phira-mp crates import. Idempotent, and `--check` is the
# same assertion `wp81_registry_patch.py --check` makes.
& $py (Join-Path $pyTools "wp81_phira_mp_patch.py") --check
if ($LASTEXITCODE -ne 0) {
    & $py (Join-Path $pyTools "wp81_phira_mp_patch.py") --apply
    if ($LASTEXITCODE -ne 0) { throw "phira-mp patch failed ($LASTEXITCODE)" }
}

# NOTE, if step 3 ever reports `kernel32.dll` after a patch above has changed: re-running the
# two scripts is NOT enough, and neither is `cargo clean -p`.
#
# cargo fingerprints a registry dependency by package identity, not by the bytes it was extracted
# with, so a crate compiled before a patch goes on linking against the old expansion for ever. That
# is not a theory: `windows-link` was patched at 14:26 on 2026-10-05, chrono's rlib was still the
# one built at 12:57, `port-cargo.log` showed only `phira` and `prpr-port` being compiled, and
# `link.exe` kept emitting `prpr_port.lib(kernel32.dll)` with `__imp_GetTimeZoneInformationForYear`
# in it while every other symbol had already moved to an API set.
#
# `cargo clean -p` is the obvious repair and it does not work here: this cargo keeps the rlib *and*
# its fingerprint together under
#
#   target\thumbv7a-pc-windows-msvc\release\build\<package>\<hash>\
#
# and `cargo clean -p <package>` does not remove that directory - a second run reported
# "Removed 0 files" with `libchrono-*.rlib` still dated 12:57. What makes the unit dirty is the
# directory being gone. So the repair is to *move* it (moved, not deleted, so it can be put back),
# starting from `phira-main\phira-main`:
#
#   $b    = 'target\thumbv7a-pc-windows-msvc\release\build'
#   $park = 'target\thumbv7a-pc-windows-msvc\build-parked'
#   foreach ($p in 'windows-link','windows-targets','windows-sys','chrono',
#                  'parking_lot_core','libloading','which','getrandom','xxhash-rust') {
#       Move-Item (Join-Path $b $p) (Join-Path $park $p) -Force
#   }
#
# The list is also printed by the scripts above as `REGISTRY_PATCH_CLEAN=<csv>` / `PHIRA_MP_CLEAN`
# - use that rather than this literal, so it cannot drift. Downstream crates (chrono, windows-sys,
# parking_lot_core) go dirty on their own once `windows-link` recompiles to a different metadata
# hash, but they are in the list too so a partial failure does not leave one of them stale.
#
# Deliberately not automatic: `check_imports.py` in step 3 catches this exact failure, and moving
# the directories costs a recompile of ~40 crates on every patch change.

Write-Host "`n=== 1. cargo build -p prpr-port (staticlib) ==="
# Every -Z argument containing a comma MUST be quoted: unquoted, the shell parses `std,panic_abort`
# as an array literal and fails to parse the whole script, so nothing runs at all.
# `--lib`, and it must stay: the package's bins are desktop-only. `host_m1` calls
# `tracing_subscriber`, which is a `cfg(not(target_arch = "arm"))` dependency, so building the whole
# package for ARM fails on a bin that could never link here. This is the same reason
# d3d11-rs-probe\build.ps1 passes `--lib`.
$cargoArgs = @(
    "build", "-p", "prpr-port", "--lib",
    "--target", $target,
    "-Zjson-target-spec",
    "-Zbuild-std=std,panic_abort",
    "-Zbuild-std-features=compiler-builtins-mem",
    "--release"
)
$cargoLog = Join-Path $here "port-cargo.log"
$savedEap = $ErrorActionPreference
$ErrorActionPreference = "Continue"
& $cargo @cargoArgs 2>&1 | Out-File -FilePath $cargoLog -Encoding utf8
$code = $LASTEXITCODE
$ErrorActionPreference = $savedEap
Write-Host "cargo exit code: $code"
Get-Content $cargoLog -ErrorAction SilentlyContinue | Select-Object -Last 40
if ($code -ne 0) { throw "cargo build failed ($code) - see port-cargo.log" }

$staticlib = Join-Path $ws "target\thumbv7a-pc-windows-msvc\release\prpr_port.lib"
if (-not (Test-Path $staticlib)) { throw "no staticlib at $staticlib" }
Write-Host "staticlib = $((Get-Item $staticlib).Length) bytes"

# --- 1b. compile the two TLS template markers as standalone objects -------------------------
#
# `_tls_used` cannot live in `prpr-port` or in `wp81-compat`. Both are archives, and a member of an
# archive is only added when something references it - so its contribution to `.tls$` lands wherever
# the resolution order happens to put it, which on the device-shaped build was offset 776 of 2572.
# Since offsets are computed relative to the start of the section holding the variable, a marker
# that is not at offset 0 makes `StartAddressOfRawData` disagree with every `#[thread_local]` in the
# image, and nothing faults: the values are just shifted.
#
# A *direct* object is not resolved, it is included outright, and it contributes before any library
# named after it. So: one object first on the link line (the start marker), one last (the end
# marker), each compiled from a file whose whole contents are those markers. `check_tls.py` reads
# both bytes back out of the finished PE, so if link.exe ever stops ordering them this way the build
# fails here rather than as garbage on the phone.
Write-Host "`n=== 1b. compile TLS markers (rustc --emit=obj) ==="
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$rustc = Join-Path $env:CARGO_HOME "bin\rustc.exe"
if (-not (Test-Path $rustc)) { throw "missing: $rustc" }

# `--emit=obj` still has to type-check `#![no_std]`, which means loading `core` and
# `compiler_builtins`. cargo normally passes them, built by `-Zbuild-std` in step 1, from here -
# they are not in the sysroot for this target.
#
# `--target` must be the same JSON spec cargo used, not the built-in name: the rlib records its
# triple as `thumbv7a-pc-windows-msvc-<hash of the spec>`, and asking for the bare builtin triple
# makes rustc report E0461 "couldn't find crate core with expected target triple" while printing
# the very file it was handed. A JSON spec is itself unstable, so it needs `-Zunstable-options`
# too - cargo hides that behind `-Zjson-target-spec` in step 1.
#
# The `.rlib` beside each is only a *stub*; rustc says so outright ("only metadata stub found ...
# please provide path to the corresponding .rmeta"). Type-checking needs the `.rmeta`.
$tlsDepDir = Join-Path $ws "target\thumbv7a-pc-windows-msvc\release\build"
$coreRmeta = Get-ChildItem (Join-Path $tlsDepDir "core") -Recurse -Filter "libcore-*.rmeta" `
    -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $coreRmeta) { throw "no built libcore metadata for thumbv7a - step 1 must have run first" }
$builtinRmeta = Get-ChildItem (Join-Path $tlsDepDir "compiler_builtins") -Recurse `
    -Filter "libcompiler_builtins-*.rmeta" -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $builtinRmeta) { throw "no built compiler_builtins metadata for thumbv7a - step 1 must have run first" }
Write-Host "core = $($coreRmeta.FullName)"
Write-Host "builtins = $($builtinRmeta.FullName)"

$tlsStartObj = Join-Path $outDir "tlsdir_start.obj"
$tlsEndObj   = Join-Path $outDir "tlsdir_end.obj"
$tlsEndLib   = Join-Path $outDir "tlsdir_end.lib"
$tlsLog      = Join-Path $here "port-tlsdir.log"
$libExe      = Join-Path $vs11 "VC\bin\x86_arm\lib.exe"
$tlsRustcArgs = @("-Zunstable-options", "--target", $target, "--emit=obj", "--crate-type=lib",
                  "--extern", "core=$($coreRmeta.FullName)",
                  "--extern", "compiler_builtins=$($builtinRmeta.FullName)")
foreach ($m in @(@($tlsStartSrc, $tlsStartObj), @($tlsEndSrc, $tlsEndObj))) {
    $src, $objPath = $m
    $tlsArgs = $tlsRustcArgs + @("-o", $objPath, $src)
    $saved2 = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & $rustc @tlsArgs 2>&1 | Out-File -FilePath $tlsLog -Encoding utf8
    $tlsCode = $LASTEXITCODE
    $ErrorActionPreference = $saved2
    Get-Content $tlsLog -ErrorAction SilentlyContinue | Select-Object -Last 30
    if ($tlsCode -ne 0) { throw "rustc --emit=obj failed ($tlsCode) for $src - see port-tlsdir.log" }
    if (-not (Test-Path $objPath)) { throw "rustc produced no object for $src" }
    Write-Host ("  {0} = {1} bytes" -f (Split-Path -Leaf $objPath), (Get-Item $objPath).Length)
}

# `_tls_end` goes into an ARCHIVE, not onto the command line as an object, and the difference is
# the whole point. link.exe takes every direct object first, in the order they are listed, and only
# then searches libraries - so an object named last on the line still contributed at offset 1 of
# `.tls`, immediately after `_tls_start`, giving a two-byte template. An archive member is pulled
# only when something needs it and nobody does until the libraries are being scanned, which puts it
# after `prpr_port.lib`'s own `.tls$` contributions and therefore at the far end.
#
# `check_tls.py` is what says whether that actually happened.
if (-not (Test-Path $libExe)) { throw "missing: $libExe" }
$saved3 = $ErrorActionPreference
$ErrorActionPreference = "Continue"
& $libExe "/OUT:$tlsEndLib" $tlsEndObj 2>&1 | Out-File -FilePath $tlsLog -Encoding utf8 -Append
$libCode = $LASTEXITCODE
$ErrorActionPreference = $saved3
if ($libCode -ne 0) { throw "lib failed ($libCode) for $tlsEndObj - see port-tlsdir.log" }
Write-Host ("  {0} = {1} bytes" -f (Split-Path -Leaf $tlsEndLib), (Get-Item $tlsEndLib).Length)

# ---- 1c. the XAudio2 mixer ----
#
# vcvarsall is deliberately not used, and INCLUDE is set by hand for a reason worth writing down:
# Windows Phone Kits\8.1\Include is an *overlay*, not a complete SDK. It carries 263 headers
# against the base kit's 1569, so putting it on the path first makes winbase.h come from the
# overlay while processthreadsapi.h comes from the base kit - two definitions of
# _THREAD_INFORMATION_CLASS and _MEMORY_PRIORITY_INFORMATION, which is C2011 rather than a phone
# build. (It is also where minwindef.h is missing from, which is the first thing that fails.)
#
# The overlay turns out to be unnecessary anyway: the one header this file actually cares about,
# xaudio2.h, is byte-identical in both trees - 60924 bytes, same hash. So the base kit is used in
# the order vcvars would pick (um, then shared), with VC last for <stddef.h>.
Write-Host "`n=== 1c. compile wp81mix.cpp (VC11 x86_arm, CRT-free) ==="
$mixLog    = Join-Path $here "port-mix.log"
$savedInc  = $env:INCLUDE
$env:INCLUDE = "$sdkIncUm;$sdkIncShared;$vs11\VC\include"
# /GS- is not a preference: with no CRT there is no __security_cookie to compare against, and the
# function that builds a hit sound keeps a 96-byte key buffer on the stack, so leaving it on would
# produce an unresolved __security_check_cookie rather than a working mixer. /GR- drops RTTI
# (nothing here uses dynamic_cast), /Zl stops the compiler writing defaultlib directives that
# /NODEFAULTLIB would only have to discard again, and /Oi lets the struct copies inline instead of
# reaching for a memcpy that may not resolve without a CRT.
# _CRT_BUILD_DESKTOP_APP=0 is what gets past crtdefs.h line 337, which otherwise errors out with
# "Compiling Desktop applications for the ARM platform is not supported" the moment anything
# reaches <stddef.h> - and windows.h always does. The guard above it is #ifndef, so the command
# line wins. It only decides which CRT declarations are dllimport'd, and this object calls no CRT
# function at all (there is no CRT to call: /NODEFAULTLIB, HeapAlloc for memory, loops for copies).
$mixArgs = @("/nologo", "/c", "/O2", "/Oi", "/W3", "/GS-", "/GR-", "/Zl",
             "/D_CRT_BUILD_DESKTOP_APP=0", "/DWIN32", "/D_WINDOWS",
             "/Fo$mixObj", $mixSrc)
$savedMix = $ErrorActionPreference
$ErrorActionPreference = "Continue"
& $armCl @mixArgs 2>&1 | Out-File -FilePath $mixLog -Encoding utf8
$mixCode = $LASTEXITCODE
$ErrorActionPreference = $savedMix
$env:INCLUDE = $savedInc
Get-Content $mixLog -ErrorAction SilentlyContinue | Select-Object -Last 40
if ($mixCode -ne 0) { throw "cl failed ($mixCode) for wp81mix.cpp - see port-mix.log" }
if (-not (Test-Path $mixObj)) { throw "cl produced no object for wp81mix.cpp" }
Write-Host ("  {0} = {1} bytes" -f (Split-Path -Leaf $mixObj), (Get-Item $mixObj).Length)

Write-Host "`n=== 2. link ARMNT DLL (VC11 x86_arm, CRT-free) ==="
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

# The exported surface is the whole C#-facing ABI. Keep in step with the #[no_mangle] functions in
# prpr-port/src/lib.rs and with PrprPort.cs on the C# side.
#
# `wp81_anchor_port` is NOT part of that ABI - it is the mechanism that makes the link work at all,
# and it is easy to mistake for dead code and delete. `wp81-compat`'s shims (`lstrlenW`, `atexit`,
# `LoadLibraryA`, the `Nt*` file calls, `ProcessPrng`, `_tls_index`, ...) are referenced from
# *patched std*, never from prpr-port, so rustc pulls none of them out of the compat rlib and the
# linker reports them as unresolved externals even though they are compiled and one directory away.
# An /EXPORT: for one function that takes their addresses is what drags the archive members in -
# `build-probe.ps1` does the same thing with `wp81_anchor_compat`.
$exports = @(
    "wp81_anchor_port",
    "wp81_install_callbacks",
    # The return half of the HTTP bridge: `HostCallbacks::http_request` runs Rust -> C#, and this is
    # C# -> Rust with the finished response. Same reason as every other entry - `#[no_mangle]` does
    # not export from a staticlib, and a missing line here builds cleanly and then fails every
    # DllImport at runtime.
    "wp81_http_complete",
    # The return half of the text-input bridge: `HostCallbacks::text_input` opens the dialog, this
    # hands back what the user typed (or `ok = 0` for Cancel). Same rule as the line above - a
    # missing `/EXPORT:` builds cleanly and then fails every DllImport at runtime.
    "wp81_input_complete",
    "wp81_parse_pec",
    "wp81_chart_info",
    "wp81_port_version",
    # the renderer drain - see `wp81_draw_*` in prpr-port/src/lib.rs
    "wp81_draw_encode",
    "wp81_draw_commands",
    "wp81_draw_vertices",
    "wp81_draw_indices",
    "wp81_draw_reset",
    "wp81_draw_counts",
    "wp81_draw_selftest",
    # the texture registry - see `wp81_texture_*`, which lets the shell rebuild an id as a bitmap
    "wp81_texture_count",
    "wp81_texture_id_at",
    "wp81_texture_data",
    "wp81_texture_overflow",
    # Why the last texture was refused - see the export. A dropped texture is invisible rather than
    # an error, so the count alone has twice been read as "the chart is fine".
    "wp81_texture_drop_info",
    # Particle quads in the last frame - the only evidence the hit effect reached the renderer at all.
    "wp81_sprite_quads",
    "wp81_particle_diag",
    # Shell-to-engine upload backlog push, read back by the loading gate - see the export.
    "wp81_upload_pending_set",
    "wp81_texture_dirty",
    "wp81_texture_rect_data",
    "wp81_texture_write_misses",
    "wp81_texture_write_count",
    # the game host - see `wp81_game_*`, which runs the real prpr scene
    "wp81_game_create",
    # Boot phira itself (`MainScene` + its library UI) instead of a chart - ARM-only, see the export.
    # Present on the phone build, absent from the desktop harness, which is why this list is not
    # shared with `build-m1.ps1`.
    "wp81_phira_boot",
    "wp81_game_frame",
    "wp81_game_destroy",
    "wp81_game_frames",
    "wp81_game_result",
    "wp81_game_running",
    "wp81_game_set_screen",
    # Sampled from a timer while `wp81_game_create` runs - see the note on the export. Without it a
    # create that takes the process down leaves nothing to read afterwards.
    "wp81_game_progress",
    "wp81_game_set_time",
    "wp81_game_time",
    "wp81_game_touch",
    # Read in the turn after a failed create. `tracing` has no subscriber on the phone, so this is
    # the only place the reason for `-1`/`-2`/`-100` survives at all.
    "wp81_game_last_error",
    # The XAudio2 mixer's C#-facing half, implemented in rust-wp81/wp81mix.cpp rather than in Rust,
    # so `#[no_mangle]` in lib.rs says nothing about them. The same rule as every line above: a
    # missing `/EXPORT:` builds a clean DLL whose DllImports fail only at run time.
    "wp81_mix_ready",
    # One-line state for the log: engine, error, clip/voice counts and resident PCM. Without it a
    # healthy mixer and a mixer that never came up are indistinguishable from outside.
    "wp81_mix_diag",
    # Returns a status rather than being `void` - 0 means the hit sound played, non-zero means the
    # shell must fall back to a MediaElement. See the export in wp81mix.cpp.
    "wp81_mix_sfx_play"
)

# /FORCE:MULTIPLE is load-bearing, and the reason is not obvious from the log.
#
# Several third-party crates bind Win32 entry points with `#[link(kind = "raw-dylib")]` in their own
# source - `which` (GetLastError, GetBinaryTypeW), `winapi-util` (GetConsoleMode),
# `walkdir`/`same-file` (GetFileInformationByHandle) and `getrandom` via `rand`/`twox-hash`
# (ProcessPrng from bcryptprimitives). raw-dylib makes rustc synthesise an ARM *import-descriptor
# member* into the object, so the archive holds hundreds of members literally named `kernel32.dll`,
# `ntdll.dll`, `bcryptprimitives.dll`, ... `wp81-compat` defines the same names as `#[no_mangle]`,
# so the linker sees two definitions and stops with LNK2005.
#
# /FORCE:MULTIPLE keeps the FIRST definition - compat's - and reports the other as LNK4006 "second
# definition ignored".
#
# CRITICAL, and the trap this file used to fall into: LNK4006 dropping the *duplicate symbol* does
# NOT drop the *import descriptor*. The descriptor is a separate contribution to `.idata`, and the
# linker emits it regardless of which function definition it kept. The build therefore linked
# cleanly, printed only LNK4006, and still produced a DLL whose import table named kernel32.dll and
# bcryptprimitives.dll - a load-time failure on the device, with no build error to warn anyone.
#
# The fix is NOT /FORCE:MULTIPLE alone. It is step 0 (wp81_registry_patch.py) plus the std patch
# (wp81_std_patch.py), which together delete every `raw-dylib` attribute for a symbol compat
# provides, so no descriptor is generated in the first place. /FORCE:MULTIPLE then only has to
# absorb the benign duplicates that remain.
#
# Do NOT treat a passing link as proof. `check_imports.py` in step 3 re-reads the import table and
# is the only thing that says whether the descriptors are gone. Without it, /FORCE:MULTIPLE would
# also happily link a symbol NOTHING implements, through a null IAT slot that only faults on the
# device. Do NOT remove ws2_32.lib either - std's net module imports the whole winsock set through
# it, and dropping it brings back ~35 LNK2019s.
$linkArgs = @(
    "/DLL", "/NODEFAULTLIB", "/MACHINE:ARM", "/SUBSYSTEM:WINDOWS",
    "/ENTRY:_DllMainCRTStartup",
    "/FORCE:MULTIPLE",
    # Symbol map, kept as a build artifact.
    #
    # The device reports `GameCreate -> AlreadyRunning` on a brand-new process and a frame count of
    # 325577230000753816, which is thread-local storage reading someone else's block: the ARM DLL has
    # a `.tls` section but NO TLS data directory (index 9 is zero), so the loader never assigns it an
    # index and `__tls_index` stays 0. The desktop build has the directory because its CRT supplies
    # `_tls_used`, and there is no CRT here.
    #
    # Fixing that means defining `_tls_used` ourselves, which means knowing exactly which symbols the
    # linker already resolved and from which object - not guessing from memory. `/MAP` is how we find
    # out, and it costs nothing but a file.
    "/MAP:$here\port-out\wp81prpr.map",
    "/OUT:$dll",
    "/LIBPATH:$vcLibArm",
    "/LIBPATH:$wpLibArm",
    # Input order below is load-bearing for TLS - see step 1b and the header of tlsdir_start.rs.
    # A direct object is included rather than resolved, so it contributes to `.tls$` ahead of the
    # archive members named after it, which is what puts `_tls_start` at offset 0 of `.tls`.
    $tlsStartObj,
    $staticlib,
    $chkstk,
    # The XAudio2 mixer. A direct object is always included rather than resolved, which is exactly
    # what lets the Rust half call wp81_mix_pcm with no /EXPORT: at all - and it is named after
    # $tlsStartObj so the ".tls begins at offset 0" ordering that step 1b depends on is untouched
    # (the mixer has no thread-local state, so it contributes nothing to the template).
    $mixObj,
    "WindowsPhoneCore.lib",
    "ws2_32.lib",
    # LAST, and as an archive rather than an object - see step 1b. link.exe takes all direct objects
    # first, so naming the object here would still put `_tls_end` at offset 1 of `.tls`. Pulling it
    # from a library named after `prpr_port.lib` is what places it at the far end of the template.
    # `check_tls.py` fails the build if link.exe orders it anywhere else.
    $tlsEndLib
) + ($exports | ForEach-Object { "/EXPORT:$_" })

$linkLog = Join-Path $here "port-link.log"
& $armLink @linkArgs *>&1 | Out-File -FilePath $linkLog -Encoding utf8
$linkCode = $LASTEXITCODE
Write-Host "link exit code: $linkCode"
Get-Content $linkLog -ErrorAction SilentlyContinue | Select-Object -Last 30
if ($linkCode -ne 0) { throw "link failed ($linkCode) - see port-link.log" }

# The gate that makes /FORCE:MULTIPLE safe. LNK4006 warnings are expected and printed above; an
# unexpected import is not, and must stop the build rather than crash on the device.
Write-Host "`n=== 3. verify every import is a WP8.1 API set ==="
& $py (Join-Path $here "tools\check_imports.py") $dll
if ($LASTEXITCODE -ne 0) {
    throw "import check failed ($LASTEXITCODE) - the DLL imports a library WP8.1 does not have. " +
          "compat must define the missing symbol, or the crate needs a shim."
}

# A passing link is not evidence about TLS either. The `.tls` section was present and correct in
# every image that still failed on the phone, because what the loader needs is data directory 9 and
# that comes from `_tls_used`, which /NODEFAULTLIB leaves out. Only reading the finished PE can tell
# the two apart, and getting it wrong does not fault - `#[thread_local]` just reads another module's
# block, which surfaced as `GameCreate -> AlreadyRunning` on a process that had created nothing.
Write-Host "`n=== 4. verify the TLS directory describes .tls ==="
& $py (Join-Path $here "tools\check_tls.py") $dll
if ($LASTEXITCODE -ne 0) {
    throw "TLS check failed ($LASTEXITCODE) - the image has no usable thread-local storage, so " +
          "`thread_local!` reads a foreign block. See tools/check_tls.py."
}

Write-Host "`nwp81prpr.dll = $((Get-Item $dll).Length) bytes"

if (-not $NoDeploy) {
    Copy-Item -Path $dll -Destination $deploy -Force
    Write-Host "copied -> $deploy"
}

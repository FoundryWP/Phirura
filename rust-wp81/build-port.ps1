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
$deploy  = "E:/Users/admin/Documents/visual studio 2013/Phirura/Phirura/Phirura/Phirura.WindowsPhone/wp81prpr.dll"

foreach ($p in @($cargo, $armLink, $vcLibArm, $wpLibArm, $chkstk, $target, $ws)) {
    if (-not (Test-Path $p)) { throw "missing: $p" }
}

Set-Location $ws

Write-Host "=== 1. cargo build -p prpr-port (staticlib) ==="
# Every -Z argument containing a comma MUST be quoted: unquoted, the shell parses `std,panic_abort`
# as an array literal and fails to parse the whole script, so nothing runs at all.
$cargoArgs = @(
    "build", "-p", "prpr-port",
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
    "wp81_parse_pec",
    "wp81_chart_info",
    "wp81_port_version",
    # the renderer drain - see `wp81_draw_*` in prpr-port/src/lib.rs
    "wp81_draw_encode",
    "wp81_draw_commands",
    "wp81_draw_vertices",
    "wp81_draw_indices",
    "wp81_draw_reset",
    "wp81_draw_selftest"
)

# /FORCE:MULTIPLE is load-bearing, and the reason is not obvious from the log.
#
# Seven third-party crates bind Win32 entry points with `#[link(kind = "raw-dylib")]` in their own
# source - `which` (GetLastError), `winapi-util` (GetConsoleMode), `walkdir`/`same-file`
# (GetFileInformationByHandle) and `getrandom` via `rand`/`twox-hash` (ProcessPrng from
# bcryptprimitives). raw-dylib makes rustc synthesise an ARM *import-descriptor member* into the
# object, so the archive holds members literally named `kernel32.dll`, `ntdll.dll`,
# `bcryptprimitives.dll`, ... (551 of them). `wp81-compat` defines the same four names as
# `#[no_mangle]`, so the linker sees two definitions and stops with LNK2005.
#
# /FORCE:MULTIPLE keeps the FIRST definition - compat's - and drops the import descriptor. The
# image then imports no desktop DLL at all. That outcome is verified, not assumed: the check
# below re-reads the import table and fails the build if anything other than an `api-ms-win-*`
# API set got in. Without that check /FORCE:MULTIPLE would also happily link a symbol that
# NOTHING implements, through a null IAT slot that only faults on the device.
#
# Do NOT "clean this up" by removing /FORCE:MULTIPLE and patching the four crates: it is ~25,000
# lines across seven crates for four symbols. Do NOT remove ws2_32.lib either - std's net module
# imports the whole winsock set through it, and dropping it brings back ~35 LNK2019s.
$linkArgs = @(
    "/DLL", "/NODEFAULTLIB", "/MACHINE:ARM", "/SUBSYSTEM:WINDOWS",
    "/ENTRY:_DllMainCRTStartup",
    "/FORCE:MULTIPLE",
    "/OUT:$dll",
    "/LIBPATH:$vcLibArm",
    "/LIBPATH:$wpLibArm",
    $staticlib,
    $chkstk,
    "WindowsPhoneCore.lib",
    "ws2_32.lib"
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
$py = "C:\Users\admin\.workbuddy-ai\binaries\python\versions\3.13.12\python.exe"
& $py (Join-Path $here "tools\check_imports.py") $dll
if ($LASTEXITCODE -ne 0) {
    throw "import check failed ($LASTEXITCODE) - the DLL imports a library WP8.1 does not have. " +
          "compat must define the missing symbol, or the crate needs a shim."
}

Write-Host "`nwp81prpr.dll = $((Get-Item $dll).Length) bytes"

if (-not $NoDeploy) {
    Copy-Item -Path $dll -Destination $deploy -Force
    Write-Host "copied -> $deploy"
}

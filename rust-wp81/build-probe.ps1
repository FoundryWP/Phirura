# Build wp81probe.dll end to end, and drop it into the WP8.1 head.
#
# This script exists because the link step used to be done by hand, which made the pipeline
# unreproducible: the exact linker, its library paths and the /NODEFAULTLIB flag set were only
# ever in a shell history. Everything needed is captured here.
#
# Pipeline:
#   1. compat        -> rlib   (core only; no std, so no panic handler collision)
#   2. link-probe    -> staticlib (core+alloc+std, std compiled from the PATCHED rust-src)
#   3. x86_arm link  -> wp81probe.dll (ARMNT, CRT-free)
#   4. copy into Phirura.WindowsPhone so the appx picks it up
#
# Step 2 needs the patched std, so run `build-std.ps1 -KeepSwapped` first (or let this script
# do it with -BuildStd).
#
# Usage:
#   & build-probe.ps1                 # link only, assuming std is already swapped+built
#   & build-probe.ps1 -BuildStd       # also re-patch and rebuild std
#   & build-probe.ps1 -NoDeploy       # do not copy into the WP head

param(
    [switch]$BuildStd,
    [switch]$NoDeploy
)

$ErrorActionPreference = "Stop"

$here = Split-Path -Parent $MyInvocation.MyCommand.Path

# The PowerShell tool that invokes this script does not surface stdout at all, so everything is
# transcribed to a file. Read that file - not the tool result - to see what happened.
$transcript = Join-Path $here "build-probe.log"
Start-Transcript -Path $transcript -Force | Out-Null

# --- toolchain ---------------------------------------------------------------
# VS11 (Visual Studio 2012) owns the ARM cross tools that WP8.1 wants. VS12/VS2013 has no
# VC\bin\arm at all on this machine, so pointing at it fails with "path not found".
$vs11     = "C:\Program Files (x86)\Microsoft Visual Studio 11.0"
$link     = Join-Path $vs11 "VC\bin\x86_arm\link.exe"
$vcLibArm = Join-Path $vs11 "VC\lib\arm"
$wpLibArm = "C:\Program Files (x86)\Windows Phone Kits\8.1\lib\ARM"
$chkstk   = Join-Path $vcLibArm "chkstk.obj"

# link.exe is a native x86 binary that loads mspdb110.dll from Common7\IDE at startup. Without
# it on PATH the process dies with exit 127 and prints nothing at all.
$ideDir = Join-Path $vs11 "Common7\IDE"

$env:RUSTUP_HOME = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { "C:\Users\admin\.workbuddy-ai\binaries\rust\rustup" }
$env:CARGO_HOME  = if ($env:CARGO_HOME)  { $env:CARGO_HOME }  else { "C:\Users\admin\.workbuddy-ai\binaries\rust\cargo" }
$cargo = Join-Path $env:CARGO_HOME "bin\cargo.exe"

$target  = Join-Path $here "targets\thumbv7a-pc-windows-msvc.json"
$outDir  = Join-Path $here "link-probe\out"
$staticlib = Join-Path $here "link-probe\target\thumbv7a-pc-windows-msvc\release\wp81_link_probe.lib"
$dll     = Join-Path $outDir "wp81probe.dll"
$deploy  = "E:\Users\admin\Documents\visual studio 2013\Phirura\Phirura\Phirura\Phirura.WindowsPhone\wp81probe.dll"

foreach ($p in @($link, $vcLibArm, $wpLibArm, $chkstk)) {
    if (-not (Test-Path $p)) { throw "missing toolchain path: $p" }
}

if ($BuildStd) {
    Write-Host "=== 0. patch + rebuild std (leaving the junction in place) ==="
    & (Join-Path $here "build-std.ps1") -KeepSwapped
}

Write-Host "`n=== 1. compat (rlib) ==="
Set-Location (Join-Path $here "compat")
& $cargo +nightly build "-Zjson-target-spec" "-Zbuild-std=core" --target $target --release
Write-Host "compat cargo exit code: $LASTEXITCODE"
if ($LASTEXITCODE -ne 0) { throw "compat build failed ($LASTEXITCODE)" }

Write-Host "`n=== 2. link-probe (staticlib) ==="
Set-Location (Join-Path $here "link-probe")
# -Zbuild-std-features=compiler-builtins-mem is REQUIRED: without it compiler_builtins does not
# export memcpy/memmove/memset/memcmp, and a CRT-free link (/NODEFAULTLIB) has nothing else to
# supply them from.
#
# The commas in `-Zbuild-std=core,alloc,std` MUST be inside quotes. Unquoted, PowerShell reads
# `core,alloc,std` as an array literal and fails to parse the whole script ("missing argument in
# parameter list"), so nothing runs at all and the error never reaches stdout.
& $cargo +nightly build "-Zjson-target-spec" "-Zbuild-std=core,alloc,std" `
    "-Zbuild-std-features=compiler-builtins-mem" --target $target --release
Write-Host "link-probe cargo exit code: $LASTEXITCODE"
if ($LASTEXITCODE -ne 0) { throw "link-probe build failed ($LASTEXITCODE)" }
if (-not (Test-Path $staticlib)) { throw "no staticlib at $staticlib" }

Write-Host "`n=== 3. link ARMNT DLL (VC11 x86_arm, CRT-free) ==="
New-Item -ItemType Directory -Force -Path $outDir | Out-Null
$log = Join-Path $here "link.log"

# /NODEFAULTLIB is the whole point: the desktop static CRT's startup drags in ~40 KERNEL32
# imports, and WP8.1 has no KERNEL32.dll - only the api-ms-win-* API sets. Dropping the CRT and
# supplying _DllMainCRTStartup from the probe keeps the import table to API-set names only.
# NOT $args - that name is reserved in PowerShell, and assigning to it silently makes the whole
# command a no-op.
#
# The /EXPORT: flags are NOT optional. `#[no_mangle]` only stops rustc mangling the name; it does
# not put the symbol in the DLL's export table, and with /NODEFAULTLIB there is no .def file to do
# it either. Without these the linker sees no reason to pull the probe objects out of the
# staticlib at all and quietly emits a 17 KB DLL that has no exports - it loads, and then every
# DllImport fails at run time. (The exports pull in `wp81_anchor_compat`, which is what drags the
# whole compat crate in; that is why the real DLL is ~126 KB.)
$exports = @(
    "wp81_anchor_compat",
    "wp81_api_failures",
    "wp81_link_probe",
    "wp81_probe_api_sets",
    "wp81_raw_createthread",
    "wp81_raw_qpc",
    # Panic reporting. Without /EXPORT the linker drops them and the DllImport fails at run time,
    # which would take out the very mechanism meant to explain failures.
    "wp81_set_log_path",
    "wp81_install_panic_hook"
)

$linkArgs = @(
    "/DLL", "/NODEFAULTLIB", "/MACHINE:ARM", "/SUBSYSTEM:WINDOWS",
    "/ENTRY:_DllMainCRTStartup",
    "/OUT:$dll",
    "/LIBPATH:$vcLibArm",
    "/LIBPATH:$wpLibArm",
    $staticlib,
    $chkstk,
    "WindowsPhoneCore.lib",
    # std links the winsock layer unconditionally, so every socket entry point (WSAStartup,
    # connect, accept, select, GetHostNameW, ...) is an unresolved external without this - 25 of
    # them, all __imp_*.
    "ws2_32.lib"
) + ($exports | ForEach-Object { "/EXPORT:$_" })

$env:PATH = "$ideDir;$env:PATH"

# Called with splatting, NOT through `cmd /c "<joined string>"`.
#
# The paths contain spaces ("Microsoft Visual Studio 11.0", "Program Files (x86)"), and joining
# the arguments into one string hands them to cmd unquoted - cmd then splits them, and the linker
# reports the wreckage as `LNK1181: cannot open input file 'studio.obj'`. Splatting lets
# PowerShell quote each argument itself.
& $link @linkArgs *>&1 | Out-File -FilePath $log -Encoding utf8
$code = $LASTEXITCODE
Write-Host "link exit code: $code   (log: $log)"
Get-Content $log -ErrorAction SilentlyContinue | Select-Object -Last 30
if ($code -ne 0) { throw "link failed ($code) - see $log" }

$size = (Get-Item $dll).Length
Write-Host "`nwp81probe.dll = $size bytes"

if (-not $NoDeploy) {
    Write-Host "`n=== 4. deploy into the WP head ==="
    Copy-Item -Path $dll -Destination $deploy -Force
    Write-Host "copied -> $deploy"
}

Stop-Transcript | Out-Null

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
$exports = @(
    "wp81_install_callbacks",
    "wp81_parse_pec",
    "wp81_chart_info",
    "wp81_port_version"
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
    "ws2_32.lib"
) + ($exports | ForEach-Object { "/EXPORT:$_" })

$linkLog = Join-Path $here "port-link.log"
& $armLink @linkArgs *>&1 | Out-File -FilePath $linkLog -Encoding utf8
$linkCode = $LASTEXITCODE
Write-Host "link exit code: $linkCode"
Get-Content $linkLog -ErrorAction SilentlyContinue | Select-Object -Last 30
if ($linkCode -ne 0) { throw "link failed ($linkCode) - see port-link.log" }

Write-Host "`nwp81prpr.dll = $((Get-Item $dll).Length) bytes"

if (-not $NoDeploy) {
    Copy-Item -Path $dll -Destination $deploy -Force
    Write-Host "copied -> $deploy"
}

# Build the D3D11 Rust probe and report its import table.
#
# This is the feasibility gate for Path A (Rust renders itself). It does not touch the phira
# workspace or the shims: the probe is its own workspace on purpose, so it links against the real
# `windows` crate exactly as a real renderer would.
#
# The output that matters is the LAST section - the import table. Two acceptable answers:
#   * only d3d11.dll / dxgi.dll      -> the binding is directly usable
#   * api-ms-win-*                   -> also fine; map them onto the phone's API sets
#   * kernel32.dll / anything else   -> the binding cannot ship, and needs a compat layer

param(
    [switch]$NoDeps
)

$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path

$vs11   = "C:/Program Files (x86)/Microsoft Visual Studio 11.0"
$wpKit  = "C:/Program Files (x86)/Windows Phone Kits/8.1"
$sdkLib = "C:/Program Files (x86)/Windows Kits/8.1/Lib/winv6.3"

$env:RUSTUP_HOME = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { "C:/Users/admin/.workbuddy-ai/binaries/rust/rustup" }
$env:CARGO_HOME  = if ($env:CARGO_HOME)  { $env:CARGO_HOME }  else { "C:/Users/admin/.workbuddy-ai/binaries/rust/cargo" }
$env:RUSTUP_TOOLCHAIN = "nightly"

$env:PATH = "$vs11\VC\bin;$vs11\Common7\IDE;" + $env:PATH
$env:LIB  = "$vs11\VC\lib\amd64;$sdkLib\um\x64"

$cargo    = Join-Path $env:CARGO_HOME "bin\cargo.exe"
$armLink  = Join-Path $vs11 "VC\bin\x86_arm\link.exe"
$vcLibArm = Join-Path $vs11 "VC\lib\arm"
$wpLibArm = Join-Path $wpKit "lib\ARM"
$chkstk   = Join-Path $vcLibArm "chkstk.obj"
$target   = Join-Path $here "..\targets\thumbv7a-pc-windows-msvc.json"
$outDir   = Join-Path $here "out"
$dll      = Join-Path $outDir "wp81d3d11rs.dll"
$py       = "C:/Users/admin/.workbuddy-ai/binaries/python/versions/3.13.12/python.exe"

Write-Host "=== 1. cargo build (staticlib) ==="
Push-Location $here
$env:MSYS_NO_PATHCONV = "1"
$env:MSYS2_ARG_CONV_EXCL = "*"
# Every -Z argument containing a comma MUST be quoted. Unquoted, PowerShell parses `std,panic_abort`
# as an array literal and fails to parse the whole script, so nothing runs at all - the same trap
# build-port.ps1 documents.
$cargoArgs = @(
    "build",
    "--target", $target,
    "-Zjson-target-spec",
    "-Zbuild-std=std,panic_abort",
    "-Zbuild-std-features=compiler-builtins-mem",
    "--release"
)
& $cargo @cargoArgs 2>&1 | Tee-Object -FilePath (Join-Path $here "cargo.log")
$code = $LASTEXITCODE
Pop-Location
if ($code -ne 0) { throw "cargo build failed ($code)" }

$staticlib = Join-Path $here "target\thumbv7a-pc-windows-msvc\release\d3d11_rs_probe.lib"
if (-not (Test-Path $staticlib)) {
    $staticlib = Join-Path $here "target\thumbv7a-pc-windows-msvc\release\libd3d11_rs_probe.lib"
}
if (-not (Test-Path $staticlib)) { throw "no staticlib found in target dir" }
Write-Host "staticlib = $((Get-Item $staticlib).Length) bytes"

Write-Host "`n=== 2. link ARMNT DLL ==="
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

# No /NODEFAULTLIB and no compat here: this probe is allowed to import whatever the binding wants.
# The point is to *read* the import table, not to make it shippable. A real build would add the
# same /NODEFAULTLIB + compat treatment as build-port.ps1.
$linkArgs = @(
    "/DLL", "/MACHINE:ARM", "/SUBSYSTEM:WINDOWS",
    "/ENTRY:_DllMainCRTStartup",
    "/FORCE:MULTIPLE",
    "/OUT:$dll",
    "/LIBPATH:$vcLibArm",
    "/LIBPATH:$wpLibArm",
    $staticlib,
    $chkstk,
    "WindowsPhoneCore.lib",
    "ws2_32.lib",
    "/EXPORT:wp81_d3d11_rs_touch"
)
& $armLink @linkArgs *>&1 | Tee-Object -FilePath (Join-Path $here "link.log")
if ($LASTEXITCODE -ne 0) { throw "link failed ($LASTEXITCODE)" }

Write-Host "`n=== 3. import table (THE ANSWER) ==="
& $py (Join-Path $here "..\tools\check_imports.py") $dll
Write-Host "check_imports exit = $LASTEXITCODE  (1 = a non-API-set import; read the list above)"

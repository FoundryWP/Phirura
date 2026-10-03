# Build wp81d3d11.dll - the D3D11 feasibility probe - and drop it into the WP8.1 head.
#
# This is deliberately CRT-free, for the same reason the Rust probe is: the only ARM CRT on this
# machine is VS11's desktop one (`VC\lib\arm\libcmt.lib`), whose startup objects import
# KERNEL32.dll, and WP8.1 has no KERNEL32.dll. Linking it yields a DLL that fails to load with
# ERROR_MOD_NOT_FOUND. The WP8.1 kit ships no CRT import lib of its own, so `/NODEFAULTLIB` plus
# the two hand-written routines in d3d11probe.cpp is the whole story.
#
# Usage:
#   & build-d3d11.ps1
#   & build-d3d11.ps1 -NoDeploy

param(
    [switch]$NoDeploy
)

$ErrorActionPreference = "Stop"

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$transcript = Join-Path $here "build-d3d11.log"
Start-Transcript -Path $transcript -Force | Out-Null

$vs11     = "C:\Program Files (x86)\Microsoft Visual Studio 11.0"
$wpKit    = "C:\Program Files (x86)\Windows Phone Kits\8.1"

# The ARM cross compiler and linker. VS12/VS2013 has no VC\bin\x86_arm on this machine, so the
# ARM tools have to come from VS11.
$cl       = Join-Path $vs11 "VC\bin\x86_arm\cl.exe"
$link     = Join-Path $vs11 "VC\bin\x86_arm\link.exe"
$vcInc    = Join-Path $vs11 "VC\include"
$vcLibArm = Join-Path $vs11 "VC\lib\arm"
$wpInc    = Join-Path $wpKit "Include"
$wpLibArm = Join-Path $wpKit "lib\ARM"
$ideDir   = Join-Path $vs11 "Common7\IDE"

# The WP kit's headers are not self-contained: its windef.h includes minwindef.h, which lives in
# the Windows SDK. So the SDK's shared/um/winrt trees have to be on the include path too.
#
# Order matters - the WP kit's Include comes FIRST so D3D11.h and friends resolve to the Windows
# Phone versions rather than the desktop SDK's.
$sdkInc   = "C:\Program Files (x86)\Windows Kits\8.1\Include"

$src      = Join-Path $here "d3d11-probe\d3d11probe.cpp"
$objDir   = Join-Path $here "d3d11-probe\out"
$obj      = Join-Path $objDir "d3d11probe.obj"
$dll      = Join-Path $objDir "wp81d3d11.dll"
$deploy   = "E:\Users\admin\Documents\visual studio 2013\Phirura\Phirura\Phirura\Phirura.WindowsPhone\wp81d3d11.dll"

foreach ($p in @($cl, $link, $vcInc, $wpInc, $wpLibArm, $src)) {
    if (-not (Test-Path $p)) { throw "missing path: $p" }
}

New-Item -ItemType Directory -Force -Path $objDir | Out-Null
$env:PATH = "$ideDir;$env:PATH"

Write-Host "=== 1. compile (ARM) ==="
# WINAPI_FAMILY=WINAPI_FAMILY_PHONE_APP is not optional: without it windows.h hands out the
# desktop API surface, which is a superset that includes things WP8.1 does not have - so a missing
# API becomes a load-time failure instead of a compile error.
#
# /GS- suppresses the stack-cookie check, which would otherwise need __security_cookie from the CRT.
$clArgs = @(
    "/nologo", "/c", "/EHsc", "/GS-", "/O2", "/W3",
    "/DWINAPI_FAMILY=WINAPI_FAMILY_PHONE_APP",
    "/DUNICODE", "/D_UNICODE",
    "/I`"$wpInc`"",
    "/I`"$sdkInc\shared`"", "/I`"$sdkInc\um`"", "/I`"$sdkInc\winrt`"",
    "/I`"$vcInc`"",
    "/Fo`"$obj`"",
    "`"$src`""
)
& $cl @clArgs *>&1 | Out-File -FilePath (Join-Path $here "d3d11-cl.log") -Encoding utf8
Write-Host "cl exit code: $LASTEXITCODE"
Get-Content (Join-Path $here "d3d11-cl.log") | Select-Object -Last 25
if ($LASTEXITCODE -ne 0) { throw "compile failed ($LASTEXITCODE)" }

Write-Host "`n=== 2. link (ARMNT DLL, /NOENTRY, no CRT) ==="
$linkArgs = @(
    "/DLL", "/NOENTRY", "/NODEFAULTLIB", "/MACHINE:ARM", "/INCREMENTAL:NO",
    "/OUT:`"$dll`"",
    "/LIBPATH:`"$wpLibArm`"",
    "/LIBPATH:`"$vcLibArm`"",
    "`"$obj`"",
    "d3d11.lib",
    "dxgi.lib",
    # __uuidof(IDXGIDevice) needs the interface GUID, and those live in dxguid.lib.
    "dxguid.lib",
    "WindowsPhoneCore.lib"
)
& $link @linkArgs *>&1 | Out-File -FilePath (Join-Path $here "d3d11-link.log") -Encoding utf8
$code = $LASTEXITCODE
Write-Host "link exit code: $code"
Get-Content (Join-Path $here "d3d11-link.log") | Select-Object -Last 30
if ($code -ne 0) { throw "link failed ($code)" }

Write-Host "`nwp81d3d11.dll = $((Get-Item $dll).Length) bytes"

if (-not $NoDeploy) {
    Write-Host "`n=== 3. deploy into the WP head ==="
    Copy-Item -Path $dll -Destination $deploy -Force
    Write-Host "copied -> $deploy"
}

Stop-Transcript | Out-Null

<#
.SYNOPSIS
    Probe: can `phira` (the full Rust UI crate) compile for thumbv7a at all?

.DESCRIPTION
    `prpr-port` builds `prpr`, which is why the phone can render a chart but has no Rust menus,
    settings, login or pause UI - those all live in the `phira` crate, which has never been built
    for this target. This script exists to measure that gap instead of guessing at it: it reuses
    `build-port.ps1`'s environment verbatim (same toolchain, same target JSON, same registry patch)
    and runs `cargo check`, which stops at type resolution and so reports the *whole* dependency
    wall in one pass rather than one link error at a time.

    It deliberately does NOT touch build-port.ps1 or the shipping DLL: a probe that can break the
    release path is a probe nobody dares to run.

    Output goes to `phira-check.log`. Only errors are summarised here, because the useful number
    is "how many crates are blocked, and which ones" - not the individual messages.

.EXAMPLE
    & check-phira.ps1
#>

param(
    # Extra package to check alongside phira, e.g. `phira-mp-client`.
    [string]$Package = "phira"
)

$ErrorActionPreference = "Continue"

$here = Split-Path -Parent $MyInvocation.MyCommand.Path

$vs11    = "C:/Program Files (x86)/Microsoft Visual Studio 11.0"
$wpKit   = "C:/Program Files (x86)/Windows Kits/8.1"
$sdkLib  = "$wpKit/Lib/winv6.3"

# --- toolchain, copied from build-port.ps1 so the two agree -----------------
$env:RUSTUP_HOME      = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { "C:/Users/admin/.workbuddy-ai/binaries/rust/rustup" }
$env:CARGO_HOME       = if ($env:CARGO_HOME)  { $env:CARGO_HOME }  else { "C:/Users/admin/.workbuddy-ai/binaries/rust/cargo" }
$env:RUSTUP_TOOLCHAIN = "nightly"
$env:PATH = "$vs11\VC\bin;$vs11\Common7\IDE;" + $env:PATH
$env:LIB  = "$vs11\VC\lib\amd64;$sdkLib\um\x64"

$cargo   = Join-Path $env:CARGO_HOME "bin\cargo.exe"
$target  = Join-Path $here "targets\thumbv7a-pc-windows-msvc.json"
$ws      = "E:/Users/admin/Documents/visual studio 2013/Phirura/phira-main/phira-main"
$log     = Join-Path $here "phira-check.log"
$py      = "C:\Users\admin\.workbuddy-ai\binaries\python\versions\3.13.12\python.exe"

foreach ($p in @($cargo, $target, $ws, $py)) {
    if (-not (Test-Path $p)) { throw "missing: $p" }
}

Set-Location $ws

# Step 0 from build-port.ps1. Needed even for `check`: the registry crates carry a `raw-dylib`
# attribute that does not apply to this target, and a build script that cannot compile stops the
# check before any of `phira`'s own code is reached - which would make this probe measure the
# patch instead of the port.
$patch = Join-Path $here "wp81_registry_patch.py"
if (Test-Path $patch) {
    Write-Host "=== 0. registry patch ==="
    & $py $patch --apply
    if ($LASTEXITCODE -ne 0) { Write-Host "registry patch failed ($LASTEXITCODE)" -ForegroundColor Red; exit 1 }
}

# `check`, not `build`: no link step, so the missing CRT and the DLL import budget - which are
# separate gates with their own scripts - stay out of this measurement entirely.
# Commas in -Z values are safe here because each is its own array element.
$cargoArgs = @(
    "check", "-p", $Package, "--lib",
    "--target", $target,
    "-Zjson-target-spec",
    "-Zbuild-std=std,panic_abort",
    "-Zbuild-std-features=compiler-builtins-mem"
)

Write-Host "`n=== cargo check -p $Package  (thumbv7a) ==="
$saved = $ErrorActionPreference
$ErrorActionPreference = "Continue"
& $cargo @cargoArgs 2>&1 | Out-File -FilePath $log -Encoding utf8
$code = $LASTEXITCODE
$ErrorActionPreference = $saved

Write-Host "cargo exit code: $code"

$errs = @(Select-String -Path $log -Pattern "^error(\[E\d+\])?:" -ErrorAction SilentlyContinue)
Write-Host "error count: $($errs.Count)"

if ($errs.Count -eq 0) {
    Write-Host "OK: $Package type-checks for thumbv7a." -ForegroundColor Green
    exit 0
}

# Group by the crate the error came from - `--> path/to/file.rs` tells us which dependency is
# blocked, and that is the unit of work, not the individual diagnostic.
$byFile = @{}
foreach ($e in $errs) {
    $loc = $e.Line
    if ($loc -match '-->\s+(.+)$') {
        $p = $Matches[1]
        # Reduce a full path to the crate it belongs to: the 4th-from-last segment for a normal
        # registry/path crate (`.../reqwest-0.13.2/src/x.rs` -> `reqwest`).
        $parts = $p -split '[\\/]'
        $key = if ($parts.Count -ge 4) { $parts[$parts.Count - 4] } else { $p }
        if (-not $byFile.ContainsKey($key)) { $byFile[$key] = 0 }
        $byFile[$key] += 1
    }
}

Write-Host "`n=== blocked crates (unit of work) ===" -ForegroundColor Cyan
$byFile.GetEnumerator() | Sort-Object Value -Descending | ForEach-Object {
    Write-Host ("  {0,4}  {1}" -f $_.Value, $_.Key)
}

Write-Host "`n=== first 30 errors ===" -ForegroundColor Cyan
$errs | Select-Object -First 30 | ForEach-Object { Write-Host "  $($_.Line)" }
$more = $errs.Count - 30
if ($more -gt 0) { Write-Host "  ... and $more more - see phira-check.log" }

exit $code

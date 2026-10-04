# Run the M1 host smoke test: the real `prpr` game, headless, on the PC.
#
# Exists for the same reason every other script in this directory does - the environment, not the
# command, is the hard part. `prpr-port` normally only ever cross-compiles for the phone, so its
# host build needs the three things build-prpr.ps1 already documented:
#
#   1. MSVC's `link.exe` first on PATH (Git Bash's coreutils `link` shadows it otherwise).
#   2. `Common7\IDE` on PATH, or link.exe dies with 0xc0000135 - `mspdb110.dll` lives there.
#   3. `LIB` pointing at the x64 import libraries, or `LNK1181: cannot open kernel32.lib`.
#
# Plus `RUSTUP_TOOLCHAIN=nightly`, which is what the rest of this tree is built with.
#
# Usage:
#   & build-m1.ps1 "<charts-dir>"                 # demo chart, 120 frames
#   & build-m1.ps1 "<charts-dir>" demo chart.json 300
#
# <charts-dir> is the directory *containing* the chart folder, playing the part of the app's
# `ApplicationData\LocalFolder\charts`.

param(
    [Parameter(Mandatory = $true)][string]$ChartsDir,
    [string]$Folder = "demo",
    [string]$Chart  = "chart.json",
    [int]$Frames    = 120,

    # Where to write one drained frame as a fixture for the Stage 3 replay (`scene.rs`) to read
    # back. Empty writes nothing. The bytes and why they are the right ones are in `dump_fixture`.
    [string]$Fixture = "",

    [switch]$BuildOnly
)

$ErrorActionPreference = "Continue"

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$log  = Join-Path $here "host-m1.log"

$vs11   = "C:\Program Files (x86)\Microsoft Visual Studio 11.0"
$sdkLib = "C:\Program Files (x86)\Windows Kits\8.1\Lib\winv6.3"

$env:RUSTUP_HOME      = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { "C:\Users\admin\.workbuddy-ai\binaries\rust\rustup" }
$env:CARGO_HOME       = if ($env:CARGO_HOME)  { $env:CARGO_HOME }  else { "C:\Users\admin\.workbuddy-ai\binaries\rust\cargo" }
$env:RUSTUP_TOOLCHAIN = "nightly"

$env:PATH = "$vs11\VC\bin;$vs11\Common7\IDE;" + $env:PATH
$env:LIB  = "$vs11\VC\lib\amd64;$sdkLib\um\x64"

$cargo = Join-Path $env:CARGO_HOME "bin\cargo.exe"
$ws    = "E:\Users\admin\Documents\visual studio 2013\Phirura\phira-main\phira-main"

foreach ($p in @($cargo, $ws)) {
    if (-not (Test-Path $p)) { throw "missing: $p" }
}

Set-Location $ws

if ($BuildOnly) {
    & $cargo build -p prpr-port --bin host_m1 *> $log
    "EXIT=$LASTEXITCODE"
    Get-Content $log | Select-Object -Last 40
    return
}

$runArgs = @($ChartsDir, $Folder, $Chart, "$Frames")
if ($Fixture) { $runArgs += $Fixture }

& $cargo run -q -p prpr-port --bin host_m1 -- @runArgs *> $log
"EXIT=$LASTEXITCODE"

# Print everything except tracing's `DEBUG` spam.
#
# Not `Select-Object -Last N` instead: a failed build writes its `error[...]` blocks at the *end*
# so those survive either way, but the `[m1]` asset-request log runs the whole length of a
# successful one and truncating would hide exactly the early misses that matter. Filtering on the
# level keeps both and drops only detail lines - the first run produced 34334 of them from a single
# loop (`creating cache texture: 0x0`), 5.4 MB of log to find two lines in.
$lines  = Get-Content $log
$hidden = 0
$shown  = foreach ($l in $lines) {
    if ($l -match ' DEBUG ') { $hidden++ } else { $l }
}
if ($hidden -gt 0) { "... ($hidden DEBUG lines hidden)" }
$shown

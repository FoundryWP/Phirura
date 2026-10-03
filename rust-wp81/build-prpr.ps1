# Check (and eventually build) prpr for Windows Phone 8.1.
#
# Exists because the environment is the hard part, not the command. Three things have to be right
# before rustc can link even its own host build scripts:
#
#   1. MSVC's `link.exe` must come first on PATH. Git Bash ships `/usr/bin/link.exe`, which is
#      coreutils' `link` - a completely different program. When it wins, rustc's link step fails
#      with `link: extra operand '....rcgu.o'`, which reads like a rustc bug and is not one. This
#      is why every bash-invoked build failed while the same command from PowerShell worked.
#
#   2. `Common7\IDE` must be on PATH or link.exe exits 0xc0000135 (STATUS_DLL_NOT_FOUND) because
#      it cannot load mspdb110.dll, which lives there rather than in VC\bin.
#
#   3. LIB must point at the x64 import libraries, or the host link fails with
#      `LNK1181: cannot open input file 'kernel32.lib'`.
#
# Usage:
#   & build-prpr.ps1                 # cargo check -p prpr for the target
#   & build-prpr.ps1 -Release        # cargo build instead of check
#   & build-prpr.ps1 -Offline        # skip network (fails on git deps; see README)

param(
    [switch]$Release,
    [switch]$Offline
)

$ErrorActionPreference = "Stop"

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$log  = Join-Path $here "prpr-build.log"
Start-Transcript -Path $log -Force | Out-Null

$vs11   = "C:\Program Files (x86)\Microsoft Visual Studio 11.0"
$wpKit  = "C:\Program Files (x86)\Windows Phone Kits\8.1"
$sdkLib = "C:\Program Files (x86)\Windows Kits\8.1\Lib\winv6.3"

$env:RUSTUP_HOME = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { "C:\Users\admin\.workbuddy-ai\binaries\rust\rustup" }
$env:CARGO_HOME  = if ($env:CARGO_HOME)  { $env:CARGO_HOME }  else { "C:\Users\admin\.workbuddy-ai\binaries\rust\cargo" }
$env:RUSTUP_TOOLCHAIN = "nightly"

# (1) and (2): the host linker, ahead of anything Git Bash provides.
$env:PATH = "$vs11\VC\bin;$vs11\Common7\IDE;" + $env:PATH

# (3): x64 import libraries for the host link of build scripts.
$env:LIB = "$vs11\VC\lib\amd64;$sdkLib\um\x64"

$cargo  = Join-Path $env:CARGO_HOME "bin\cargo.exe"
$target = Join-Path $here "targets\thumbv7a-pc-windows-msvc.json"
$ws     = "E:\Users\admin\Documents\visual studio 2013\Phirura\phira-main\phira-main"

foreach ($p in @($cargo, $target, $ws)) {
    if (-not (Test-Path $p)) { throw "missing: $p" }
}

Set-Location $ws

$verb = if ($Release) { "build" } else { "check" }
$cargoArgs = @(
    $verb, "-p", "prpr",
    "--target", $target,
    "-Zjson-target-spec",
    "-Zbuild-std=std,panic_abort",
    "-Zbuild-std-features=compiler-builtins-mem"
)
if ($Offline) { $cargoArgs += "--offline" }

# Cargo output is redirected to its own file rather than left to the transcript.
#
# Start-Transcript does not capture native command output in the 5.1 shell - the transcript
# from the first run of this script recorded the exit code and nothing else, which is worse
# than useless when what you want is compiler diagnostics.
$cargoLog = Join-Path $here "prpr-cargo.log"
Write-Host "=== cargo $verb -p prpr -> $cargoLog ==="
# ErrorActionPreference must be relaxed for the duration of the call.
#
# With it set to 'Stop', a single *warning* written to stderr by cargo becomes a terminating
# NativeCommandError and the script aborts before cargo has done anything - the first run of
# this script died in three seconds on `warning: unused workspace dependency`. Cargo writes
# warnings to stderr as a matter of course, so this is not an edge case.
$savedEap = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
& $cargo @cargoArgs 2>&1 | Out-File -FilePath $cargoLog -Encoding utf8
$code = $LASTEXITCODE
$ErrorActionPreference = $savedEap
Write-Host "cargo exit code: $code"
Get-Content $cargoLog -ErrorAction SilentlyContinue | Select-Object -Last 40

Stop-Transcript | Out-Null
exit $code

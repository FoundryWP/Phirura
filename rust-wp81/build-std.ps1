# Build Rust's std for Windows Phone 8.1.
#
# `-Z build-std` reads the library sources from the active toolchain's rust-src component, so
# the patched copy has to be swapped in at that path. This script does that with a directory
# junction, builds, and then puts the original back.
#
# Usage:
#   & build-std.ps1                # patch + build std + report
#   & build-std.ps1 -KeepSwapped   # leave the junction in place (faster rebuilds)

param(
    [switch]$KeepSwapped,
    [string]$Crate = "E:\_phirura_rust_probe\nostd"
)

$ErrorActionPreference = "Stop"

$env:RUSTUP_HOME = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { "C:\Users\admin\.workbuddy-ai\binaries\rust\rustup" }
$env:CARGO_HOME  = if ($env:CARGO_HOME)  { $env:CARGO_HOME }  else { "C:\Users\admin\.workbuddy-ai\binaries\rust\cargo" }
$cargo = Join-Path $env:CARGO_HOME "bin\cargo.exe"

$here    = Split-Path -Parent $MyInvocation.MyCommand.Path
$patched = Join-Path $here "work\library"
$target  = Join-Path $here "targets\thumbv7a-pc-windows-msvc.json"

$srcRoot  = Join-Path $env:RUSTUP_HOME "toolchains\nightly-x86_64-pc-windows-msvc\lib\rustlib\src\rust"
$live     = Join-Path $srcRoot "library"
$original = Join-Path $srcRoot "library.orig"

Write-Host "=== 1. patch std ==="
$python = "C:\Users\admin\.workbuddy-ai\binaries\python\versions\3.13.12\python.exe"
& $python (Join-Path (Split-Path -Parent $here) "tools\wp81_std_patch.py") --apply
if (-not (Test-Path $patched)) { throw "patch produced no output at $patched" }

Write-Host "`n=== 2. swap the rust-src library for the patched copy ==="
if (Test-Path $original) {
    Write-Host "junction already in place (library.orig exists); reusing it"
} else {
    Rename-Item -Path $live -NewName "library.orig"
    New-Item -ItemType Junction -Path $live -Target $patched | Out-Null
    Write-Host "swapped: $live -> $patched"
}

try {
    Write-Host "`n=== 3. build std for thumbv7a-pc-windows-msvc ==="
    Set-Location $Crate
    $log = Join-Path $here "build-std.log"
    # Redirect rather than pipe: PowerShell turns any stderr line into a NativeCommandError
    # and truncates the real rustc diagnostics.
    #
    # -Zbuild-std-features=compiler-builtins-mem is REQUIRED, not optional: without it
    # compiler_builtins does not export memcpy/memmove/memset/memcmp, and on a CRT-free link
    # (/NODEFAULTLIB, which WP8.1 forces) those show up as unresolved externals.
    & cmd /c "`"$cargo`" +nightly build -Zjson-target-spec -Z build-std=std,panic_abort -Z build-std-features=compiler-builtins-mem --target `"$target`" --release > `"$log`" 2>&1"
    $code = $LASTEXITCODE
    Write-Host "cargo exit code: $code   (log: $log)"
    Get-Content $log | Select-Object -Last 45
} finally {
    if (-not $KeepSwapped) {
        Write-Host "`n=== 4. restore the original rust-src library ==="
        if (Test-Path $original) {
            cmd /c rmdir "$live" 2>$null | Out-Null
            Rename-Item -Path $original -NewName "library"
            Write-Host "restored"
        }
    } else {
        Write-Host "`n(leaving the junction in place)"
    }
}

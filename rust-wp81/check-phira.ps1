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
# `--no-default-features` is not an optimisation, it is the shape of the real build: `prpr-port`
# declares `prpr = { default-features = false }`, and `phira`'s only default feature is `video`,
# which drags in `prpr-avc -> ureq -> rustls -> ring`. `ring` is a C library whose build script
# fails here for the same reason `aws-lc-sys` did. Checking with default features on would be
# measuring a dependency combination the shipping DLL never uses.
$cargoArgs = @(
    "check", "-p", $Package, "--lib", "--no-default-features",
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

# `-Context 0,6`: rustc prints the location on the line *after* the message, and both the message
# and the path are wrapped by the console before they reach this log - so the assembler below needs
# several lines to reconstruct one `--> path:line:col`. One context line was not enough, which is
# why the "blocked crates" list was still empty while the error was in a crate named two lines down.
$errs = @(Select-String -Path $log -Pattern "^error(\[E\d+\])?:" -Context 0, 6 -ErrorAction SilentlyContinue)
Write-Host "error count: $($errs.Count)"

# A `[patch]` Cargo declined to apply is worse than an error: it is a *warning*, so it never
# reaches the count above, and the real crate keeps building - which means this probe can report
# "OK" about a dependency graph the shipping DLL will never have. Cargo only says so once, mid-log,
# with a suggestion to run `cargo update`. Checked before the success exit for that reason.
# (Matched per line: cargo wraps this message, so only its first half survives as a line.)
$unused = @(Select-String -Path $log -Pattern "was not used" -ErrorAction SilentlyContinue)
if ($unused.Count -gt 0) {
    Write-Host "`n=== [patch] entries cargo did NOT apply ===" -ForegroundColor Yellow
    foreach ($u in $unused) { Write-Host "  $($u.Line)" }
    Write-Host "  -> the REAL crate is still building. Pin the shim's version to the one in Cargo.lock." -ForegroundColor Yellow
}

# Build-script failures (`error: failed to run custom build command for ...`) carry no `--> file.rs`,
# so they are invisible to the grouping below - which is why every round so far printed an empty
# "blocked crates" list while the whole measurement was one of them.
$buildFail = @(Select-String -Path $log -Pattern "failed to run custom build command for" -ErrorAction SilentlyContinue)

if ($errs.Count -eq 0) {
    if ($unused.Count -gt 0 -or $buildFail.Count -gt 0) {
        Write-Host "`nNOT OK: no type errors, but the graph is still wrong (see above)." -ForegroundColor Red
        exit 1
    }
    Write-Host "OK: $Package type-checks for thumbv7a." -ForegroundColor Green
    exit 0
}

# Group by the crate the error came from - `--> path/to/file.rs` tells us which dependency is
# blocked, and that is the unit of work, not the individual diagnostic.
#
# The location has to be reassembled: it arrives on the following line, and both the message and the
# path are wrapped by the console before they reach this log, so keep appending until the `:line:col`
# coordinates appear - then cut back to whatever follows the last `-->`, because the (wrapped)
# message text may have been appended along with it.
$byFile = @{}
foreach ($e in $errs) {
    $loc = ""
    foreach ($line in $e.Context.PostContext) {
        if ($line -match '^\s*[|=]') { break }  # source excerpt or a note/help block
        # Any following diagnostic ends this one. Without this, `error: could not compile X` - which
        # has no location of its own - kept walking into whatever was printed next and attributed
        # the *following* crate's location to it (macroquad appeared in "blocked crates" while the
        # only thing it produced was a warning).
        if ($line -match '^(error|warning|note|help)(\[E\d+\])?:') { break }
        $loc += $line.Trim()
        if ($loc -match ':\d+:\d+\s*$') { break }
    }
    if ($loc -match '.*-->\s*(.+)$') { $loc = $Matches[1] }
    # Two path shapes must both be read, and they differ by exactly one thing. A workspace-relative
    # path (`prpr\src\log.rs`) has no separator before the crate directory, so a pattern demanding
    # one never matches - which is why this list stayed empty even when every error came from one
    # crate. A registry path (`...\registry\src\index.crates.io-...\xxhash-rust-0.8.15\src\xh3.rs`)
    # has *two* `src` segments, so a leftmost match would report `registry`. Prepending a separator
    # and matching greedily resolves both: the expression below always stops at the last `X\src\`.
    if ($loc -notmatch '^[\\/]') { $loc = '\' + $loc }
    # The crate directory also reads correctly for git checkouts (`.../phira-mp-client/src/x.rs`)
    # and workspace members (`phira/src/x.rs`). Trailing `-1.2.3` is dropped so a patch bump cannot
    # split one crate across two buckets.
    if ($loc -match '.*[\\/]([^\\/]+)[\\/]src[\\/]') {
        $key = $Matches[1] -replace '-\d+\.\d+\.\d+.*$', ''
        if (-not $byFile.ContainsKey($key)) { $byFile[$key] = 0 }
        $byFile[$key] += 1
    }
}

# Add the build-script failures, which are the same unit of work by a different route.
foreach ($b in $buildFail) {
    if ($b.Line -match 'for `(.+?) v') {
        $key = $Matches[1]
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

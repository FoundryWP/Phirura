<#
.SYNOPSIS
    Mirror a chart + the engine's shared assets onto the phone's LocalFolder.

.DESCRIPTION
    `PrprPort.ReadAsset` resolves EVERYTHING under one `AssetRoot`, and the engine asks for two
    different shapes of path through it: `190/57649733.json` (a chart folder, prefixed) and
    `respack/info.yml`, `back.png`, `rank/A.png` (shared assets, relative to the root). So the
    phone's root has to contain both, exactly like the desktop layout this script mirrors -
    `E:\_phirura_m1\charts`, which is the layout `build-m1.ps1` has already been run against.

    The push is `ISETool rs`. Its semantics differ from `ts` in two ways that were measured on the
    real device rather than read off the help text:

      * `ts` WRAPS what it pulls in `<dir>\IsolatedStore\` (you get `<dir>\IsolatedStore\phirura.log`).
        `rs` does NOT unwrap that on the way back - it pushes the desktop directory's *contents*
        straight into `LocalFolder`. So you stage `<out>\charts\...`, NOT `<out>\IsolatedStore\charts\...`.
        A probe written the `ts` way landed at `LocalFolder\IsolatedStore\charts` and `dir:charts`
        answered "path not found".
      * `rs` REPLACES LocalFolder rather than merging into it. Root went from holding `phirura.log`
        to holding only what was pushed. That is why `-Deploy` (uninstall + install) runs before the
        push: otherwise the install wipes the charts that were just staged.

    Therefore the only correct order is:  -Deploy  ->  -Push  ->  -Launch  ->  -PullLog.
    Running them in another order produces a failure that looks like a Rust bug.

.EXAMPLE
    # appx unchanged: just refresh the chart and restart the app
    .\stage-chart.ps1 -Push -Launch

.EXAMPLE
    # rebuilt wp81d3d11rs.dll / PrprPort.dll: reinstall first, then stage, then run, then read the log
    .\stage-chart.ps1 -Deploy -Push -Launch -PullLog
#>
[CmdletBinding()]
param(
    # What to mirror. Must already contain the chart folder and the shared assets - the desktop
    # layout that `build-m1.ps1 -ChartsDir` was run against is the known-good source.
    [string]$Source = "E:\_phirura_m1\charts",

    # Staging area on the PC. Its CONTENTS become the contents of LocalFolder, so the chart lives
    # at $Out\charts\<folder> and not at $Out\<folder>.
    [string]$Out = "E:\_phirura_stage",

    # Chart folder inside $Source. Kept as a parameter so a wrong source fails here rather than as
    # a `-2` from `wp81_game_create` on the phone.
    [string]$Folder = "190",

    [string]$Appx = "E:\Users\admin\Documents\visual studio 2013\Phirura\Phirura\Phirura" +
        "\Phirura.WindowsPhone\AppPackages\Phirura.WindowsPhone_1.0.0.0_ARM_Debug_Test" +
        "\Phirura.WindowsPhone_1.0.0.0_ARM_Debug.appx",

    # From AppxManifest.xml; also the id `ISETool` takes.
    [string]$ProductId = "fa2c9634-5ad8-487e-be3c-9893871fda9f",

    [string]$Device = "de",

    [switch]$Deploy,   # uninstall + install  (no launch - see the note at the switch itself)
    [switch]$Push,     # ISETool rs  (also wipes LocalFolder - must come after -Deploy)
    [switch]$Launch,   # AppDeployCmd /launch  (must come after -Push)
    [switch]$PullLog,  # ISETool ts into a fresh dir, then print the tail of phirura.log

    # Seconds to let the app run before pulling the log.
    #
    # Without this the pull lands a moment after `launch` and reads back a log written by a
    # process that has not done anything yet - which reads exactly like "the engine never ran",
    # the one conclusion the whole round exists to avoid. GameCreate alone parses an 8.8 MB chart
    # and decodes 75 textures before its first line, so the default is generous rather than tight.
    [int]$WaitSeconds = 15
)

$ErrorActionPreference = "Continue"   # native tools report through stderr; Stop would mask their exit codes

$sdk = "C:\Program Files (x86)\Microsoft SDKs\Windows Phone\v8.1\Tools"
$ise = Join-Path $sdk "IsolatedStorageExplorerTool\ISETool.exe"
$adp = Join-Path $sdk "AppDeploy\AppDeployCmd.exe"

foreach ($exe in @($ise, $adp)) {
    if (-not (Test-Path $exe)) { throw "missing tool: $exe" }
}

# ---------------------------------------------------------------------------
# 1. Build the mirror. LocalFolder is generated output, so it is rebuilt rather
#    than merged - a stale file from a previous chart would otherwise be served
#    to `exists()` probes and silently change what the engine loads.
# ---------------------------------------------------------------------------
if (-not (Test-Path $Source)) { throw "source not found: $Source" }
if (-not (Test-Path (Join-Path $Source $Folder))) { throw "chart folder not found: $Source\$Folder" }
if (-not (Test-Path (Join-Path (Join-Path $Source $Folder) "info.yml"))) {
    # Without one the engine keeps `ChartInfo::default()`'s placeholder music file name and fails
    # with "asset not found: <folder>/song.mp3" on a chart whose RPE META has no metadata at all.
    throw "$Source\$Folder has no info.yml - GameCreate will look for song.mp3"
}

if (Test-Path $Out) {
    $resolved = (Resolve-Path $Out).Path
    if ($resolved -ieq "E:\_phirura_stage") { Get-ChildItem $resolved -Force | Remove-Item -Recurse -Force }
    else { throw "refusing to clear an unexpected -Out: $resolved" }
}
New-Item -ItemType Directory -Path $Out -Force | Out-Null

# The source is copied AS `charts`, not unpacked into LocalFolder's root.
#
# `$Source` defaults to `E:\_phirura_m1\charts`, and `rs` pushes the CONTENTS of `$Out` to
# LocalFolder. The engine reads from `<LocalFolder>\charts` - `RustStage1Page` builds exactly
# `LocalFolder.Path + "\charts"`, which is also the desktop layout, since `E:\_phirura_m1` holds
# nothing but `charts`. So copying `$Source\*` straight into `$Out` put `190`, `demo`, `respack`
# and every shared png/ogg at LocalFolder's root instead, and `GetFolderFromPathAsync(<LocalState>\charts)`
# had nothing to open.
#
# That is what `GameCreate -> -2 (ParseFailed)` was on the phone: not a parser, not IO permissions,
# and not TLS - all ten asset requests failed in ~80 ms each with the charts folder absent, so the
# engine fell through every `info.yml`/`info.txt`/`info.csv` candidate and gave up. The comment on
# `$Out` above had it right; this line did not.
$dest = Join-Path $Out "charts"
New-Item -ItemType Directory -Path $dest -Force | Out-Null
Copy-Item "$Source\*" -Destination $dest -Recurse -Force

$staged = Get-ChildItem $Out -Recurse -File
"staged: {0} files, {1} MB -> {2}" -f $staged.Count,
    [math]::Round(($staged | Measure-Object Length -Sum).Sum / 1MB, 2), $Out

# ---------------------------------------------------------------------------
# 2. Deploy. `update` fails 0x81030137 when the version is unchanged, so this is
#    uninstall + install: a clean app AND a clean LocalFolder.
# ---------------------------------------------------------------------------
if ($Deploy) {
    & $adp /uninstall $ProductId /targetdevice:$Device 2>&1 | Out-Null
    # `/install`, NOT `/installlaunch`.
    #
    # `installlaunch` starts the app before the charts are pushed, and `-Push` below REPLACES
    # LocalFolder - including `phirura.log`. So the lines this whole script exists to collect (the
    # launch, `GameCreate`'s result, the last asset read) were written by that first process and
    # then deleted by the push, while the relaunch that follows merely *activates* the still-living
    # process and therefore writes no new launch line at all. The result reads as a log that starts
    # mid-run with no explanation, which is exactly what the first three device runs produced.
    #
    # Installing without launching means there is no process alive across the push, and `-Launch`
    # then starts the one process whose whole log is ours to keep.
    & $adp /install $Appx /targetdevice:$Device 2>&1 | Out-Null
    "installed (appx = $(Split-Path $Appx -Leaf))"
}

# ---------------------------------------------------------------------------
# 3. Push. Replaces LocalFolder, so anything staged before this point is lost.
# ---------------------------------------------------------------------------
if ($Push) {
    & $ise rs $Device $ProductId $Out 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) { Write-Warning "ISETool rs exit $LASTEXITCODE" }
    # Confirm with the path that would have failed had the `ts` wrapping been replicated here.
    & $ise "dir:charts" $Device $ProductId 2>&1 | Out-Null
    "pushed -> LocalFolder\charts"
}

if ($Launch) {
    & $adp /launch $ProductId /targetdevice:$Device 2>&1 | Out-Null
    # `/launch` returns 0x80131500 whenever the phone screen is locked, and the single line
    # that says so is inside the output being discarded. Printing "launched" unconditionally
    # makes a locked screen look identical to an engine that booted and produced no log -
    # which sends the next round off to bisect Rust that was never executed. Same treatment
    # as the `rs` exit code above; pull the log either way, the warning is the signal.
    if ($LASTEXITCODE -ne 0) {
        Write-Warning "AppDeployCmd /launch exit $LASTEXITCODE - the app did not start (screen locked?)"
    } else {
        "launched"
    }
}

# ---------------------------------------------------------------------------
# 4. Pull the log. The directory must NOT already exist - that is the whole
#    "works iff <dir> does not already exist" rule - and the payload lands one
#    level down under IsolatedStore\, which is the opposite of what `rs` expects.
# ---------------------------------------------------------------------------
if ($PullLog) {
    if ($Launch -and $WaitSeconds -gt 0) {
        "waiting $WaitSeconds s for the app to get past GameCreate"
        Start-Sleep -Seconds $WaitSeconds
    }
    $logdir = "E:\_phirura_log_pull_{0:HHmmss}" -f (Get-Date)

    // The output used to be discarded (`| Out-Null`), which turned every failure into the same
    // one line - "no log at ..." - regardless of cause. `ts` has at least three distinct
    // outcomes (store not readable, wrong device, target dir already existing) and they need
    // different fixes, so the message is kept and printed only when it produced nothing: the
    // success path would otherwise be flooded with ISETool's progress text.
    $tsout = (& $ise ts $Device $ProductId $logdir 2>&1 | Out-String)

    $logfile = Join-Path $logdir "IsolatedStore\phirura.log"
    if (-not (Test-Path $logfile)) { Write-Warning "no log at $logfile"; if ($tsout) { "ISETool ts said:"; $tsout }; return }
    "=== $logfile ==="
    Get-Content $logfile | Select-Object -Last 80
}

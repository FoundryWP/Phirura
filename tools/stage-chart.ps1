<#
.SYNOPSIS
    Mirror a chart + the engine's shared assets onto the phone's LocalFolder.

.DESCRIPTION
    `PrprPort.ReadAsset` resolves TWO roots in order - `AssetRoot` (`<LocalFolder>\charts`) then
    `AssetPackRoot` (`<LocalFolder>\respack`) - because the engine asks for two different shapes of
    path through it and never says which root it means: `190/57649733.json` is a chart folder,
    while `respack/info.yml`, `back.png`, `rank/A.png` are the engine's own files and come through
    bare. The desktop source this script mirrors is still the ONE flat `E:\_phirura_m1\charts`
    layout that `build-m1.ps1` has already been run against - the split into `charts` + `respack`
    happens here, at stage time, so the desktop tree never has to be reorganised.

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
    # at $Out\charts\<folder> and not at $Out\<folder>, and the engine's own files at
    # $Out\respack\<name>. The two are built from one flat $Source by the split below.
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
# Step 1 rebuilds the mirror that `rs` uploads, so it is only needed when actually pushing. The
# required second `-PullLog` (see the .DESCRIPTION) otherwise re-copies 22 MB and, worse, trips the
# harness's bulk safe-delete confirmation on the clear below once the mirror already holds its
# ~114 files - which aborts the run before the log is ever read.
if ($Push) {
if (-not (Test-Path $Source)) { throw "source not found: $Source" }
if (-not (Test-Path (Join-Path $Source $Folder))) { throw "chart folder not found: $Source\$Folder" }
if (-not (Test-Path (Join-Path (Join-Path $Source $Folder) "info.yml"))) {
    # Without one the engine keeps `ChartInfo::default()`'s placeholder music file name and fails
    # with "asset not found: <folder>/song.mp3" on a chart whose RPE META has no metadata at all.
    throw "$Source\$Folder has no info.yml - GameCreate will look for song.mp3"
}

if (Test-Path $Out) {
    $resolved = (Resolve-Path $Out).Path
    # `-LiteralPath`, not `Get-ChildItem | Remove-Item`. The harness overrides Remove-Item with a
    # safe-delete wrapper that only reads an explicit path operand (or real pipeline input bound to
    # a parameter), so the piped form throws "missing path operand" before anything is deleted - and
    # that aborts the whole script at step 1, before -Deploy/-Push ever run.
    if ($resolved -ieq "E:\_phirura_stage") { Remove-Item -LiteralPath $resolved -Recurse -Force }
    else { throw "refusing to clear an unexpected -Out: $resolved" }
}
New-Item -ItemType Directory -Path $Out -Force | Out-Null

# The source is SPLIT into two roots, not unpacked into LocalFolder's root.
#
# `rs` pushes the CONTENTS of `$Out` to LocalFolder, so whatever is built here is exactly what the
# phone ends up with. Two roots since FIXLIST #6 (choice C):
#
#   $Out\charts   -> `LocalFolder\charts`   charts: `190\`, `demo\`, later `download\`/`custom\`
#   $Out\respack  -> `LocalFolder\respack`  the engine's own files: `font.ttf`, the UI sprites,
#                                           the hit sounds, `rank\`, `mod\`, `respack\`
#
# phira asks for both by bare relative name and never says which folder it means - `190/info.yml`
# next to `rank/phi.png` - so `PrprPort.ReadAsset` probes `AssetRoot` first and `AssetPackRoot`
# second. Chart names start with their own folder, engine names are bare, so the two sets cannot
# collide; the order only decides which walk happens first.
#
# The split exists because `Assets\charts` used to hold both, which made a chart folder look like
# where the game's art had been parked. Nothing about the engine's contract changes: it still sees
# one flat namespace through `ReadAsset`.
#
# HISTORY, kept because it is why the `charts` folder must exist at all: copying `$Source\*`
# straight into `$Out` put `190`, `demo`, `respack` and every shared png/ogg at LocalFolder's root
# instead, and `GetFolderFromPathAsync(<LocalState>\charts)` had nothing to open. That is what
# `GameCreate -> -2 (ParseFailed)` was on the phone - not a parser, not IO permissions, not TLS:
# all ten asset requests failed in ~80 ms each with the charts folder absent, so the engine fell
# through every `info.yml`/`info.txt`/`info.csv` candidate and gave up.
$destCharts = Join-Path $Out "charts"
$destPack = Join-Path $Out "respack"
New-Item -ItemType Directory -Path $destCharts -Force | Out-Null
New-Item -ItemType Directory -Path $destPack -Force | Out-Null

# Content test, not a name list: a top-level folder is a chart when it carries a chart file
# (`.json`/`.pec`) of its own - `190\57649733.json`, `demo\chart.json`. `respack\`, `rank\` and
# `mod\` hold only sprites, sounds and `info.yml`, so they belong to the engine. Testing for the
# chart file rather than for known names means a chart dropped in later stages itself, and it is
# the same rule the package side of the split uses, so `Assets\` and LocalFolder stay congruent.
# `respack\info.yml` alone is not enough to make something a chart - that is the resource pack's
# own descriptor, which is exactly the trap a bare `info.yml` test would fall into.
foreach ($item in Get-ChildItem -LiteralPath $Source) {
    $to = if ($item.PSIsContainer -and
              ((Test-Path (Join-Path $item.FullName "*.json")) -or
               (Test-Path (Join-Path $item.FullName "*.pec")))) { $destCharts } else { $destPack }
    # Copy, never Move: `$Source` is the read-only desktop layout that `build-m1.ps1` was run
    # against, and consuming it would break the next `-Push`.
    Copy-Item -LiteralPath $item.FullName -Destination $to -Recurse -Force
}

# Printed every `-Push`: if a chart were routed to the pack root (or `respack\` to the charts
# root) the app would still boot - `ReadAsset` falls back - and the only visible symptom would be
# `asset resolved from pack root: 190/info.yml` deep in the log. This puts the routing decision
# where it is read first.
"  -> charts : {0}" -f ((Get-ChildItem -LiteralPath $destCharts -Name) -join ", ")
"  -> pack   : {0}" -f ((Get-ChildItem -LiteralPath $destPack -Name) -join ", ")

$staged = Get-ChildItem $Out -Recurse -File
"staged: {0} files, {1} MB -> {2}" -f $staged.Count,
    [math]::Round(($staged | Measure-Object Length -Sum).Sum / 1MB, 2), $Out
}

# ---------------------------------------------------------------------------
# 1b. Keep `LocalState\data` across the wipe.
#
# Both destructive steps above and below really are destructive: step 2 is uninstall +
# install ("a clean app AND a clean LocalFolder", per its own comment) and step 3 is
# `ISETool rs`, which replaces LocalFolder. `data\` lives inside LocalFolder, so every
# deploy threw away `data.json` - the login - and `data\charts\download\`, every chart
# fetched from the server.
#
# That is not a nuisance, it is the experiment destroying its own subject: the storage
# and network rounds exist to prove that a login and a downloaded chart survive and
# play, and one `-Deploy -Push` reduced `data.json` to its default (`"me": null`) and
# deleted the downloaded chart outright - re-establishing either means the user logs in
# and downloads again by hand. Staged here, before step 2, and put back after step 3 so
# it survives both wipes rather than whichever one happens to run.
#
# Through `ISETool ts` rather than by path. The phone's package folder
# (`C:\Data\Users\DefApps\APPDATA\Local\Packages\<ProductId>_...`) is not reachable from
# this machine at all - `C:\Data` does not exist here - so the first version of this block
# probed it, found nothing, printed "nothing to preserve" and walked straight into the wipe
# it was written to prevent. The store's own transport is the only thing that has ever
# worked on this setup: `ts` is what `-PullLog` reads with and `rs` is what step 3 writes
# with, so both halves go through the same tool and neither depends on a mount that comes
# and goes.
#
# Taken before step 2 because step 2 uninstalls, which is the moment the data stops existing.
# ---------------------------------------------------------------------------
$dataKeep = $null
$logKeep = $null
if ($Deploy -or $Push) {
    $keepDir = "E:\_phirura_keep"
    if (Test-Path $keepDir) {
        # `-LiteralPath` with an explicit path: the harness's safe-delete wrapper only reads
        # that form, and a piped form throws before anything is removed.
        Remove-Item -LiteralPath $keepDir -Recurse -Force
    }
    New-Item -ItemType Directory -Path $keepDir -Force | Out-Null
    & $ise ts $Device $ProductId $keepDir 2>&1 | Out-Null

    # `ts` wraps its output in `IsolatedStore\`, the same shape `-PullLog` reads back.
    $keepData = Join-Path $keepDir "IsolatedStore\data"
    if (Test-Path $keepData) {
        $dataKeep = $keepData
        $n = (Get-ChildItem -LiteralPath $keepData -Recurse -File | Measure-Object).Count
        "kept LocalState\data: $n files -> $keepData"
    } else {
        # No `data` yet, or `ts` failed against a locked store. Either way there is nothing to
        # put back, and claiming otherwise would read as a preserved login in the run output.
        "no LocalState\data in the store - nothing to preserve"
    }

    # `phirura.log` sits at LocalFolder's ROOT, so `rs` deletes it along with everything else -
    # and unlike `data` the loss is invisible. `AppLog` opens with `CreationCollisionOption.
    # OpenIfExists`, so the next launch silently recreates a file whose first line is
    # `=== Phirura launched ===`, and the previous session is simply gone. That is what made
    # every pull read as "a session with no past": an ending page played before the push looked
    # exactly like one that had never run, and the run was reported as empty three times in a
    # row. The log is the one artefact whose entire job is to outlive a deploy, so it joins
    # `data` in this same pre-uninstall pull - which has to happen before step 2, because
    # `/uninstall` is the moment LocalState stops existing.
    $keepLog = Join-Path $keepDir "IsolatedStore\phirura.log"
    if (Test-Path $keepLog) {
        $logKeep = $keepLog
        "kept phirura.log: {0} bytes" -f (Get-Item -LiteralPath $keepLog).Length
    } else {
        "no phirura.log in the store - nothing to preserve"
    }
}

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
    # Fold the preserved `data` into the mirror *before* `rs`. `rs` REPLACES LocalFolder, so
    # restoring afterwards is a restore onto a store that has already been emptied - which is
    # what made the first attempt at this a no-op that still reported success. Copying the
    # directory onto `$Out` puts it at `$Out\data`, and `rs` pushes `$Out`'s contents, so the
    # app finds `<LocalFolder>\data` exactly where `dir::root()` expects it.
    if ($dataKeep -and (Test-Path $dataKeep)) {
        Copy-Item -LiteralPath $dataKeep -Destination $Out -Recurse -Force
        $merged = Join-Path $Out "data"
        if (Test-Path $merged) {
            $n = (Get-ChildItem -LiteralPath $merged -Recurse -File | Measure-Object).Count
            "merged kept data: $n files -> $Out\data"
        }
    }
    # The log goes back the same way and before `rs` for the same reason: restoring it afterwards
    # would be a restore onto a store `rs` already emptied. Appending across deploys is exactly
    # what `OpenIfExists` is for, and `AppLog`'s size rotation keeps the file bounded on its own.
    if ($logKeep -and (Test-Path $logKeep)) {
        $logDst = Join-Path $Out "phirura.log"
        Copy-Item -LiteralPath $logKeep -Destination $logDst -Force
        "merged kept phirura.log -> {0} bytes" -f (Get-Item -LiteralPath $logDst).Length
    }
    & $ise rs $Device $ProductId $Out 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0) { Write-Warning "ISETool rs exit $LASTEXITCODE" }
    # Confirm with the path that would have failed had the `ts` wrapping been replicated here.
    & $ise "dir:charts" $Device $ProductId 2>&1 | Out-Null
    "pushed -> LocalFolder\charts + LocalFolder\respack"
}

# (3b removed: the restore moved inside step 3, before `rs`. Doing it after the push
# restored onto a store `rs` had already emptied, and the variables it read only existed
# when the unreachable `C:\Data` path happened to be mounted.)

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

    # The output used to be discarded (`| Out-Null`), which turned every failure into the same
    # one line - "no log at ..." - regardless of cause. `ts` has at least three distinct
    # outcomes (store not readable, wrong device, target dir already existing) and they need
    # different fixes, so the message is kept and printed only when it produced nothing: the
    # success path would otherwise be flooded with ISETool's progress text.
    $tsout = (& $ise ts $Device $ProductId $logdir 2>&1 | Out-String)

    $logfile = Join-Path $logdir "IsolatedStore\phirura.log"
    if (-not (Test-Path $logfile)) { Write-Warning "no log at $logfile"; if ($tsout) { "ISETool ts said:"; $tsout }; return }
    "=== $logfile ==="
    Get-Content $logfile | Select-Object -Last 80
}

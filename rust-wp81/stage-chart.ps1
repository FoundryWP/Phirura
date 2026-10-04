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

    [switch]$Deploy,   # uninstall + installlaunch  (wipes LocalFolder - must come first)
    [switch]$Push,     # ISETool rs  (also wipes LocalFolder - must come after -Deploy)
    [switch]$Launch,   # AppDeployCmd /launch  (must come after -Push)
    [switch]$PullLog   # ISETool ts into a fresh dir, then print the tail of phirura.log
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
Copy-Item "$Source\*" -Destination $Out -Recurse -Force

$staged = Get-ChildItem $Out -Recurse -File
"staged: {0} files, {1} MB -> {2}" -f $staged.Count,
    [math]::Round(($staged | Measure-Object Length -Sum).Sum / 1MB, 2), $Out

# ---------------------------------------------------------------------------
# 2. Deploy. `update` fails 0x81030137 when the version is unchanged, so this is
#    uninstall + install: a clean app AND a clean LocalFolder.
# ---------------------------------------------------------------------------
if ($Deploy) {
    & $adp /uninstall $ProductId /targetdevice:$Device 2>&1 | Out-Null
    & $adp /installlaunch $Appx /targetdevice:$Device 2>&1 | Out-Null
    "deployed (appx = $(Split-Path $Appx -Leaf))"
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
    "launched"
}

# ---------------------------------------------------------------------------
# 4. Pull the log. The directory must NOT already exist - that is the whole
#    "works iff <dir> does not already exist" rule - and the payload lands one
#    level down under IsolatedStore\, which is the opposite of what `rs` expects.
# ---------------------------------------------------------------------------
if ($PullLog) {
    $logdir = "E:\_phirura_log_pull_{0:HHmmss}" -f (Get-Date)
    & $ise ts $Device $ProductId $logdir 2>&1 | Out-Null
    $logfile = Join-Path $logdir "IsolatedStore\phirura.log"
    if (-not (Test-Path $logfile)) { Write-Warning "no log at $logfile"; return }
    "=== $logfile ==="
    Get-Content $logfile | Select-Object -Last 80
}

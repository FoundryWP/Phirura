<#
.SYNOPSIS
    Follow the phone's phirura.log while the app is running.

.DESCRIPTION
    WP8.1 has no attached debugger here, so "runtime logging" means polling the app's
    LocalFolder and printing what is new. `ISETool ts` copies the whole store, which is why
    this keeps the previous snapshot in memory and prints only the delta instead of dumping
    200 KB every poll.

    Two things make a naive version useless, and both are handled here:

      * `ISETool ts` fails with "another program is using this file" whenever the app happens
        to hold phirura.log open - which is often, because AppLog opens and closes it per
        write. That is a *race*, not a hard failure: retrying a few times wins it. A poll that
        gives up on the first error reports "no log" for a perfectly healthy run.
      * AppLog rotates the file when it passes 256 KB (it keeps the tail now, but the file can
        still be rewritten). When the content stops being a prefix of what was printed, the
        whole tail is re-printed rather than silently skipping the rotated-in lines.

.PARAMETER Seconds
    Delay between polls. `ts` copies the whole store (~22 MB of chart art, not just the log), so
    polling faster than every few seconds costs real device and USB time without showing anything
    sooner. 5 s is the default for that reason; go to 2-3 s only for a short, focused watch.

.PARAMETER Filter
    Regex; only matching lines are printed. Use it to watch one subsystem, e.g.
    `-Filter 'port frame|touch down|GameCreate'`.

.PARAMETER MaxPolls
    0 (default) polls until Ctrl-C.

.EXAMPLE
    .\tail-log.ps1
    .\tail-log.ps1 -Filter 'touch down|GameCreate|noCarrier' -Seconds 2
#>
param(
    [double]$Seconds = 5,
    [string]$Filter = "",
    [int]$MaxPolls = 0
)

$ErrorActionPreference = "Continue"   # ISETool reports through stderr; Stop would mask its exit code

$sdk = "C:\Program Files (x86)\Microsoft SDKs\Windows Phone\v8.1\Tools"
$ise = Join-Path $sdk "IsolatedStorageExplorerTool\ISETool.exe"
$productId = "fa2c9634-5ad8-487e-be3c-9893871fda9f"
$device = "de"
$work = "E:\_phirura_tail"

if (-not (Test-Path $ise)) { throw "missing tool: $ise" }
New-Item -ItemType Directory -Path $work -Force | Out-Null

$printed = 0
$polls = 0
"=== following phirura.log (Ctrl-C to stop) ==="

while ($true) {
    $polls++
    if ($MaxPolls -gt 0 -and $polls -gt $MaxPolls) { break }

    $snap = Join-Path $work ("snap_" + (Get-Date -Format HHmmssfff))
    $text = $null
    for ($try = 1; $try -le 4; $try++) {
        # The destination must not already exist, so each attempt gets its own name.
        $out = (& $ise ts $device $productId $snap 2>&1 | Out-String)
        $lf = Join-Path $snap "IsolatedStore\phirura.log"
        if (Test-Path $lf) {
            $text = [System.IO.File]::ReadAllText($lf)
            break
        }
        Start-Sleep -Milliseconds 700
    }

    if ($null -ne $text) {
        $lines = $text -split "`r?`n"
        if ($lines.Count -lt $printed) {
            # The file shrank: AppLog rotated it, so what we printed no longer applies.
            "--- log rotated; reprinting ---"
            $printed = 0
        }
        for ($i = $printed; $i -lt $lines.Count; $i++) {
            $line = $lines[$i]
            if ($line -eq "") { continue }
            if ($Filter -eq "" -or $line -match $Filter) { Write-Output $line }
        }
        $printed = $lines.Count
    }

    # Reclaim the snapshot. `ts` copies the WHOLE store - charts included, ~22 MB - so leaving one
    # behind per poll fills the disk within a minute. The safe-delete wrapper around Remove-Item
    # cannot be used here (it fails closed on a >50-file directory, which this always is), so the
    # delete is done through .NET - and bounded to the scratch root this script created, checked
    # explicitly rather than assumed, so a bug in the path maths cannot reach anything else.
    if (Test-Path $snap) {
        $full = [System.IO.Path]::GetFullPath($snap)
        $root = [System.IO.Path]::GetFullPath($work)
        if ($full.StartsWith($root + [System.IO.Path]::DirectorySeparatorChar)) {
            try { [System.IO.Directory]::Delete($full, $true) } catch { }
        } else {
            Write-Warning "refusing to clean a snapshot outside $root : $full"
        }
    }
    Start-Sleep -Seconds $Seconds
}

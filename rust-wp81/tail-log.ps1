<#
.SYNOPSIS
    Follow the phone's phirura.log while the app is running.

.DESCRIPTION
    WP8.1 has no attached debugger here, so "runtime logging" means polling the app's store and
    printing what is new. This keeps the previous snapshot in memory and prints only the delta.

    Three things make a naive version useless, and all are handled here:

      * It reads the app's **temp** store (`appxfolder:Temp`), not Local. AppLog mirrors every line
        into both, and Local is the wrong place to poll from: `ts` copies the whole 22 MB of chart
        art and *aborts* on the first unreadable file - which is the chart's music, held open by the
        MediaElement for as long as the game is playing. The log is then never reached, so every pull
        during gameplay failed with a sharing violation that had nothing to do with the log. The temp
        store holds only the log, which also makes each poll instant instead of a 22 MB walk.
      * `ts` refuses a destination that already exists, and a *failed* attempt still leaves one
        behind - so the directory name has to be fresh per attempt, not per poll. Reusing one made
        every retry fail for the same reason as the first, and the retry loop could never win.
      * AppLog rotates the file when it passes 256 KB (it keeps the tail now). When the content
        stops being a prefix of what was printed, the whole tail is re-printed rather than silently
        skipping the rotated-in lines.

.PARAMETER Seconds
    Delay between polls. The temp store is small, so this is cheap; 5 s is still the default because
    the panel is read by a human and every poll is a device round-trip.

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

    $text = $null
    $snaps = @()
    # Patient on purpose. The lock correlates with *activity*: the more the app logs, the more often
    # AppLog holds phirura.log open, so the polls most likely to fail are the ones during gameplay -
    # exactly the ones worth watching. Four attempts was not enough; a failed poll that gives up
    # early turns a busy run into "no output", which reads as a broken logger.
    #
    # A fresh directory per *attempt*, not per poll. `ISETool ts` refuses a destination that already
    # exists, and a failed attempt leaves its directory behind - so reusing one name made every
    # retry fail for the same reason as the first, and the retry loop could never succeed.
    for ($try = 1; $try -le 10; $try++) {
        $snap = Join-Path $work ("snap_" + (Get-Date -Format HHmmssfff) + "_$try")
        $snaps += $snap
        # `appxfolder:Temp`, not the default Local store. `ts` on Local copies the whole 22 MB of
        # chart art and ABORTS on the first unreadable file - which is the chart's music, held open
        # by the MediaElement for as long as the game is playing. The log is then never reached, and
        # the failure looks like "the log is locked" when it is really "the copy died at the mp3".
        # AppLog mirrors every line into the temp store, which holds nothing else, so this is both
        # reliable and instant.
        $out = (& $ise ts appxfolder:Temp $device $productId $snap 2>&1 | Out-String)
        $found = @(Get-ChildItem -Path $snap -Recurse -Filter "phirura.log" -ErrorAction SilentlyContinue)
        if ($found.Count -gt 0) {
            $text = [System.IO.File]::ReadAllText($found[0].FullName)
            break
        }
        if ($try -eq 10) {
            Write-Warning ("poll {0}: 10 attempts, last said: {1}" -f $polls, ($out -replace '\s+', ' ').Trim())
        }
        Start-Sleep -Milliseconds 900
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
    foreach ($snap in $snaps) {
        if (Test-Path $snap) {
            $full = [System.IO.Path]::GetFullPath($snap)
            $root = [System.IO.Path]::GetFullPath($work)
            if ($full.StartsWith($root + [System.IO.Path]::DirectorySeparatorChar)) {
                try { [System.IO.Directory]::Delete($full, $true) } catch { }
            } else {
                Write-Warning "refusing to clean a snapshot outside $root : $full"
            }
        }
    }
    Start-Sleep -Seconds $Seconds
}

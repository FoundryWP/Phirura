param(
    [string]$Out = "E:\_phirura_tmp2\log_census.txt",
    [string]$Url = "http://192.168.1.7:8765/log?tail=999999"
)

$PhoneGuid = "fa2c9634-5ad8-487e-be3c-9893871fda9f"
$Sdk = "C:\Program Files (x86)\Microsoft SDKs\Windows Phone\v8.1\Tools"
$Via = "http"

# Removed first, because a failed `curl -o` leaves the previous run's file in place and
# `Test-Path` would then happily report a stale census as if it were this run's.
if (Test-Path $Out) { Remove-Item -LiteralPath $Out -Force -ErrorAction SilentlyContinue }
curl.exe -sS --max-time 30 $Url -o $Out

if ((-not (Test-Path $Out)) -or ((Get-Item $Out).Length -eq 0)) {
    # The HTTP log server is a StreamSocketListener owned by the *app process*, so a crash takes
    # the log endpoint down with it - which is precisely the moment the log is worth having. This
    # fallback is the reason to reach for it: `ISETool ts` reads the isolated store straight off
    # the device and needs no running process at all, so a launch-crash still leaves its trace
    # here. Reported as `PULL via ISETool` rather than as an error, because dying HTTP on a
    # crashed app is the expected outcome, not a fault in the pull.
    $Via = "ISETool"
    $ise = Join-Path $Sdk "IsolatedStorageExplorerTool\ISETool.exe"
    if (-not (Test-Path $ise)) { "PULL FAILED (http dead, no ISETool at $ise)"; exit 1 }

    $pull = "E:\_phirura_tmp2\ise_pull"
    New-Item -ItemType Directory -Path $pull -Force | Out-Null
    $iseLog = Join-Path (Join-Path $pull "IsolatedStore") "phirura.log"
    # Cleared first: ISETool answers success either way, and a leftover from the last run would
    # be read back as this crash's evidence.
    if (Test-Path $iseLog) { Remove-Item -LiteralPath $iseLog -Force -ErrorAction SilentlyContinue }

    & $ise ts appxfolder:Temp deviceindex:0 $PhoneGuid $pull | Out-Null
    if (-not (Test-Path $iseLog)) { "PULL FAILED (http dead and no $iseLog)"; exit 1 }
    Copy-Item -LiteralPath $iseLog -Destination $Out -Force
}

"PULL via $Via, bytes=" + (Get-Item $Out).Length

# A crash leaves no exception behind unless something wrote one down on the way out, so these
# markers are scanned up front: they are the whole reason a run gets re-pulled after the phone
# has already died. `PANIC` comes from the Rust hook installed in `set_log_sink`; `UNHANDLED`
# and the Win2D lines are the C# side of the same event.
$crash = Select-String -Path $Out -Pattern "PANIC ", "UNHANDLED", "port draw failed", "port draw device lost" -SimpleMatch
if ($crash) {
    "=== CRASH MARKERS ==="
    $crash | Select-Object -Last 20 | ForEach-Object { "  L" + $_.LineNumber + "  " + $_.Line }
} else {
    "=== no PANIC / UNHANDLED / Win2D draw-failure marker in the log ==="
}

# Strip the timestamp first: without it every line is unique and Group-Object
# returns one group per line, which is what made the first census useless.
function Group-Msg([string]$pat, [int]$first) {
    Select-String -Path $Out -Pattern $pat | ForEach-Object {
        $l = $_.Line
        $i = $l.IndexOf("Win2D: ")
        if ($i -ge 0) { $l.Substring($i + 7) } else { $l }
    } | Group-Object | Sort-Object Count -Descending | Select-Object -First $first |
      ForEach-Object { "x" + $_.Count.ToString().PadLeft(4) + "  " + $_.Name }
}

$e = Select-String -Path $Out -Pattern "ending:" | Select-Object -First 1
if ($e) { "FIRST ending line = " + $e[0].LineNumber } else { "NO ending yet" }

"=== ord ===";    Group-Msg "port frame" 8
"=== untext ==="; Group-Msg "Win2D: untext" 60
"=== texdraw ==="; Group-Msg "texdraw" 40

# Two health samples five seconds apart. A dead phone times out here, and a live app whose
# `chars=` stops moving has stopped logging - those two look identical from the log file alone,
# which is what made the last two runs ambiguous.
#
# Skipped entirely on the ISETool path: there is no server to ask, and 11 s of curl timeouts to
# reach a conclusion the pull itself has already established.
if ($Via -ne "http") {
    "ALIVE=n/a (pulled via ISETool - app not running, see CRASH MARKERS above)"
} else {
    $h1 = curl.exe -sS --max-time 6 "$Url/../health"
    Start-Sleep -Seconds 5
    $h2 = curl.exe -sS --max-time 6 "$Url/../health"
    if (-not $h1) { "ALIVE=no (phone off or app gone)" } else {
        "A: $h1"
        "B: $h2"
        if ($h1 -eq $h2) { "ALIVE=yes but chars=STATIC -> app stopped logging" }
        else { "ALIVE=yes and chars=MOVING -> app healthy, log really is complete" }
    }
}
"last log line: " + (Get-Content $Out -Tail 1).Substring(0, 40)

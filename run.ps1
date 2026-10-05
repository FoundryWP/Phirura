<#
.SYNOPSIS
    One button for the whole device loop: ARM DLL -> appx -> deploy -> launch -> log.

.DESCRIPTION
    The loop was four commands in three different directories, and every round trip between them
    was a place to lose track of which artifact was actually on the phone. This runs them in
    order, stops at the first failure, and prints one summary - so the last thing on screen is
    always "this is what the phone has", never a partial chain nobody noticed broke in the middle.

    "换 DLL" needs no step of its own: `build-port.ps1` copies `wp81prpr.dll` into
    `Phirura.WindowsPhone\` as its last action, and `build_wp.ps1` picks it up from there because
    it is a content item in the project. It is listed below as its own line only so the summary
    says where the DLL on the phone came from.

    Iteration is the point, so the expensive steps are individually skippable:

        run.ps1                      everything, start to finish
        run.ps1 -NoRust              DLL is already built  (C# + deploy only)
        run.ps1 -NoRust -NoCSharp    just redeploy and pull the log
        run.ps1 -LogOnly             pull and print, touch nothing on the phone

    Exit codes are read from `$LASTEXITCODE` after each script rather than from its output, and
    each script is run inside try/catch: `build-port.ps1` signals failure with `throw` (not an
    exit code), while MSBuild signals it with an exit code (not an exception). Checking only one
    of the two would pass a broken build half the time.

.PARAMETER Wait
    Seconds between launch and the log pull. `GameScene::new` measured 8153 ms on the phone, so a
    pull sooner than that catches the asset loads and misses the frame loop entirely.

.EXAMPLE
    & run.ps1
.EXAMPLE
    & run.ps1 -NoRust -NoCSharp
#>

param(
    [switch]$NoRust,
    [switch]$NoCSharp,
    [switch]$NoDeploy,
    [switch]$LogOnly,
    [string]$Folder = "190",
    [int]$Wait = 30
)

# `Continue`, not `Stop`: piping a script's stderr through `2>&1` under `Stop` promotes every
# warning to a NativeCommandError and reports a successful build as a crash. Failures here are
# decided by exit code and by caught exceptions, not by how loudly a step complained.
$ErrorActionPreference = "Continue"

$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$port   = Join-Path $root "rust-wp81\build-port.ps1"
$wp     = Join-Path $root "build_wp.ps1"
$stage  = Join-Path $root "rust-wp81\stage-chart.ps1"

$steps = New-Object System.Collections.ArrayList
$failedStep = $null

# Run one step, catch both shapes of failure, and remember how long it took.
function Invoke-Step {
    param([string]$Name, [scriptblock]$Body)
    if ($failedStep) { return }          # fail fast: report the first break, not every consequence

    Write-Host ""
    Write-Host "=== $Name ===" -ForegroundColor Cyan
    $t = [System.Diagnostics.Stopwatch]::StartNew()
    $script:LASTEXITCODE = 0             # sentinel: a step that ran no native command is a success
    $err = $null
    try { & $Body } catch { $err = $_ }
    $t.Stop()
    $code = $script:LASTEXITCODE

    $ok = (-not $err) -and ($code -eq 0)
    [void]$steps.Add([pscustomobject]@{
        Step = $Name; Exit = $code; Seconds = [math]::Round($t.Elapsed.TotalSeconds, 1); Ok = $ok
    })
    if (-not $ok) {
        $script:failedStep = $Name
        Write-Host ""
        if ($err) { Write-Host "FAILED: $err" -ForegroundColor Red }
        else { Write-Host "FAILED: $Name exited $code" -ForegroundColor Red }
    }
}

# Resolve the appx by glob rather than hard-coding the version folder: the path in
# stage-chart.ps1's default names `1.0.0.0`, and a version bump would otherwise silently deploy
# yesterday's build while printing "installed".
function Find-Appx {
    $pkg = Join-Path $root "Phirura\Phirura\Phirura.WindowsPhone\AppPackages"
    $hit = Get-ChildItem $pkg -Recurse -Filter "*.appx" -ErrorAction SilentlyContinue |
           Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if ($hit) { return $hit.FullName }
    return $null
}

# The lines worth looking at. Everything else in the log is per-asset byte counts.
function Show-KeyLines([string]$path) {
    $keys = "rust create", "engine frame", "GameCreate", "frame#1",
            "ERROR", "Exception", "FAILED", "-> -"
    $lines = Get-Content $path -ErrorAction SilentlyContinue
    $hits = @($lines | Where-Object {
        foreach ($k in $keys) { if ($_ -like "*$k*") { return $true } }
        return $false
    })
    if ($hits.Count -eq 0) {
        Write-Host "  (日志里没有 GamePage / 引擎的行 - 应用起来了但没进到选曲或关卡)" -ForegroundColor Yellow
    } else {
        foreach ($h in $hits) { Write-Host "  $h" }
    }
    Write-Host ""
    Write-Host "--- 日志末尾 30 行 ---" -ForegroundColor DarkGray
    $lines | Select-Object -Last 30 | ForEach-Object { Write-Host "  $_" -ForegroundColor DarkGray }
}

Write-Host "Phirura device loop" -ForegroundColor White
Write-Host "root = $root"
if ($LogOnly) { $NoRust = $true; $NoCSharp = $true; $NoDeploy = $true }

# ---------------------------------------------------------------- step 1: ARM DLL
if (-not $NoRust) {
    Invoke-Step "1/3  ARM DLL  (cargo build -p prpr-port + link + import/TLS gate)" {
        & $port
        if ($LASTEXITCODE -ne 0) { throw "build-port.ps1 failed" }
    }
}

# ---------------------------------------------------------------- step 2: appx
if (-not $NoCSharp) {
    Invoke-Step "2/3  appx     (MSBuild 12.0, ARM Debug, wp target)" {
        & $wp -Debug -Target wp
        if ($LASTEXITCODE -ne 0) { throw "build_wp.ps1 failed" }
    }
    if (-not $failedStep) {
        $dll = Join-Path $root "Phirura\Phirura\Phirura.WindowsPhone\wp81prpr.dll"
        if (Test-Path $dll) {
            $d = Get-Item $dll
            Write-Host ("      wp81prpr.dll  {0:N0} B  {1}" -f $d.Length, $d.LastWriteTime)
        }
        $appx = Find-Appx
        if ($appx) { Write-Host ("      appx         {0}  {1}" -f (Split-Path $appx -Leaf), (Get-Item $appx).LastWriteTime) }
        else { $script:failedStep = "2/3 appx"; Write-Host "FAILED: no .appx produced" -ForegroundColor Red }
    }
}

# ---------------------------------------------------------------- step 3: deploy
if (-not $NoDeploy -and -not $failedStep) {
    $appx = Find-Appx
    if (-not $appx) { $script:failedStep = "3/3 deploy"; Write-Host "FAILED: no .appx to deploy" -ForegroundColor Red }
    else {
        Invoke-Step "3/3  deploy  (uninstall+install appx, push charts\, launch)" {
            & $stage -Deploy -Push -Launch -Folder $Folder -Appx $appx
            if ($LASTEXITCODE -ne 0) { throw "stage-chart.ps1 failed" }
        }
    }
}

# ---------------------------------------------------------------- log
if ($failedStep) {
    Write-Host ""
    Write-Host "链路断在 $failedStep - 后面的步骤没有执行。" -ForegroundColor Red
} else {
    Write-Host ""
    Write-Host "部署完成。在手机上点开 $Folder ..." -ForegroundColor Yellow
    if ($Wait -gt 0) {
        Write-Host "    $Wait 秒后拉日志（GameScene::new 实测 8153 ms，拉早了只看得到资源加载）" -ForegroundColor Yellow
        Start-Sleep -Seconds $Wait
    }
    Invoke-Step "log   pull   (ISETool ts -> phirura.log)" {
        & $stage -PullLog -Folder $Folder
    }
    if (-not $failedStep) {
        $log = Get-ChildItem "E:\_phirura_log_pull_*\IsolatedStore\phirura.log" -ErrorAction SilentlyContinue |
               Sort-Object LastWriteTime -Descending | Select-Object -First 1
        if ($log) {
            Write-Host ""
            Write-Host "=== $($log.FullName)  ($($log.Length) B, $($log.LastWriteTime)) ===" -ForegroundColor White
            Show-KeyLines $log.FullName
        } else {
            Write-Host "没有拉到 phirura.log。" -ForegroundColor Yellow
        }
    }
}

# ---------------------------------------------------------------- summary
Write-Host ""
Write-Host "=== summary ===" -ForegroundColor White
$steps | Format-Table -AutoSize | Out-String | Write-Host
if ($failedStep) { exit 1 }
Write-Host "全部通过。" -ForegroundColor Green
exit 0

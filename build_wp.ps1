param(
    [string]$Configuration = "Debug",
    [string]$Target = "wp",
    [string]$Platform = "",
    [switch]$Isolated,
    # Extra MSBuild switches, semicolon separated (e.g. "/t:ResolveAssemblyReferences;/v:diag").
    # Used to interrogate the build without changing the normal build path.
    [string]$Extra = ""
)

# Builds the Phirura Windows Phone 8.1 head with MSBuild 12.0 (VS2013).
# Direct MSBuild invocation is blocked in the sandbox; this script is the only
# working entry point. Keep this file ASCII-only (PowerShell 5.1 reads BOM-less
# files as ANSI and mangles non-ASCII characters).
#
# The script never deletes directories. A leftover MSBuild worker node can hold
# the .pdb for a moment, which makes CSC fail with "fatal error CS0042"; that is
# transient, so we simply retry a few times with a short pause.

$ErrorActionPreference = "Continue"

$root = "E:\Users\admin\Documents\visual studio 2013\Phirura"
$msbuild = "C:\Program Files (x86)\MSBuild\12.0\Bin\MSBuild.exe"
$wpProj = "$root\Phirura\Phirura\Phirura.WindowsPhone\Phirura.WindowsPhone.csproj"
$winProj = "$root\Phirura\Phirura\Phirura.Windows\Phirura.Windows.csproj"

$proj = $wpProj
if ($Target -eq "win") {
    $proj = $winProj
}

# Win2D ships native binaries for Phone only as arm/x86, so the WP head can no
# longer be AnyCPU. ARM is the real device target and the default here; pass
# -Platform x86 for the emulator. The Win head stays AnyCPU and stays on XAML.
if ($Platform -eq "") {
    if ($Target -eq "win") { $Platform = "AnyCPU" } else { $Platform = "ARM" }
}

if (-not (Test-Path $msbuild)) {
    Write-Output "=== MSBUILD NOT FOUND: $msbuild ==="
    exit 2
}
if (-not (Test-Path $proj)) {
    Write-Output "=== PROJECT NOT FOUND: $proj ==="
    exit 3
}

$argsList = @(
    $proj,
    "/t:Build",
    "/p:Configuration=$Configuration",
    "/p:Platform=$Platform",
    "/v:minimal",
    "/nologo",
    "/nodeReuse:false"
)

# A previous build can leave the project's obj\Debug pdb held by a child process,
# which makes CSC fail with "fatal error CS0042" on every subsequent run. The retry
# loop below cannot clear that (the handle never goes away), so -Isolated points the
# intermediate directory at a scratch path nobody else holds.
# The scratch path is deliberately space-free: MSBuild 12 splits arguments that
# contain spaces, and a quoted value ending in a backslash corrupts the command line.
if ($Isolated) {
    $scratch = "E:\_phirura_build"
    New-Item -ItemType Directory -Path $scratch -Force | Out-Null
    $argsList += "/p:BaseIntermediateOutputPath=$scratch\obj\"
    Write-Output "=== ISOLATED: intermediate -> $scratch\obj\ ==="
}

if ($Extra -ne "") {
    foreach ($a in $Extra.Split(";")) {
        if ($a.Trim() -ne "") { $argsList += $a.Trim() }
    }
}

$attempt = 0
$maxAttempts = 3
$exit = 1

while ($attempt -lt $maxAttempts) {
    $attempt++
    if ($attempt -gt 1) {
        Write-Output "=== RETRY $attempt (waiting for the pdb handle to clear) ==="
        Start-Sleep -Seconds 3
    }

    $out = & $msbuild @argsList 2>&1 | Out-String -Width 400
    $exit = $LASTEXITCODE
    Write-Output $out

    # The calling tool does not always surface this script's stdout, so the build
    # log is also written to disk. Read that file when a build fails silently.
    $out | Out-File -FilePath "E:\_phirura_build.log" -Encoding utf8

    if ($exit -eq 0) {
        break
    }

    $locked = ($out -match "CS0042") -or ($out -match "另一个程序正在使用此文件")
    if (-not $locked) {
        break
    }
}

Write-Output "=== EXITCODE: $exit (attempts: $attempt) ==="

# Propagate msbuild's code instead of only printing it. As written the script always exited 0 -
# a `.ps1` run with `-File` returns 0 unless it calls `exit` - so every caller chaining on
# $LASTEXITCODE took a failed compile for a successful one and went on to deploy whatever appx
# was left from the last good build. That reads as "the changes had no effect", which is a far
# more expensive misunderstanding than a build that plainly failed: it sends the next round to
# the device to look for a bug that was never shipped.
exit $exit

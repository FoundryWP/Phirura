# Ask the WP8.1 WinRT projection itself what IID the CoreWindow runtime class and its
# interfaces have. Compiling a `typeof(...).GUID` reference is ground truth: the projection
# emits the GUID as a literal, so if the toolchain knows it, we read it verbatim.
#
# Also probes whether ICoreWindowInterop (the Win32/WP8.1-only private interface that exposes
# the raw HWND/CoreWindow pointer) is reachable from C# at all.
param(
    [string]$Out = "E:\_phirura_iidprobe"
)

$csc      = "C:\Program Files (x86)\MSBuild\12.0\bin\Csc.exe"
$winmd    = "C:\Program Files (x86)\Windows Phone Kits\8.1\References\CommonConfiguration\Neutral\Windows.winmd"
$platform = "C:\Program Files (x86)\Microsoft SDKs\WindowsPhoneApp\v8.1\ExtensionSDKs\Microsoft.VCLibs\12.0\References\CommonConfiguration\neutral\platform.winmd"
$fx       = "C:\Program Files (x86)\Reference Assemblies\Microsoft\Framework\WindowsPhoneApp\v8.1"

New-Item -ItemType Directory -Force -Path $Out | Out-Null

$refs = @($winmd, $platform)
$refs += (Get-ChildItem "$fx\*.dll" | ForEach-Object { $_.FullName })
$refs = $refs | Where-Object { Test-Path $_ }

$snippets = @(
    @{ Name = "CoreWindow runtime class GUID";
       Body = 'public static string D(){ return typeof(Windows.UI.Core.CoreWindow).GUID.ToString("D"); }' },
    @{ Name = "ICoreWindow cast-to interface (as ICoreWindow)";
       Body = 'public static void D(){ var cw = Windows.UI.Core.CoreWindow.GetForCurrentThread(); object o = (object)cw; }' },
    @{ Name = "ICoreWindowInterop by name (should fail on WP8.1 projection)";
       Body = 'public static void D(){ }' }
)

foreach ($s in $snippets) {
    $src = Join-Path $Out "probe.cs"
    $code = "using System;`r`npublic static class P { $($s.Body) }`r`n"
    Set-Content -Path $src -Value $code -Encoding UTF8

    $cargs = @("/noconfig", "/nostdlib+", "/nologo", "/warn:0", "/target:library", "/platform:arm",
               "/define:DEBUG;TRACE;NETFX_CORE;WINDOWS_PHONE_APP", "/out:$Out\probe.dll", $src)
    foreach ($r in $refs) { $cargs += "/reference:$r" }

    Write-Host "=== $($s.Name) ==="
    $res = & $csc @cargs 2>&1
    if ($LASTEXITCODE -eq 0) {
        Write-Host "  COMPILED OK"
    } else {
        $res | Where-Object { $_ -match "error" } | Select-Object -First 4 | ForEach-Object { Write-Host "  $_" }
    }
}

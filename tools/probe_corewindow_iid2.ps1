# Follow-up probe: WP8.1's .NET profile has no Type.GUID, so print the IID a different way and
# establish whether the CoreWindow's ICoreWindow interface is reachable/nameable from C#.
#
# The question these snippets answer, in order:
#   1. Is there a nameable `Windows.UI.Core.ICoreWindow` in the WP8.1 projection?
#   2. Does `CoreWindow` expose a WindowHandle (the WP8.1-only ICoreWindowInterop surface)?
#   3. Can we QI an RCW for an arbitrary IID without the interface being projected (the
#      Marshal.QueryInterface route, which is what the fix likely needs)?
param(
    [string]$Out = "E:\_phirura_iidprobe2"
)

$csc      = "C:\Program Files (x86)\MSBuild\12.0\bin\Csc.exe"
$winmd    = "C:\Program Files (x86)\Windows Phone Kits\8.1\References\CommonConfiguration\Neutral\Windows.winmd"
$platform = "C:\Program Files (x86)\Microsoft SDKs\WindowsPhoneApp\v8.1\ExtensionSDKs\Microsoft.VCLibs\12.0\References\CommonConfiguration\neutral\platform.winmd"
$fx       = "C:\Program Files (x86)\Reference Assemblies\Microsoft\Framework\WindowsPhoneApp\v8.1"

New-Item -ItemType Directory -Force -Path $Out | Out-Null
$refs = @($winmd, $platform) + (Get-ChildItem "$fx\*.dll" | ForEach-Object { $_.FullName })
$refs = $refs | Where-Object { Test-Path $_ }

$snippets = @(
    @{ Name = "1. nameable ICoreWindow";
       Body = 'public static void D(){ Windows.UI.Core.ICoreWindow w = null; if (w != null) { } }' },
    @{ Name = "2a. CoreWindow.WindowHandle property";
       Body = 'public static void D(){ var cw = Windows.UI.Core.CoreWindow.GetForCurrentThread(); var h = cw.WindowHandle; }' },
    @{ Name = "2b. ICoreWindowInterop.WindowHandle";
       Body = 'public static void D(){ object o = null; var i = (Windows.UI.Core.ICoreWindowInterop)o; var h = i.WindowHandle; }' },
    @{ Name = "3a. Marshal.QueryInterface on RCW for a made-up IID";
       Body = 'public static void D(){ var cw = Windows.UI.Core.CoreWindow.GetForCurrentThread(); var p = System.Runtime.InteropServices.Marshal.GetIUnknownForObject(cw); IntPtr outP; var g = new Guid("C2FE2616-8FBB-4A55-A3E3-CC4F4E1FB1E0"); int hr = System.Runtime.InteropServices.Marshal.QueryInterface(p, ref g, out outP); }' },
    @{ Name = "3b. typeof(CoreWindow).GetTypeInfo() then reflection for Interface";
       Body = 'public static string D(){ var t = typeof(Windows.UI.Core.CoreWindow).GetTypeInfo(); return t.FullName; }' }
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
    if ($LASTEXITCODE -eq 0) { Write-Host "  COMPILED OK" }
    else { $res | Where-Object { $_ -match "error" } | Select-Object -First 3 | ForEach-Object { Write-Host "  $_" } }
}

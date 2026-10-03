# Get the ICoreWindow IID as the WP8.1 compiler actually emits it, by compiling a probe that
# *references* the interface (forcing the projection to resolve it) and then disassembling the
# resulting IL. csc bakes the GUID into the referenced type's metadata; ildasm shows it.
#
# Why this and not reflection: Windows PowerShell 5.1 cannot load a WinRT .winmd as a managed
# assembly (needs System.Runtime.WindowsRuntime), and the raw #GUID heap is not stored as bare
# 16-byte GUIDs, so a byte scan is a false negative. The compiler is the same ground truth the
# project's probe_api.py already relies on.
param(
    [string]$Out = "E:\_phirura_iid3"
)

$csc      = "C:\Program Files (x86)\MSBuild\12.0\bin\Csc.exe"
$ildasm   = "C:\Program Files (x86)\Microsoft SDKs\Windows\v8.1A\bin\NETFX 4.5.1 Tools\ildasm.exe"
if (-not (Test-Path $ildasm)) {
    $ildasm = (Get-ChildItem "C:\Program Files (x86)\Microsoft SDKs\Windows" -Recurse -Filter ildasm.exe -ErrorAction SilentlyContinue |
               Select-Object -First 1).FullName
}
$winmd    = "C:\Program Files (x86)\Windows Phone Kits\8.1\References\CommonConfiguration\Neutral\Windows.winmd"
$platform = "C:\Program Files (x86)\Microsoft SDKs\WindowsPhoneApp\v8.1\ExtensionSDKs\Microsoft.VCLibs\12.0\References\CommonConfiguration\neutral\platform.winmd"
$fx       = "C:\Program Files (x86)\Reference Assemblies\Microsoft\Framework\WindowsPhoneApp\v8.1"

New-Item -ItemType Directory -Force -Path $Out | Out-Null
$refs = @($winmd, $platform) + (Get-ChildItem "$fx\*.dll" | ForEach-Object { $_.FullName })
$refs = $refs | Where-Object { Test-Path $_ }

$src = Join-Path $Out "probe.cs"
$code = @'
using System;
using Windows.UI.Core;
public static class P {
    // Both references force the projection to emit the interface's IID into the assembly.
    public static Type CoreWindowIface() { return typeof(ICoreWindow); }
    public static Type CoreWindowClass() { return typeof(CoreWindow); }
}
'@
Set-Content -Path $src -Value $code -Encoding UTF8

$cargs = @("/noconfig", "/nostdlib+", "/nologo", "/warn:0", "/target:library", "/platform:arm",
           "/define:DEBUG;TRACE;NETFX_CORE;WINDOWS_PHONE_APP", "/out:$Out\probe.dll", $src)
foreach ($r in $refs) { $cargs += "/reference:$r" }

Write-Host "=== compile ==="
$res = & $csc @cargs 2>&1
if ($LASTEXITCODE -ne 0) { $res | Select-Object -First 8; exit 1 }
Write-Host "  OK: $Out\probe.dll"

Write-Host "`n=== ildasm .class of ICoreWindow usage ==="
if (-not (Test-Path $ildasm)) { Write-Host "ildasm not found"; exit 1 }

$il = & $ildasm /text /nobar "$Out\probe.dll" 2>&1
$il | Select-String -Pattern "Windows.UI.Core|ICoreWindow|GuidAttribute|extern" | Select-Object -First 40

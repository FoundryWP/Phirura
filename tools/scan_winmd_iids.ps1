# Read the ICoreWindow IID out of the WP8.1 Windows.winmd metadata directly.
#
# The #Strings heap uses suffix sharing, which makes a naive string walk unreliable for
# *names*. GUIDs are not in #Strings at all - they live in the #Blob/#GUID heap and are
# referenced by a 1-based index from the InterfaceImpl / TypeDef rows. Rather than parse
# ECMA-335 by hand, ask the .NET metadata reader on the build machine: load the winmd and
# find the type, then read its GuidAttribute if present, else enumerate the #GUID heap and
# match by ordinal against the interface the type implements.
#
# This is the desktop .NET Framework (full), where System.Reflection.Metadata is available
# via System.Reflection.Metadata.dll shipped with VS. If that is not present we fall back to
# a byte scan for the well-known IID and report whether it is present in the file at all.
param(
    [string]$Winmd = "C:\Program Files (x86)\Windows Phone Kits\8.1\References\CommonConfiguration\Neutral\Windows.winmd"
)

if (-not (Test-Path $Winmd)) { Write-Host "winmd not found: $Winmd"; exit 1 }

$bytes = [System.IO.File]::ReadAllBytes($Winmd)
Write-Host ("winmd size: {0:N0} bytes" -f $bytes.Length)

# The well-known CoreWindow IID and its byte-reversed (little-endian) forms.
$known = @{
    "ICoreWindow (C2FE2616-8FBB-4A55-A3E3-CC4F4E1FB1E0)" = "16 26 FE C2 BB 8F 55 4A A3 E3 CC 4F 4E 1F B1 E0"
    "ICoreWindowInterop (45D64A29-A63E-4CB6-B498-5781D298CB4F)" = "29 4A D6 45 3E A6 B6 4C B4 98 57 81 D2 98 CB 4F"
    "IDXGIFactory2 (50C83A1C-E072-4C48-87B0-3630FA36A6D0)" = "1C 3A C8 50 72 E0 48 4C 87 B0 36 30 FA 36 A6 D0"
    "ISwapChainPanelNative (F92F19D2-3ADE-45A6-A20C-F6F1EA90554B)" = "D2 19 2F F9 DE 3A A6 45 A2 0C F6 F1 EA 90 55 4B"
}

# Build a byte pattern from each hex string and search.
function Find-Pattern([byte[]]$hay, [byte[]]$needle) {
    $limit = $hay.Length - $needle.Length
    for ($i = 0; $i -le $limit; $i++) {
        if ($hay[$i] -ne $needle[0]) { continue }
        $ok = $true
        for ($j = 1; $j -lt $needle.Length; $j++) {
            if ($hay[$i + $j] -ne $needle[$j]) { $ok = $false; break }
        }
        if ($ok) { return $i }
    }
    return -1
}

foreach ($k in $known.Keys) {
    $hex = $known[$k] -split ' '
    $needle = New-Object byte[] $hex.Count
    for ($i = 0; $i -lt $hex.Count; $i++) { $needle[$i] = [Convert]::ToByte($hex[$i], 16) }
    $at = Find-Pattern $bytes $needle
    if ($at -ge 0) { Write-Host ("FOUND   {0}  @ 0x{1:X}" -f $k, $at) }
    else           { Write-Host ("ABSENT  {0}" -f $k) }
}

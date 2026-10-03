# Read the ICoreWindow GUID the WP8.1 projection actually uses, by loading Windows.winmd as a
# reflection-only assembly on the build machine and reading the Windows.Foundation.Metadata
# GuidAttribute that the WinRT metadata generator emits for every interface type.
#
# This replaces the byte-scan (which failed because the #GUID heap in a .winmd is not stored as
# bare 16-byte GUIDs in the file body). Reflection over the metadata is the authoritative reader.
param(
    [string]$Winmd = "C:\Program Files (x86)\Windows Phone Kits\8.1\References\CommonConfiguration\Neutral\Windows.winmd"
)

if (-not (Test-Path $Winmd)) { Write-Host "winmd not found"; exit 1 }

$asm = [System.Reflection.Assembly]::ReflectionOnlyLoadFrom($Winmd)
$asm = [System.Reflection.Assembly]::LoadFrom($Winmd)

$targets = @(
    "Windows.UI.Core.ICoreWindow",
    "Windows.UI.Core.CoreWindow",
    "Windows.UI.Core.ICoreWindowInterop",
    "Windows.UI.Xaml.Controls.ISwapChainPanelNative"
)

foreach ($name in $targets) {
    $t = $asm.GetType($name)
    if ($null -eq $t) { Write-Host ("MISSING  {0}" -f $name); continue }
    $attrs = $t.GetCustomAttributesData()
    $guidAttr = $attrs | Where-Object { $_.AttributeType.Name -like "*Guid*" }
    if ($guidAttr) {
        foreach ($a in $guidAttr) {
            $val = ($a.ConstructorArguments | ForEach-Object { $_.Value }) -join ","
            Write-Host ("GUID     {0}  ->  {1}" -f $name, $val)
        }
    } else {
        Write-Host ("NO-GUID  {0} (attributes: {1})" -f $name,
            (($attrs | ForEach-Object { $_.AttributeType.Name }) -join ", "))
    }
}

# Also dump the interfaces CoreWindow implements, which is what DXGI will QI for.
$cw = $asm.GetType("Windows.UI.Core.CoreWindow")
if ($cw) {
    Write-Host "`nCoreWindow implements:"
    $cw.GetInterfaces() | ForEach-Object { Write-Host ("  - " + $_.FullName) }
}

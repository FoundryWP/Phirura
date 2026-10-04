# Read the real GUIDs of the XAML surface-interop interfaces the WP8.1 projection uses, straight
# out of Windows.winmd via reflection. A hard-coded IID that is wrong is a silent E_NOINTERFACE at
# runtime, so every GUID this port passes to RoGetActivationFactory or QueryInterface is read from
# metadata rather than remembered.
param(
    [string]$Winmd = "C:\Program Files (x86)\Windows Phone Kits\8.1\References\CommonConfiguration\Neutral\Windows.winmd"
)

if (-not (Test-Path $Winmd)) { Write-Host "winmd not found"; exit 1 }

$asm = [System.Reflection.Assembly]::LoadFrom($Winmd)

$targets = @(
    "Windows.UI.Xaml.Media.Imaging.ISurfaceImageSourceFactory",
    "Windows.UI.Xaml.Media.Imaging.SurfaceImageSource",
    "Windows.UI.Xaml.Media.Imaging.ISurfaceImageSource",
    "Windows.UI.Xaml.Media.Imaging.VirtualSurfaceImageSource",
    "Windows.UI.Xaml.Media.Imaging.IVirtualSurfaceImageSourceFactory",
    "Windows.UI.Xaml.Controls.SwapChainPanel",
    "Windows.UI.Xaml.Controls.ISwapChainPanelFactory",
    "Windows.UI.Xaml.Controls.ISwapChainPanelStatics"
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
        Write-Host ("NO-GUID  {0} (attrs: {1})" -f $name,
            (($attrs | ForEach-Object { $_.AttributeType.Name }) -join ", "))
    }
}

Write-Host "`n--- SurfaceImageSource: members ---"
$sis = $asm.GetType("Windows.UI.Xaml.Media.Imaging.SurfaceImageSource")
if ($sis) {
    $sis.GetConstructors() | ForEach-Object {
        Write-Host ("  .ctor(" + (($_.GetParameters() | ForEach-Object { $_.ParameterType.Name + " " + $_.Name }) -join ", ") + ")")
    }
    $sis.GetInterfaces() | ForEach-Object { Write-Host ("  impl: " + $_.FullName) }
}

Write-Host "`n--- ISurfaceImageSourceFactory: methods ---"
$f = $asm.GetType("Windows.UI.Xaml.Media.Imaging.ISurfaceImageSourceFactory")
if ($f) {
    $f.GetMethods() | Where-Object { -not $_.IsSpecialName } | ForEach-Object {
        Write-Host ("  " + $_.ReturnType.Name + " " + $_.Name + "(" +
            (($_.GetParameters() | ForEach-Object { $_.ParameterType.Name + " " + $_.Name }) -join ", ") + ")")
    }
    $f.GetInterfaces() | ForEach-Object { Write-Host ("  impl: " + $_.FullName) }
}

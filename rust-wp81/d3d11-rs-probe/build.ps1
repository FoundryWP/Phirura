# Build the D3D11 Rust probe and report its import table.
#
# This is the feasibility gate for Path A (Rust renders itself). It does not touch the phira
# workspace or the shims: the probe is its own workspace on purpose, so it links against the real
# `windows` crate exactly as a real renderer would.
#
# The output that matters is the LAST section - the import table. Two acceptable answers:
#   * only d3d11.dll / dxgi.dll      -> the binding is directly usable
#   * api-ms-win-*                   -> also fine; map them onto the phone's API sets
#   * kernel32.dll / anything else   -> the binding cannot ship, and needs a compat layer
#
# Two probes are built from `src/`:
#   * `direct.rs` - binds D3D11/DXGI by hand to `d3d11.dll` / `dxgi.dll`. THIS is the one that
#     ships, and the import table below is its.
#   * `lib.rs`    - uses the real `windows` 0.62.2 crate. Kept because it documents what that crate
#     drags in (kernel32, oleaut32, combase, rpcrt4, user32, ntdll), which is why `direct.rs`
#     exists at all.
#
# `wp81-compat` is linked in, exactly as `build-port.ps1` does, so the 31 unresolved std symbols
# (`GetEnvironmentStringsW`, `NtCreateFile`, ...) are supplied the same way they are in the real
# port. Without it the link stops at LNK1120 and says nothing about the import table.

param(
    [switch]$NoDeps,
    [string]$CompatRlib = ""
)

$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path

$vs11   = "C:/Program Files (x86)/Microsoft Visual Studio 11.0"
$wpKit  = "C:/Program Files (x86)/Windows Phone Kits/8.1"
$sdkLib = "C:/Program Files (x86)/Windows Kits/8.1/Lib/winv6.3"

$env:RUSTUP_HOME = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { "C:/Users/admin/.workbuddy-ai/binaries/rust/rustup" }
$env:CARGO_HOME  = if ($env:CARGO_HOME)  { $env:CARGO_HOME }  else { "C:/Users/admin/.workbuddy-ai/binaries/rust/cargo" }
$env:RUSTUP_TOOLCHAIN = "nightly"

$env:PATH = "$vs11\VC\bin;$vs11\Common7\IDE;" + $env:PATH
$env:LIB  = "$vs11\VC\lib\amd64;$sdkLib\um\x64"

$cargo    = Join-Path $env:CARGO_HOME "bin\cargo.exe"
$armLink  = Join-Path $vs11 "VC\bin\x86_arm\link.exe"
$vcLibArm = Join-Path $vs11 "VC\lib\arm"
$wpLibArm = Join-Path $wpKit "lib\ARM"
$chkstk   = Join-Path $vcLibArm "chkstk.obj"
$target   = Join-Path $here "..\targets\thumbv7a-pc-windows-msvc.json"
$outDir   = Join-Path $here "out"
$dll      = Join-Path $outDir "wp81d3d11rs.dll"
$py       = "C:/Users/admin/.workbuddy-ai/binaries/python/versions/3.13.12/python.exe"

Write-Host "=== 1. cargo build (staticlib) ==="
Push-Location $here
$env:MSYS_NO_PATHCONV = "1"
$env:MSYS2_ARG_CONV_EXCL = "*"
# Every -Z argument containing a comma MUST be quoted. Unquoted, PowerShell parses `std,panic_abort`
# as an array literal and fails to parse the whole script, so nothing runs at all - the same trap
# build-port.ps1 documents.
$cargoArgs = @(
    "build",
    "--lib",
    "--target", $target,
    "-Zjson-target-spec",
    "-Zbuild-std=std,panic_abort",
    "-Zbuild-std-features=compiler-builtins-mem",
    "--release"
)
# cargo writes its progress and its "Finished" line to *stderr*, and with $ErrorActionPreference set
# to Stop a native command's stderr output is turned into a terminating NativeCommandError. The same
# guard `build-port.ps1` uses: drop to Continue around the call and read $LASTEXITCODE instead.
$savedEap = $ErrorActionPreference
$ErrorActionPreference = "Continue"
& $cargo @cargoArgs *> (Join-Path $here "cargo.log")
$code = $LASTEXITCODE
$ErrorActionPreference = $savedEap
Pop-Location
if ($code -ne 0) {
    Get-Content (Join-Path $here "cargo.log") -Tail 30
    throw "cargo build failed ($code)"
}

$staticlib = Join-Path $here "target\thumbv7a-pc-windows-msvc\release\d3d11_rs_probe.lib"
if (-not (Test-Path $staticlib)) { throw "no staticlib at $staticlib" }
Write-Host "staticlib = $((Get-Item $staticlib).Length) bytes"

# Locate the compat rlib the phira workspace already built. It is a build-script-style artifact,
# so the path carries a content hash: `build/wp81-compat/<hash>/out/libwp81_compat-<hash>.rlib`.
if (-not $CompatRlib) {
    $compatRoot = "E:/Users/admin/Documents/visual studio 2013/Phirura/phira-main/phira-main/target/thumbv7a-pc-windows-msvc/release/build/wp81-compat"
    $found = Get-ChildItem -Path $compatRoot -Recurse -Filter "libwp81_compat-*.rlib" -ErrorAction SilentlyContinue |
             Select-Object -First 1
    if ($found) { $CompatRlib = $found.FullName }
}
if (-not $CompatRlib) {
    throw "cannot find the wp81-compat rlib; build the port first (build-port.ps1) or pass -CompatRlib"
}
Write-Host "compat rlib = $CompatRlib"

Write-Host "`n=== 2. link ARMNT DLL ==="
New-Item -ItemType Directory -Force -Path $outDir | Out-Null

$linkArgs = @(
    "/DLL", "/NODEFAULTLIB", "/MACHINE:ARM", "/SUBSYSTEM:WINDOWS",
    "/ENTRY:_DllMainCRTStartup",
    "/FORCE:MULTIPLE",
    "/OUT:$dll",
    "/LIBPATH:$vcLibArm",
    "/LIBPATH:$wpLibArm",
    $staticlib,
    $CompatRlib,
    $chkstk,
    "d3d11.lib",
    "dxgi.lib",
    "d3dcompiler.lib",
    "WindowsPhoneCore.lib",
    "ws2_32.lib",
    # `runtimeobject.lib` is the WP8.1 kit's WinRT activation library and the *only* sanctioned way
    # for an app to reach the WinRT runtime. It imports from `api-ms-win-core-winrt-l1-1-0.dll`
    # (RoActivateInstance / RoGetActivationFactory), `...-string-l1-1-0.dll` (WindowsCreateString)
    # and `...-error-l1-1-0.dll` - all API sets an app container is allowed to load, which is
    # exactly why Win2D's own ARM DLL imports `RoGetActivationFactory` from that same API set.
    # This replaces the `LoadLibraryExW("winrt.dll")` attempt, which is provably impossible here:
    # `winrt.dll` cannot be loaded from an app container (handle 0 on device) and `d3d11.dll` does
    # not export the Direct3D interop helpers either.
    "runtimeobject.lib",
    "/EXPORT:wp81_d3d11_rs_probe",
    "/EXPORT:wp81_d3d11_rs_report",
    "/EXPORT:wp81_rs_start",
    "/EXPORT:wp81_rs_start_upto",
    "/EXPORT:wp81_rs_clear",
    "/EXPORT:wp81_rs_draw_test_quad",
    "/EXPORT:wp81_rs_present",
    "/EXPORT:wp81_rs_stop",
    "/EXPORT:wp81_rs_stage",
    "/EXPORT:wp81_rs_last_hr",
    "/EXPORT:wp81_rs_last_hr_name",
    "/EXPORT:wp81_rs_vertex_count",
    "/EXPORT:wp81_rs_sc_step",
    "/EXPORT:wp81_rs_sc_step_name",
    "/EXPORT:wp81_rs_sc_variant",
    "/EXPORT:wp81_rs_sc_variant_name",
    "/EXPORT:wp81_rs_tick",
    "/EXPORT:wp81_rs_tick_name",
    "/EXPORT:wp81_rs_reached",
    "/EXPORT:wp81_rs_vt_count",
    "/EXPORT:wp81_rs_vt_entry",
    "/EXPORT:wp81_rs_factory_is_current",
    "/EXPORT:wp81_rs_factory_qi_hr",
    "/EXPORT:wp81_rs_panel_iface",
    "/EXPORT:wp81_rs_sc_hr",
    "/EXPORT:wp81_rs_start_corewindow",
    "/EXPORT:wp81_rs_attach_panel",
    "/EXPORT:wp81_rs_corewindow_vtbl",
    "/EXPORT:wp81_rs_corewindow_obj",
    "/EXPORT:wp81_rs_corewindow_qi",
    "/EXPORT:wp81_rs_cw_factory_stereo",
    "/EXPORT:wp81_rs_start_offscreen",
    "/EXPORT:wp81_rs_gfx_surface",
    "/EXPORT:wp81_rs_gfx_device",
    "/EXPORT:wp81_rs_route",
    "/EXPORT:wp81_rs_ot_step",
    "/EXPORT:wp81_rs_ot_step_name",
    "/EXPORT:wp81_rs_ot_surface_kind",
    "/EXPORT:wp81_rs_interop_source",
    "/EXPORT:wp81_rs_interop_trace",
    "/EXPORT:wp81_rs_start_surface",
    # The split halves of the route-3 bring-up. `SurfaceImageSource` is a XAML object, so creating
    # it (and SetDevice) must run on the UI thread, while the device+pipeline half belongs on a
    # pool thread where the UI thread survives to flush the log if a driver call faults.
    "/EXPORT:wp81_rs_start_surface_prepare",
    "/EXPORT:wp81_rs_start_surface_attach",
    "/EXPORT:wp81_rs_sis_source",
    "/EXPORT:wp81_rs_sis_diag",
    "/EXPORT:wp81_rs_sis_step",
    "/EXPORT:wp81_rs_sis_step_name",
    "/EXPORT:wp81_rs_sis_hr"
)
$savedEap2 = $ErrorActionPreference
$ErrorActionPreference = "Continue"
& $armLink @linkArgs *> (Join-Path $here "link.log")
$linkCode = $LASTEXITCODE
$ErrorActionPreference = $savedEap2
if ($linkCode -ne 0) {
    Get-Content (Join-Path $here "link.log") | Select-String -Pattern "error|LNK1120" | Select-Object -Last 15
    throw "link failed ($linkCode)"
}
Write-Host "link OK - $((Get-Item $dll).Length) bytes"

Write-Host "`n=== 3. import table (THE ANSWER) ==="
& $py (Join-Path $here "..\tools\check_imports.py") $dll
Write-Host "check_imports exit = $LASTEXITCODE  (0 = shippable; 1 = a library the phone does not have)"

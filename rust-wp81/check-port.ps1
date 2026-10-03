# Fast type-check of prpr-port for the WP8.1 target.
#
# The full build (build-port.ps1) compiles then links; this only runs cargo check, which is what you
# want while iterating on the host - it surfaces API errors in about a minute instead of five.
#
# The comma inside `-Zbuild-std=std,panic_abort` MUST be inside a quoted element. Unquoted,
# PowerShell parses it as an array literal and fails before running anything at all.
$ErrorActionPreference = "Continue"

$env:RUSTUP_HOME = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { "C:/Users/admin/.workbuddy-ai/binaries/rust/rustup" }
$env:CARGO_HOME  = if ($env:CARGO_HOME)  { $env:CARGO_HOME }  else { "C:/Users/admin/.workbuddy-ai/binaries/rust/cargo" }
$env:RUSTUP_TOOLCHAIN = "nightly"

$vs11 = "C:/Program Files (x86)/Microsoft Visual Studio 11.0"
$env:PATH = "$vs11\VC\bin;$vs11\Common7\IDE;" + $env:PATH
$env:LIB  = "$vs11\VC\lib\amd64;C:/Program Files (x86)/Windows Kits/8.1/Lib/winv6.3/um/x64"

$ws     = "E:/Users/admin/Documents/visual studio 2013/Phirura/phira-main/phira-main"
$target = "E:/Users/admin/Documents/visual studio 2013/Phirura/rust-wp81/targets/thumbv7a-pc-windows-msvc.json"
$log    = "E:/Users/admin/Documents/visual studio 2013/Phirura/rust-wp81/check-port.log"

Set-Location $ws

$cargo = Join-Path $env:CARGO_HOME "bin\cargo.exe"
$args = @(
    "check", "-p", "prpr-port",
    "--target", $target,
    "-Zjson-target-spec",
    "-Zbuild-std=std,panic_abort",
    "-Zbuild-std-features=compiler-builtins-mem"
)
& $cargo @args 2>&1 | Out-File -FilePath $log -Encoding utf8
Write-Host "cargo check exit: $LASTEXITCODE"
Get-Content $log -ErrorAction SilentlyContinue | Select-String -Pattern "^error|^warning: unused|-->" | Select-Object -First 60

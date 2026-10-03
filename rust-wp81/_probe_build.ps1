param()
$ErrorActionPreference = "Continue"
$env:RUSTUP_HOME = "C:/Users/admin/.workbuddy-ai/binaries/rust/rustup"
$env:CARGO_HOME  = "C:/Users/admin/.workbuddy-ai/binaries/rust/cargo"
$env:RUSTUP_TOOLCHAIN = "nightly"
$vs11 = "C:/Program Files (x86)/Microsoft Visual Studio 11.0"
$env:PATH = "$vs11\VC\bin;$vs11\Common7\IDE;" + $env:PATH
$env:LIB  = "$vs11\VC\lib\amd64;C:/Program Files (x86)/Windows Kits/8.1/Lib/winv6.3/um/x64"
$cargo = Join-Path $env:CARGO_HOME "bin\cargo.exe"
$ws = "E:/Users/admin/Documents/visual studio 2013/Phirura/phira-main/phira-main"
$target = "E:/Users/admin/Documents/visual studio 2013/Phirura/rust-wp81/targets/thumbv7a-pc-windows-msvc.json"
Set-Location $ws
$cargoArgs = @(
    "build", "-p", "prpr-port",
    "--target", $target,
    "-Zjson-target-spec",
    "-Zbuild-std=std,panic_abort",
    "-Zbuild-std-features=compiler-builtins-mem",
    "--release", "-v"
)
& $cargo @cargoArgs 2>&1 | Out-File "E:/Users/admin/Documents/visual studio 2013/Phirura/rust-wp81/port-verbose.log" -Encoding utf8
"exit=$LASTEXITCODE"
Select-String -Path "E:/Users/admin/Documents/visual studio 2013/Phirura/rust-wp81/port-verbose.log" -Pattern "which|getrandom" | Select-Object -First 30 | ForEach-Object { $_.Line }

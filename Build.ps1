param([ValidateSet('build','release','check','test','lint','fmt','fetch')][string]$Task = 'release')
$ErrorActionPreference = 'Stop'
Set-Location -LiteralPath $PSScriptRoot
$env:CARGO_HOME = Join-Path $PSScriptRoot '.build/cargo'
$env:CARGO_TARGET_DIR = Join-Path $PSScriptRoot '.build/target'
$env:TEMP = Join-Path $PSScriptRoot '.build/tmp'
$env:TMP = $env:TEMP
New-Item -ItemType Directory -Force -Path $env:CARGO_HOME,$env:CARGO_TARGET_DIR,$env:TEMP | Out-Null
if ($env:OS -eq 'Windows_NT') {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path -LiteralPath $vswhere)) { throw 'Install Visual Studio Build Tools with Desktop development with C++.' }
    $vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    $msvc = Get-ChildItem -LiteralPath (Join-Path $vs 'VC\Tools\MSVC') -Directory | Sort-Object Name -Descending | Select-Object -First 1
    $env:PATH = "$(Join-Path $msvc.FullName 'bin\Hostx64\x64');$env:PATH"
    $env:LIB = "$(Join-Path $msvc.FullName 'lib\x64');$env:LIB"
    # Optional relocated SDK, otherwise find the standard installed Windows SDK.
    $sdk = $env:OMNIDOWNLOADER_WINDOWS_SDK
    if (-not $sdk) {
        $sdkRoot = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\Lib'
        $version = Get-ChildItem -LiteralPath $sdkRoot -Directory -ErrorAction SilentlyContinue | Sort-Object Name -Descending | Select-Object -First 1
        if ($version) { $sdk = $version.FullName }
    }
    if (-not $sdk) { throw 'Windows SDK libraries missing. Set OMNIDOWNLOADER_WINDOWS_SDK to a directory containing um and ucrt libraries.' }
    $um = Join-Path $sdk 'um\x64'; $ucrt = Join-Path $sdk 'ucrt\x64'
    if (-not (Test-Path -LiteralPath $um)) { $um = Join-Path $sdk 'um'; $ucrt = Join-Path $sdk 'ucrt' }
    $env:LIB = "$um;$ucrt;$env:LIB"
    # Use Windows' system SQLite; the import library still targets winsqlite3.dll.
    $sqlite = Join-Path $PSScriptRoot '.build\sqlite'
    New-Item -ItemType Directory -Force -Path $sqlite | Out-Null
    Copy-Item -LiteralPath (Join-Path $um 'winsqlite3.lib') -Destination (Join-Path $sqlite 'sqlite3.lib') -Force
    $env:SQLITE3_LIB_DIR = $sqlite
}
switch ($Task) {
    fetch { & cargo fetch }
    check { & cargo check --locked --workspace }
    build { & cargo build --locked --workspace }
    test { & cargo test --locked --workspace }
    lint { & cargo clippy --locked --workspace --all-targets -- -D warnings }
    fmt { & cargo fmt --all -- --check }
    release { & cargo build --locked --release --workspace }
}
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
if ($Task -in @('release','build')) {
    $configuration = if ($Task -eq 'release') { 'release' } else { 'debug' }
    New-Item -ItemType Directory -Force -Path (Join-Path $PSScriptRoot 'dist') | Out-Null
    $binary = if ($env:OS -eq 'Windows_NT') { 'omnidownloader.exe' } else { 'omnidownloader' }
    Copy-Item -LiteralPath (Join-Path $env:CARGO_TARGET_DIR "$configuration/$binary") -Destination (Join-Path $PSScriptRoot "dist/$binary") -Force
}

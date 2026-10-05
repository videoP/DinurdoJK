[CmdletBinding()]
param([switch]$Build)
$ErrorActionPreference = 'Stop'
$gammaRepo = Split-Path -Parent $PSScriptRoot
Push-Location $gammaRepo
try {
    if ($Build) { & (Join-Path $gammaRepo 'build.ps1') -Fast }
    $gammaToolchain = Join-Path $env:USERPROFILE '.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin\rustc.exe'
    if (-not (Test-Path -LiteralPath $gammaToolchain)) { $gammaToolchain = 'rustc' }
    $gammaWindows = Get-ChildItem target/release/deps/libwindows_sys-*.rlib |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
    $gammaJson = Get-ChildItem target/release/deps/libserde_json-*.rlib |
        Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $gammaWindows -or -not $gammaJson) {
        throw 'Build the client with build.ps1 -Fast first, or run this script with -Build.'
    }
    $gammaOutput = Join-Path $gammaRepo 'target/gamma-audit'
    New-Item -ItemType Directory -Path $gammaOutput -Force | Out-Null
    $gammaSource = (Join-Path $gammaRepo 'crates/jka-client/src').Replace('\', '/')
    $gammaPrefix = @"
#![allow(dead_code, unused_imports)]
#[path = "$gammaSource/gamma.rs"] mod gamma;
#[path = "$gammaSource/display_gamma.rs"] mod display_gamma;
"@
    $gammaUnitSource = Join-Path $gammaOutput 'tests.rs'
    $gammaProcessSource = Join-Path $gammaOutput 'process-tests.rs'
    [System.IO.File]::WriteAllText($gammaUnitSource, $gammaPrefix)
    $gammaProcessMain = 'fn main() -> std::process::ExitCode { std::process::ExitCode::from(display_gamma::test_process_entry().unwrap_or(2) as u8) }'
    [System.IO.File]::WriteAllText($gammaProcessSource, $gammaPrefix + "`n" + $gammaProcessMain)
    $gammaArguments = @('--edition=2021', '-C', 'opt-level=2', '-L', 'dependency=target/release/deps',
        '--extern', "windows_sys=$($gammaWindows.FullName)", '--extern', "serde_json=$($gammaJson.FullName)")
    $gammaUnitExe = Join-Path $gammaOutput 'tests.exe'
    & $gammaToolchain @gammaArguments --test $gammaUnitSource -o $gammaUnitExe
    if ($LASTEXITCODE -ne 0) { throw 'Gamma unit test compilation failed.' }
    & $gammaUnitExe --test-threads=1
    if ($LASTEXITCODE -ne 0) { throw 'Gamma unit tests failed.' }
    $gammaProcessExe = Join-Path $gammaOutput 'process-tests.exe'
    & $gammaToolchain @gammaArguments --cfg test $gammaProcessSource -o $gammaProcessExe
    if ($LASTEXITCODE -ne 0) { throw 'Gamma process test compilation failed.' }
    & $gammaProcessExe --process-driver
    if ($LASTEXITCODE -ne 0) { throw 'Gamma process tests failed.' }
} finally {
    Pop-Location
}

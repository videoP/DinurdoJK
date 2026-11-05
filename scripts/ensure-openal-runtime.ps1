function Ensure-OpenALRuntime([string]$WorkspaceRoot) {
    $ErrorActionPreference = 'Stop'
    # One native build tree, shared by Cargo's fast and release outputs.
    $nativeBuild = Join-Path $WorkspaceRoot 'target\fast\build\openal-soft'
    $runtimeDll = Join-Path $WorkspaceRoot 'target\fast\OpenALSoft.x86_64.dll'
    $nativeSource = Join-Path $WorkspaceRoot 'vendor\openal-soft'
    $inputs = @('CMakeLists.txt', 'openal-soft-1.25.2.tar.gz') | ForEach-Object { Join-Path $nativeSource $_ }
    $inputHash = ($inputs | ForEach-Object { (Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash }) -join ':'
    $stamp = Join-Path $nativeBuild 'runtime-inputs.sha256'
    $needsBuild = -not (Test-Path -LiteralPath $runtimeDll)
    if (-not $needsBuild) {
        $runtime = Get-Item -LiteralPath $runtimeDll
        $needsBuild = $runtime.VersionInfo.FileVersion -ne '1.25.2' -or
            -not (Test-Path -LiteralPath $stamp) -or
            (Get-Content -Raw -LiteralPath $stamp).Trim() -ne $inputHash
    }
    foreach ($name in @('msvcp140.dll', 'msvcp140_atomic_wait.dll', 'vcruntime140.dll', 'vcruntime140_1.dll', 'licenses\OpenAL-Soft-LGPL-2.0.txt')) {
        if (-not (Test-Path -LiteralPath (Join-Path $WorkspaceRoot "target\fast\$name"))) { $needsBuild = $true }
    }
    if ($needsBuild) {
        & cmake -S $nativeSource -B $nativeBuild -G 'Visual Studio 17 2022' -A x64 | Out-Host
        if ($LASTEXITCODE -ne 0) { throw 'OpenAL Soft configuration failed.' }
        & cmake --build $nativeBuild --config Release --target OpenAL --parallel 4 | Out-Host
        if ($LASTEXITCODE -ne 0) { throw 'OpenAL Soft build failed.' }
        Set-Content -LiteralPath $stamp -Value $inputHash -Encoding ascii
    }
    return $runtimeDll
}

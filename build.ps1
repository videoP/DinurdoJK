[CmdletBinding()]
param(
    [ValidateSet('Debug', 'Release')][string]$Configuration = 'Release',
    [switch]$Check,
    [switch]$Fetch,
    [switch]$Fast
)
$ErrorActionPreference = 'Stop'
$stableCargoPath = Join-Path $env:USERPROFILE '.rustup\toolchains\stable-x86_64-pc-windows-msvc\bin\cargo.exe'
$cargoCommand = Get-Command cargo -ErrorAction SilentlyContinue
if (Test-Path -LiteralPath $stableCargoPath) {
    $cargoPath = $stableCargoPath
} elseif ($cargoCommand) {
    $cargoPath = $cargoCommand.Source
} else {
    $cargoPath = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
    if (-not (Test-Path -LiteralPath $cargoPath)) {
        throw 'Rust was not found. Install Rust with rustup, then run this script again.'
    }
}
# Keep local absolute paths (user name, source tree) out of the shipped binary.
# rustc bakes the full path of every source file into panic messages via file!(),
# which otherwise exposes the build machine's user profile in crash reports.
$remapRoots = [System.Collections.Generic.List[string[]]]::new()
# Least specific first: rustc applies the LAST matching rule.
if ($env:USERPROFILE) { $remapRoots.Add(@($env:USERPROFILE, 'home')) }
if ($env:CARGO_HOME)  { $remapRoots.Add(@($env:CARGO_HOME,  'cargo-home')) }
if ($env:RUSTUP_HOME) { $remapRoots.Add(@($env:RUSTUP_HOME, 'rustup-home')) }
$remapRoots.Add(@($PSScriptRoot, 'jka'))

$remapFlags = @()
foreach ($root in $remapRoots) {
    $from = $root[0].TrimEnd('\', '/')
    if ($from) { $remapFlags += "--remap-path-prefix=$from=$($root[1])" }
}
# Drop the absolute .pdb path that the linker records in the executable's debug directory.
$remapFlags += '-Clink-arg=/PDBALTPATH:%_PDB%'

# CARGO_ENCODED_RUSTFLAGS is 0x1f-separated, so flags may contain spaces. Plain
# RUSTFLAGS is split on whitespace and would mangle a source path like
# "D:\Code\Jedi Knight Rust". Setting the encoded form makes cargo ignore RUSTFLAGS,
# so fold any pre-existing value in first.
$existingRustFlags = $env:RUSTFLAGS
$existingEncoded = $env:CARGO_ENCODED_RUSTFLAGS
$separator = [char]0x1f
$inherited = @()
if ($existingEncoded) {
    $inherited = $existingEncoded.Split($separator) | Where-Object { $_ }
} elseif ($existingRustFlags) {
    $inherited = $existingRustFlags.Split(' ') | Where-Object { $_ }
}
$env:CARGO_ENCODED_RUSTFLAGS = ($inherited + $remapFlags) -join $separator

# Call the toolchain binaries directly so build scripts that shell out to `rustc`
# skip the rustup proxy (source of the "could not canonicalize path" warnings).
$toolchainBin = Split-Path -Parent $cargoPath
$existingPath = $env:Path
if ($toolchainBin -and (Test-Path -LiteralPath (Join-Path $toolchainBin 'rustc.exe'))) {
    $env:Path = "$toolchainBin;$env:Path"
}

# Build modes ---------------------------------------------------------------
# (no flag)  Cargo 'release' profile: fat LTO, codegen-units=1. Use it for final FPS
#            numbers and shipping. A one-line change costs ~6.5 min (the LTO step).
# -Fast      Cargo 'fast' profile: ThinLTO with persistent incremental compilation.
#            Keeps cached local/path crate outputs for faster small edits.
# The fast profile keeps its own target\fast cache (see Cargo.toml), so switching
# modes never recompiles every dependency. -Fast copies the exe, .pdb and
# runtime DLLs into target\release, where the base folder lives, so the game is
# always run from target\release\DinurdoJK.exe. DinurdoJK.exe.mode records which
# mode produced the exe currently there.
if ($Fast -and $Configuration -eq 'Debug') {
    throw '-Fast is an optimized profile; omit -Configuration Debug.'
}
if ($Fast) {
    $buildMode = 'fast'
    $profileArguments = @('--profile', 'fast')
} elseif ($Configuration -eq 'Release') {
    $buildMode = 'release'
    $profileArguments = @('--release')
} else {
    $buildMode = 'debug'
    $profileArguments = @()
}
$buildDirectory = Join-Path $PSScriptRoot "target\$buildMode"
$runtimeDirectory = Join-Path $PSScriptRoot "target\$($Configuration.ToLowerInvariant())"
$executable = Join-Path $runtimeDirectory 'DinurdoJK.exe'
$previousExecutable = "$executable.prev"

# Cache reporter is read-only; it never deletes or truncates build artifacts.
. (Join-Path $PSScriptRoot 'scripts\build-cache-ntfs.ps1')
. (Join-Path $PSScriptRoot 'scripts\build-cache-report.ps1')

# Restore upstream SMAA 0.21 with WGPU default backends disabled.
. (Join-Path $PSScriptRoot 'scripts\ensure-smaa-vendor.ps1')
. (Join-Path $PSScriptRoot 'scripts\ensure-openal-runtime.ps1')

# Move the live executable aside (Windows allows renaming an exe that is running)
# so a game that is already open keeps using the old image while the new
# DinurdoJK.exe is written beside it.
function Preserve-RunningExecutable {
    if (-not (Test-Path -LiteralPath $executable)) { return }

    # Clean up old backups once their game processes have exited. Locked
    # executables are kept and never have to be deleted for a build to succeed.
    Get-ChildItem -LiteralPath $runtimeDirectory -Filter 'DinurdoJK.exe.prev.inuse-*' -File -ErrorAction SilentlyContinue | ForEach-Object {
        try { Remove-Item -LiteralPath $_.FullName -Force -ErrorAction Stop } catch { }
    }

    # Move-Item -Force does not replace an existing .prev on Windows.
    # An in-use older backup gets a unique name, so we can preserve the
    # current executable as .prev without breaking the running game.
    if (Test-Path -LiteralPath $previousExecutable) {
        try {
            Remove-Item -LiteralPath $previousExecutable -Force -ErrorAction Stop
        } catch {
            $parked = "$previousExecutable.inuse-$([guid]::NewGuid().ToString('N'))"
            Move-Item -LiteralPath $previousExecutable -Destination $parked -ErrorAction Stop
            Write-Host "Older executable still in use; preserved: $parked" -ForegroundColor DarkYellow
        }
    }
    Move-Item -LiteralPath $executable -Destination $previousExecutable -ErrorAction Stop
    Write-Host "Preserved current executable: $previousExecutable"
}

# Copy a build output into the runtime folder unless an identical file is already
# there. A file the running game holds open (phonon.dll, or the .pdb under a
# debugger) is reported and skipped instead of failing the build.
function Sync-RuntimeFile([string]$Source) {
    if (-not (Test-Path -LiteralPath $Source)) { return }
    $destination = Join-Path $runtimeDirectory (Split-Path -Leaf $Source)
    if ((Test-Path -LiteralPath $destination) -and
        (Get-FileHash -LiteralPath $Source).Hash -eq (Get-FileHash -LiteralPath $destination).Hash) {
        return
    }
    try {
        Copy-Item -LiteralPath $Source -Destination $destination -Force -ErrorAction Stop
    } catch {
        Write-Warning "Could not update $destination (in use?): $($_.Exception.Message)"
    }
}

# Preserve identical support files: Copy-Item -Force always rewrites them.
function Sync-IfDifferent([string]$Source, [string]$Destination) {
    if (-not (Test-Path -LiteralPath $Source)) { throw "Missing build support file: $Source" }
    if (Test-Path -LiteralPath $Destination) {
        $sourceInfo = Get-Item -LiteralPath $Source
        $destinationInfo = Get-Item -LiteralPath $Destination
        if ($sourceInfo.Length -eq $destinationInfo.Length -and
            (Get-FileHash -LiteralPath $Source).Hash -eq (Get-FileHash -LiteralPath $Destination).Hash) {
            return
        }
    }
    Copy-Item -LiteralPath $Source -Destination $Destination -Force
}

# The fast profile must be incremental even when an inherited Cargo setting
# disables it. Restore the caller's environment after the build.
$previousCargoIncremental = $env:CARGO_INCREMENTAL
if ($Fast) { $env:CARGO_INCREMENTAL = '1' }
$cacheReport = New-CargoArtifactReport
$cacheSnapshot = $null
Push-Location $PSScriptRoot
try {
    $cacheSnapshot = Get-BuildOutputSnapshot -BuildDirectory $buildDirectory -RuntimeDirectory $runtimeDirectory
    Ensure-PatchedSmaaVendor -WorkspaceRoot $PSScriptRoot
    $openalRuntime = Ensure-OpenALRuntime -WorkspaceRoot $PSScriptRoot
    New-Item -ItemType Directory -Force -Path $runtimeDirectory | Out-Null
    Sync-RuntimeFile $openalRuntime
    foreach ($name in @('msvcp140.dll', 'msvcp140_atomic_wait.dll', 'vcruntime140.dll', 'vcruntime140_1.dll')) {
        Sync-RuntimeFile (Join-Path $PSScriptRoot "target\fast\$name")
    }
    $runtimeLicenses = Join-Path $runtimeDirectory 'licenses'
    New-Item -ItemType Directory -Force -Path $runtimeLicenses | Out-Null
    # These licenses do not change between normal builds. Avoid rewriting them.
    Sync-IfDifferent -Source (Join-Path $PSScriptRoot 'target\fast\licenses\OpenAL-Soft-LGPL-2.0.txt') -Destination (Join-Path $runtimeLicenses 'OpenAL-Soft-LGPL-2.0.txt')
    Sync-IfDifferent -Source (Join-Path $PSScriptRoot 'vendor\ealman\LICENSE') -Destination (Join-Path $runtimeLicenses 'EalMan-LGPL-2.1.txt')
    Sync-IfDifferent -Source (Join-Path $PSScriptRoot 'vendor\bevy-solari-ris\LICENSE-MIT') -Destination (Join-Path $runtimeLicenses 'Bevy-Solari-MIT.txt')
    # Refresh the dependency graph once when upgrading an older checkout.
    $lockfile = Join-Path $PSScriptRoot 'Cargo.lock'
    $lockText = if (Test-Path -LiteralPath $lockfile) { [System.IO.File]::ReadAllText($lockfile) } else { '' }
    $staleGraphicsLock = (-not $lockText) -or
        ($lockText -match '(?m)^name = "wgpu"\r?\nversion = "29\.') -or
        ($lockText -match '(?m)^name = "smaa"\r?\nversion = "0\.20\.0"')
    if ($Fetch -or $staleGraphicsLock) {
        & $cargoPath fetch
        if ($LASTEXITCODE -ne 0) { throw 'Dependency download failed.' }
    }
    if ($Check) {
        & $cargoPath fmt --all -- --check
        if ($LASTEXITCODE -ne 0) { throw 'Formatting check failed.' }
        & $cargoPath test --workspace --offline --locked @profileArguments
        if ($LASTEXITCODE -ne 0) { throw 'Tests failed.' }
        & $cargoPath clippy --workspace --all-targets --offline --locked @profileArguments -- -D warnings
        if ($LASTEXITCODE -ne 0) { throw 'Clippy failed.' }
    }
    # Release/Debug link straight into the runtime folder, so make room first.
    # -Fast builds elsewhere and replaces the exe only after a successful build.
    $buildsInPlace = $buildDirectory -eq $runtimeDirectory
    if ($buildsInPlace) { Preserve-RunningExecutable }
    # Keep JSON artifact accounting, but let Cargo render rustc diagnostics to its
    # own stderr. The previous plain-JSON rendering lost the normal error/warning
    # colors when PowerShell reprinted it. stderr is not part of this pipeline.
    $cargoArguments = @('build', '--package', 'DinurdoJK', '--offline', '--locked') + $profileArguments + @('--message-format=json,json-render-diagnostics')
    & $cargoPath @cargoArguments | ForEach-Object {
        $line = [string]$_
        if (-not $line.StartsWith('{')) { Write-Host $line; return }
        try { $msg = $line | ConvertFrom-Json -ErrorAction Stop } catch {
            Write-Host $line
            return
        }
        switch ($msg.reason) {
            'compiler-artifact' {
                # Reporting must never turn a successful Cargo build into a failed one.
                try {
                    Add-CargoArtifactMessage -Report $cacheReport -Message $msg
                } catch {
                    Write-Warning "Could not process Cargo artifact metadata: $($_.Exception.Message)"
                }
            }
            # json-render-diagnostics already prints diagnostics via Cargo's stderr;
            # never echo the JSON copies or warnings/errors would appear twice.
            'compiler-message' { }
            'build-finished' { }
            'build-script-executed' { }
            default { Write-Host $line }
        }
    }
    $cargoExitCode = $LASTEXITCODE
    if ($cargoExitCode -ne 0) {
        if ($buildsInPlace -and -not (Test-Path -LiteralPath $executable) -and (Test-Path -LiteralPath $previousExecutable)) {
            Move-Item -LiteralPath $previousExecutable -Destination $executable
            Write-Host 'Build failed; restored the previous executable.'
        }
        throw 'Build failed. Check the Rust and MSVC toolchain messages above.'
    }
    if (-not $buildsInPlace) {
        New-Item -ItemType Directory -Force -Path $runtimeDirectory | Out-Null
        $builtExecutable = Join-Path $buildDirectory 'DinurdoJK.exe'
        # An unchanged rebuild must not overwrite .prev with a copy of the same exe.
        $unchanged = (Test-Path -LiteralPath $executable) -and
            (Get-FileHash -LiteralPath $builtExecutable).Hash -eq (Get-FileHash -LiteralPath $executable).Hash
        if (-not $unchanged) {
            Preserve-RunningExecutable
            Copy-Item -LiteralPath $builtExecutable -Destination $executable -Force
        }
        Sync-RuntimeFile (Join-Path $buildDirectory 'DinurdoJK.pdb')
        Get-ChildItem -LiteralPath $buildDirectory -Filter '*.dll' | ForEach-Object { Sync-RuntimeFile $_.FullName }
    }
    Set-Content -LiteralPath "$executable.mode" -Value "$buildMode $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')" -Encoding ascii
    $runtimeBase = Join-Path $runtimeDirectory 'base'
    New-Item -ItemType Directory -Force -Path $runtimeBase | Out-Null
    $assetPackage = Join-Path $PSScriptRoot 'assets\dinurdo-assets.pk3'
    $runtimeAssetPackage = Join-Path $runtimeBase 'dinurdo-assets.pk3'
    if (-not (Test-Path -LiteralPath $runtimeAssetPackage) -or
        (Get-FileHash -LiteralPath $assetPackage).Hash -ne
        (Get-FileHash -LiteralPath $runtimeAssetPackage).Hash) {
        Copy-Item -LiteralPath $assetPackage -Destination $runtimeAssetPackage -Force
    }
    $shaderModes = Join-Path $PSScriptRoot 'scripts\shader_modes_ab.cfg'
    $runtimeShaderModes = Join-Path $runtimeBase 'shader_modes_ab.cfg'
    if (-not (Test-Path -LiteralPath $runtimeShaderModes) -or
        (Get-FileHash -LiteralPath $shaderModes).Hash -ne
        (Get-FileHash -LiteralPath $runtimeShaderModes).Hash) {
        Copy-Item -LiteralPath $shaderModes -Destination $runtimeShaderModes -Force
    }
    Write-Host "`nBuilt ($buildMode): $executable"
    if ($Fast) { Write-Host '(-Fast: ThinLTO; persistent incremental compilation enabled)' }
} finally {
    if ($null -ne $cacheSnapshot) {
        try {
            Show-BuildCacheReport -BuildDirectory $buildDirectory -RuntimeDirectory $runtimeDirectory -Before $cacheSnapshot -Artifacts $cacheReport
        } catch {
            Write-Warning "Build-cache reporting failed (build artifacts are unaffected): $($_.Exception.Message)"
        }
    }
    Pop-Location
    $env:CARGO_ENCODED_RUSTFLAGS = $existingEncoded
    $env:Path = $existingPath
    $env:CARGO_INCREMENTAL = $previousCargoIncremental
}

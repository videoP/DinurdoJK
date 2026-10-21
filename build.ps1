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
# -Fast      Cargo 'fast' profile: ThinLTO + incremental. ~1 min per change and
#            about 2% below release FPS; fine for day-to-day FPS checks.
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

# Restore upstream SMAA 0.21 with WGPU default backends disabled.
. (Join-Path $PSScriptRoot 'scripts\ensure-smaa-vendor.ps1')

# Move the live executable aside (Windows allows renaming an exe that is running)
# so a game that is already open keeps using the old image while the new
# DinurdoJK.exe is written beside it.
function Preserve-RunningExecutable {
    if (Test-Path -LiteralPath $executable) {
        Move-Item -LiteralPath $executable -Destination $previousExecutable -Force
        Write-Host "Preserved current executable: $previousExecutable"
    }
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

Push-Location $PSScriptRoot
try {
    Ensure-PatchedSmaaVendor -WorkspaceRoot $PSScriptRoot
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
    $cargoArguments = @('build', '--workspace', '--offline', '--locked') + $profileArguments
    & $cargoPath @cargoArguments
    if ($LASTEXITCODE -ne 0) {
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
    $rainGlassPackagePath = Join-Path $PSScriptRoot 'assets\dinurdo-assets.pk3'
    if (-not (Test-Path -LiteralPath $rainGlassPackagePath)) {
        throw "RainyGlass package is missing: $rainGlassPackagePath"
    }
    $runtimeBase = Join-Path $runtimeDirectory 'base'
    New-Item -ItemType Directory -Force -Path $runtimeBase | Out-Null
    $runtimeRainGlassPackagePath = Join-Path $runtimeBase 'dinurdo-assets.pk3'
    # As with runtime DLLs, an identical package needs no write (and may be
    # held open by the running game or shared with other installs via hardlink).
    if (-not (Test-Path -LiteralPath $runtimeRainGlassPackagePath) -or
        (Get-FileHash -LiteralPath $rainGlassPackagePath).Hash -ne
        (Get-FileHash -LiteralPath $runtimeRainGlassPackagePath).Hash) {
        Copy-Item -LiteralPath $rainGlassPackagePath -Destination $runtimeRainGlassPackagePath -Force
    }
    Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'scripts\shader_modes_ab.cfg') -Destination (Join-Path $runtimeBase 'shader_modes_ab.cfg') -Force
    Write-Host "`nBuilt ($buildMode): $executable"
    if ($Fast) { Write-Host '(-Fast: ThinLTO + incremental; about 2% below a full release build in FPS)' }
    Write-Host 'Place a GameData-style base folder beside the executable. Assets may be loose files or live in any .pk3.'
    Write-Host 'Launch the client for mp/ffa3. Choose Join Game or Join Spectate; Escape opens the menu.'
    Write-Host 'WASD moves; Space jumps; hold Space for Force jump; Ctrl crouches/rolls; Shift walks.'
    Write-Host 'Offline OpenJK movement with compiled BSP collision. Multiplayer connections are not implemented yet.'
    Write-Host "jka-probe.exe remains available in $buildDirectory for protocol and map diagnostics."
} finally {
    Pop-Location
    $env:CARGO_ENCODED_RUSTFLAGS = $existingEncoded
    $env:Path = $existingPath
}

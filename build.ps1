[CmdletBinding()]
param(
    [ValidateSet('Debug', 'Release')][string]$Configuration = 'Release',
    [switch]$Check,
    [switch]$Fetch,
    [switch]$Fast,
    [switch]$Faster,
    [switch]$Dlss,
    [switch]$CacheAudit,
    # Copy ONLY DinurdoJK.exe (plus DinurdoJK.pdb when the build produced one) to
    # this folder instead of staging the game in target\release (no DLLs,
    # licenses, base\ or .mode file). Created if missing; relative paths resolve
    # against the current directory.
    [Alias('OutDir')][string]$Output
)
$ErrorActionPreference = 'Stop'
# Measure the whole command, not just the Cargo subprocess.
$buildWallClock = [System.Diagnostics.Stopwatch]::StartNew()
$beforeCargoSeconds = $null
$cargoEndSeconds = $null
$cargoBuildElapsed = $null
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
# Existing flags can already include this script's remapping flags (for example,
# after a previous failed invocation). Do not append a second copy: Cargo
# fingerprints the entire flag list and would rebuild unchanged dependencies.
$inherited = @($inherited | Where-Object { $remapFlags -cnotcontains $_ })
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
# -Fast      Optimized iteration: ThinLTO, codegen-units=16, incremental ON.
# -Faster    Visual-only iteration: opt-level=0, LTO off, incremental OFF, PDB enabled.
# Each profile has its own cache (target\fast, target\faster).
# Neither builds jka-probe. Without -Output nothing is copied anywhere: the exe
# stays where Cargo writes it, target\<mode>\DinurdoJK.exe (plain release builds
# land in target\release, Cargo's own folder for that profile).
# -Output <folder>  Copy only DinurdoJK.exe (+ .pdb if built) there (any mode). Use
#            -Output target\release to update the game folder after -Fast/-Faster.
# -Dlss      Adds Cargo feature nvidia-dlss (NVIDIA DLAA, Vulkan only). Needs
#            DLSS_SDK (NVIDIA DLSS SDK 310.9.1), VULKAN_SDK and libclang, and
#            stages nvngx_dlss.dll beside the exe. Toggling it rebuilds only the
#            game crate, but Cargo keeps a second copy of that crate's cache.
if ($Fast -and $Faster) { throw 'Choose -Fast or -Faster, not both.' }
if (($Fast -or $Faster) -and $Configuration -eq 'Debug') {
    throw 'For -Fast or -Faster, omit -Configuration Debug.'
}
if ($Fast) {
    $buildMode = 'fast'
    $profileArguments = @('--profile', 'fast')
} elseif ($Faster) {
    $buildMode = 'faster'
    $profileArguments = @('--profile', 'faster')
} elseif ($Configuration -eq 'Release') {
    $buildMode = 'release'
    $profileArguments = @('--release')
} else {
    $buildMode = 'debug'
    $profileArguments = @()
}
$featureArguments = @()
if ($Dlss) {
    foreach ($name in 'DLSS_SDK', 'VULKAN_SDK') {
        $value = [Environment]::GetEnvironmentVariable($name)
        if (-not $value -or -not (Test-Path -LiteralPath $value)) {
            throw "-Dlss needs $name set to an existing directory (see crates\jka-client\DLAA_VULKAN.md)."
        }
    }
    $featureArguments = @('--features', 'nvidia-dlss')
}
$buildDirectory = Join-Path $PSScriptRoot "target\$buildMode"
$runtimeDirectory = $buildDirectory
if ($Output) {
    $runtimeDirectory = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Output).TrimEnd('\', '/')
    if ($runtimeDirectory -like '*.exe') {
        throw "-Output takes a folder, not an exe path (the exe is staged with its DLLs and base\): $Output"
    }
}
$executable = Join-Path $runtimeDirectory 'DinurdoJK.exe'
$previousExecutable = "$executable.prev"

# Near-zero-overhead disk snapshot: list only immediate files in the active
# profile root (not deps/build/incremental); query volume free space via Windows.
# These are net size changes, NEVER write-byte counters.
function Get-ProfileRootFileBytes([string]$Folder) {
    [long]$sum = 0
    if ([System.IO.Directory]::Exists($Folder)) {
        foreach ($file in [System.IO.Directory]::EnumerateFiles($Folder, '*', [System.IO.SearchOption]::TopDirectoryOnly)) {
            try { $sum += ([System.IO.FileInfo]::new($file)).Length } catch { }
        }
    }
    return $sum
}
function Format-SignedBuildBytes([long]$Bytes) {
    if ($Bytes -eq 0) { return '0 B' }
    $prefix = if ($Bytes -gt 0) { '+' } else { '-' }
    $absolute = [math]::Abs([double]$Bytes)
    if ($absolute -ge 1GB) { return ('{0}{1:N2} GiB' -f $prefix, ($absolute / 1GB)) }
    if ($absolute -ge 1MB) { return ('{0}{1:N1} MiB' -f $prefix, ($absolute / 1MB)) }
    if ($absolute -ge 1KB) { return ('{0}{1:N1} KiB' -f $prefix, ($absolute / 1KB)) }
    return ('{0}{1} B' -f $prefix, [long]$absolute)
}
$profileRootBytesBefore = Get-ProfileRootFileBytes $buildDirectory
$volumeDisk = $null
$volumeFreeBefore = $null
try {
    $volumeDisk = [System.IO.DriveInfo]::new([System.IO.Path]::GetPathRoot($PSScriptRoot))
    if ($volumeDisk.IsReady) { $volumeFreeBefore = [long]$volumeDisk.AvailableFreeSpace }
} catch { $volumeDisk = $null }

# Cargo's per-target reuse is lightweight. A full directory/NTFS scan is not.
# Load NTFS probes only for explicit -CacheAudit requests.
if ($CacheAudit) {
    . (Join-Path $PSScriptRoot 'scripts\build-cache-ntfs.ps1')
}
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

# Keep the two profiles' incremental settings deterministic and independent of
# the caller's environment. Original values are restored in finally.
$previousCargoIncremental = $env:CARGO_INCREMENTAL
if ($Fast) { $env:CARGO_INCREMENTAL = '1' }
if ($Faster) { $env:CARGO_INCREMENTAL = '0' }
# The auto-updater's Ed25519 verification key is baked in at compile time. It is
# the public half of the signing pair, so it is committed in update-public-key.hex.
# An already-set variable (CI, or a local key override) wins. Restored in finally.
$previousUpdateKey = $env:DINURDOJK_UPDATE_PUBLIC_KEY_HEX
$updateKeyFile = Join-Path $PSScriptRoot 'update-public-key.hex'
if (-not $env:DINURDOJK_UPDATE_PUBLIC_KEY_HEX -and (Test-Path -LiteralPath $updateKeyFile)) {
    $env:DINURDOJK_UPDATE_PUBLIC_KEY_HEX = ([System.IO.File]::ReadAllText($updateKeyFile)).Trim()
}
$cacheReport = New-CargoArtifactReport
$cacheSnapshot = $null
Push-Location $PSScriptRoot
try {
    if ($CacheAudit) {
        $cacheSnapshot = Get-BuildOutputSnapshot -BuildDirectory $buildDirectory -RuntimeDirectory $runtimeDirectory
    }
    Ensure-PatchedSmaaVendor -WorkspaceRoot $PSScriptRoot
    Ensure-OpenALRuntime -WorkspaceRoot $PSScriptRoot | Out-Null
    # Refresh the dependency graph once when upgrading an older checkout.
    $lockfile = Join-Path $PSScriptRoot 'Cargo.lock'
    $lockText = if (Test-Path -LiteralPath $lockfile) { [System.IO.File]::ReadAllText($lockfile) } else { '' }
    $staleGraphicsLock = (-not $lockText) -or
        ($lockText -match '(?m)^name = "wgpu"\r?\nversion = "29\.') -or
        ($lockText -match '(?m)^name = "smaa"\r?\nversion = "0\.20\.0"')
    # -Dlss needs dlss_wgpu/bindgen, which ordinary offline builds never download.
    if ($Fetch -or $staleGraphicsLock -or $Dlss) {
        & $cargoPath fetch
        if ($LASTEXITCODE -ne 0) { throw 'Dependency download failed.' }
    }
    if ($Check) {
        & $cargoPath fmt --all -- --check
        if ($LASTEXITCODE -ne 0) { throw 'Formatting check failed.' }
        & $cargoPath test --workspace --offline --locked @profileArguments @featureArguments
        if ($LASTEXITCODE -ne 0) { throw 'Tests failed.' }
        & $cargoPath clippy --workspace --all-targets --offline --locked @profileArguments @featureArguments -- -D warnings
        if ($LASTEXITCODE -ne 0) { throw 'Clippy failed.' }
    }
    # Without -Output Cargo links straight into the runtime folder. A game running
    # from there holds the exe, so move it aside first; otherwise leave it alone
    # (no .prev copy kept on disk).
    $buildsInPlace = $buildDirectory -eq $runtimeDirectory
    $executableRunning = @(Get-Process -Name 'DinurdoJK' -ErrorAction SilentlyContinue |
        Where-Object { try { $_.Path -eq $executable } catch { $false } }).Count -gt 0
    if ($buildsInPlace -and $executableRunning) { Preserve-RunningExecutable }
    # Keep JSON artifact accounting, but let Cargo render rustc diagnostics to its
    # own stderr. The previous plain-JSON rendering lost the normal error/warning
    # colors when PowerShell reprinted it. stderr is not part of this pipeline.
    $cargoArguments = @('build', '--package', 'DinurdoJK', '--offline', '--locked') + $profileArguments + $featureArguments + @('--message-format=json,json-render-diagnostics')
    $beforeCargoSeconds = $buildWallClock.Elapsed.TotalSeconds
    $cargoBuildTimer = [System.Diagnostics.Stopwatch]::StartNew()
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
    $cargoBuildTimer.Stop()
    $cargoBuildElapsed = $cargoBuildTimer.Elapsed
    $cargoEndSeconds = $buildWallClock.Elapsed.TotalSeconds
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
        $unchanged = (Test-Path -LiteralPath $executable) -and
            (Get-FileHash -LiteralPath $builtExecutable).Hash -eq (Get-FileHash -LiteralPath $executable).Hash
        # -Output: exe only. Overwrite in place; set the old one aside (.prev)
        # only if a running game holds it open.
        if (-not $unchanged) {
            try {
                Copy-Item -LiteralPath $builtExecutable -Destination $executable -Force -ErrorAction Stop
            } catch {
                Preserve-RunningExecutable
                Copy-Item -LiteralPath $builtExecutable -Destination $executable -Force
            }
        }
        # The pdb only exists for profiles that emit one.
        Sync-RuntimeFile (Join-Path $buildDirectory 'DinurdoJK.pdb')
        Write-Host "`nBuilt ($buildMode): $executable"
        return
    }
    if ($Dlss) {
        # NGX loads the DLAA runtime from the executable's folder.
        Sync-IfDifferent -Source (Join-Path $env:DLSS_SDK 'lib\Windows_x86_64\rel\nvngx_dlss.dll') -Destination (Join-Path $runtimeDirectory 'nvngx_dlss.dll')
    }
    $modeLabel = if ($Dlss) { "$buildMode+dlss" } else { $buildMode }
    Set-Content -LiteralPath "$executable.mode" -Value "$modeLabel $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')" -Encoding ascii
    Write-Host "`nBuilt ($buildMode): $executable"
    if ($Fast) { Write-Host '(-Fast: ThinLTO, codegen-units 16, incremental ON)' }
    if ($Faster) { Write-Host '(-Faster: visual-only, opt-level 0, LTO off, incremental OFF, PDB enabled)' }
} finally {
    # Normal builds never recursively scan target output or query NTFS file IDs.
    # Full file sizes and hard-link accounting are opt-in via -CacheAudit.
    $stagingSeconds = if ($null -ne $cargoEndSeconds) {
        $buildWallClock.Elapsed.TotalSeconds - $cargoEndSeconds
    } else { [double]0 }
    $auditClock = [System.Diagnostics.Stopwatch]::StartNew()
    if ($CacheAudit -and $null -ne $cacheSnapshot) {
        try {
            Show-BuildCacheReport -BuildDirectory $buildDirectory -RuntimeDirectory $runtimeDirectory -Before $cacheSnapshot -Artifacts $cacheReport
        } catch {
            Write-Warning "Build-cache reporting failed (build artifacts are unaffected): $($_.Exception.Message)"
        }
    } else {
        Write-Host ("[JKA CACHE] Cargo: {0} fresh, {1} compiled" -f $cacheReport.FreshTargets, $cacheReport.BuiltTargets) -ForegroundColor Green
        Write-Host '  Size and NTFS hard-link scan skipped. Run with -CacheAudit for details.' -ForegroundColor DarkGray
    }
    $auditClock.Stop()
    Pop-Location
    $env:CARGO_ENCODED_RUSTFLAGS = $existingEncoded
    $env:Path = $existingPath
    $env:CARGO_INCREMENTAL = $previousCargoIncremental
    $env:DINURDOJK_UPDATE_PUBLIC_KEY_HEX = $previousUpdateKey
    $buildWallClock.Stop()
    $cargoSeconds = if ($null -ne $cargoBuildElapsed) { $cargoBuildElapsed.TotalSeconds } else { [double]0 }
    $preSeconds = if ($null -ne $beforeCargoSeconds) { $beforeCargoSeconds } else { $buildWallClock.Elapsed.TotalSeconds - $auditClock.Elapsed.TotalSeconds }
    Write-Host ("[JKA TIME] Setup {0:N2}s | Cargo {1:N2}s | Staging {2:N2}s | Cache report {3:N2}s | TOTAL {4:N2}s" -f $preSeconds, $cargoSeconds, $stagingSeconds, $auditClock.Elapsed.TotalSeconds, $buildWallClock.Elapsed.TotalSeconds) -ForegroundColor Cyan
    # Cheap disk summary: no recursive scans and no per-file NTFS queries.
    $profileRootDelta = (Get-ProfileRootFileBytes $buildDirectory) - $profileRootBytesBefore
    Write-Host ("[JKA DISK] target\{0} top-level files: {1} (nonrecursive)" -f $buildMode, (Format-SignedBuildBytes $profileRootDelta)) -ForegroundColor Cyan
    if ($null -ne $volumeFreeBefore -and $null -ne $volumeDisk) {
        try {
            $volumeUsedDelta = $volumeFreeBefore - [long]$volumeDisk.AvailableFreeSpace
            Write-Host ("           Drive {0} net used: {1} (volume-wide, includes other activity)" -f $volumeDisk.Name, (Format-SignedBuildBytes $volumeUsedDelta)) -ForegroundColor DarkGray
        } catch { }
    }
    Write-Host '           Net size changes only; not SSD write bytes. -CacheAudit for full breakdown.' -ForegroundColor DarkGray
}

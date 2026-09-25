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

# -Fast overrides the release profile's LTO/codegen-units via env vars (no Cargo.toml
# edit needed) so local iteration doesn't pay the whole-program-optimization link cost.
$existingLto = $env:CARGO_PROFILE_RELEASE_LTO
$existingCodegenUnits = $env:CARGO_PROFILE_RELEASE_CODEGEN_UNITS
if ($Fast) {
    $env:CARGO_PROFILE_RELEASE_LTO = 'false'
    $env:CARGO_PROFILE_RELEASE_CODEGEN_UNITS = '16'
}

# WGPU backend hygiene -------------------------------------------------------
# DinurdoJK only supports Vulkan and DX12. The upstream smaa 0.20.0 crate
# requests wgpu with its default features, which otherwise pulls the GLES/OpenGL
# backend (glow/WGL/opengl32.dll) into the executable even though we never select it.
# Keep a tiny repo-local vendor of exactly smaa 0.20.0 and patch only that
# dependency edge to `default-features = false`. Its required `glsl` shader-input
# feature stays enabled; GLSL input is translated by Naga and is not an OpenGL
# rendering backend.
function Set-SmaaWgpuDefaultsOff([string]$ManifestPath) {
    if (-not (Test-Path -LiteralPath $ManifestPath)) { return }

    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    $text = [System.IO.File]::ReadAllText($ManifestPath)

    # crates.io's normalized Cargo.toml uses a [dependencies.wgpu] table.
    $sectionPattern = '(?ms)^\[dependencies\.wgpu\]\r?\n.*?(?=^\[|\z)'
    $match = [regex]::Match($text, $sectionPattern)
    if ($match.Success) {
        $section = $match.Value
        if ($section -notmatch '(?m)^default-features\s*=') {
            $newline = if ($section.Contains("`r`n")) { "`r`n" } else { "`n" }
            $section = $section.TrimEnd("`r", "`n") + $newline + 'default-features = false' + $newline
            $text = $text.Substring(0, $match.Index) + $section + $text.Substring($match.Index + $match.Length)
            [System.IO.File]::WriteAllText($ManifestPath, $text, $utf8NoBom)
        }
        return
    }

    # Cargo.toml.orig uses the compact inline dependency form in upstream smaa.
    $inlinePattern = '(?m)^(\s*wgpu\s*=\s*\{)([^}\r\n]*)(\}\s*)$'
    $match = [regex]::Match($text, $inlinePattern)
    if ($match.Success) {
        if ($match.Groups[2].Value -notmatch 'default-features\s*=') {
            $replacement = $match.Groups[1].Value + ' default-features = false,' + $match.Groups[2].Value + $match.Groups[3].Value
            $text = $text.Substring(0, $match.Index) + $replacement + $text.Substring($match.Index + $match.Length)
            [System.IO.File]::WriteAllText($ManifestPath, $text, $utf8NoBom)
        }
        return
    }

    throw "Could not find smaa's wgpu dependency in $ManifestPath"
}

function Ensure-PatchedSmaaVendor {
    $vendorRoot = Join-Path $PSScriptRoot 'vendor'
    $vendorSmaa = Join-Path $vendorRoot 'smaa-0.20.0'
    $vendorManifest = Join-Path $vendorSmaa 'Cargo.toml'

    if (-not (Test-Path -LiteralPath $vendorManifest)) {
        New-Item -ItemType Directory -Force -Path $vendorRoot | Out-Null

        # Prefer the already-downloaded Cargo registry copy so normal builds stay
        # completely offline. This will exist on machines that have built the
        # current project at least once.
        $cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
        $registrySrc = Join-Path $cargoHome 'registry\src'
        $cachedSmaa = $null
        if (Test-Path -LiteralPath $registrySrc) {
            $cachedSmaa = Get-ChildItem -Path $registrySrc -Directory -ErrorAction SilentlyContinue |
                ForEach-Object { Join-Path $_.FullName 'smaa-0.20.0' } |
                Where-Object { Test-Path -LiteralPath (Join-Path $_ 'Cargo.toml') } |
                Select-Object -First 1
        }

        if ($cachedSmaa) {
            Copy-Item -LiteralPath $cachedSmaa -Destination $vendorSmaa -Recurse -Force
        } else {
            # Fresh machine: bootstrap the exact locked crate without asking Cargo
            # to parse the workspace first (the workspace now points at this path).
            $crateUrl = 'https://static.crates.io/crates/smaa/smaa-0.20.0.crate'
            $crateFile = Join-Path $env:TEMP 'smaa-0.20.0.crate'
            Write-Host 'Bootstrapping patched smaa 0.20.0 vendor...'
            Invoke-WebRequest -UseBasicParsing -Uri $crateUrl -OutFile $crateFile
            & tar.exe -xzf $crateFile -C $vendorRoot
            if ($LASTEXITCODE -ne 0) { throw 'Failed to extract smaa-0.20.0.crate.' }
            Remove-Item -LiteralPath $crateFile -Force -ErrorAction SilentlyContinue
        }
    }

    Set-SmaaWgpuDefaultsOff (Join-Path $vendorSmaa 'Cargo.toml')
    Set-SmaaWgpuDefaultsOff (Join-Path $vendorSmaa 'Cargo.toml.orig')

    $patched = [System.IO.File]::ReadAllText((Join-Path $vendorSmaa 'Cargo.toml'))
    if ($patched -notmatch '(?ms)^\[dependencies\.wgpu\].*?^default-features\s*=\s*false') {
        throw 'smaa vendor patch did not disable wgpu default features.'
    }
}

Ensure-PatchedSmaaVendor

Push-Location $PSScriptRoot
try {
    if ($Fetch) {
        & $cargoPath fetch
        if ($LASTEXITCODE -ne 0) { throw 'Dependency download failed.' }
    }
    $profileArguments = @()
    if ($Configuration -eq 'Release') { $profileArguments += '--release' }
    if ($Check) {
        & $cargoPath fmt --all -- --check
        if ($LASTEXITCODE -ne 0) { throw 'Formatting check failed.' }
        & $cargoPath test --workspace --offline --locked @profileArguments
        if ($LASTEXITCODE -ne 0) { throw 'Tests failed.' }
        & $cargoPath clippy --workspace --all-targets --offline --locked @profileArguments -- -D warnings
        if ($LASTEXITCODE -ne 0) { throw 'Clippy failed.' }
    }
    $cargoArguments = @('build', '--workspace', '--offline', '--locked') + $profileArguments
    & $cargoPath @cargoArguments
    if ($LASTEXITCODE -ne 0) { throw 'Build failed. Check the Rust and MSVC toolchain messages above.' }
    $executable = Join-Path $PSScriptRoot "target\$($Configuration.ToLowerInvariant())\DinurdoJK.exe"
    Write-Host "`nBuilt: $executable"
    if ($Fast) { Write-Host '(-Fast build: LTO and codegen-units=1 disabled, larger/slower binary for quicker iteration)' }
    Write-Host 'Place a GameData-style base folder beside the executable. Assets may be loose files or live in any .pk3.'
    Write-Host 'Launch the client for mp/ffa3. Choose Join Game or Join Spectate; Escape opens the menu.'
    Write-Host 'WASD moves; Space jumps; hold Space for Force jump; Ctrl crouches/rolls; Shift walks.'
    Write-Host 'Offline OpenJK movement with compiled BSP collision. Multiplayer connections are not implemented yet.'
    Write-Host 'jka-probe.exe remains available for protocol and map diagnostics.'
} finally {
    Pop-Location
    $env:CARGO_ENCODED_RUSTFLAGS = $existingEncoded
    $env:Path = $existingPath
    $env:CARGO_PROFILE_RELEASE_LTO = $existingLto
    $env:CARGO_PROFILE_RELEASE_CODEGEN_UNITS = $existingCodegenUnits
}

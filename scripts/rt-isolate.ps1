[CmdletBinding()]
param(
    [ValidateSet('baseline', 'glass', 'full')][string]$Mode = 'baseline',
    [string]$Map = '',
    [switch]$TintedGlass,
    [switch]$Serial,
    [string]$Executable = '',
    [switch]$Build
)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not $Executable) { $Executable = Join-Path $repoRoot 'target\release\DinurdoJK.exe' }
if (Get-Process -Name DinurdoJK -ErrorAction SilentlyContinue) {
    throw 'Close the running DinurdoJK before starting an isolation run.'
}
if ($Build) { & (Join-Path $repoRoot 'build.ps1') -Fast; if ($LASTEXITCODE) { throw 'Build failed.' } }
$Executable = (Resolve-Path -LiteralPath $Executable).Path
$exeDirectory = Split-Path -Parent $Executable
$runDirectory = Join-Path $repoRoot ('target\rt-isolation\runs\' + (Get-Date -Format 'yyyyMMdd-HHmmss-fff') + '-' + $Mode)
New-Item -ItemType Directory -Force -Path $runDirectory | Out-Null
$settingsBackups = @{}
foreach ($gameDirectory in @('base', 'japro')) {
    $settingsPath = Join-Path $repoRoot "target\release\$gameDirectory\DinurdoJK.cfg"
    if (Test-Path -LiteralPath $settingsPath) {
        $backupPath = Join-Path $runDirectory "$gameDirectory-DinurdoJK.cfg"
        Copy-Item -LiteralPath $settingsPath -Destination $backupPath
        $settingsBackups[$settingsPath] = $backupPath
    } else {
        $settingsBackups[$settingsPath] = $null
    }
}
$glass = if ($Mode -in @('glass', 'full')) { '1' } else { '0' }
$values = @{
    JKA_RT_RECEIVER_GLASS = $glass
    JKA_RT_PIPELINE_TRACE = '1'
    JKA_RT_SERIALIZE_WORLD_PIPELINES = $(if ($Serial) { '1' } else { '0' })
    JKA_RT_SHADER_DUMP_DIR = (Join-Path $runDirectory 'shaders')
}
$previous = @{}
foreach ($name in $values.Keys) {
    $previous[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
    [Environment]::SetEnvironmentVariable($name, $values[$name], 'Process')
}
$launchArgs = @('--base', (Join-Path $repoRoot 'target\release\base'),
    '+r_tintedGlass', $(if ($TintedGlass) { '1' } else { '0' }),
    '+r_dynamicLights', 'ray_traced')
if ($Map) { $launchArgs += @('--map', $Map) }
$started = Get-Date
Write-Host "RT isolation: $Mode, serial=$Serial. Evidence: $runDirectory"
Write-Host 'Use the same map and settings for each run; close the client after checking whether it crashes.'
Push-Location (Join-Path $repoRoot 'target\release')
$previousErrorAction = $ErrorActionPreference
try {
    $ErrorActionPreference = 'Continue'
    & $Executable @launchArgs
    $clientExit = $LASTEXITCODE
} finally {
    $ErrorActionPreference = $previousErrorAction
    Pop-Location
    foreach ($name in $previous.Keys) {
        [Environment]::SetEnvironmentVariable($name, $previous[$name], 'Process')
    }
    foreach ($settingsPath in $settingsBackups.Keys) {
        if ($settingsBackups[$settingsPath]) {
            Copy-Item -LiteralPath $settingsBackups[$settingsPath] -Destination $settingsPath -Force
        } elseif (Test-Path -LiteralPath $settingsPath) {
            $gameDirectory = Split-Path -Leaf (Split-Path -Parent $settingsPath)
            Copy-Item -LiteralPath $settingsPath -Destination (Join-Path $runDirectory "$gameDirectory-created-DinurdoJK.cfg")
            Remove-Item -LiteralPath $settingsPath
        }
    }
    $log = Join-Path $exeDirectory 'latest.log'
    if (Test-Path -LiteralPath $log) {
        Copy-Item -LiteralPath $log -Destination (Join-Path $runDirectory 'latest.log')
        $inFlight = @{}
        foreach ($line in Get-Content -LiteralPath $log) {
            if ($line -match '\[RT pipeline BEGIN\] id=(\d+)') { $inFlight[$Matches[1]] = $line }
            if ($line -match '\[RT pipeline END\] id=(\d+)') { $inFlight.Remove($Matches[1]) }
        }
        $inFlight.Values | Set-Content -LiteralPath (Join-Path $runDirectory 'in-flight-pipelines.txt')
    }
    foreach ($dump in Get-ChildItem -LiteralPath $exeDirectory -Filter 'crash-*.dmp' -File) {
        if ($dump.LastWriteTime -ge $started) { Copy-Item -LiteralPath $dump.FullName -Destination $runDirectory }
    }
    [ordered]@{ mode = $Mode; receiver_glass = $glass;
        tinted_glass = [bool]$TintedGlass; serial_world = [bool]$Serial;
        map = $Map; executable = $Executable; exit_code = $clientExit;
        started = $started.ToString('o'); ended = (Get-Date).ToString('o') } |
        ConvertTo-Json | Set-Content -LiteralPath (Join-Path $runDirectory 'run.json')
    Write-Host "Saved RT isolation evidence: $runDirectory"
}

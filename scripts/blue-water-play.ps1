[CmdletBinding()]
param(
    [string]$SourceBase = 'D:\Code\DinurdoJK\target\release\base',
    [string]$Map = 'costadelsol',
    [switch]$Environment,
    [switch]$CloudsOff,
    [switch]$Hybrid,
    [switch]$Lakes,
    [switch]$Coast,
    [string]$Executable = '',
    [ValidateRange(640, 7680)]
    [int]$Width = 1920,
    [ValidateRange(480, 4320)]
    [int]$Height = 1080
)
$ErrorActionPreference = 'Stop'
$workspaceRoot = Split-Path -Parent $PSScriptRoot
if ($Lakes -and $Coast) { throw 'Choose either -Lakes or -Coast.' }
$executable = if ($Executable) { $Executable } elseif ($Lakes) { Join-Path $workspaceRoot 'target\blue-water-lakes\DinurdoJK.exe' } elseif ($Coast) { Join-Path $workspaceRoot 'target\blue-water-coast\DinurdoJK.exe' } elseif ($Environment) { Join-Path $workspaceRoot 'target\blue-water-environment\DinurdoJK.exe' } else { Join-Path $workspaceRoot 'target\release\DinurdoJK.exe' }
if (-not (Test-Path -LiteralPath $executable)) { throw 'Build the release client first.' }
$playRoot = Join-Path $workspaceRoot $(if ($Lakes) { 'target\blue-water-lakes-play' } elseif ($Coast) { 'target\blue-water-coast-play' } elseif ($Environment) { 'target\blue-water-environment-play' } else { 'target\blue-water-play' })
$playBase = Join-Path $playRoot 'base'
New-Item -ItemType Directory -Force -Path $playBase | Out-Null
# Promotion must be enabled before the initial BSP load, not just by the later play commands.
$coastConfig = Join-Path $playBase 'DinurdoJK.cfg'
if ($Coast -and -not (Test-Path -LiteralPath $coastConfig)) {
    [System.IO.File]::WriteAllLines($coastConfig, @('seta r_ocean "1"', 'seta r_oceanType "3"'))
}
foreach ($package in Get-ChildItem -LiteralPath $SourceBase -File -Filter '*.pk3') {
    $destination = Join-Path $playBase $package.Name
    if (-not (Test-Path -LiteralPath $destination)) {
        New-Item -ItemType HardLink -Path $destination -Value $package.FullName | Out-Null
    }
}
foreach ($dll in Get-ChildItem -LiteralPath (Split-Path -Parent $SourceBase) -File -Filter '*.dll') {
    $destination = Join-Path (Split-Path -Parent $executable) $dll.Name
    if (-not (Test-Path -LiteralPath $destination)) {
        Copy-Item -LiteralPath $dll.FullName -Destination $destination
    }
}
# Dedicated profile with normal input and no benchmark capture or automatic quit.
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'blue-water-optimized.cfg') -Destination (Join-Path $playBase 'blue_water_optimized.cfg') -Force
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'blue-water-compare.cfg') -Destination (Join-Path $playBase 'blue_water_compare.cfg') -Force
$commands = @(
    'set r_backend "vulkan"',
    'set r_mode "-1"',
    "set r_customwidth `"$Width`"",
    "set r_customheight `"$Height`"",
    'set r_fullscreen "0"',
    'set r_swapInterval "0"',
    'set r_worldPath "unified"',
    'set r_fastsky "0"',
    'set r_ocean "1"',
    'set r_oceanSkyExtend "2"',
    'exec blue_water_optimized.cfg',
    'set sensitivity "5"',
    'set cl_mouseAccel "0"',
    'set con_notifytime "3"',
    'set cg_thirdPerson "0"',
    'team free',
    'noclip',
    'setviewpos -2397 11562 817 292 -353'
)
if ($Environment) {
    $commands += @('set r_clouds "1"', 'set r_cloudBackend "1"', 'set r_skyBackend "1"',
        'set r_blueSkyWeatherBackend "1"', 'set r_blueSkyRain "0.3"',
        'set r_blueSkyLightning "2"', 'set r_blueSkyWorldLighting "0"',
        'set r_blueSkyFilmic "1"')
}
if ($CloudsOff) { $commands += 'set r_clouds "0"' }
if ($Hybrid) { $commands += 'set r_oceanType "3"' }
if ($Lakes) { $commands += 'set r_oceanType "4"' }
if ($Coast) { $commands += @('set r_oceanType "3"','set r_blueWaterCoast "1"','set r_blueWaterCoastShelter "0"') }
[System.IO.File]::WriteAllLines((Join-Path $playBase 'blue_water_play.cfg'), $commands)
$process = Start-Process -FilePath $executable -ArgumentList @(
    '--base', ('"' + $playBase + '"'), '--map', $Map, '+exec', 'blue_water_play.cfg'
) -WorkingDirectory $playRoot -WindowStyle Normal -PassThru
Write-Output "Blue Water free-flight session launched (PID $($process.Id)); isolated profile: $playBase"

param(
    [string]$Executable = 'C:\Games\JK\DinurdoJK.exe',
    [string]$Config = 'C:\Games\JK\JKA GameData\base\DinurdoJK.cfg',
    [string]$Label = 'transport',
    [switch]$Baseline,
    [switch]$NormalOnly,
    [string[]]$Views = @('-643 310 64 83 14', '889 -799 227 202 32'),
    [string[]]$ExtraCaptureCommands = @()
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$run = Join-Path $root "artifacts\sky-transport-ffa3\$Label"
$base = Join-Path $run 'base'
New-Item -ItemType Directory -Force -Path $base | Out-Null
Copy-Item -LiteralPath $Config -Destination (Join-Path $base 'DinurdoJK.cfg')
$savedShaderMode = [regex]::Matches([IO.File]::ReadAllText($Config), '(?m)^seta?\s+r_fragmentShaderMode\s+"?(\d+)')
$shaderMode = if ($savedShaderMode.Count) { $savedShaderMode[$savedShaderMode.Count - 1].Groups[1].Value } else { '5' }
# Preserve the user's lighting configuration; isolate captures and config writes.
$settings = @(
    'set r_mode -1', 'set r_customwidth 1280', 'set r_customheight 720',
    'set r_fullscreen 0', 'set r_swapInterval 0', 'set com_maxfps 120',
    'set cg_draw2D 0', 'set cg_drawGun 0', 'set cg_thirdPerson 0', 'set cg_fpls 1', 'set sensitivity 0',
    'set con_notifytime -1', 'set r_lightmapSignal 0', 'set r_lightmap 0',
    'set r_skyWorldLightIsolation 0'
)
[IO.File]::AppendAllLines((Join-Path $base 'DinurdoJK.cfg'), [string[]]$settings)
# setviewpos_exact uses player origin (+1 teleport lift, +36 eye height).
# PNG metadata records the actual camera position/angles for verification.
$commands = $settings + @('team free', 'noclip',
    'perfsample 3 startup', 'r_sunPitch', 'r_sunYaw', 'r_relightTime', 'r_skyBackend',
    'r_skyWorldLightIsolation', 'r_dynamicShadows', 'r_blueSkyWorldLighting')
for ($i = 0; $i -lt $views.Count; $i++) {
    $commands += @("setviewpos_exact $($views[$i])", "perfsample 2 spot$($i+1)_off", 'viewpos', 'screenshot_png', 'wait 30')
}
$captures = @()
if (!$Baseline) {
    $commands += @('set r_skyWorldLightIsolation 1', 'exec sky_transport_poll.cfg')
    for ($i = 0; $i -lt $views.Count; $i++) {
        $captures += @("setviewpos_exact $($views[$i])", 'set r_lightmap 0', 'set r_lightmapSignal 0',
            "waitshadermode $shaderMode 120", "perfsample 2 spot$($i+1)_protected", 'viewpos', 'screenshot_png', 'wait 30')
        if (!$NormalOnly) {
            $captures += @('set r_lightmap 1', "waitshadermode $shaderMode 120")
            foreach ($mode in 9, 10, 11) {
                $captures += @("set r_lightmapSignal $mode", "perfsample 2 spot$($i+1)_signal$mode", 'screenshot_png', 'wait 30')
            }
        }
    }
    $captures += $ExtraCaptureCommands
    # Verify that a diagnostic selection cannot keep the feature alive when OFF.
    $captures += @('set r_skyWorldLightIsolation 0', 'set r_lightmap 0',
        "waitshadermode $shaderMode 120", 'perfsample 3 disabled_after_diagnostic', 'screenshot_png', 'wait 30', 'quit')
    [IO.File]::WriteAllLines((Join-Path $base 'sky_transport_poll.cfg'), @('wait 60', 'exec sky_transport_poll.cfg'))
} else {
    $commands += 'quit'
}
[IO.File]::WriteAllLines((Join-Path $base 'sky_transport_test.cfg'), $commands)
$stdout = Join-Path $run 'stdout.log'
$stderr = Join-Path $run 'stderr.log'
$pathsFile = Join-Path (Split-Path -Parent $Executable) 'DinurdoJK\DinurdoJK-paths.ini'
$originalHome = if (Test-Path -LiteralPath $pathsFile) {
    [regex]::Match([IO.File]::ReadAllText($pathsFile), '(?m)^fs_homepath=[^\r\n]*').Value
} else { $null }
$process = Start-Process -FilePath $Executable -WorkingDirectory (Split-Path -Parent $Executable) `
    -ArgumentList @('--base', '"C:\Games\JK\JKA GameData\base"', '--map', 'mp/ffa3',
        '+set', 'fs_homepath', ('"' + $run + '"'), '+exec', 'sky_transport_test.cfg') `
    -WindowStyle Hidden -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
Write-Output "Sky transport test PID $($process.Id); captures/logs: $run"
$null = $process.Handle
$deadline = [DateTime]::UtcNow.AddMinutes(10)
$released = [bool]$Baseline
try {
while (!$process.HasExited) {
    if ([DateTime]::UtcNow -gt $deadline) {
        $process.Kill()
        throw "Sky transport capture timed out; inspect $stdout and $stderr"
    }
    if (!$released) {
        [string]$log = Get-Content -LiteralPath $stdout -Raw -ErrorAction SilentlyContinue
        if ($log.Contains('RENDERER ERROR:') -or $log.Contains('[SKY TRANSPORT] skipped:')) {
            throw "Renderer/transport failure; inspect $stdout and $stderr"
        }
        # Wait for transport AND the following render-thread texture upload.
        $at = $log.LastIndexOf('[SKY TRANSPORT] white sky,')
        if ($at -ge 0 -and $log.IndexOf('[SUN LIGHTMAP] static visibility ready', $at) -gt $at) {
            [IO.File]::WriteAllLines((Join-Path $base 'sky_transport_poll.cfg'), $captures)
            $released = $true
            Write-Output "Transport uploaded; capturing $($views.Count) requested viewpoints."
        }
    }
    Start-Sleep -Milliseconds 500
    $process.Refresh()
}
$process.WaitForExit()
Write-Output "Sky transport test exit code: $($process.ExitCode)"
if ($process.ExitCode -ne 0) { throw "Game exited with $($process.ExitCode); inspect $stdout and $stderr" }
if (!$Baseline -and !$released) { throw "Transport never completed; inspect $stdout and $stderr" }
} finally {
    if (!$process.HasExited) { $process.Kill() }
    # The runtime +set fs_homepath is archived in the installation's paths file.
    # Restore only our own override, preserving any other path edits.
    if ($originalHome -and (Test-Path -LiteralPath $pathsFile)) {
        $paths = [IO.File]::ReadAllText($pathsFile)
        $override = "fs_homepath=$run"
        if ($paths.Contains($override)) {
            [IO.File]::WriteAllText($pathsFile, $paths.Replace($override, $originalHome))
        }
    }
}

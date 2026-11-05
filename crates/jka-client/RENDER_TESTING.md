# Unattended renderer tests on Windows

This documents the workflow used for the glass tests in this repo. It keeps test
configuration, screenshots, caches and runtime logs separate from the normal game
installation and does not require mouse/keyboard automation.

## Two different kinds of test

| Test | What runs | Game window |
| --- | --- | --- |
| Shader validation | Naga parses and validates the composed WGSL variants | None |
| GPU regression | wgpu adapter, textures, pipelines, command encoder, readback assertions | None; no winit application or swapchain |
| Runtime screenshot/performance check | Normal client, real map, queued console commands | A native game window can appear; commands run unattended |

The runtime checks were launched through Codex's background command runner with
stdout/stderr redirected to files. The runner returned a session ID while the
client continued running. Screenshots came from the client's GPU render target,
not a desktop screenshot, and the client quit automatically. No interaction with
the user's existing game window or desktop was used.

**This is not a native headless runtime mode.** The current client deliberately
reveals its initially hidden window at `RendererReady`; keeping a Windows
swapchain hidden can report `Occluded` and stall startup. See
[lifecycle.rs](src/app/lifecycle.rs). The fact that a tool-launched instance was
unobtrusive in this session does not guarantee a hidden game window when these
commands are launched from a normal desktop terminal. `Start-Process
-WindowStyle Hidden` suppresses the launcher/console; the client can still show
its own window. For tests that must never create a game window, use the first two
rows above.

## 1. Shader and GPU tests without a window

Run from the repo root:

```powershell
cargo test -p DinurdoJK promoted_glass_shader_variants_validate --quiet
cargo test -p DinurdoJK glass_controls_gpu_preserve_pane_edges_and_exclude_foreground_halos --quiet -- --ignored
cargo test -p DinurdoJK optics_gpu_capture_matches_beer_lambert --quiet -- --ignored
cargo test -p DinurdoJK decal --quiet
```

GPU tests are explicitly ignored by default because they require a GPU adapter.
The foreground test creates synthetic color/depth/coverage textures, renders the
real glass helpers, and reads pixels back. It checks blur, foreground rejection,
pane boundaries, independent opacity/frost controls, recovery of hidden world
pixels, and preservation of a model behind the pane. The optics test covers
single-sample and 4x MSAA capture/composition. The decal tests check default
polygon-offset sorting and explicit late-sort precedence.

These checks complement runtime screenshots: shader validation cannot catch
incorrect game draw order, and a visual capture alone is not a pixel assertion.

## 2. Create an isolated runtime base and executable directory

The directories used during the glass work were:

```text
glass-preview-base/                  test cfg, assets, cache, screenshots, renderperf
glass-preview-run/                   copied executable, DinurdoJK/dependencies/phonon.dll, latest.log
artifacts/glass-investigation/       retained images, stdout logs, test/perf reports
```

Example setup, from the repo root. Set `$sourceBase` to an existing installation
containing the retail assets. Do not copy the user's active configuration.

```powershell
$repoRoot = (Resolve-Path -LiteralPath '.').Path
$sourceBase = 'D:\Games\JKA\GameData\base'
$testBase = Join-Path $repoRoot 'glass-preview-base'
$testRun = Join-Path $repoRoot 'glass-preview-run'
$testOutput = Join-Path $repoRoot 'artifacts/glass-investigation'

New-Item -ItemType Directory -Force -Path $testBase, $testRun, $testOutput | Out-Null

foreach ($name in @('assets0.pk3', 'assets1.pk3', 'assets2.pk3', 'assets3.pk3')) {
    $sourceArchive = Join-Path $sourceBase $name
    $testArchive = Join-Path $testBase $name
    if (-not (Test-Path -LiteralPath $testArchive)) {
        New-Item -ItemType HardLink -Path $testArchive -Target $sourceArchive | Out-Null
    }
}

cargo build -p DinurdoJK --quiet
Copy-Item -LiteralPath 'target/debug/DinurdoJK.exe' -Destination $testRun
$testDependencies = Join-Path $testRun 'DinurdoJK/dependencies'
New-Item -ItemType Directory -Force -Path $testDependencies | Out-Null
Copy-Item -LiteralPath 'target/debug/DinurdoJK/dependencies/phonon.dll' -Destination $testDependencies
```

The PK3s are read by the client. A hard link shares the original file's contents:
never edit or repack a linked archive. If test assets need modification, use a
separate copied archive or loose files inside the test base instead. Hard links
require both locations to be on the same volume; `Copy-Item` is the alternative.

Put latched settings into the test cfg **before starting the client**:

```powershell
@'
seta r_mode "-1"
seta r_customwidth "1280"
seta r_customheight "800"
seta r_fullscreen "0"
seta r_windowMaximized "0"
seta r_backend "vulkan"
seta r_ext_multisample "4"
seta r_glassPromotion "1"
seta r_glassBackgroundRecovery "0"
'@ | Set-Content -LiteralPath (Join-Path $testBase 'DinurdoJK.cfg')
```

`r_glassPromotion` applies at map load. A startup `+r_glassPromotion 1` issued
after that load is too late for the first map. The client may archive its current
cvars on exit, so later runs inherit this **test** config unless it is reset.
For comparisons, record all rendering settings, not only glass settings.

## 3. Queue an unattended visual check

The actual tool-run launches used a native command of this form:

```powershell
$testExe = Join-Path $testRun 'DinurdoJK.exe'
& $testExe --base $testBase --map mp/ctf4 `
    +wait 600 `
    +team free +noclip `
    +cg_thirdPerson 1 +cg_thirdPersonRange 50 +cg_thirdPersonVertOffset 0 `
    +setviewpos 700 -805 -140 16 19 `
    +r_glassStyle liquid `
    +r_glassBlur 12 +r_glassFrost 0.75 +r_glassOpacity 0.04 `
    +r_glassRimStrength 0.15 +r_glassBackgroundRecovery 0 `
    +wait 600 +screenshot_png +wait 600 `
    +r_glassBackgroundRecovery 1 `
    +wait 600 +screenshot_png +wait 1200 `
    +quit > (Join-Path $testOutput 'visual-check.log') 2>&1
```

This shell command waits for exit when used directly in a terminal. Codex's
command runner can return its ongoing session and let the agent inspect logs
while the process continues. To use a background process from ordinary
PowerShell, the same arguments can be launched as follows:

```powershell
$testArguments = @(
    '--base', ('"{0}"' -f $testBase), '--map', 'mp/ctf4',
    '+wait', '600', '+team', 'free', '+noclip',
    '+cg_thirdPerson', '1', '+cg_thirdPersonRange', '50',
    '+cg_thirdPersonVertOffset', '0',
    '+setviewpos', '700', '-805', '-140', '16', '19',
    '+r_glassStyle', 'liquid', '+r_glassBlur', '12',
    '+r_glassFrost', '0.75', '+r_glassOpacity', '0.04',
    '+r_glassBackgroundRecovery', '1',
    '+wait', '600', '+screenshot_png', '+wait', '1200', '+quit'
)
$testProcess = Start-Process -FilePath $testExe -ArgumentList $testArguments `
    -WorkingDirectory $repoRoot -WindowStyle Hidden -PassThru `
    -RedirectStandardOutput (Join-Path $testOutput 'runtime-stdout.log') `
    -RedirectStandardError (Join-Path $testOutput 'runtime-stderr.log')

# Poll the process/log files while doing other work. This does not click or type
# into any desktop window. The game window visibility caveat still applies.
$testProcess.Refresh()
$testProcess.HasExited
Get-Content -LiteralPath (Join-Path $testOutput 'runtime-stdout.log') -Tail 20
```

### Command sequencing details

- Startup console commands wait for the map-load barrier, then execute in order.
- `+wait N` means **N rendered frames**, not N milliseconds. Slow frames and lazy
  pipeline creation make a run longer; higher FPS makes each wait shorter.
- `+team free` joins the local game before `+noclip`; spectator noclip is rejected.
- `setviewpos` sets player origin, yaw, and optional pitch. Third-person camera
  range/offset changes the resulting camera position. Match all of them for A/B.
- Choose `r_glassStyle` first: choosing a style resets its tuning controls.
- `screenshot_png` queues asynchronous readback/encoding. Leave time before quit;
  `PNG SCREENSHOT QUEUED` alone does **not** prove a file was written.
- Look for `WROTE ...screenshots/shot....png`, map-load success, and any panic or
  GPU validation errors. Inspect the actual PNG using an image viewer. If the
  file is absent, increase the post-screenshot wait and inspect worker errors.
- `+quit` shuts down the test instance; it does not quit another running client.

To capture the settings UI, queue `+uipage setup video glass +wait 300
+screenshot_png +wait 1200 +quit`. The optional third argument chooses an existing
Video rail subpage (for example Glass, Water or Geometry), allowing menu checks
without mouse automation.

## 4. Compare GPU cost at an identical view

Enable `r_gpuTimings 1`, settle the camera, and issue:

```text
r_glassBackgroundRecovery 0
render_perf_capture 240 recovery_off1
wait 2000
r_glassBackgroundRecovery 1
render_perf_capture 240 recovery_on1
wait 2000
render_perf_capture 240 recovery_on2
wait 2000
r_glassBackgroundRecovery 0
render_perf_capture 240 recovery_off2
wait 2000
quit
```

These can be queued as `+command args` at launch. Capture waits two seconds,
skips 120 warmup frames, then records the requested frames. Increase the waits
for very high FPS; confirm each `RENDER PERF ... wrote` log message before
treating the capture as complete. Output is under `$testBase/renderperf/`.

Read the GPU medians from JSON:

```powershell
Get-ChildItem -LiteralPath (Join-Path $testBase 'renderperf') -Filter '*recovery_*.json' |
    ForEach-Object {
        $report = Get-Content -LiteralPath $_.FullName -Raw | ConvertFrom-Json
        [pscustomobject]@{
            File = $_.Name
            GPUmedianMs = $report.metrics.gpu_latest_available_ms.p50
            GPU95thMs = $report.metrics.gpu_latest_available_ms.p95
            Settings = $report.settings_fingerprint_at_arm
        }
    }
```

Use an Off/On/On/Off sequence to expose drift. Compare GPU timestamp time rather
than screenshot FPS; debug validation, CPU work, screenshot encoding, and
profiler hooks affect frame rate. The asynchronous GPU result can lag the CSV
frame, so use settled multi-frame captures. This measures total GPU frame time,
including sampling changes, rather than isolating a single capture pass.

The glass check measured about 0.162 ms additional GPU time for optional recovery
on an RTX 3060, Vulkan, 1280x800, 4x MSAA in one ctf4 view. That is a scene-specific
measurement, not a guarantee for other resolutions or a release-build FPS gain.

## 5. Keep fixture results distinct from retail-map results

Stock ctf4 has patches and exercises the unified renderer. The `glass_fast.bsp`
fixture generated by
`artifacts/glass-investigation/make_glass_fast_fixture.py` triangulates its patch
control grids to exercise the fast path while retaining glass. Curves are
approximate. Do not present that fixture as an unmodified retail-map test.

For the Imperial-symbol decal check, use stock ctf4, join/noclip, first person,
and `setviewpos 848 -624 -200 -13 -24`. This puts the symbol behind the glass
roof. Compare effect strength 0, then blur 20/frost 1/opacity 0, then opacity 1.
The retained `artifacts/glass-investigation/decal-clear.png`, `decal-frosted.png`
and `decal-opaque.png` show the symbol becoming blurred and then covered by the
glass fill. A view looking over the roof is not a valid test of decal capture.

## 6. Keep reports; remove only the disposable run directory

The copied EXE/DLL and its logs can be removed after the test process exits.
Verify the exact resolved directory before recursively deleting it:

```powershell
$testProcess.Refresh()
if (-not $testProcess.HasExited) { throw 'The diagnostic client is still running' }
$resolvedRun = (Resolve-Path -LiteralPath $testRun).Path
if ($resolvedRun -ne (Join-Path $repoRoot 'glass-preview-run')) {
    throw 'Unexpected diagnostic run directory'
}
Remove-Item -LiteralPath $resolvedRun -Recurse -Force
```

Retain screenshots, config, performance JSON and stdout/stderr logs as evidence.
Do not remove or alter the normal installation or the user's active config.

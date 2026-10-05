# Brightness methods

In the main menu or in-game, open **Setup > Video > Color > Brightness method**.
The existing Brightness / gamma slider controls all three methods.

| Method | Console value | Behavior |
| --- | --- | --- |
| Shader (default) | `r_gammaMethod shader` or `0` | Corrects the finished game image. Uses the existing post pass, or enables the compact gamma pass when needed. |
| Baked textures | `r_gammaMethod baked` or `1` | Changes scene color texels before lighting. Restores originals on leaving this method. Extra RAM and temporary upload work; no gamma-only pass once settled. |
| Hardware | `r_gammaMethod hardware` or `2` | Changes the game monitor's Windows SDR output ramp. No game gamma-only pass. Pauses while unfocused. |

`r_gamma` remains the brightness value (0.5 to 3.0, neutral 1.0). The selected
method is archived. Legacy `r_bakedBrightness` config entries migrate, with an
explicit `r_gammaMethod` taking precedence. No `vid_restart` is needed.
Baked textures look different because they change materials before lighting;
see [Color grading](color-grading.md#baked-brightness) for details. Hardware
preserves the saved per-channel desktop calibration when composing its curve;
it is not guaranteed to look pixel-identical to Shader. Ordinary game screenshots
capture game pixels before the monitor ramp and will not show hardware brightness.

## Hardware restoration

Hardware brightness uses a hidden instance of the same executable as a guardian;
it loads no game assets, creates no window and runs no rendering. The game sends
setting/focus/monitor events to a background controller, which coalesces pending
adjustments. The render thread does not wait for display-driver calls.

Before changing a monitor, the guardian acquires an exclusive monitor lease and
atomically saves its original ramp under
`%LOCALAPPDATA%/DinurdoJK/gamma-recovery`. The backup includes monitor identity,
original/current/previous ramps and a checksum. Updates are durable before each
hardware write. Readback verifies driver acceptance. Other DinurdoJK instances
cannot capture an active guardian's altered ramp as a fresh desktop baseline.

Focus loss, monitor changes, method changes, renderer restarts/failures and normal
exit request restoration. Method changes wait for desktop restoration and any
pending baked-texture restoration before applying the next brightness method.
A separate guardian thread waits on the game process handle, so an unhandled
crash or forced game termination also triggers restoration. Input-pipe EOF is a
second restoration trigger. Explicit exit restoration covers the game's fast
exit, which skips ordinary Rust destructors.

At the next launch, stale backups are recovered before new baselines are captured,
even if Shader or Baked textures is selected. A backup is deleted only after
successful restoration. If the guardian itself is also terminated, recovery
waits until the next launch. Disconnecting/replacing a monitor, a failed driver
call or HDR activation can postpone recovery; the backup remains for a retry.
If another application has installed an unrelated calibration, recovery retains
the backup and reports the conflict rather than overwriting that calibration.

Windows HDR/advanced color and displays that reject or misreport gamma updates
fall back to Shader after desktop restoration is confirmed. If restoration cannot
be confirmed, game output stays neutral to avoid combining two brightness curves,
and the Color page reports the pending restoration. Driver behavior prevents an
absolute crash-restoration guarantee on every display.

## Performance and validation

Hardware brightness introduces no fullscreen pass, brightness shader or per-frame
display polling. Display work happens on settings/lifecycle events. Renderer
handoff retains one inactive preparation check per frame. Baked brightness also
retains its existing idle work check. LUTs, split toning, bloom and other enabled
post effects still have their ordinary costs. These paths have not been measured
as an end-to-end 3000 FPS gameplay benchmark.

Focused actual-source tests cover calibration-preserving curves, strict durable
snapshot validation, repeat adjustments, monitor handoff, driver failures/lying
readbacks, external calibration conflicts and Windows monitor mutex ownership.
Hidden process tests use a file-backed simulated display to check graceful exit,
forced parent termination and next-launch recovery after both processes die.
Frame-plan tests check Shader/Baked/Hardware gamma pass selection and retained
LUT/bloom behavior. Existing windowless Vulkan and DirectX 12 bake tests check
mip handling, original/alpha restoration, stale jobs and texture reuse.
These tests do not write the developer desktop's real gamma ramp; physical display
driver acceptance still needs an interactive check through the new menu.

Run the focused restoration tests with `scripts/test-gamma-recovery.ps1` after a
`build.ps1 -Fast` build (or pass `-Build`). This isolates the actual guardian
sources from unrelated client tests. It writes only under `target/gamma-audit`,
uses a simulated monitor, opens no visible windows and never changes real gamma.

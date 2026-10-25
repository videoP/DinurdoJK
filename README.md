# DinurdoJK

DinurdoJK is a Rust Jedi Academy (JKA) client targeting protocol 26. It aims to support demo playback and normal multiplayer while preserving OpenJK gameplay and presentation behavior. Rendering, assets, protocol handling, and client systems are implemented in Rust; pinned OpenJK and TaystJK/JAPRO movement code remains native C, alongside OpenJK collision. See [THIRD_PARTY.md](THIRD_PARTY.md) for source and license details.

## AI use

AI tools were used extensively to create and modify this codebase. AI-generated code and documentation may contain errors; review changes and validate them before relying on them.

## Current status

- Connects to live protocol-26 servers, processes gamestates and snapshots, sends client commands, and predicts player movement.
- Detects JAPRO servers and selects native JAPRO movement prediction. The Mod menu exposes movement preferences; see [mod support scope and review items](crates/jka-movement/MOD_SUPPORT.md).
- Plays demos through the same CGame presentation path used for live sessions.
- Loads user-provided Jedi Academy PK3 assets and BSP maps. No game assets are included.
- Ships DinurdoJK's optional RainyGlass masks in `assets/dinurdo-assets.pk3`; the build output copies it into the runtime base directory.
- Includes a native winit/wgpu renderer, Ghoul2 character rendering, world entities, effects, sabers, and a diagnostic tool (`jka-probe`). Compatibility and visual gaps remain.

## Near-term plan

Improve reliability and completeness of normal play: handle servers whose maps are missing locally, support vehicle state and presentation, add the scoreboard and remaining HUD/chat behavior, and complete predicted player events and audio. Continue validating demo playback and rendering against OpenJK behavior.

## Build and run on Windows

Install Rust, then double-click `build.cmd`, or run in PowerShell:

```powershell
.\build.ps1 -Fetch  # first build: fetch pinned dependencies
.\build.ps1         # later builds
.\build.ps1 -Fast  # quicker release iteration; larger/slower binary
```

Place your own game assets in `GameData/base` beside the executable, or pass a base directory with `--base`. Launch `launch.cmd` or `target/release/DinurdoJK.exe`. The diagnostic tool is `target/release/jka-probe.exe`.

## Emotes

The client can play local, cosmetic emotes from compiled bundles (none are included in this
repository). See [base/emotes/readme.md](base/emotes/readme.md) to use them and
[crates/jka-emotes](crates/jka-emotes/README.md) to build them.

## Source layout

See [the client source layout](docs/source-layout.md) for module responsibilities and frame-time constraints.

The Video -> Color menu groups film/external LUTs, split toning and brightness controls. See [color grading](docs/color-grading.md) for the saved settings and rendering cost.

`r_bakedSunCache 1` caches projected baked-sun subtraction in GPU lightmaps when
`r_bakedSunRemoval` changes. It defaults to `0` (per-pixel evaluation). The cache
uses half-float lightmaps and rebuilds when its source lightmaps change; warm
frames use normal filtered sampling. Heatmap and visibility previews stay live.
Turning removal off restores the original lightmaps, and turning the cache off
releases its extra textures. Unsupported bake formats and vertex-lit surfaces
retain their existing shader path. This changes rendering cost, not the sunlight
reconstruction or its current visual limitations.

`r_sunLighting 1` adds the live sun to BSP lighting after baked-sun subtraction,
using the current direction, color and intensity independently of removal strength
and its cache. Enable `r_dynamicShadows csm` for occlusion. It defaults to
`0`, which preserves legacy baked lighting. This light is evaluated during rendering.

`r_sunShadowStrength` controls legacy CSM shadow darkening from `0` to `1`
when `r_sunLighting` is off.
Its default `0.3` preserves the previous strength. Sun intensity and surface
orientation still affect the result. Live sun shadows also work while baked-sun
removal, its cache, or other baked-light adjustments are enabled.

`r_dynamicLights ray_traced` traces shadows for transient weapon and FX lights.
Static map point lights, authored surface lights, promoted shader emitters and
inferred lamp shadows use raster visibility. `r_localLightShadows` controls
cached cubemap shadows for static local lights, including in RT dynamic-light
mode. Sun shadows use `r_dynamicShadows off|blob|entity|csm`; CSM is the highest
sun setting. Saved RT/hybrid sun settings migrate to CSM.

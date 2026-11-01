--- a/README.md
+++ b/README.md
@@ -1,71 +1,89 @@
-# DinurdoJK
-
-DinurdoJK is a Rust Jedi Academy (JKA) client targeting protocol 26. It aims to support demo playback and normal multiplayer while preserving OpenJK gameplay and presentation behavior. Rendering, assets, protocol handling, and client systems are implemented in Rust; pinned OpenJK and TaystJK/JAPRO movement code remains native C, alongside OpenJK collision. See [THIRD_PARTY.md](THIRD_PARTY.md) for source and license details.
-
-## AI use
-
-AI tools were used extensively to create and modify this codebase. AI-generated code and documentation may contain errors; review changes and validate them before relying on them.
-
-## Current status
-
-- Connects to live protocol-26 servers, processes gamestates and snapshots, sends client commands, and predicts player movement.
-- Detects JAPRO servers and selects native JAPRO movement prediction. The Mod menu exposes movement preferences; see [mod support scope and review items](crates/jka-movement/MOD_SUPPORT.md).
-- Plays demos through the same CGame presentation path used for live sessions.
-- Loads user-provided Jedi Academy PK3 assets and BSP maps. No game assets are included.
-- Ships DinurdoJK's optional RainyGlass masks in `assets/dinurdo-assets.pk3`; the build output copies it into the runtime base directory.
-- Includes a native winit/wgpu renderer, Ghoul2 character rendering, world entities, effects, sabers, and a diagnostic tool (`jka-probe`). Compatibility and visual gaps remain.
-
-## Near-term plan
-
-Improve reliability and completeness of normal play: handle servers whose maps are missing locally, support vehicle state and presentation, add the scoreboard and remaining HUD/chat behavior, and complete predicted player events and audio. Continue validating demo playback and rendering against OpenJK behavior.
-
-## Build and run on Windows
-
-Install Rust, then double-click `build.cmd`, or run in PowerShell:
-
-```powershell
-.\build.ps1 -Fetch  # first build: fetch pinned dependencies
-.\build.ps1         # later builds
-.\build.ps1 -Fast  # quicker release iteration; larger/slower binary
-```
-
-Place your own game assets in `GameData/base` beside the executable, or pass a base directory with `--base`. Launch `launch.cmd` or `target/release/DinurdoJK.exe`. The diagnostic tool is `target/release/jka-probe.exe`.
-
-## Emotes
-
-The client can play local, cosmetic emotes from compiled bundles (none are included in this
-repository). See [base/emotes/readme.md](base/emotes/readme.md) to use them and
-[crates/jka-emotes](crates/jka-emotes/README.md) to build them.
-
-## Source layout
-
-See [the client source layout](docs/source-layout.md) for module responsibilities and frame-time constraints.
-
-The Video -> Color menu groups film/external LUTs, split toning and brightness controls. See [color grading](docs/color-grading.md) for the saved settings and rendering cost.
-
-`r_bakedSunCache 1` caches projected baked-sun subtraction in GPU lightmaps when
-`r_bakedSunRemoval` changes. It defaults to `0` (per-pixel evaluation). The cache
-uses half-float lightmaps and rebuilds when its source lightmaps change; warm
-frames use normal filtered sampling. Heatmap and visibility previews stay live.
-Turning removal off restores the original lightmaps, and turning the cache off
-releases its extra textures. Unsupported bake formats and vertex-lit surfaces
-retain their existing shader path. This changes rendering cost, not the sunlight
-reconstruction or its current visual limitations.
-
-`r_sunLighting 1` adds the live sun to BSP lighting after baked-sun subtraction,
-using the current direction, color and intensity independently of removal strength
-and its cache. Enable `r_dynamicShadows csm` for occlusion. It defaults to
-`0`, which preserves legacy baked lighting. This light is evaluated during rendering.
-
-`r_sunShadowStrength` controls legacy CSM shadow darkening from `0` to `1`
-when `r_sunLighting` is off.
-Its default `0.3` preserves the previous strength. Sun intensity and surface
-orientation still affect the result. Live sun shadows also work while baked-sun
-removal, its cache, or other baked-light adjustments are enabled.
-
-`r_dynamicLights ray_traced` traces shadows for transient weapon and FX lights.
-Static map point lights, authored surface lights, promoted shader emitters and
-inferred lamp shadows use raster visibility. `r_localLightShadows` controls
-cached cubemap shadows for static local lights, including in RT dynamic-light
-mode. Sun shadows use `r_dynamicShadows off|blob|entity|csm`; CSM is the highest
-sun setting. Saved RT/hybrid sun settings migrate to CSM.
+# DinurdoJK
+
+**A modern multiplayer client for *Star Wars Jedi Knight: Jedi Academy*, built in Rust with a custom WGPU renderer.**
+
+DinurdoJK lets you play on existing Jedi Academy multiplayer servers with a new rendering engine, extensive graphics and performance options, and tools for demos, cinematics, and modded content. It uses the original game's maps and assets and aims to preserve its gameplay and movement rather than reinvent them.
+
+**Status:** Playable, actively developed, and still being refined. Compatibility and visual issues remain; see [Project status](#project-status).
+
+## Features
+
+### A modern renderer, your choice of visuals
+
+- Render classic JKA maps, shaders, models, lightsabers, and effects through a native Rust/WGPU renderer.
+- Choose between a restrained, classic-style look and optional enhancements such as advanced lighting, dynamic shadows, reflections, volumetric fog and clouds, weather, and enhanced water.
+- Customize post-processing with anti-aliasing, bloom, depth of field, HDR-related controls, and color grading.
+- Experiment with optional visual effects and physics, including grass, cloth, and character jiggle physics.
+
+### Performance and responsiveness
+
+- Performance-focused graphics presets let you trade visual effects for frame rate.
+- Visibility culling, batching, and shader/pipeline optimizations target the high frame rates common in JKA multiplayer.
+- Raw mouse input, subframe input handling, and configurable display/presentation settings help keep controls responsive.
+- In-game diagnostics make it easier to identify rendering and performance problems.
+
+*Frame rates depend on your hardware, map, server, and enabled effects; no fixed FPS is guaranteed.*
+
+### Multiplayer and movement
+
+- Connect to existing **Jedi Academy protocol-26** servers without requiring a custom server.
+- Play on vanilla servers and test compatibility with popular mods such as **JA+** and **jaPRO**. Some mod-specific behavior is still a work in progress.
+- Browse Internet and LAN servers, save favorites, revisit recent servers, and filter or search for players.
+- Download missing maps and other required server assets through the client when supported.
+- Use familiar player, Force, and lightsaber customization, chat, HUD, scoreboard, and spectator features.
+- Explore movement-focused tools such as race ghosts, strafe helpers, trails, and spectator camera controls.
+
+Movement prediction uses pinned native code from OpenJK and TaystJK/jaPRO where appropriate, with the aim of matching established JKA behavior.
+
+### Demos and Render Studio
+
+- Record and play JKA multiplayer demos, with playback controls, camera modes, and demo browsing.
+- Use **Render Studio**, the built-in timeline editor, to assemble scenes from demos and character animations.
+- Edit camera motion, shot timing, lighting, transitions, and audio while previewing the result in-engine.
+- Export finished sequences using FFmpeg, with formats including H.264, ProRes, VP9, GIF, and NVIDIA GPU-encoded H.264/HEVC where supported. Some formats also support transparency or separate depth output.
+
+### Tools for players and modders
+
+- **Asset Viewer** — browse and inspect installed game assets and models.
+- **Map Viewer/Editor** — inspect and edit supported `.map` source files.
+- **PK3 Manager** — enable and disable installed PK3 archives and rebuild the active asset set.
+- **Material and trace inspection** — investigate world surfaces, shaders, and rendering behavior in-game.
+- **Solo Game** — load maps locally without joining an Internet server.
+
+## Project status
+
+DinurdoJK is a **playable development build**, not a finished replacement for every JKA setup. Multiplayer, demos, rendering, and the tools above are implemented, but some server/mod combinations, custom assets, and rendering situations still need fixes.
+
+The goal is compatibility with existing Jedi Academy multiplayer and its community, not changes to server-side game rules. The graphics enhancements and creator tools are client-side features.
+
+## Getting started (Windows)
+
+You need a Windows machine, a Rust/MSVC build environment, and **your own copy of Jedi Academy's game assets**. Retail assets are not included in this repository.
+
+From PowerShell in the repository root:
+
+```powershell
+.\build.ps1 -Fetch   # First build: fetch pinned dependencies
+.\build.ps1          # Later release builds
+.\build.ps1 -Fast    # Faster development iteration
+```
+
+Run `target\release\DinurdoJK.exe`. Place your game's `base` folder (with the original `assets0.pk3`–`assets3.pk3`) beside the executable, or point the client at an existing installation:
+
+```powershell
+.\target\release\DinurdoJK.exe --base "C:\Games\Jedi Academy\GameData\base"
+```
+
+In the main menu, choose **Play → Join a Game**, **Solo Game**, or **Play Demo**. Render Studio, Asset Viewer, Map Viewer, and PK3 Manager are under **Developer Tools**.
+
+**Optional:** Put a compatible `ffmpeg.exe` next to `DinurdoJK.exe` to enable Render Studio video export.
+
+## Technical notes and credits
+
+DinurdoJK uses **Rust**, **WGPU/WGSL**, **winit**, **taffy**, and **egui** for most client systems, graphics, and UI. Selected native **OpenJK**, **TaystJK/jaPRO**, and collision/movement code is retained for compatibility. It is an independent client, not an official Jedi Academy release or a single-player campaign port.
+
+For implementation details, see [mod compatibility notes](crates/jka-movement/MOD_SUPPORT.md), [emote documentation](crates/jka-emotes/README.md), and [third-party source and license information](THIRD_PARTY.md).
+
+## AI-assisted development
+
+AI tools have been used extensively to help develop and document DinurdoJK. AI-generated changes can contain mistakes and should be reviewed and tested against actual behavior and authoritative upstream implementations.

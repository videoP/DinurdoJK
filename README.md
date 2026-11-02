# DinurdoJK

**A modern multiplayer client for *Star Wars Jedi Knight: Jedi Academy*, built in Rust with a custom WGPU renderer.**

DinurdoJK lets you play on existing Jedi Academy multiplayer servers with a new rendering engine, extensive graphics and performance options, and tools for demos, cinematics, and modded content. It uses the original game's maps and assets and aims to preserve its gameplay and movement rather than reinvent them.

**Status:** Playable, actively developed, and still being refined. Compatibility and visual issues remain; see [Project status](#project-status).

## Features

### A modern renderer, your choice of visuals

- Render classic JKA maps, shaders, models, lightsabers, and effects through a native Rust/WGPU renderer.
- Choose between a restrained, classic-style look and optional enhancements such as advanced lighting, dynamic shadows, reflections, volumetric fog and clouds, weather, and enhanced water.
- Customize post-processing with anti-aliasing, bloom, depth of field, HDR-related controls, and color grading.
- Experiment with optional visual effects and physics, including grass, cloth, and character jiggle physics.

### Performance and responsiveness

- Performance-focused graphics presets, including **Minimal (Unified)**, let you trade visual effects for frame rate.
- Visibility culling, batching, and shader/pipeline optimizations target the high frame rates common in JKA multiplayer.
- Raw mouse input, subframe input handling, and configurable display/presentation settings help keep controls responsive.
- In-game diagnostics make it easier to identify rendering and performance problems.

*Frame rates depend on your hardware, map, server, and enabled effects; no fixed FPS is guaranteed.*

### Multiplayer and movement

- Connect to existing **Jedi Academy protocol-26** servers without requiring a custom server.
- Play on vanilla servers and test compatibility with popular mods such as **JA+** and **jaPRO**. Some mod-specific behavior is still a work in progress.
- Browse Internet and LAN servers, save favorites, revisit recent servers, and filter or search for players.
- Download missing maps and other required server assets through the client when supported.
- Use familiar player, Force, and lightsaber customization, chat, HUD, scoreboard, and spectator features.
- Explore movement-focused tools such as race ghosts, strafe helpers, trails, and spectator camera controls.

Movement prediction uses pinned native code from OpenJK and TaystJK/jaPRO where appropriate, with the aim of matching established JKA behavior.

### Demos and Render Studio

- Record and play JKA multiplayer demos, with playback controls, camera modes, and demo browsing.
- Use **Render Studio**, the built-in timeline editor, to assemble scenes from demos and character animations.
- Edit camera motion, shot timing, lighting, transitions, and audio while previewing the result in-engine.
- Export finished sequences using FFmpeg, with formats including H.264, ProRes, VP9, GIF, and NVIDIA GPU-encoded H.264/HEVC where supported. Some formats also support transparency or separate depth output.

### Tools for players and modders

- **Asset Viewer** — browse and inspect installed game assets and models.
- **Map Viewer/Editor** — inspect and edit supported `.map` source files.
- **PK3 Manager** — enable and disable installed PK3 archives and rebuild the active asset set.
- **Material and trace inspection** — investigate world surfaces, shaders, and rendering behavior in-game.
- **Solo Game** — load maps locally without joining an Internet server.

## Project status

DinurdoJK is a **playable development build**, not a finished replacement for every JKA setup. Multiplayer, demos, rendering, and the tools above are implemented, but some server/mod combinations, custom assets, and rendering situations still need fixes.

The goal is compatibility with existing Jedi Academy multiplayer and its community, not changes to server-side game rules. The graphics enhancements and creator tools are client-side features.

## Getting started (Windows)

You need a Windows machine, a Rust/MSVC build environment, and **your own copy of Jedi Academy's game assets**. Retail assets are not included in this repository.

From PowerShell in the repository root:

```powershell
.\build.ps1 -Fetch   # First build: fetch pinned dependencies
.\build.ps1          # Later release builds
.\build.ps1 -Fast    # Faster development iteration
```

Run `target\release\DinurdoJK.exe`. Place your game's `base` folder (with the original `assets0.pk3`–`assets3.pk3`) beside the executable, or point the client at an existing installation:

```powershell
.\target\release\DinurdoJK.exe --base "C:\Games\Jedi Academy\GameData\base"
```

In the main menu, choose **Play → Join a Game**, **Solo Game**, or **Play Demo**. Render Studio, Asset Viewer, Map Viewer, and PK3 Manager are under **Developer Tools**.

**Optional:** Put a compatible `ffmpeg.exe` next to `DinurdoJK.exe` to enable Render Studio video export.

On the `jk2-support` branch, the JK2 installation is configured separately from
JKA's `fs_cdpath`:

```text
set fs_jk2path "C:\Games\JK21.02\GameData"
```

The setting persists in `DinurdoJK-paths.ini` and takes effect after restarting.
It records the JK2 root for upcoming JK2 support; it does not add those packages
to the JKA search path. Inspect a JK2 1.02/1.04 demo without starting the renderer:

```powershell
.\target\release\DinurdoJK.exe --jk2-demo-info "CoS-WickedR1"
.\target\release\DinurdoJK.exe --jk2-demo-info "imp495xAMoMAx"
```

The in-game command `jk2_demoInfo <name or absolute path>` provides the same
version, map, duration, and snapshot-integrity report. Names resolve under the
JK2 installation's `base/demos` and `base/jk2demos` folders. JK2 visual playback,
live connections, and gameplay are not implemented yet. See
[JK2 implementation notes](JK2_SUPPORT.md) for source revisions and scope.

## Technical notes and credits

DinurdoJK uses **Rust**, **WGPU/WGSL**, **winit**, and **egui** for most client systems, graphics, and UI. Selected native **OpenJK**, **TaystJK/jaPRO**, and collision/movement code is retained for compatibility. It is an independent client, not an official Jedi Academy release or a single-player campaign port.

For implementation details, see [mod compatibility notes](crates/jka-movement/MOD_SUPPORT.md), [emote documentation](crates/jka-emotes/README.md), and [third-party source and license information](THIRD_PARTY.md).

## AI-assisted development

AI tools have been used extensively to help develop and document DinurdoJK. AI-generated changes can contain mistakes and should be reviewed and tested against actual behavior and authoritative upstream implementations.

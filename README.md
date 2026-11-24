# DinurdoJK

DinurdoJK is a multiplayer client for **Jedi Knight: Jedi Academy (JKA)** and **Jedi Knight II: Jedi Outcast (JK2)**. It uses a custom engine written primarily in Rust with a WGPU renderer.

The project prioritizes performance, low input latency, compatibility with existing multiplayer servers, and accuracy to the original games. Graphics enhancements are optional.

## Features

### Multiplayer and Gameplay

- JKA 1.01 and JK2 1.02/1.04 support.
- Compatibility with base JKA, JA+, jaPRO and JK2MV servers.
- Native movement and prediction code from established upstream implementations.
- Race ghosts, strafe helpers and 3D strafe trails.
- Customizable HUD, scoreboard and spectator modes.
- Server browser, automatic downloads and automatic login.
- Custom character animations and emotes.
- Local AI-powered chat translation and filtering.

### Performance

- Vulkan and DirectX 12 rendering.
- Raw mouse input, subframe input and camera late latching.
- Heavily multithreaded, asset loading and pipeline compilation.
- Optimized map loadtimes.
- Graphics presets for competitive play and visual quality.
- CPU, GPU, frame time and input latency diagnostics.

### Rendering

- Original Quake 3 shader and material support.
- GPU-accelerated Ghoul2 skeletal animation.
- Physically based rendering (PBR).
- Clustered dynamic lighting.
- Hardware ray-traced lighting and shadows.
- Cascaded shadow maps and point light shadows.
- Planar and screen-space reflections.
- Baked and real-time world relighting.
- Automatic light generation from existing map effects.
- HDR rendering and tone mapping.
- TAA, FXAA, SMAA and NVIDIA DLAA (optional).
- Bloom, halation, depth of field, motion blur and color grading.
- CRT display emulation.

### Environment and Simulation

- Dynamic time of day and atmospheric sky rendering.
- Volumetric clouds, fog and cloud shadows.
- FFT and Gerstner ocean simulation.
- GPU-based river and shallow-water simulation.
- Water foam, caustics, wakes and underwater effects.
- Rain, snow, surface wetness, puddles and ripples.
- Procedural grass.
- Wind simulation.
- Ragdoll, cloth and jiggle physics.
- NVG, IR, Thermal vision simulation.

### Audio

- Original game audio and positional sound.
- Valve Steam Audio integration.
- Binaural audio with HRTF.
- Geometry-based sound occlusion and transmission.
- Original EAX environmental audio through EAL files and OpenAL Soft.
- Separate volume controls for effects, voices and music.

### Demos and Render Studio

- Multiplayer demo recording and playback.
- Demo timeline with playback and seek controls.
- First-person, third-person, orbit and free cameras.
- Smooth camera motion and variable playback speed.
- Integrated cinematic timeline editor.
- Camera, character, lighting and audio tracks.
- Video export through FFmpeg.
- H.264, HEVC, ProRes, VP9 and GIF export.
- Hardware video encoding.
- Transparency and depth export.
- Spout2 real-time video output.

### Developer Tools

- Asset Viewer.
- Map Viewer and Editor.
- PK3 Manager.
- Material and shader inspection.
- Collision and trace inspection.
- Rendering and physics debugging tools.
- Second-monitor companion window.

## Project Status

DinurdoJK is playable and under active development. JK2 support and some rendering, simulation and mod compatibility features are experimental.

The project does not require modifications to existing multiplayer servers. Single-player campaign support is not a project goal.

## Build

### Requirements

- Windows
- Rust
- Microsoft Visual C++ build tools
- CMake and libclang
- Original game assets

Run the following commands from the repository root in PowerShell:

```powershell
.\build.ps1 -Fetch
.\build.ps1
```

Additional build options:

```powershell
.\build.ps1 -Fast
.\build.ps1 -Faster
.\build.ps1 -Check
.\build.ps1 -Dlss
```

The release executable is located at:

`target\release\DinurdoJK.exe`

Copy the original game's `base` directory next to the executable or specify an existing installation:

```powershell
.\target\release\DinurdoJK.exe --base "C:\Games\Jedi Academy\GameData\base"
```

For video export, place `ffmpeg.exe` next to the executable.

Original game assets are not included in this repository.

## Technology

- Rust
- WGPU / WGSL
- winit
- egui
- Rapier
- OpenAL Soft
- Rodio
- llama.cpp
- FFmpeg

## Credits

DinurdoJK uses code and technical references from OpenJK, TaystJK, jaPRO, JK2MV, MVSDK and other open-source projects.

AI-assisted development is used extensively throughout the project.
# DinurdoJK

DinurdoJK is a Rust Jedi Academy (JKA) client targeting protocol 26. It aims to support demo playback and normal multiplayer while preserving OpenJK gameplay and presentation behavior. Rendering, assets, protocol handling, and client systems are implemented in Rust; pinned OpenJK and TaystJK/JAPRO movement code remains native C, alongside OpenJK collision. See [THIRD_PARTY.md](THIRD_PARTY.md) for source and license details.

## AI use

AI tools were used extensively to create and modify this codebase. AI-generated code and documentation may contain errors; review changes and validate them before relying on them.

## Current status

- Connects to live protocol-26 servers, processes gamestates and snapshots, sends client commands, and predicts player movement.
- Detects JAPRO servers and selects native JAPRO movement prediction. The Mod menu exposes movement preferences; see [mod support scope and review items](crates/jka-movement/MOD_SUPPORT.md).
- Plays demos through the same CGame presentation path used for live sessions.
- Loads user-provided Jedi Academy PK3 assets and BSP maps. No game assets are included.
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

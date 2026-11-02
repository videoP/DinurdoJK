# JK2 1.02 / 1.04 support

## Agreed scope

Recorded November 2, 2026, following review of the initial handoff and user
clarifications. These decisions supersede conflicting instructions in that handoff.

- Extend DinurdoJK into one executable supporting JKA protocol 26 and JK2
  1.02 / 1.04 multiplayer. Preserve the existing JKA prediction implementations.
- Develop on `jk2-support`, created from the clean committed `main` baseline
  `40f6eac188c22af3591e11f895361717b3cb7993`. Keep this work separate until stable.
- Use JK2MV for engine/protocol behavior and MVSDK for native shared gameplay
  and prediction. Preserve MVSDK's intentional gameplay fixes, including its
  differences from original Raven 1.02 behavior. Do not introduce a separate
  original-bug compatibility mode without a demonstrated requirement.
- Extend the native C / Rust FFI approach; do not implement approximate Rust
  physics or reinterpret JKA native structures as JK2 structures.
- Keep DinurdoJK's current menus, configurable HUD, and visual presentation.
  Reuse presentation infrastructure while mapping game-specific identifiers and
  behavior correctly. Ask the user about any substantial presentation difference.
- Apply source changes directly. Zipped patch deliveries are not required.
- Keep `fs_cdpath` as the JKA retail fallback. Add a separate `fs_jk2path`
  pointing to the JK2 GameData directory rather than a combined list of roots.
  JK2/JKA package names and qpaths overlap, so selecting packages must follow
  the active game and version. Never mount the JK2 root in the JKA search path.
- Protect the JKA rendering fast path and Minimal (Unified) performance by design.
  The user will evaluate performance as implementation progresses; no numerical
  acceptance threshold or benchmark has been agreed.
- The user has now authorized normal fast builds and focused tests for the JK2
  milestone. Follow `AGENTS.md` and the existing fast-profile/cache configuration.
- Use live in-game cvars for diagnostics. Preserve upstream licensing and notices
  when source is incorporated.

## Source provenance

Both reference checkouts were clean when inspected. The standalone MVSDK HEAD
matches the gitlink in JK2MV exactly. Use these revisions as the initial pins;
do not silently follow upstream branch changes.

| Source | Local checkout | Upstream | Commit |
| --- | --- | --- | --- |
| JK2MV | `D:\Code\jk2mv` | <https://github.com/mvdevs/jk2mv> | `7d601454c3db68492289d4d4e3dc30bff39e4246` |
| MVSDK | `D:\Code\mvsdk` | <https://github.com/mvdevs/mvsdk> | `4c7c463638192aead1ff3de8a88cc5de266bf3a2` |

The older `jk2mv-master` / `mvsdk-master` archive extractions are superseded by
these Git checkouts. External checkouts are reference sources; implementation
must not depend on absolute paths to their files at build time.

## Assets and validation material

User-provided installation: `C:\Games\JK21.02\GameData`.

Read-only inspection found `base/assets0.pk3`, `assets1.pk3`, `assets2.pk3`,
`assets5.pk3`, additional addon PK3s, and JK2MV client / dedicated-server
executables. Package contents, checksums, and executable versions have not been
validated. The directory name must not determine the selected protocol or gameplay.

JK2MV `src/qcommon/files.cpp` marks assets0/1 as compatible with all three JK2
versions, assets2 with 1.03/1.04, and assets5 with 1.04. Mount and pure-reference
selection must respect the active version even when one installation contains
all packages. Keep JK2 and JKA search paths and caches correctly separated.

The user supplied `base/demos/jk2ffa.dm_15`, which identifies itself as JK2
1.03. The user explicitly kept support limited to 1.02/1.04; reject that recording
with a version-specific error rather than decoding it as 1.02. Full 1.03 support
remains deferred.

The user subsequently supplied `base/jk2demos`. At validation time it contained
24 unpacked demos: 20 identified as 1.02 and four as 1.04, plus one RAR archive
not inspected or extracted. Another recording arrived during implementation;
the final corpus run covered all 24 unpacked demos then present.
The diagnostics search both `base/demos` and `base/jk2demos` for named recordings.
No live test servers have been agreed. Do not claim connection, movement,
pure-server, rendering, or performance validation until actually performed.

## Initial source audit

- JK2MV `src/qcommon/common.cpp`, `MV_GetCurrentProtocol`: 1.02 and 1.03 use
  protocol 15; 1.04 uses protocol 16. `src/client/cl_main.cpp` also consults the
  server status/version to distinguish 1.03. Protocol number alone is insufficient.
  JK2MV `src/qcommon/msg.cpp` selects its `*Fields15` tables specifically for
  1.02 and the other tables for later versions. Keep version detection distinct
  from gameplay selection. Treat 1.03 support as deferred and detect it explicitly
  rather than silently decoding it as 1.02.
- JK2MV message/configstring limits and default server port differ from the
  current JKA values: JK2 has `MAX_MSGLEN=16384`, `MAX_CONFIGSTRINGS=1400`, and
  port 28070. Audit all networking constants rather than changing only netfields.
- `crates/jka-protocol/src/server.rs` always consumes JKA-specific playerstate
  selectors and can decode a separate vehicle playerstate. JK2 needs its own
  decoding path, schemas, and limits. Existing protocol-26 defaults must retain
  their behavior.
- `crates/jka-client/src/server_browser.rs` currently filters and queries using
  the single JKA protocol constant. Discovery, connection preflight, and session
  construction all need game/version selection.
- `crates/jka-client/src/pure.rs` hardcodes JKA assets3 and its checksum, including
  cgame/ui references. JK2 needs its own authoritative pure-reference rules;
  changing a filename is insufficient. Retail-package classification and download
  handling also currently assume JKA assets0 through assets3.
- MVSDK `code/game/bg_multiversion.c` owns `jk2version`, `jk2gameplay`, and
  `jk2startversion`. `MV_SetGamePlay` also selects weapon-data and Force-cost
  tables. Preserve these distinctions, initialization, and mutable-global safety
  in the native bridge. Include multiversion dependencies and animation mapping,
  not just the movement source files listed in the original handoff.
- MVSDK `code/game/bg_pmove.c` deliberately changes 1.02 spectator movement,
  the client-0 roll restriction, and swimming animation behavior. Those choices
  are accepted as part of the selected MVSDK authority. Prediction matching an
  original server in those cases must be measured rather than assumed.
- Weapons, events, animation IDs, configstring ranges, and other gameplay enums
  need explicit interpretation by game even where the visible assets are shared.
  Reusing the HUD must not mean passing raw JK2 weapon IDs to JKA-only consumers.
- `crates/jka-assets/src/ghoul2.rs` already offers native GLM loading alongside
  legacy JK2-to-JKA bone remapping, and the skeleton code supports JK2 rigs.
  Audit the caller's rig/animation selection before adding another model loader.

## Implementation and validation order

1. Source provenance and agreed-scope audit (recorded here).
2. JK2 1.04 connection, gamestate, snapshots, and isolated asset selection.
3. Stable 1.04 spectating/rendering using the existing presentation.
4. JK2 1.02 network and asset compatibility with explicit version detection.
5. Native MVSDK prediction, state conversion, usercmd replay, and gameplay tables.
6. Differential command replay against pinned MVSDK, reporting the first divergent
   command and field. Include origin, velocity, flags, timers, Force, and saber state;
   do not hide discrepancies through visual smoothing.
7. Validate joining, spawning, spectating, pure negotiation, map changes, chat,
   duels, weapons, saber transitions, Force behavior, and reconnection separately
   on both versions. Check JKA regressions and user-observed performance.

Initial scope is stock-compatible multiplayer. Specific mod targets remain
unspecified; arbitrary client QVM/module compatibility is not promised. Demo
decode/replay fixtures can support validation without making complete JK2 demo
tooling or Render Studio parity a prerequisite for the first playable milestone.

## Current implementation status

The first foundation change adds `fs_jk2path` to the root settings, console cvars,
startup `+set` handling, bootstrap INI load/save, `writeconfig`, and `path`
diagnostics. This setting is separate from the JKA mount search and currently
only records the root for upcoming JK2 session support. The existing filesystem
priority regression test now configures both retail roots and still expects only
the JKA root to be searched.

This installation's ignored `target/release/DinurdoJK-paths.ini` now records
`fs_jk2path=C:\Games\JK21.02\GameData`, preserving its existing JKA path. No personal
path is hardcoded as a source default.

The next increment adds an isolated `jka_protocol::jk2` message decoder with
pinned generated 1.02/1.04 netfields, JK2 opcodes/limits, gamestates, baselines,
playerstate arrays, packet-entity deltas, snapshot history, and configstring
updates. No JKA field layouts, native structs, or vehicle selectors are reused.

The game exposes a headless diagnostic entry point:

```powershell
.\target\release\DinurdoJK.exe --jk2-demo-info "CoS-WickedR1"
.\target\release\DinurdoJK.exe --jk2-demo-info "imp495xAMoMAx"
```

The in-game console command `jk2_demoInfo <name or absolute path>` performs the
same inspection on the existing background I/O worker. Neither entry point
mounts JK2 assets or exercises JKA native prediction. `jka-probe jk2-demo <path>`
also exposes the streaming inspector if that optional diagnostic binary is built.

Two small regression fixtures retain the first 16 consecutive records of actual
1.02/1.04 recordings, with synthetic end markers. Payloads are unmodified; source
and fixture hashes are recorded in `crates/jka-protocol/tests/fixtures/jk2-demo-prefixes.json`.

All 54 protocol unit tests passed, including five JK2 tests covering real-demo
full/delta snapshots, signed arrays, baseline inheritance/removal, raw float bits,
lost-delta recovery, truncation rejection, and explicit 1.03 rejection. Existing
JKA unit tests passed in the same run. Both existing filesystem-layer tests also
passed, including the case configuring JK2 and JKA retail roots together.

Validation on November 2, 2026:

- `build.ps1 -Fast` succeeded and staged `target/release/DinurdoJK.exe`.
  It reused the standard cache (430 fresh packages, seven compiled), with about
  276 MiB additional drive usage reported by the build script. No separate target
  directory, alternate profile, or compiler-flag configuration was introduced.
- The built game's headless inspector completed all 24 recordings successfully:
  1,157,366 records, 1,157,342 accepted snapshots (703 full and 1,156,639 delta),
  zero missing-base snapshot drops, zero nonfinite player origin/velocity poses.
  Every recording reached its end marker.
- Bare-name lookup succeeded for `CoS-WickedR1` and `imp495xAMoMAx` using the
  configured JK2 root. Absolute paths, including filenames containing spaces,
  were exercised across the corpus. The original `jk2ffa` recording failed with
  the expected explicit 1.03 error.
- Per-recording reports are in the local ignored
  `artifacts/jk2-demo-validation.json`; totals are in
  `artifacts/jk2-demo-validation-summary.json`.
- This validates decoding against real recordings and synthetic edge cases. It
  is not a differential comparison against a running JK2MV decoder, nor a test of
  native prediction, visual presentation, server admission, or JKA FPS. The
  in-game command uses the same tested inspector on a background worker; its GUI
  dispatch has not been manually exercised in a game window.

Live JK2 connectivity, version-specific asset mounting, rendering/playback,
native prediction, and gameplay remain to be implemented. Diagnostic decoding
is a first testable milestone, not a claim of playable JK2 support.

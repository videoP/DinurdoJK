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
- Do not compile locally until explicitly requested. When builds are requested,
  follow `AGENTS.md` and the existing fast-profile/cache configuration.
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

No replay fixtures or agreed test servers have been supplied. The user will
provide validation material when needed. Do not claim connection, movement,
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

JK2 networking, version-specific mounting, prediction, and gameplay are not yet
implemented. No builds, test execution, or gameplay/performance validation have
been run.

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
diagnostics. This setting is separate from the JKA mount search and supplies the
root for JK2 demo sessions. The filesystem priority regression test configures
both retail roots and still expects only JKA roots while JKA is active.

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

## Visual demo playback milestone

The built game now plays 1.02/1.04 recordings with the existing renderer, HUD,
demo clock, pause, seek, and camera controls. In the console:

```
jk2_demo CoS-WickedR1
jk2_demo imp495xAMoMAx
```

The first recording is 1.02 on ctf_yavin; the second is 1.04 on ctf_imperial.
Both maps are present in the supplied retail assets. An absolute demo path also
works with `jk2_demo` or as a positional executable argument. Name lookup uses
the configured JK2 installation's `base/demos` and `base/jk2demos` directories.

An offline adapter decodes the original JK2 recording, maps semantic identifiers,
and constructs an internal in-memory stream for the existing presentation code.
It does not overwrite or export the original demo or run movement prediction.
The mapping includes configstrings, weapons/items, entity types, events, saber
moves, animation IDs and restart bits, flags, stats, and powerups. Generated
tables come from pinned MVSDK and vendored OpenJK enums; regeneration is optional
and reference checkouts are not needed to build. JK2-only animation names retain
their actual frames from JK2's animation.cfg in 353 additional presentation slots.
Only a JK2 asset reader enables those slots; JKA keeps its original 1,543 slots.
The native JK2 saber hilt/length is installed for JK2 sessions.

JK2 asset reads use a separate local `jk2` namespace and `fs_jk2path`; they never
fall back to JKA's overlapping packages. Version 1.02 excludes assets2/5, while
1.04 includes them. Session replacement invalidates pending work and catalogs;
disconnect restores the JKA roots. Existing user settings remain shared.

Validation on November 2, 2026:

- Fast build succeeded using the existing standard cache and flags.
- All 55 protocol tests passed in the initial implementation run. After fixing
  animation restart-bit and client-info separator handling, the focused real-demo
  presentation test passed again. Five focused JK2 asset tests passed after the
  final parser isolation change, including existing rig tests and the new authored
  clip and filesystem isolation cases.
- The final conversion check passed all 24 recordings / 1,157,342 snapshots.
  Re-decoded presentation state preserved player poses, velocities, view angles,
  timing, area masks, entity identities/trajectories, and player animation restart
  mapping. This compares the adapter with the source decoder; it is not an
  independent comparison against a running JK2MV client.
- Offscreen rendered playback loaded both retail maps in one process, sought to
  40 seconds / 4 seconds, paused, and captured screenshots. After disconnect,
  JKA loaded its menu world and original animation table from JKA packages.
  The user's saved game config hash was unchanged. Model-frame diagnostics
  confirmed an authored jump clip was selected during the 1.02 capture. A second
  capture after advancing one second showed the animated jump/saber pose.
- Local reports: `artifacts/jk2-presentation-validation-final.json` and
  `artifacts/jk2-playback-final.log`. The render check is not an FPS regression
  benchmark or proof of native JK2 pose/effect parity.

Known limits: conversion currently runs before playback starts and can stall the
UI on long recordings; the generated stream is limited to 512 MiB. Some JK2-only
events are omitted and counted (including body-queue copies and certain taunts);
complete corpse/effect parity is not claimed. Custom maps still need their PK3s.
Saga and 1.03 are explicitly unsupported. Render Studio parity is unvalidated.
Pausing immediately after a seek can freeze the initial animation blend in a
neutral pose; briefly resuming playback allows the pose to settle. This remains
a presentation limitation to address before treating paused seeks as exact poses.

User validation accepted both demos. The later report of incorrect head/hair
textures in `imp495xAMoMAx.dm_16` led to the Patch20 skin-selection fix below.
Confirmation against the user's original close-up remains a visual test.

## Skin surface lookup fix (Patch20)

Applied `Development Docs/Patch20/jk2_skin_surface_fix.patch` with its paths rooted
under `crates/`, plus the worker-context correction found during review.

- Local JK2MV `tr_image.cpp` preserves `_off` names unless JKA skin conversion
  mode 2 is requested; its default mode 1 preserves them. `tr_ghoul2.cpp` chooses
  the first matching skin entry. Both behaviors agree with the patch's approach.
- Retail Jedi skins contain separate `head`/`head_off` and
  `head_face`/`head_face_off` entries. The native GLM marks the first pair members
  visible (`0x0`) and the `_off` counterparts hidden (`0x2`). Default skins use
  Jedi textures; red/blue skins use Prisoner textures. Normalizing these names
  and keeping the last duplicate assigned the alternate head texture.
- JK2 skin loading now preserves original names. JKA retains its existing
  `_off` normalization, with first-match precedence for duplicate names. Both
  character and static-model registration use the same ordered lookup.
- Background workers previously reconstructed their VFS from directory names,
  losing non-pure JK2 identity. The patch alone would therefore miss demo and
  non-pure worker loads. Workers now fork a captured VFS context, preserving
  game identity, package policy and override preference. No filesystem mount
  order, mesh, UV, or visibility-flag changes are needed.
- Five skin-parser tests and two client regressions pass. The worker regression
  covers JKA, 1.02 and 1.04 after a session change, using the actual worker skin
  loader. A pre-existing malformed tag fixture also failed in the October 23
  cached test executable; it now uses the native `tag_weapon,` syntax, without
  changing the parser's tag behavior.
- Retail surface/texture review: `artifacts/jk2-skin-retail-review.txt`.
  Test reports: `artifacts/jk2-skin-assets-tests.log` and
  `artifacts/jk2-skin-client-test.log`.
- The fast build succeeded (`artifacts/jk2-skin-build.log`). The xAMo replay
  loaded `models/players/jedi/model_blue.skin`, rendered with the preserved
  surface names, captured a screenshot, and returned to JKA. The top-of-head
  view shows the young brown hair texture; the user's exact face close-up still
  needs confirmation. Saved config hashes were unchanged. Runtime report:
  `artifacts/jk2-skin-replay.log`.

## Live spectating preview

The next increment adds explicit live connections for both versions:

```
jk2_connect 1.04 host:28070
jk2_connect 1.02 host:28070
```

Omitting the port uses 28070; ordinary `connect` retains JKA's 29070 default.
Reconnect remembers the selected game/version. Connection preflight checks the
advertised protocol and the gamestate independently verifies the version.
Protocol 15 gamestates identifying 1.03 are rejected.

The JK2 wire decoder owns JK2 delta history. A separate presentation adapter
supplies existing CGame consumers with mapped snapshots/configstrings without
re-encoding or retaining a demo stream. The network reliable-command ring keeps
the original server text and sequence numbers for acknowledgements, XOR, and
command hashes. Split configstrings are assembled before mapping their indices.
JK2MV's 16 KiB limit, high-ASCII/percent key normalization, usercmd hash rules,
and full-time encoding for backwards usercmd times are selected only for JK2.
JKA keeps its existing protocol defaults and byte behavior.

This preview is for spectating: the native JKA predictor is explicitly bypassed
for JK2 live sessions, JKA selection/generic-command IDs are not transmitted,
and `team spectator` is requested on admission. Commands to join playing teams
are rejected until native MVSDK prediction is integrated. Stock JK2 userinfo
uses plain color/team fields without modifying the user's JKA cosmetic settings.
No physics accuracy or playable movement claim is made by this milestone.

Pure-server admission now uses the separate JK2 implementation described below.
Autodownload and live JK2 demo recording remain disabled with explanatory
messages; their existing JKA implementations are not used. Installed custom maps
can be placed under `<client>/jk2/base`.

For a private local test, run from this checkout:

```powershell
.\scripts\jk2-test-server.ps1
# In DinurdoJK: jk2_connect 1.04 127.0.0.1:28079
# Follow the bot with: follow 0
.\scripts\jk2-test-server.ps1 -Stop
# Optional alternate version, after stopping:
.\scripts\jk2-test-server.ps1 -Version 1.02
```

The helper reads `fs_jk2path` or accepts `-GameData`, starts the installed
`jk2mvded.exe` bound to loopback with one Kyle bot and `sv_pure 1` (use `-Pure 0`
for a non-pure test), and reuses
`target/release/jk2` as its runtime home. It copies no binaries/build trees and
does not write to the retail installation. Python 3 is required to launch this
old server without an inherited console: PowerShell's hidden-window launch
triggered a JK2MV 1.4.1 console-resizing crash before networking. The helper
checks the actual protocol response and records PID/creation-time ownership
before managing shutdown. Its temporary RCON credential and logs stay under
ignored `artifacts`.

Validation on November 2, 2026:

- All 61 protocol tests passed, including existing JKA tests, both real-demo
  mappings, JK2 handshakes, wire/presentation command separation, key rules,
  backwards command times, version mismatch rejection, and pure rejection before
  the owner receives any gamestate event.
- The standard fast build succeeded and staged the game executable. Builds
  reused the existing profile/cache/flags; no alternate target tree was created.
- The supplied installed server reports JK2MV 1.4.1 (February 2018) / basemv;
  it is an independent runtime check, not a build of the pinned reference tree.
  Both 1.02 and 1.04 admitted the client on non-pure loopback servers, rendered
  Bespin and followed the bot. Diagnostics measured 20 received snapshots/second
  with 50 ms gaps. Saved game config hashes were unchanged in both checks.
- The 1.04 test also accepted chat containing a percent sign, changed to Yavin,
  re-entered CGame, disconnected, reconnected with the right game/version, then
  restored JKA packages on return to the menu. The final executable repeated the
  same sequence. Screenshot captures show authored moving character poses.
- The initial preview refused pure admission before JKA stock-PK3 handling;
  the pure-admission increment below supersedes that restriction.
- Reports: `artifacts/jk2-live-final.log`, `artifacts/jk2-102-live-smoke.log`,
  and `artifacts/jk2-pure-rejection.log`; associated screenshots/server dumps
  are under the normal runtime `base` output directories. Free spectator movement
  still needs a focused-window user check: the offscreen renderer intentionally
  suppresses capture, so its scripted `+forward` did not create movement input.
  No numerical JKA performance regression claim is made.

## Live connection bootstrap snapshots

The public 1.04 server `78.47.50.174:28070` reproduced an immediate
`JK2 snapshot before gamestate` disconnect. JK2MV's `SV_SendClientMessages`
sends snapshots to connected clients before they necessarily receive their
gamestate, marking non-active clients with `SNAPFLAG_NOT_ACTIVE`. The native
client consumes these packets; activation waits for a real gamestate/CGame.

Live JK2 decoders now use the explicitly selected protocol to consume and
validate such snapshots, while withholding them from presentation, timing and
delta history. The netchan sequence is still acknowledged, allowing the server
to deliver/resend the gamestate. Serverinfo remains the authority for version
identification and mismatch rejection. Offline/demo decoding still requires a
gamestate first, and malformed live bootstrap snapshots remain errors.

All 72 protocol checks passed (64 unit and eight integration tests). New tests
cover both versions, encrypted session packet/ack flow, no premature activation
or history publication, malformed areamasks, and recovery after the gamestate.
The reproduced failure and test reports are `artifacts/jk2-remote-admission.log`
and `artifacts/jk2-bootstrap-tests.log`.

The final fast build succeeded (`artifacts/jk2-bootstrap-build.log`). The updated
executable joined that exact public server, received its gamestate in about
1.53 seconds, entered active spectating at about 1.69 seconds, and rendered
`ctf_bespin`. Its basemv / JK2MV 1.4.1 server reported protocol 16 and `sv_pure 0`;
the client received 29.9 snapshots/second (205 measured gaps, usually 33 ms).
The scripted check disconnected and restored JKA afterward. Runtime report:
`artifacts/jk2-remote-fixed.log`. Full playing-team/native prediction support is
still pending; this fixes connection startup only.

## JK2 pure-server admission

JK2 1.02 and 1.04 now mount a session-specific pure namespace and send the
native JK2 checksum reply. This does not change JKA's TaystJK pure implementation.

- `sv_paks` is parsed as bounded signed package checksums, not trusted paths.
  Local PK3s are checksummed with the existing CRC-array / MD4 algorithm and
  matched to the server's allowed set. PK3 search priority remains local: the
  pinned JK2MV implementation filters packages without reordering to `sv_paks`.
  Version 1.02 still excludes assets2/5. Companion ZIPs are not native JK2 PK3s.
- A new VFS index contains only permitted packages and JK2MV's allowed loose
  file types (`cfg`, `fcf`, `menu`, `game`, `dat`, and that version's demos).
  Ordinary reads, prefix reads, explicit-source reads and provider probes share
  this namespace. Native pure priority takes precedence over the optional
  stock-first asset preference. Existing readers retain their captured policy;
  JKA asset reads collect no JK2 references.
- General references are collected for map reads and retail-package reads;
  non-base mod packages are referenced at mount time. Actual permitted providers
  of `vm/cgame.qvm` and `vm/ui.qvm` supply their feed-keyed checksums. The client
  continues using its ported CGame; it does not execute those QVMs or promise
  arbitrary mod-module compatibility. Equivalent duplicate packages count once
  in the general list. The final word is feed XOR general checksums XOR count.
- Gamestate installation cancels speculative map/CGame work and rebuilds asset
  catalogs under the new policy before sending `cp`. Missing required providers,
  malformed lists or oversized replies fail with an explicit error. A changed
  pure list/setting during an active map requires reconnecting; normal map
  gamestates install a fresh policy and feed. Disconnects and offline demo
  transitions clear the pure policy. Refresh also retains JK2 animation identity.

Validation on November 2, 2026:

- Six focused JK2 asset tests, all 17 filesystem tests, and all 61 protocol tests
  passed. Filesystem regression checks reused the already-built test executable.
  The new synthetic PK3 fixture covers unlisted modules/textures, loose-file
  restrictions, explicit-source reads, stock-first precedence, duplicate checksums,
  missing modules, version filtering, and old-reader isolation across feed changes.
- Fast builds succeeded with the normal shared profile/cache/flags. The first
  build compiled seven packages and reported about 177 MiB additional drive usage;
  the final active-session guard change rebuilt only the client. No separate
  target directory or binary baseline was created.
- Native JK2MV 1.4.1 accepted both versions with `sv_pure 1`, delivered live
  snapshots, and allowed following the Kyle bot. The 1.02 reply selected
  assetsmv2 for cgame/UI and referenced assets0; the 1.04 registration referenced
  assets2/0. Assets1 can gain a reference as models/textures load later.
- On 1.04, Bespin admission, change to Yavin, disconnect/reconnect and restoration
  to JKA succeeded with separate feed-keyed replies. Saved config hashes were
  unchanged. The final executable repeated the sequence and transitioned directly
  from pure live play into the supplied 1.04 demo before returning to JKA.
- A disposable client admitted with valid checksums was then sent an intentionally
  invalid `cp`. The native server dropped it and logged
  `Unpure client detected. Invalid .PK3 files referenced!`. This verifies actual
  server enforcement independently of the client's unit tests. The old server's
  disconnect command itself omitted the reason; the native broadcast log supplied it.
- Local reports: `artifacts/jk2-pure-104-final.log`,
  `artifacts/jk2-pure-102-live.log`, `artifacts/jk2-pure-invalid-reply-confirmed.log`,
  `artifacts/jk2-pure-native-verdict.txt`, and
  `artifacts/jk2-pure-server-systeminfo.txt`. No public-server/mod matrix or JKA
  FPS benchmark has been performed.

## Native MVSDK movement bridge

The isolated native movement host now builds the pinned MVSDK shared gameplay
directly. This is the foundation for live prediction; live sessions still use
the accepted spectator preview and do not call this host yet.

- `jka-movement/vendor/mvsdk` contains 27 verbatim files from
  `4c7c463638192aead1ff3de8a88cc5de266bf3a2`, including the upstream license.
  `PROVENANCE.json` lists the ten compiled source files and every SHA256.
  `reference/vendor_mvsdk.py` reproduces the include closure and generated host
  tables from that Git revision. Git preserves vendor bytes on Windows.
- Compiler-only symbol prefixes isolate MVSDK from OpenJK and TaystJK. The Rust
  `jk2::Player` owns native state and animation data, and serializes shared SDK
  globals/callbacks with the existing native collision/movement lock. The JKA
  backend dispatcher and prediction paths are unchanged.
- Input uses the JK2 wire schema, copying individual field bits into the SDK's
  own `playerState_t` by C-computed offsets. No JKA player-state struct is cast
  into a JK2 layout. The neutral command and trace ABI is checked before use;
  native traces are populated member by member. Animation IDs, toggle bits,
  conditional dodge/taunt animations, and saber-block IDs receive the same 1.02
  conversions as pinned `CG_GetSnapshot`. Native CGame state uses 1.04 animation
  IDs even on 1.02; export can return either wire or normalized identifiers.
- Wire/start version and gameplay version are separate. Each call selects
  native weapon and Force tables through `MV_SetGamePlay`. Fixed timing follows
  CGame's view-angle update, acknowledged-command skip, and timestamp rounding
  order, then native Pmove's subdivision/backlog behavior. Rust supplies no
  acceleration, friction, jump, weapon, saber, or Force calculations.
- Invalid fields/commands fail before native execution. Panicking or nonfinite
  collision callbacks return an error and restore the complete native player
  state. Recursive movement entry from a callback is rejected. Saber locks need
  remote player context and currently fail explicitly. The flat-world test
  fixtures contain no remote players; this is not full multiplayer prediction.
- A separate C replay loop calls native Pmove without per-command host-step
  calls or wire export/import between commands. The diagnostic compares every
  native player-state byte after every command, reporting the first command,
  server time, native field/array element, offset and differing bytes. This
  compares two call paths through the same compiled SDK; it is not an independent
  JK2MV server, 32-bit/QVM comparison, or proof of server prediction agreement.

Reproduce the diagnostic from this checkout:

```powershell
.\target\release\DinurdoJK.exe --jk2-pmove-test
.\target\release\DinurdoJK.exe --jk2-pmove-test imp495xAMoMAx.dm_16
.\target\release\DinurdoJK.exe --jk2-pmove-test CoS-WickedR1.dm_15
```

It loads the installed JK2 animation.cfg through `fs_jk2path`, then checks 96
cases / 18,432 replayed commands: both wire versions and both gameplay versions,
saber/blaster, floor/wall/water, jump/crouch/attack inputs, variable timing and
fixed 8/16/33 ms steps, duplicate/backwards commands and a stale backlog. An
optional demo additionally streams its actual snapshots through native import
and export. The 1.02 animation map deliberately collapses some unavailable
clips; these are counted separately. Other fields and all arrays must roundtrip.
Demo recordings do not contain usercmd history, so this checks state conversion,
not movement reproduction of a recorded player. Unit tests use synthetic clip
timings; the executable diagnostic uses installed animation data.

Validation on November 2, 2026:

- The standard fast build succeeded and staged the executable. The final build
  reused 435 fresh Cargo targets and compiled two; no additional profile, flags,
  target tree, or cache cleanup was used. Runtime asset staging needed normal
  file access outside the restricted tool process; the authorized build completed
  with that access. Final drive-space change was about 5 MiB (volume-wide).
- All 34 focused tests passed: 18 movement-library tests (eight new JK2 tests),
  eight existing native-backend tests, three JKA network-state tests and five
  JKA saber-prediction tests. Coverage includes every 1.02 animation ID/toggle,
  native field naming, an intentionally divergent replay, collision failure
  rollback, callback panic/reentry, and interleaved 1.02/1.04 instances.
- The broader JKA movement suite passed 25/26 tests. The existing
  `equipped_saber_drives_stock_idle_stance_style_and_crouch_torso` expectation
  failed (slow stance versus fast at line 390). The same failure reproduced in
  the already-built October 21 `movement-203b241bc50d32ca.exe`, predating this
  bridge. Its source and JKA movement behavior were not changed for this work.
- The final executable loaded the 36,167-byte JK2 retail animation.cfg from
  assets0.pk3 and passed all 96 cases / 18,432 command comparisons. All 24 supplied
  1.02/1.04 demos then passed native import/export: 1,157,342 snapshots, no field
  or array differences, and no lossy animation mappings in this corpus. The RAR
  was not unpacked. These checks do not reconstruct recorded usercmds.
- All 27 vendor hashes match the provenance manifest. An audit of the twelve
  native COFF objects found 323 isolated SDK/host external definitions, with only
  compiler literals and standard MSVC CRT COMDATs outside their prefixes.
- Reports: `artifacts/jk2-native-build.log`, `artifacts/jk2-native-tests.log`,
  and `artifacts/jk2-native-corpus.log`. No new live prediction, server-correction
  comparison, full saber-lock test, or JKA FPS benchmark is claimed.

Next: wire this host into live command replay using raw JK2 snapshots, supply
remote entity/player context, and validate server corrections before allowing
playing teams. Public discovery, downloads, arbitrary mod client modules, and
full gameplay presentation remain separate work items.

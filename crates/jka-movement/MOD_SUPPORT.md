# Native mod prediction

## Ownership and sources

Physics remains C-owned. The build links two isolated implementations:

- Existing stock OpenJK for offline play, base servers, and unimplemented mods.
- TaystJK's JAPRO client movement for servers whose `gamename` starts with
  `japro`, case-insensitively.

Rust reads current configstrings and passes settings, commands, and entity inputs
through the bridge. `native/mod_dispatch.c` owns native backend selection and
player allocation. It converts between backends through the existing wire-field
contract, never by casting one backend's `playerState_t` to the other's layout.
Both backends use the existing recursive native lock.

The JAPRO source is pinned to TaystJK revision
[`b35ed06fec41c53644352743c6b199a5d5d500f3`](https://github.com/taysta/TaystJK/tree/b35ed06fec41c53644352743c6b199a5d5d500f3).
The source import is byte-identical; adaptations are in the native host and
forced-include namespace headers. The host preserves the reference's alias
between `pmove.ps` and `cg.predictedPlayerState`, including direct crouch-jump
writes. It installs remote player velocities for the player collision fix.

The user identified upstream as a more authoritative client reference than the
local `D:/Code/Japro` fork. Relevant recent fixes include
[stand-up prediction](https://github.com/taysta/TaystJK/commit/4a6b1f95b5643f3e00d3cb657e01dc2e85d7e94e)
and [JA+ DFA flag selection](https://github.com/taysta/TaystJK/commit/0b7350b908c0d7f53e48cca82ec0460bf0aedc15).
Those changes are present in the imported source. JA+ is detected but still uses
the stock backend; importing a source branch does not enable that mod's support.
Likewise the new base-game stand-up selection is not applied to the isolated
stock backend in this milestone.

Both C builds use `/fp:precise` on MSVC, disable strict aliasing where supported,
and disable floating-point contraction where supported. They use the same
`Sys_SnapVector` implementation. No fast-math flag is introduced. This preserves
the existing build policy; it is not a claim of bit parity with every compiler,
architecture, or server binary.

## Implemented

- Fresh server identity and native settings from current configstrings; JAPRO
  feature bits do not carry into base or unknown profiles.
- `jcinfo`, `jcinfo2`, `taystJKinfo`, `dmflags`, `restricts`, hook strength,
  pmove settings, and `CS_LEGACY_FIXES` reach the C host. Legacy animation tables
  are refreshed when their flags change.
- The pinned C implementation includes all 18 non-vehicle player styles.
  Native pmove retains their timing and floating-point rules. JAPRO's accepted
  `pmove_msec` range is 1–66; the existing stock range remains 8–33.
- Native race/duel trace filtering, including bobbing-platform exceptions.
  Point-contents queries retain the reference's separate behavior.
- JAPRO userinfo negotiation after gamestate, refreshed when serverinfo or user
  preferences change. `cjp_client=1.4JAPRO` and `cp_pluginDisable` are sent only
  to JAPRO. The latter is archived through the existing network cvar system.
- The top-level **Mod** menu exposes movement-related preference bits. Other
  bits remain intact when a checkbox changes. `cp_pluginDisable` also works in
  the console/config. Tribes' dash/ski input uses the existing `+button13` bind.

## Review items and next phases

This is the first movement integration, not complete JAPRO client parity.

1. **Live parity:** run against the intended JAPRO server binary using actual
   animation assets. Compare authoritative snapshots and replayed states on
   slopes, steps, walls, water, race restrictions, and style changes. Current
   synthetic replay tests establish isolation and determinism, not full server
   parity or all special-move combinations.
2. **Mover timing:** DinurdoJK currently traces presented entity transforms. The
   reference also uses `cg.physicsTime` for mover evaluation, with an OCPM cap.
   Port that timeline and mover adjustment together; changing only command times
   would be incorrect. Moving-platform edge cases remain a compatibility limit.
3. **Duel/combat events:** JAPRO's `EV_PRIVATE_DUEL` type tracking, gun-duel
   overrides, full/no-force duel behavior, and speculative projectile knockback
   need the Rust event layer connected to the native inputs. Duel passthrough is
   implemented, but the host currently initializes duel-type metadata to zero.
   Review these before claiming combat prediction parity.
4. **Presentation:** common stock animation IDs align, but TaystJK adds animations
   before the cinematic range. Native prediction parses the reference table;
   Rust's existing presentation helpers still use stock tables. Custom emotes,
   ledge animations, and corresponding model assets need a separate pass.
5. **Local fork extensions:** the local fork's sailing, boat, podracer, and other
   vehicle changes are not imported. Native vehicle simulation still requires a
   vehicle entity host. Upstream player physics also needs a targeted comparison
   with any locally modified server physics before promising exact agreement.
6. **Other mods:** detection recognizes JA+, OpenJK Alt, Base Enhanced, and
   Lugormod. They retain the existing stock prediction fallback until their
   settings, snapshot semantics, and native branches are deliberately enabled.
7. **Remaining client features:** inventory race timers, speed/jump displays,
   trails/ghosts, score extensions, chat, sounds, visual preferences, commands,
   and existing Rust strafe-helper behavior before porting missing pieces.

## Validation and source maintenance

`tests/mod_prediction.rs` covers backend state transfer, snapshot reset,
race collision isolation, fixed-step validation, actual CPM-versus-stock
selection, and interleaved deterministic replay across non-vehicle styles.
FFI contract tests check both native layouts. Client networking tests cover
detection, server changes, settings parsing, and scoped userinfo/preferences.

Windows x64 validation: 31 movement tests passed with the existing failing
saber-stance test excluded after baseline verification; all 6 JAPRO integration
tests also passed with release optimization. The focused client network suite
passed 8 tests; 2 tests requiring a running server or installed game assets were
not run. Source manifests and native symbol isolation were checked. The client
was built with `build.ps1 -Fast`; interactive/live-server parity remains untested.

The existing `equipped_saber_drives_stock_idle_stance_style_and_crouch_torso`
test fails at its fast-stance assertion in both the pre-change baseline and this
implementation. It is recorded as an existing failure, not modified or hidden.

To reproduce the source import, place the pinned GitHub codeload archive at
`target/taystjk-client.zip` and run `python scripts/vendor-movement.py --japro`.
Run `python scripts/vendor-movement.py --japro --check` and the stock
`python scripts/vendor-movement.py --check` after changes. If adding a native
translation unit, update `build.rs` and `native/japro_namespace.h` to isolate its
external symbols. The tiny generated-engine-version substitute is under
`native/japro/qcommon/q_version.h`, outside the verified vendor tree.

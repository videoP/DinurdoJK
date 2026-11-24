# Native mod prediction

## Ownership and sources

Physics remains C-owned. There is one native implementation for JKA: the pinned
TaystJK shared client movement (TaystJK is an OpenJK fork). It serves offline
play, Base, JA+, jaPRO and any server that advertises `taystJKinfo`. The former
stock OpenJK copy and its backend switch were removed; the OpenJK clip-map
collision sources now come from the same pinned TaystJK tree.

Rust reads current configstrings and passes settings, commands, and entity inputs
through the bridge. `native/movement.c` owns player allocation and sets `cgs`
(`serverMod`, `baseGame`, `taystJKinfo`, ...) from `jka_predict_settings`, so the
shared source's own `cgs.serverMod`/`cgs.baseGame` branches select Base, JA+ or
jaPRO behavior at runtime. The native layer uses the existing recursive lock.
`jka_player_new` starts from OpenJK-style settings (non-fixed, Base identity).

The JAPRO source is pinned to TaystJK revision
[`b35ed06fec41c53644352743c6b199a5d5d500f3`](https://github.com/taysta/TaystJK/tree/b35ed06fec41c53644352743c6b199a5d5d500f3).
The source import is byte-identical except for the jaPRO sailing hunks below;
other adaptations are in the native host and forced-include namespace headers.

**jaPRO sailing (local fork exception).** The analytic ocean exists only in the
local `D:/Code/jaPRO` `sailing` branch, so it is taken from there, not upstream:
`codemp/game/bg_sailing.{c,h}` and `bg_weather.{c,h}` are copied unmodified, and
`bg_pmove.c` carries that branch's swimmer hunks verbatim between
`DinurdoJK: jaPRO sailing` markers (sample cache, `PM_SetWaterLevel` analytic
contents, `PM_WaterMove` current/orbital-relative swimming and float spring,
shallow-river current, waterspout wind). Boat, paddle and vehicle hunks are not
ported (including `PM_SetWaterLevel`'s boat-rider exemption). The rest of the
sailing branch's `bg_pmove.c` is not taken: it forked before upstream movement
fixes this import already has. `native/japro_sailing.c` mirrors the jaPRO cgame's
`SP_misc_sailing_ocean/shore/current` from the client's BSP lump, and Rust applies
`CS_SAILING_OCEANS` (1660+i) through `BG_SailingApplyOceanWeatherString`. Rivers
and waterspouts need runtime state (solved river field, snapshot waterspout) that
is not fed yet, so those paths stay inactive. Refresh these files from the fork
whenever its BG sailing code changes; prediction must match the server exactly. The host preserves the reference's alias
between `pmove.ps` and `cg.predictedPlayerState`, including direct crouch-jump
writes. It installs remote player velocities for the player collision fix.

The user identified upstream as a more authoritative client reference than the
local `D:/Code/Japro` fork. Relevant recent fixes include
[stand-up prediction](https://github.com/taysta/TaystJK/commit/4a6b1f95b5643f3e00d3cb657e01dc2e85d7e94e)
and [JA+ DFA flag selection](https://github.com/taysta/TaystJK/commit/0b7350b908c0d7f53e48cca82ec0460bf0aedc15).
Those changes are present in the imported source. JA+ now selects the shared
TaystJK backend and supplies `jp_cinfo`, while jaPRO supplies `jcinfo`/`jcinfo2`.
The shared source's `cgs.serverMod` branches execute exactly for Base, JA+, or
jaPRO, and Base servers that advertise `taystJKinfo` keep `cgs.baseGame` set.

The C build uses `/fp:precise` on MSVC, disable strict aliasing where supported,
and disable floating-point contraction where supported. They use the same
`Sys_SnapVector` implementation. No fast-math flag is introduced. This preserves
the existing build policy; it is not a claim of bit parity with every compiler,
architecture, or server binary.

## Implemented

- Fresh server identity and native settings from current configstrings. JA+
  `jp_cinfo` and jaPRO `jcinfo` remain mod-scoped, while `taystJKinfo` is a
  server capability mask independent of gamename, matching current TaystJK.
- `jp_cinfo`, `jcinfo`, `jcinfo2`, `taystJKinfo`, `dmflags`, `restricts`, hook strength,
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
- The top-level **Mod** menu exposes TaystJK's complete jaPRO `pluginDisable`
  set (bit 3 and bits 19-28) in a scrollable page. Other/legacy bits remain
  intact when a checkbox changes. `/plugin` and `/pluginDisable` list/toggle
  the same central table; `cp_pluginDisable` still works directly in the
  console/config. Tribes' dash/ski input uses the existing `+button13` bind.

## Review items and next phases

This is the first movement integration, not complete JAPRO client parity.

1. **Live parity:** run against the intended JAPRO server binary using actual
   animation assets. Compare authoritative snapshots and replayed states on
   slopes, steps, walls, water, race restrictions, and style changes. Current
   synthetic replay tests establish isolation and determinism, not full server
   parity or all special-move combinations.
2. **Mover timing:** prediction now clips against mover brush models at
   `cg.physicsTime` (with the OCPM cap) and applies `CG_AdjustPositionForMover`
   to the replayed and displayed states. Riding a *rotating* mover still does
   not carry the origin around it, exactly like the reference ("FIXME: origin
   change when on a rotating object").
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
6. **Other mods:** JA+ now uses the TaystJK backend. Detection also recognizes
   OpenJK Alt, Base Enhanced, and Lugormod; those retain stock prediction unless
   they explicitly advertise generic `taystJKinfo` capabilities, pending their
   own targeted settings/snapshot compatibility passes.
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
`target/taystjk-client.zip` (or pass `--archive PATH`) and run
`python scripts/vendor-movement.py`. It overwrites files, including the local
sailing hunks in `bg_pmove.c`, so use it only to refresh the pin deliberately.
`python scripts/vendor-movement.py --check` verifies the manifest hashes. If
adding a native translation unit, add it to `build.rs`; `native/host_prelude.h`
is force-included and supplies the jump-trace declarations the vendored BG
sources expect. The tiny generated-engine-version substitute is under
`native/japro/qcommon/q_version.h`, outside the verified vendor tree.

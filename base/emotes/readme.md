# Emotes

The client plays **cosmetic emotes** from compiled bundles (`*.djk_fnemote`).
In-game selections send the ordinary game taunt, with a permanent emote ID packed
into the initial yaw/pitch angles. Compatible DinurdoJK clients with the same
performance installed display the emote, props, effects and positional audio.
Other clients see the normal taunt. Profile previews remain local.

The bundles are not part of this repository (they are large). Build your own with
[`djk-fnemote`](../../crates/jka-emotes/README.md), or install a library someone
gives you. With none installed the emote UI simply shows an empty library.

## Installing

Put bundles in the game's active `base` directory, either loose under
`emotes/` or packed (one or many PK3s, bundles stored uncompressed so they can be
read in place):

```
GameData/base/emotes/EID_DanceMoves.djk_fnemote      loose
GameData/base/my-emotes.pk3                          packed (djk-fnemote pack-library)
```

`base/emotes/` in the source tree is only this readme and an ignore rule; generated
bundles and libraries are never committed. Props and particle effects live in an
optional native-assets PK3 next to the bundles (`models/fnemote/...`, see
[EFFECTS.md](../../crates/jka-emotes/EFFECTS.md)).

Keep `emotes/favorites.json` writable: it holds your starred emotes, categories and
chosen variations, and is never packaged.

## Using them

* **Profile → Emotes** lists the library: search, preview on your player model with
  sound, pick a variation, choose a category, star favorites, change the backdrop
  (an installed map's levelshot).
* **The `/emote` wheel**: hold the key bound to `emote` (set it under *Controls →
  Other → Emote Wheel*, or `bind B emote`), left-click **LIBRARY**, choose a
  category (Taunt, Flex, Dance, or Miscellaneous), then click an emote to play.
  Categories and assignments follow your Profile settings. **FAVORITES** opens
  your starred emotes directly. Right-click goes back one level; releasing the
  key cancels.
  Playing emotes inherit the accepted native taunt's interruption rules.
  Full-body taunts stop on attack, alt-attack, Force action buttons, and
  jump/crouch input. Torso-only taunts allow independent leg movement,
  including crouching; an action that replaces the torso animation stops them.
* **Console**: `djk_emote list`, `djk_emote 12`, `djk_emote EID_DanceMoves`,
  `djk_emote stop`, `djk_emote refresh` (rescan after installing).
  Numbers are permanent network IDs, including the exact variation, rather than
  positions in your installed library. `djk_emote list` shows installed IDs;
  for example, `djk_emote 876` selects Dance Moves.

## Multiplayer identity

`crates/jka-client/src/emote_network_ids.tsv` is the built-in, append-only
authority: source EID plus source variation name, case-normalized. Titles,
favorites, catalogue grouping, installation order, and missing bundles never
change IDs. Unknown IDs/assets retain the ordinary taunt. Unregistered custom
emotes play locally and still send a normal taunt; IDs are never guessed with
a truncated hash or assigned from installed-library order.

The v2 codec uses the existing 22 angle payload bits: 13 ID bits and 9 signature
bits. Each angle changes by at most 5.625 degrees; server `delta_angles` are
compensated, and encoding skips pitch positions that would be clamped. The
payload repeats for up to 160 ms to cross a snapshot boundary, while the game
taunt command is sent once. ID 8191 is reserved for cosmetic stop and does not
send another taunt. Check bits identify the codec; they do not authenticate peers.
The earlier experimental v1 `emote_net_test` codec is incompatible.

Native taunt restrictions still apply: a server may reject taunting while moving,
attacking, or already taunting. Remote starts require a newly observed accepted
taunt; entering PVS during an existing performance does not start it midway.
Long emotes continue cosmetically when the stock taunt returns to idle, retaining
that taunt's cancellation rules. Native cancel inputs transmit stop; remote death,
teleport, PVS exit and actions that interrupt the original taunt's animation channels
also invalidate playback. Remote poses follow render server time; audio begins
when prepared and cannot seek to compensate for a late asset load.
If a short emote finishes before its native taunt, presentation returns to idle
until that taunt ends instead of showing the replaced JKA animation again.

When shipping additional performances, run:

```
python scripts/update-emote-network-ids.py path/to/library.pk3
```

The script reads metadata in place and only appends new assignments, in sorted
EID/variation order. Commit the registry with the client update. Never delete,
renumber, or reuse existing rows, even when removing an asset. The registry has
8191 performance slots (0 through 8190); older clients ignore unknown additions.

| Setting | Meaning |
| --- | --- |
| `cg_emoteCubic 1` | cubic interpolation of body, props and ground contact at render rate (smooth above the 30 Hz source rate) |
| `cg_emoteEffects 1` | draw baked particle effects |
| `s_emoteMusicVolume 0.5` | emote music level, independent of effects and map music |

All three are also in the Settings menu (Audio, Models).

## Offline QA gallery (developers)

`emote_gallery <first> [count] [samples] | ids <EID,...> [samples] | stop` renders emotes
offline on the isolated Profile stage (no map) from four angles at scrubbed times, using the
normal screenshot path, and writes a `report.json` (missing props, track mismatches, audio
status, effect counts). `DJK_HEADLESS=1` keeps the window hidden and the process at
below-normal priority. `scripts/emote-gallery.sh` (Git Bash on Windows) runs several instances
in parallel from a test base folder and builds one contact sheet per emote.

## How it fits together (for contributors)

| Where | What |
| --- | --- |
| `crates/jka-client/src/emotes.rs` | catalogue (built on a background worker, shared by `Arc`), playback clock, prepared-asset caches (64 MiB each, favorites kept) |
| `app/emote_wheel.rs`, `egui_emote_radial.rs`, `egui_menu/profile_emotes.rs` | favorites, categories and variants; the wheel tier; the Profile tab |
| `audio.rs`, `cgame/sound_presenter.rs` | emote audio lanes (music / foley), sample-accurate loops, bounded decode cache |
| `cgame/player_presenter/{presentation,emote_props,emote_effects}.rs` | pose tracks drive local and remote players; props and baked particles follow the final pose |
| `emotes/network.rs`, `emote_network_ids.tsv`, `emote_net{,_codec}.rs` | permanent IDs, angle transport, snapshot validation and independent remote playback |
| `cgame/entity_presenter.rs`, `fx/draw.rs` | prop MD3 / skinned props and particle draws |

Threading: nothing reads a bundle on the game thread. The catalogue scan, favorite
warm-up and cold-selection preparation run on bounded workers; a stale selection
never starts after a newer one or a stop. A damaged or duplicate bundle is logged
and skipped, never fatal to the rest of the library.

**Where a streaming store would plug in:** `emotes::read_catalog` / `read_bundle`
find bundles through the game's asset search path and open each one with
`jka_emotes::Bundle::open_located_index`. Replace that lookup with your own
`jka_emotes::BundleSource` (see the
[streaming note](../../crates/jka-emotes/README.md#where-bundles-live)) and the
rest of the client is unchanged: the catalogue only reads the ZIP directory and
manifest, and playback validates lazily.

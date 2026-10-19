# Emote recipe example

`dance-moves.recipe.json` is a template for `djk-fnemote compile`. It needs
*your own* exported assets, which are not part of this repository:

* `source_root` - a folder (relative to the recipe) with the exported PSA
  animation and any WAV files, for example `export/EID_DanceMoves/Animations/DanceMoves.psa`;
* `target_base` - a Jedi Academy `base` directory (relative to the recipe, or
  absolute) holding the stock player model used as the retarget target.

```sh
djk-fnemote compile examples/emotes/dance-moves.recipe.json base/emotes/EID_DanceMoves.djk_fnemote
djk-fnemote inspect base/emotes/EID_DanceMoves.djk_fnemote
```

The recipe trims the animation to 0-6.8 s (204 frames at 30 fps) and starts one
music cue at zero; the second event, without `wav`, stops the music lane at the
end. Every recipe field is described in [FORMAT.md](../../crates/jka-emotes/FORMAT.md).

`manifest.json` is the older loose-manifest form that the game still reads next
to bundles; new work should use bundles.

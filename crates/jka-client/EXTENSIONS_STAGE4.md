# DinurdoJK extensions — Stage 4 (replacing native HUD elements)

An extension replaces native HUD elements by listing them in `extension.toml`:

```toml
api_version = 1
name = "My HUD"
hides_native_hud = ["health", "shield", "ammo", "force", "speedometer"]
```

Elements: `health`, `shield`, `ammo` (also the saber-style panel), `force`,
`speedometer` (readout, jump counter and speed graph), plus four independent
selection-UI flags:

- `weapon_select`: native weapon carousel/icons and weapon name, both JK2 and JKA.
- `force_select`: native Force-power carousel/icons and name, both JK2 and JKA.
- `inventory_select`: native item carousel/icons and name, both JK2 and JKA.
- `jk2_prongs`: JK2 HUD style 1 edge prongs (idle and selected) and animated
  selection background. Has no effect in JKA HUD mode.

These flags suppress **rendering only**, not selecting, cycling, activating, or
using the items and powers. The selector flags are separate from `ammo`/`force`,
which govern the permanent status HUD clusters. Choose only the pieces the
extension actually replaces: for example a permanent weapon bar should request
`weapon_select`, not `force_select` or `inventory_select` unless it also provides
a way for the player to see those selections. `jk2_prongs` is independent of
all three selectors: turning it off alone does not remove native icons/labels.

An unknown name makes the manifest fail to load (the previous revision stays
active).

## Rules

- Runtime overlay only. No cvar or config value is written, and cvars such as
  `cg_speedometer` and the HUD Edit layout keep working untouched.
- Active only while the extension is enabled and running. Disabling or removing it
  restores the native element immediately. A failed hot-reload keeps the previous
  revision, so its elements stay hidden (no flicker). If the extension's
  `on_update` hook is disabled by a runtime error, its elements come back.
- Several extensions combine (union).
- Extension widgets only draw under `cg_drawHud 1`, so the native element is only
  replaced under `cg_drawHud 1` as well. `cg_drawHud 0` never hides both.
- HUD Edit does not offer handles for a replaced native element.
- Setup -> Extensions shows "Replaces native HUD: ..." for the selected extension.

## Bundled example

`extensions/ups-hud/` is a replacement speedometer (UPS, 2 s peak, speed bar).
Copy it to `<base>/extensions/ups-hud/` to try it.

## Images, anchors and screen size

- `hud_image(widget, "gfx/hud/hudleft", x, y, w, h, [r,g,b,a])` draws a game image
  (any `gfx/...` path without extension, from the normal asset search path), tinted by
  the colour. A negative `w`/`h` mirrors it. Images are clipped to the widget rect.
  At most 32 distinct images across all widgets; they are packed into one atlas on
  demand (no cost when unused).
- `hud_register(id, name, w, h, anchor)` takes an optional anchor: `"top-left"` (default,
  old behaviour), `"top-right"`, `"bottom-left"`, `"bottom-right"`. Placements are offsets
  from that corner, so anchored widgets keep their place across resolutions and scale
  inward. Calling `hud_register` again (e.g. on a resolution change) replaces the widget.
- `session_info()` now has `screen_width` / `screen_height` (framebuffer pixels).
- `playerstate()` gained `max_health`, `force_power`, `saber_style`, `saber_active_style`,
  `ammo_index`, `ammo_current` (or `()`), `ammo_infinite`. **`armor` was reading the wrong
  stat (holdable item) and is fixed.**
- Rhai functions called from `on_update` cannot see top-level `let`/`const` values
  (only `on_update` itself can); pass what helpers need as arguments.

## Native: Jedi Academy HUD

The retail multiplayer HUD is now a built-in Rust style, selected with
`cg_hudStyle 1` or the Interface / HUD page. It uses the original `gfx/hud/*`
and `gfx/2d/numbers/t_*` assets from the mounted JKA files, plus the existing
asynchronous UI image loader. It is not a Rhai extension and no longer needs
`extensions/jka-hud/`. Remove any separately installed legacy `jka-hud`
extension to avoid overlaying the same artwork twice.

`cg_hudStyle 0` returns to the DinurdoJK status panels. In HUD Edit, Classic's
left health/armor cluster uses the Health handle and its right force/ammo cluster
uses the Ammo handle; the old Shield and Force single-bar handles apply only
in style 0.

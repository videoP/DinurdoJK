# Client source layout

The client stays in one crate (`DinurdoJK`). These folders separate existing responsibilities; they do not introduce services, new runtime objects, message passing, or additional calls in the frame loop.

```text
crates/jka-client/src/
??? app.rs                     App state and shared imports
??? app/
?   ??? initialize.rs          Startup and initial state
?   ??? lifecycle.rs           winit application/event handling
?   ??? frame.rs, tick.rs      Frame updates and ticking
?   ??? commands/              Console buffering, dispatch and cvars
?   ??? connection/            Joining, downloads and live networking
?   ??? demos/                 Recording, streams, playback and probes
?   ??? session/               Session state, events, advance and sampling
?   ??? settings/              Applying graphics, audio and timing settings
?   ??? egui_menu/             Frontend and browser pages
?   ??? egui_settings/         Settings pages by subject
?   ??? tests/                 App unit tests
??? renderer.rs                Renderer state and stable facade
??? renderer/
?   ??? api.rs                 Render commands, submissions and public types
?   ??? initialize.rs          Device/resource initialization
?   ??? context.rs             Surface, size and presentation settings
?   ??? render_thread.rs       Render-thread transport and pacing
?   ??? frame/                 Main/fast/preview rendering, plan, late latch
?   ??? world/                 Map GPU resources, upload, pipelines and draws
?   ??? models/                Dynamic-model state, draws, pipelines, skinning
?   ??? lighting/              Lights, shadows, cascades and ray tracing
?   ??? visibility/            PVS, GPU visibility, compaction and readback
?   ??? environment/           Clouds, fog, water, snow and surface sprites
?   ??? reflections/           Planar reflection resources and selection
?   ??? post/                  Post-process state, targets and pipelines
?   ??? static_ao/             AO data, BVH, baking and runtime application
?   ??? diagnostics/           Profiling, inspector and debug geometry
?   ??? tests/                 Renderer tests and shader audits
??? scene.rs                   Prepared-map and scene data contracts
??? scene/
?   ??? prepare.rs             BSP preparation orchestration
?   ??? source_map/            Editable .map brushes, geometry and preparation
?   ??? material_batches.rs    Material/stage conversion and batching
?   ??? collision.rs           Collision/physics mesh preparation
?   ??? entities.rs            Authored entity interpretation
?   ??? lighting.rs            Authored lights, light grid and voxel GI
?   ??? lightmaps.rs           Lightmap/deluxemap loading
?   ??? vegetation.rs          Grass and surface-sprite preparation
?   ??? visibility.rs          Portal/PVS draw plans
?   ??? tests/                 Scene preparation tests
??? ui.rs                      UI snapshots and shared facade
??? ui/
?   ??? settings/             Video, rendering and environment settings
?   ??? hud/                  Layout, movement, scores, notices and crosshair
?   ??? compose.rs            UI vertex composition
?   ??? text.rs, geometry.rs  Fonts, text and primitive geometry
?   ??? console.rs, chat.rs   Console and chat drawing
?   ??? tests/                UI tests
??? cgame/player_presenter.rs  Presenter state and public entry points
    ??? player_presenter/
        ??? assets/           Model/material loading and asset types
        ??? presentation.rs   Snapshot/player presentation
        ??? submission/       Mesh construction and renderer submissions
        ??? bodies/           Corpses, dismemberment and gore
        ??? sabers/           Saber state and presentation
        ??? physics.rs        Impulse/ragdoll coordination
        ??? cloth.rs          Cloth coordination
        ??? attachments.rs    Weapons and body attachments
        ??? effects.rs        Player effects and shared effect helpers
        ??? previews.rs       Asset/profile previews
        ??? tests/            Presenter tests
```

The tree highlights the main responsibilities; existing focused modules such as the map editor, frontend, race ghosts and companion window remain alongside them.

## Adding or changing code

Put implementation in its owning folder. Keep existing caller paths through the facade and use explicit imports in child modules. Internal items use visibility limited to their owning facade where sibling access is necessary. An existing public type or method keeps its existing visibility.

State ownership and field order are unchanged. `PreparedMap` continues to carry prepared CPU data; GPU resources remain in the renderer. Settings types remain available through `ui`, avoiding a simultaneous API migration across the client.

Tests live in separate source files but retain their original module namespaces. WGSL assets retain their existing locations and contents; moved Rust code uses adjusted relative `include_str!` paths.

## Frame-time constraints

The main renderer draw function, initialization function and large console dispatch functions remain intact inside their owning files. Splitting their bodies into extra calls is a separate change that needs performance evidence. File-size limits should not force changes to these paths.

`build.ps1 -Fast` still disables LTO and uses 16 codegen units. Rust module boundaries can change codegen-unit placement and inlining in this configuration. Validate performance with alternating before/after runs at the same map, view, resolution and settings; unchanged algorithms alone do not establish identical FPS.

## Restructure validation (2026-10-04)

- The source audit compared all 7,920 client functions and data declarations with the original Git revision. Function logic, declaration attributes, field order and included asset references match after accounting for visibility, relative paths and formatting.
- Changed Rust files pass `rustfmt --check`. The client builds with the `-Fast` release profile (LTO off, 16 codegen units); build configuration and dependencies are unchanged.
- A stationary Vulkan smoke benchmark on `mp/ffa3`, at 640 ? 480 with uncapped FPS and vsync disabled, averaged **4,295 FPS before** and **4,328 FPS after** across four five-second samples per build, taken in alternating build order. Mouse scaling and bindings were disabled in an isolated config. This view showed no slowdown; the difference is within observed run variation. It does not establish identical performance on every map or settings combination.
- Client test compilation has the same three pre-existing errors: missing `collect_courses` and `collect_demos` in race-ghost web tests, and a missing `build_crosshair` argument in the crosshair fixture. Protocol/asset/movement baseline tests also identified the existing skin comma-parser and equipped-saber idle-stance failures. Those fixtures and gameplay behavior were left unchanged.

Benchmark executables, isolated settings, logs and audit reports are retained under the ignored `target/refactor/` directory. A running client prevented replacing the normal release executable during validation. Close it and run `build.ps1 -Fast` to install the restructured build normally.

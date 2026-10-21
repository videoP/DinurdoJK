# World shader program modes

The console control is `r_fragmentShaderMode 0..4`. Changes compile in the
background and activate a complete pipeline set without a map reload. Mode 1
remains the default; quality presets leave this experiment control alone.

| Mode | Policy |
| --- | --- |
| 0 | Existing general world program. |
| 1 | Existing material specialization through override constants. |
| 2 | Existing specialization plus globally compatible compact entry points. |
| 3 | Shared basic/enhanced families, each with alpha-tested and untested variants. |
| 4 | Generated vertex/fragment pairs for normalized stage recipes encountered by the map. |

Modes 3 and 4 target Minimal Unified, Low and Medium. Their eligibility follows
effective renderer features rather than a preset name. Unhandled configurations,
autosprite2, special tcGen and three-stage legacy RGB composites use the existing
specialized program. Sky and fused effect programs retain their dedicated paths.
High and Ultra settings have not been changed or included in the validation scope.

Four common mode-3 families do not mean four pipeline objects: blend/depth/cull
state, vertex entry, format and MSAA remain independent pipeline dependencies.

Mode 4 generates matching varying declarations, fixes tcGen, emits the actual
tcMod opcode sequence, specializes wave kinds and evaluates uniform colour/alpha
waves in the vertex shader with flat output. Numeric parameters, texture identities,
remap offsets and clamp time remain data. It keeps the existing vertex storage.
Enhanced stages reuse the existing shading equations and retain conservative
inputs for those operations. It does not promise an instruction-count or FPS gain.

## Trying the modes

Select Minimal Unified, Low or Medium and keep a stationary view. Enter:

```
r_fragmentShaderMode 3
r_fragmentShaderMode 4
```

Wait until the requested mode is active before interpreting performance. For a
short scripted comparison, use `exec shader_modes_ab`; the builder installs that
config into the runtime base folder. It changes only shader mode and ends on mode 1.
Sorting remains independently controlled by `r_worldSortMode`.

`perfsample` prints `shader_mode=requested/active` and actual general, specialized,
minimal, shared and generated main-pass program binds, including cached transparent
bundle commands. The older spec/general aggregate is retained for compatibility.
`render_perf_capture` includes the same counters, sorting mode, active variant,
HDR output and presentation-copy controls. GPU timings remain opt-in.

Cold `[WORLD STAGE RECIPES]` logs show prepared unique recipes, program pairs,
pipeline objects, basic/enhanced coverage and fallback reasons. Prepared recipe
counts are not executed draw counts or GPU timings.

## Implementation and validation

Stage data and policy live in `programs.rs`; tests live in `program_tests.rs`.
Program tag and vertex recipe participate in compilation, reuse and binding
identities. Existing reflection/fog/dlight/glow redraw implementations remain in use.

The final renderer regression run passed 113 tests, with 24 hardware tests
ignored. The explicit Vulkan image test passed 85 renders across modes 0..4,
including alpha tests, vertex colours, tcGen/tcMods, all six wave types, active fog
and Medium feature combinations. Maximum compared 8-bit channel difference was
at most one. Hardware validation used an RTX 3060, not an iGPU.

Map smoke validation uses `scripts/test-world-program-modes.ps1`, deriving the
three scoped presets from `app/quality.rs` and sharing only read-only asset
packages with an isolated config/screenshot/capture directory. It checks active
shared/generated program use and switching back to a cached variant on mp/ffa3
and costadelsol. Logs and captures are under `target/shader-mode-validation`.
All six map/preset checks passed on the final release executable, including mode switching and cached reuse. The machine-readable summary is target/shader-mode-validation/validation-summary.json. Those short 640x480 runs are smoke checks, not comparable iGPU benchmarks.

Medium costadelsol exposed an existing deferred transparent bundle format mismatch:
its descriptor used swapchain BGRA8 while scene pipelines targeted FP16 for authored
glow. The descriptor now follows the actual scene attachment format. An explicit
GPU regression covers FP16 bundle creation, reuse and program-bind accounting.

The full client suite run had 1011 passing, 18 failing and 62 ignored tests. The
failing assertions concern other cvar/config, material/scene, input and presentation
tests; none was in the renderer test group. Their log is
`target/shader-modes-full-tests.log`. This change does not claim a green full suite.

The requested `DinurdoJK_TextureCompression_UI_export_compile_fix.patch` was also
applied: `VIDEO_ROW_TEXTURE_COMPRESSION` is exported by `ui.rs`.

### Shader fallback diagnostics

With `r_frameTimeGraph 1`, modes 2–4 show **MAP BSP MODE N ELIGIBILITY**:
these are coarse BSP batches in the loaded map (including ones not visible),
**not** GPU fragment counts or the number of draw submissions this frame.
A batch is assigned a single primary fallback category. Global variant
features, special sky/fused/ocean pipelines, autosprite2, three-stage sRGB,
special tcGen and glass are distinguished. These counters are diagnostic only.

`/trace` on a BSP surface adds **FRAGMENT SHADER / PSO ELIGIBILITY**. It shows:
- Requested vs installed active shader mode (background compilation may lag).
- The material's actual installed base-pass program from the active PSO set.
- What modes 2, 3 and 4 would select with the current active global features.
- Every relevant exclusion reason for the traced stage, not just the first.

Prediction of another mode's program does not compile or activate that mode.
A surface that is eligible but lacks the expected installed program in the
active mode is flagged for investigation. Shader diagnosis does not modify
rendering, pipeline selection, stage fusion, or ocean promotion.

## Hybrid Ocean, baked relight and benchmarking (2026-10-20)

**Dry stages under Hybrid Ocean.** `r_ocean 1` puts the whole variant in the
Enhanced family (bsp.wgsl + its layout). Only the reserved ocean pipeline shades
ocean geometry (bsp.wgsl detects it per vertex; world batches never contain
clipmap vertices), so `dry_variant()` judges every other stage with the ocean
compiled out. Those programs are built from bsp_lean.wgsl against the Enhanced
pipeline layout (identical groups 0/1/3/4; Lean group 2 is a subset), so they
draw with the frame's existing bind groups. Mode 1 installs `SpecializedLean`;
modes 2-4 select their Minimal/Shared/Generated programs instead of falling
back. Point/area lights, irradiance volume, voxel GI and RT keep Enhanced.

**Baked relight.** `baked_sun_relight` no longer disqualifies modes 3/4. Stages
that cannot reach `light_bsp_surface` (no implicit/explicit lightmap, no
vertex-lit vertex colour) compile it out; lightmapped stages use
GeneratedEnhanced (fs_main equations) with the surface-id stream and a flat
`baked_sun_fit` varying, exactly like vs_main_legacy. Mode 3 keeps lightmapped
stages specialized under relight: its uniform-driven SharedEnhanced measured
4-13% slower (RTX 3060). Mode 2 still requires relight off.

**Explicit `$lightmap` alpha.** `fs_world_minimal` (mode 2, SharedBasic,
GeneratedBasic) now forces the source alpha of explicit lightmap stages to 1 as
fs_main does; lightmap alpha carries sun flux on sun-baked maps. The flag is a
compile-time key bit in every specialized mode.

**Per-batch CPU cost.** `base_pipeline()` looks up the specialized PSO for every
world batch every frame; the map now uses the fold hasher instead of SipHash.

### Benchmark workflow

`python scripts/bench_shader_modes.py` launches the client per (spot, trial,
exe, case), measures every requested mode twice in mirrored order (A B C C B A,
odd trials C B A A B C), waits with `waitshadermode` until the requested mode
is active and nothing compiles, settles, then records `render_perf_capture 10s`.
It writes `bench/results/shader-modes-<tag>-<scenario>-<time>/` (summary
JSON/CSV, report.txt, gzip per-frame columns, logs) and restores the cfg.
`--exe name=path` (repeatable) interleaves builds; `--case name=cvar:v;...`
interleaves settings. Captures whose view is not the requested camera (input
during a run) are dropped from the report. `--report <dirs>` re-reports and
merges; `--exclude-launch` drops a disturbed launch. Spots use
`setviewpos_exact` (no teleport push) and are given as `viewpos` cameras.

`--shots-only --freeze-time 100` takes one PNG per mode with
`JKA_FIXED_SHADER_TIME` freezing the renderer clock; compare with
`python scripts/diff_mode_shots.py <dir> --ref 0`. Ocean simulation, spray and
cgame entities are not on that clock: measure the same-mode floor in the same
launch (e.g. `--modes 0,1,0,1`) before reading a difference.

`render_perf_capture <N>s` records for N seconds (cap 200k frames). New columns:
`frame_interval_ms` (present-to-present), `world_variant_pending`, active PSO
counts and the latest primary-BSP pipeline statistics (vertex/clipped/fragment
invocations; first world pass only, ~10 Hz, asynchronous).

# World shader specialization / cache contract

The **active pipeline** is specialized with WGSL `override` constants. Disabled
features must be compile-time dead, not retained behind per-pixel uniform flags.

## Adding or changing a feature

1. Add its setting-to-`WorldShaderVariantKey` mapping in `settings.rs`.
2. Add its WGSL `override` and guarded implementation. If the shader introduces
   different bindings / generated WGSL, ensure its key also distinguishes its
   shader module and pipeline layout.
3. Add the compile constant in `WorldShaderVariantKey::compilation_constants`.
4. Define which pass uses the feature in `WorldPassPipelineIdentity::new`.
   Normalize a flag only if that pass **actually compiles with it disabled**.
5. If the setting changes GPU textures but not shader compilation, rebuild only
   the resources. Format/MSAA transitions must rebuild their compatible PSOs.
6. Run the `shader_audit` and pass identity regression tests. Verify output,
   including planar reflections, transparent materials, fog, and Legacy dlights.

## Runtime behavior

- Cache identity = exact pass + effective compile-time feature key + native PSO
  state + scene target format + sample count. The cache never stores a universal
  all-features shader. It reuses actual GPU `RenderPipeline` objects.
- A new variant only compiles missing PSOs and atomically activates once all
  passes have completed. Prior complete variants remain usable meanwhile.
- A superseded pending job checks cancellation *between* native PSOs. Native
  `create_render_pipeline` calls are never interrupted in progress.
- The reuse catalog is a cold-path snapshot from the current map's retained
  variants. Scene-format/MSAA changes rebuild the complete set and invalidate
  previous variants. Shader source/layout changes at runtime need their own
  identity/generation, not merely a reused settings key.

The cache does not eliminate the cost of a genuinely new, complex NVIDIA/Vulkan
fragment shader permutation. Native persistent pipeline caches are separate.

## Stage programs (r_fragmentShaderMode 3/4)

`programs.rs` derives structural vertex recipes from the uploaded material.
Numeric parameters and texture handles stay in uniforms/bind groups. Mode 3
shares basic/enhanced shader families with uniform material choices; mode 4
generates matching interfaces and unrolls only the encountered operations.

`program_key` and `program_variant` are the authoritative classification and
effective base-pass dependencies. Non-snow stages can drop snow deformation;
stages without PBR companions/detail/planar operations can drop those features.
Reflected views and authored fog/dlight/glow redraws retain their existing pass
implementations. Settings outside the experimental envelope retain mode 1.

The program tag and vertex recipe are part of material/cache/binding identity.
General pass cache identities normalize the mode policy out; managed base passes
retain their actual program tag and effective feature key. Sorting compares that
same identity. A mode switch activates the new complete variant atomically.

Generated source reuses the checked-in WGSL equations. Wave evaluation uses the
same evaluator in the vertex stage and a flat varying in the fragment stage.
All linked base/redraw position outputs must agree for depth-equal rendering.

`perfsample` and `renderperf` report requested/active modes and actual base-pass
program binds. Cold `[WORLD STAGE RECIPES]` logs report recipe/program-pair/PSO
counts and fallback reasons. These counts distinguish prepared recipes, bound
programs and GPU execution; none is a measurement of shader speed.

## Shader input format A/B (r_shaderInputMode 0/1/2)

This is independent of r_fragmentShaderMode. Each shader-input choice is
part of WorldShaderVariantKey and hence PSO reuse/cold-compile identity.

- 0 = unchanged WGSL + per-pipeline WGSL override values.
- 1 = on-demand WGSL-to-SPIR-V compilation, persistent stage binary cache,
  **mandatory re-import and validation of emitted and cached SPIR-V**, and
  per-pipeline WGSL fallback when the round-trip fails. Rejected variants are
  cached in memory to prevent repeated bad imports during a session.
- 2 = **disabled**. The original native passthrough path fed emitted SPIR-V
  directly to the NVIDIA driver, which raised an `0xC0000005` access violation.
  WGPU explicitly makes no safety guarantees for passthrough inputs. The
  runtime now refuses mode 2 rather than reporting a misleading successful A/B.

The stage cache key was bumped from `v1` to `validated-v2`; older binaries
are never loaded. Mode 1 still compiles its WGSL on first use and does not
ship SPIR-V offline. Shader input mode and material program mode are independent.

Other render subsystems (models/post/compute, RT BSP, sky-box/cloud/ocean
extra passes) remain WGSL. Compilation/conversion happens on the existing
pipeline worker on a mode change. Console output explicitly reports fallbacks.

Benchmark cold versus warm native PSO cache separately: this A/B compares
shader input formats, not the driver-generated native pipeline cache.

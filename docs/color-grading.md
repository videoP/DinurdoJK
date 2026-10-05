# Color grading

Open Video -> Color for the film/external LUT, split toning, tone mapping,
auto exposure, and existing brightness controls.

Split toning is disabled by default. Its starting look uses blue shadows
(210 degrees, 30% saturation) and golden highlights (42 degrees, 25%
saturation), at 50% strength. Negative balance favors highlight toning;
positive balance favors shadow toning. Clicking a control label restores its
default through the same reset mechanism as other settings pages.

The settings are saved as these archived console variables:

| Variable | Default | Range |
| --- | --- | --- |
| r_splitToning | 0 | 0/1 |
| r_splitToningStrength | 0.5 | 0..1 |
| r_splitToningShadowHue | 210 | 0..360 degrees |
| r_splitToningShadowSaturation | 0.3 | 0..1 |
| r_splitToningHighlightHue | 42 | 0..360 degrees |
| r_splitToningHighlightSaturation | 0.25 | 0..1 |
| r_splitToningBalance | 0 | -1..1 |

Split toning works with the LUT preset Off and with both built-in and external
LUTs. Film/external LUT intensity is applied before split toning, so reducing
LUT strength to zero does not disable the separate tints. Zero tint saturation
removes that tint contribution. Shadow and highlight ranges overlap smoothly
around perceived middle grey, using scene brightness before output gamma and
before the film/external LUT. This is per-pixel grading, without neighbourhood
analysis or scene-wide exposure metering.

The transform works in linear light, preserves luminance, and scales chroma
to fit the output gamut. Pure black and pure white stay neutral. Tint strength
scales with available light, so shadow toning does not add colored fog to black.
Changing `r_gamma` rebakes its inverse/output curves into the LUT so gamma does
not move the shadow/highlight ranges. The 33-cubed RGBA8 grid still introduces
normal LUT interpolation and quantization error, especially near black at low
gamma values.

Grading is baked at settings changes into the existing 33-cubed RGBA8 LUT.
The WGSL shaders and texture lookup count are unchanged. Enabling split toning
when all grading is off incurs the normal LUT post-processing cost; combining
it with an already-enabled LUT adds no shader passes or texture lookups.
Gamma/color-only configurations retain the fast world renderer. Gamma alone
uses the existing small `post_fast.wgsl` shader; a LUT or split toning uses the
already-compiled `post_gamma.wgsl` shader in that same single post pass. With
gamma 1 and grading off, the baseline renders directly without a post pass.
Non-default gamma already required a fullscreen pass before split toning was
added, so enabling it from that direct path has a real per-frame cost.

Adjustments rebuild the LUT on the settings path, rather than every frame.
Same-size LUT updates upload texels into the existing texture and keep every
bind group. Only an edge-size change recreates the LUT texture and its post
bindings; it does not rebuild unrelated temporal, bloom, DOF or cloud bindings.
Gamma and split-tone changes never compile a new shader/pipeline.
Disabling split toning preserves the existing LUT bytes and shader strength.

`color_grading.rs` owns the parameters and CPU transform; `color_lut.rs` owns
LUT loading and composition; `app/egui_settings/color.rs` owns the Color page.

## Gamma cost audit (2026-10-05)

The renderer before the Color-page changes already skipped post processing on
the basic path when gamma was 1 and the LUT was off. Non-default gamma rendered
into the existing scene target and used one fullscreen `post_fast.wgsl` pass.
With an existing post effect, gamma is calculated inside that post pass instead
of adding another pass. Gamma 2 to gamma 3 keeps the same render path/pass count.
Gamma-only rendering still uses the original shader and three-entry bind group.

Color grading previously excluded the fast world path. Color-only settings now
keep the fast BSP/frame recorder and select the already-created compact gamma/
LUT pipeline. Advanced scene features still select the advanced renderer. The
existing asynchronous pipeline pool and lazy fast-world pipeline cache remain
unchanged; gamma/split-tone settings introduce no shader specialization or PSO
compile. The existing full post pipeline is warmed on the worker pool at startup.

A windowless Vulkan probe on an NVIDIA GeForce RTX 3060 (driver 581.80) rendered
a neutral grayscale input through the actual shaders and used GPU timestamps.
The medians below cover 28 passes after four warm-up passes, without gameplay,
presentation or CPU recording costs:

| Resolution | Gamma only (3.0) | Split toning, gamma 1.0 | Split toning, gamma 3.0 |
| --- | --- | --- | --- |
| 640 x 480 | 0.0061 ms | 0.0072 ms | 0.0072 ms |
| 2560 x 1440 | 0.0604 ms | 0.0799 ms | 0.0829 ms |

These are isolated pass costs, not an end-to-end FPS comparison. Input image,
GPU load/clocks and CPU/GPU overlap affect gameplay results. At 3000 FPS the
entire frame budget is 0.333 ms, so the fullscreen pass is a material cost.
The probe also read back cool shadows, warm highlights and neutral black/white
at gamma 1 and 3, and checked same-size LUT texture reuse.


## Baked brightness

The Color page's **Brightness method** selector uses `r_gammaMethod`:
`shader` (default), `baked`, or `hardware`. `r_gammaMethod baked` uses the existing
`r_gamma` slider to bake a 256-entry RGB curve into scene colour textures.
`r_gammaMethod shader` restores the original texels, then resumes normal output
gamma. The setting is archived in the config; old `r_bakedBrightness` config
entries migrate on load. See [Brightness methods](brightness-methods.md) for
hardware restoration and the differences between the three methods. Neither operation requires
`vid_restart`, a shader compile, texture/view recreation, or new bind groups.

The first non-neutral bake lazily captures original GPU texels on the existing
worker pool. Later changes always bake from those originals, avoiding cumulative
quantization. All uploaded mip levels change; alpha remains byte-for-byte intact.
Readback waits and CPU transforms run on workers. The render thread polls only
while work is pending and uploads finished batches into existing textures. One
batch is in flight at a time, with roughly 2 MiB of packed texture data per batch
(a single larger texture can exceed that). Rapid adjustments replace pending
work; obsolete results cannot install an old curve. Newly encountered model/FX
textures and new maps inherit the current target. Changes appear progressively
and can temporarily affect frame time while pixels are being captured/uploaded.

After completing a bake there are no brightness pixel operations, texture scans,
readbacks, uploads, or worker polling each frame. One idle check remains in the
frame dispatcher. The frame plan uses output gamma 1 in baked mode. With grading
and other post effects disabled this selects direct rendering, including the fast
world path, instead of the gamma fullscreen pass. LUTs, split toning and other
post effects retain their ordinary pass costs; baking brightness does not remove
those effects' passes.

This changes material inputs before lighting and is visually different from an
output curve. It cannot amplify a completely unlit surface. UI art, SDR/HDR
lightmaps, normal/roughness/data maps, procedural output and streaming video are
excluded. Static scene colour textures, models and textured scene FX are included.
Original pixels take extra system RAM only while baked mode or restoration is
active, approximately the packed RGBA8 mip data of affected resident textures.
They are retired when maps/assets are replaced and released after restoring.

Validation: actual-source CPU curve tests and windowless Vulkan/DirectX 12 tests
on an RTX 3060 check unaligned mip rows, exact alpha/original restoration, neutral
settings without readback, data-map exclusion, coalescing/stale jobs, new assets,
map retirement and texture-handle reuse. These tests establish correctness, not
an end-to-end gameplay FPS comparison.

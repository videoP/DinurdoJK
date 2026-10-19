# Anny Profile v1: corrected body transfer (bundle revision 2)

Four profile controls deform existing characters: muscle, weight, proportions
and age. All controls use 0..1; all four at 0.5 reproduce the authored character
exactly. Age is child at 0, young adult at 0.5, elder at 1. These are shape
anchors, not chronological years. Facial wrinkles and age-specific textures
are not authored by this system.

Pilot data covers `jedi_hm`, `jedi_hf`, `jedi_zf`, and Source Juliet (`temp`,
American Casual). Other characters still need model-specific prepared data.

Rebuild the client and restart after staging these assets. `JKBODY2` bundles
require the corrected Rust pose binding. Old cage bundles (`JKBODY1`) are
rejected. Removing `target/release/base/zz_anny_profile_v1.pk3` disables the
pilot. Middle-click a slider to reset it. Archived console controls are
`r_bodyMuscle`, `r_bodyWeight`, `r_bodyProportions`, `r_bodyAge`.

## Why the first version failed

The original global cage chose a clavicle as the chest's direction, allowing
transverse changes to become vertical stretch. Heavy regularization suppressed
local volume while affine extrapolation distorted other regions. Mesh and
helper-pivot changes used inconsistent transfers. The reused Source rebasing
path retained adult animation translation residuals on shorter child bones.
Earlier tests checked finiteness, exported numbers and triangle orientation;
they did not verify anatomical behavior or inspect actual gameplay poses.
Those checks were insufficient.

## Corrected authoring and runtime

`anatomical_transfer.py` uses anatomical segments, authored bone tails, actual
GLM/Source skin weights and Anny-derived transverse radii. It preserves the
target's identity, clothing, topology, UVs and original animation skin weights.
Coincident material seams share the same deformation. Local clothing relaxation
preserves at least 10% of rest projected triangle area; it changes only vertices
of violating triangles, rather than smoothing away the entire body shape.
Maximum local correction in the current sweep is about 1.14 JKA units.

Weight and muscle never change skeletal segment lengths or helper pivots.
Their v1 meanings are separated using Anny's authored contrasts:

- Weight uses the low-muscle cohort's fat contrast against its average-weight
  reference. This produces a visible fat range instead of the small lean-mass
  range at average muscle.
- Below-neutral muscle uses the low-weight minimum/average-muscle contrast.
  Above-neutral muscle uses the high-weight maximum/average-muscle contrast.
  Arm and chest bulk increase. The female source's thighs can become slightly
  leaner; this is recorded in the measurements, not forced to increase.
- Age/proportions alone determine segment lengths, head/extremity scale and
  skeletal pivots. Helpers follow their owning segment's skeletal transform.

These are Anny-derived product controls, not identical copies of Anny's raw
coupled muscle/weight parameters. Anny's raw coupling could reverse waist growth
for some muscular child combinations. No arbitrary whole-body height scaling
is driven by weight or muscle.

`BodyShapeBinding` first applies the existing Source baseline rebind where
needed, then scales complete animated joint links into the shaped proportions.
This includes animated translation residuals. It uses the actual inverse of
quantized parent rotations, rather than assuming exact orthonormality.
Zero-length face/helper links inherit the nearest physical segment's change.
Native gameplay, Source gameplay and externally supplied previews share this
binding. Source donor bolts are fitted after body shaping.

A 135-sample grid (muscle 3, weight 3, proportions 3, age 5) is interpolated
at registration before the existing anatomical fit and breast/glute bake.
Bundles must match both the source-file fingerprint and exact neutral bind
geometry. The neutral path keeps authored geometry and ordinary JKA poses.
Existing breast/glute detection, size controls and physics remain in place.

Settings are local presentation controls shared by prepared characters, not a
networked per-player recipe. Gameplay collision/view heights do not follow
cosmetic child size. Elder changes body proportions, not the animation style.
Clothing self-intersection and every animation in the game are not certified
by the current checks.

## Reproduce

Python needs NumPy, SciPy, Matplotlib and Pillow. The scripts read the local
`../anny-main` checkout and installed assets in `target/release/base`; they do
not execute PyTorch or download models. `anny_data.py` implements the local
Anny/MakeHuman macrodetail recipe in NumPy. All generated files stay in `output`.

```powershell
python tools/jiggle-shape-preview/anny/profile_preview.py --model jedi_hm --gender 0 --export
python tools/jiggle-shape-preview/anny/profile_preview.py --model jedi_hf --export
python tools/jiggle-shape-preview/anny/profile_preview.py --model jedi_zf --export
```

Capture Source using the exact runtime conversion, including skin weights:

```powershell
cargo run -p jka-assets --offline --locked --target-dir target/anny-profile-check --example export_body_shape_bind -- target/release/base models/player/fwhykay/lollipop_chainsaw/juliet_starling_cine/juliet_starling_american_casual/juliet_starling_american_casual.mdl tools/jiggle-shape-preview/anny/output/temp.bind
python tools/jiggle-shape-preview/anny/profile_preview.py --model temp --bind tools/jiggle-shape-preview/anny/output/temp.bind --export
```

The capture uses default Source bodygroups, skin family and rebind mode, with
optional explicit scale as the final argument. Other descriptor options need
equivalent captures. `lollipop_new` MDL v44 remains unsupported.

Validate the generated payloads and render actual gameplay poses:

```powershell
python tools/jiggle-shape-preview/anny/validate_profile.py --model jedi_hm --gender 0
python tools/jiggle-shape-preview/anny/validate_profile.py --model jedi_hf
python tools/jiggle-shape-preview/anny/validate_profile.py --model jedi_zf
python tools/jiggle-shape-preview/anny/validate_profile.py --model temp --bind tools/jiggle-shape-preview/anny/output/temp.bind
cargo test -p jka-assets --offline --locked --lib body_shape --target-dir target/anny-profile-check -- --include-ignored --nocapture
cargo run -p jka-assets --offline --locked --target-dir target/anny-profile-check --example export_body_shape_poses -- target/release/base tools/jiggle-shape-preview/anny/output
python tools/jiggle-shape-preview/anny/render_poses.py --model jedi_hm --gif
python tools/jiggle-shape-preview/anny/render_poses.py --model jedi_hf --gif
python tools/jiggle-shape-preview/anny/render_poses.py --model jedi_zf --gif
python tools/jiggle-shape-preview/anny/render_poses.py --model temp --gif
cargo check -p DinurdoJK --offline --locked --target-dir target/anny-profile-check
python tools/jiggle-shape-preview/anny/package_profiles.py --install
```

The isolated target uses the workspace's existing Steam Audio SDK cache.
Generated JSON and CSV checks identify the exact payload checked. Packaging
rejects stale reports or animation checks from a different bundle.

## Checked results

The four pilots pass 148 geometry recipes each: all 135 anchors, neutral and
mixed cases, plus eight deterministic random recipes. Across all authored LODs:
zero opened exact-position seams, reversed projected faces or relative area
collapses. Away from anchors the direct float64 authoring recipe differs from
the interpolated grid by at most 0.118 JKA units; anchor error is below 0.00002.
Rust interpolation agrees with its separate float64 grid oracle within 0.000004.
The oracle checks interpolation, not anatomical quality.

Each character also passes 135 independent anatomical measurement cases across
muscle/weight combinations and age/proportion endpoints: weight increases waist
cross-section girth monotonically; tissue controls change bind height by zero
and joint pivots by less than 0.00002 units. At neutral adult proportions,
male waist girth is 24.39 / 34.37 / 45.33 for weight 0 / 0.5 / 1, with height
63.52 in every case. Cross sections are convex-hull perimeters of actual
triangle/plane intersections, with anatomical skin-weight selection.

The Rust capture checks 12 recipes x 32 gameplay frames x 4 characters = 1,536
posed meshes and 85,632 animated joint links. Clips are BOTH_STAND1 (14651-14652),
BOTH_WALK1 (20438-20463), BOTH_CROUCH1 (2944-2947). Worst animated link-length
error against intended proportions is below 0.000062 JKA units. Source capture
also checks donor hand-bolt evaluation. Six targeted Rust tests and the client
compile check pass; existing unrelated compile warnings remain.

`*_v1.png` shows bind comparisons for each slider. `*_animated_review.png`
shows weight, bulk, child and elder; `*_stress_review.png` shows muscle,
proportions and combined child extremes. `*_walk.gif` shows actual CPU-skinned
walk frames with one camera scale and neutral floor offset per character.
These are offline geometry views using the client's Rust binding and Ghoul2
skinner. No live-game session or WGPU/material/jiggle combination was visually
verified. A release executable was not replaced.

The old breast/glute `compare_regions.py` remains a historical cage experiment;
it does not use this corrected body transfer or install a new detector/backend.
The original candidate-I glute regression passed earlier. The historical breast
density script's old-source snapshot assertion remains incompatible with the
pre-existing Source skin gates and is not reported as passing.

## Attribution

Anny: NAVER Corp., Apache-2.0, https://github.com/naver/anny.
The MakeHuman/MPFB2 mesh, rig and targets in that checkout are CC0. No
SMPL/SMPL-X topology, SOMA asset, external model download or training model is
used. Bundles require the original installed character assets. The staged PK3
includes both source license notices. Earlier flawed output is retained under
`output/previous-v1` for diagnosis.

The runtime data is exported from `D:/Code/anny-main`. It contains the original
624 phenotype targets, 256 paired local controls, 52 facial actions, template,
game-engine rig regressors, skin weights and MakeHuman body/eye/tongue triangles.
The donor uses upstream's unedited MakeHuman topology and shorter-diagonal quad
triangulation. Float32 targets are
compressed losslessly; they are not replaced by procedural bulges or scaling
curves. Anny and MakeHuman licenses are included inside `anny_targets.zip`.

Source references:

- `src/anny/models/full_model.py`: macro target construction, newborn handling,
  `target.json` ordering, positive/negative local target pairing.
- `src/anny/models/phenotype.py`: anchor interpolation, normalized ancestry
  weights, phenotype coefficient products and signed local coefficients.
- `src/anny/models/facial_actions.py`: faceunit targets and their order.
- `src/anny/models/model_transforms.py`: barycentric blendshape interpolation.
- `src/anny/utils/warp_mesh_utils.py`: planar and edge triangle projection.

Dinurdo's adapter fits the donor to the model's native bind skeleton and projects
each unique bind vertex onto that fitted donor surface. It samples target
**displacements**, preserving the game model's original neutral positions,
topology, UVs and weights. Deformed joint regressors feed the existing
`BodyShapeBinding` animation path.
Profile's five existing whole-body controls use the restored implementation
from `universal-body-shaping.patch` (`procedural.rs`, unchanged from that patch).
`apply_profile` applies added Anny details on the neutral model, then applies
those whole-body controls through the original model's bind-space deformation.
Both native GLMs and Source/Unreal imports use this same Profile entry point.
Joint changes are transferred along parent-relative segments, following the
previous `AnatomicalTransfer` authoring path. Vertex displacements carry that
same joint movement and transfer the remaining deformation relative to the
joint. Applying independently rotated full-body displacements to adjacent
joints caused the height/age hand-stretch regression; the segment transfer
preserves authored source length ratios on both native and imported rigs.
The old procedural implementation remains available to its existing tests/tools,
and is again the whole-body path used by the Profile loaders.

Different topology remains a practical limit: a game mesh cannot reproduce a
feature for which it has no geometry (for example tongue or eyelids). Fitting is
an adapter for the game's mapped humanoid rigs, rather than an exact Anny mesh
replacement. Anatomical proportions and loose clothing need visual review on
individual models. Gameplay collision bounds are unaffected.

Click selection intersects the visible animated native body triangles. The
projected donor surface selects the group of authored local targets influencing
that part. Drag rotation is separate. This UI adapter is Dinurdo-specific; the
upstream interactive demo uses parameter dropdowns.

The selected group receives a translucent red triangle overlay, following the
upstream demo's mesh-diagnostic overlay approach. Masks use the same donor
classification as picking; overlays share the actual rendered CPU vertices,
including promoted topology and jiggle, and retain ordinary depth testing.
Click a deformation label to reset its control; double-clicking its slider
also restores neutral and clears any pending drag value.
The editor footer also offers Reset whole body, restoring all Anny controls
and the existing breast/glute size offsets together without changing physics
or cosmetics. Anny cup size and firmness appear in the breast section as well
as the whole-body phenotype list, and are searchable by their visible names.
Gender remains visible above the filtered part controls. The catalog was
audited against the source's phenotype variations, target.json opposite pairs,
and FACIAL_ACTION_LABELS: all 11 phenotype sliders, 256 local controls, and 52
facial actions are present. Pose/rig manipulation is separate from deformation.

`r_bodyAnny` stores named nondefault detail values through the normal archived
cvar path. Existing `r_bodyMuscle`, `r_bodyWeight`, `r_bodyProportions`,
`r_bodyHeight` and `r_bodyAge` retain their previous whole-body deformation. All-neutral
settings skip deformation exactly. The macro demo range is 0..1; signed local
controls use -1..1 and facial actions use 0..1, with the demo's 0.05 slider step.

Regenerate and validate without PyTorch:

```powershell
python tools/jiggle-shape-preview/anny/export_runtime.py D:/Code/anny-main
python tools/jiggle-shape-preview/anny/validate_runtime.py D:/Code/anny-main
cargo test -p jka-assets --profile fast body_shape::anny
cargo test -p jka-assets --profile fast retail_models_keep_neutral_geometry_and_animate_authored_local_targets -- --ignored
cargo check -p DinurdoJK --profile fast
```

The Rust parity test compares every donor vertex in three mixed recipes with
the independent existing Python evaluator, including positive/negative local
targets. The Python validator checks all 512 local target sides bit for bit
against the checkout. The retail test checks neutral identity, changed geometry,
all LODs, unchanged topology/weights, and finite animated positions/normals on
the three built-in Jedi body models, including height/age extremes in standing,
walking and crouching poses. Segment-ratio regressions cover six global controls
at four values on both humanoid rigs and a differently proportioned imported
bind skeleton. A donor-surface test also checks retopology
identity against a mixed authored recipe and distinguishes selected regions.

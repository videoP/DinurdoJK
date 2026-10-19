# Export directory layout

`djk-ue-import` reads one directory. Producing it is the exporter's job; the
field names below match what CUE4Parse's JSON serialisation emits, so a thin
adapter is usually enough. `tests/import_export_dir.rs` is a complete minimal
example.

```
my-export/
  report.json            required   asset list and what each one exported as
  dependencies.json      required   the seed (root) assets you asked for
  materials.json         required*  material parameters, keyed by material id
  native-materials-v2.json optional output of tools/material-ports.py; preferred over materials.json
  native-labels.json     optional   { "<lower-case asset path>": "Display title" }
  texture-classes.json   optional   output of `djk-ue-import audit-textures`
  Meshes/<id>.json            static mesh
  Meshes/<id>.skeletal.json   skeletal mesh (used as a skinned prop)
  Textures/<id>.png           RGBA8 PNG
  NativeTextures/<name>       optional generated textures referenced by a port
```

## Identifiers

`<id>` is the first 24 lower-case hex digits of `SHA-256(lower-case object path)`,
for example `/Game/Props/SM_Chair.SM_Chair`. Using one stable derivation means
two exports of the same project agree on every file name and can be merged.
The importer rejects any id that is not 24 hex digits.

## report.json

```json
{"assets": [
  {"asset": "/Game/Props/SM_Chair.SM_Chair", "id": "<id>", "status": "static_mesh"}
]}
```

Only `static_mesh` and `skeletal_authoring_only` rows are converted; any other
status (`texture`, `material_approximation`, `failed`, ...) is informational and
is counted in the output report.

## Meshes/<id>.json (static)

```json
{"asset": "/Game/Props/SM_Chair.SM_Chair",
 "vertices": [[x, y, z, nx, ny, nz, u, v], ...],
 "indices": [0, 1, 2, ...],
 "sections": [{"first_index": 0, "triangles": 1, "material": "<material id or null>"}]}
```

Highest-detail LOD, UE units, UE handedness. Optional `vertex_colors` and
`extra_uvs` may be present and are ignored.

## Meshes/<id>.skeletal.json

Same `indices`/`sections`, but `vertices` are objects and a skeleton is added:

```json
{"vertices": [{"Position": {"X":0,"Y":0,"Z":0}, "Normal": {"X":0,"Y":0,"Z":1},
               "Uv": {"U":0,"V":0}, "Influences": [{"Bone": 0, "Weight": 1.0}]}],
 "bones": [{"Name": "root", "ParentIndex": -1,
            "Transform": {"Rotation": {"X":0,"Y":0,"Z":0,"W":1},
                          "Translation": {"X":0,"Y":0,"Z":0},
                          "Scale3D": {"X":1,"Y":1,"Z":1}}}]}
```

Bones must be stored parent-before-child. `Transform` is the *mesh* reference
pose.

## materials.json

```json
{"<material id>": {
   "asset": "/Game/Props/M_Wood.M_Wood",
   "diffuse": "<texture id or null>",
   "blend": "BLEND_Opaque | BLEND_Masked | BLEND_Translucent | BLEND_Additive",
   "shading_model": "MSM_DefaultLit | MSM_Unlit | ...",
   "parameters": {
     "textures": {"BaseColor": "/Game/Tex/T_Wood.T_Wood"},
     "colors":   {"Tint": {"R":1,"G":1,"B":1,"A":1}},
     "scalars":  {"Opacity": 1.0},
     "switches": {"UseEmissive": false}}}}
```

`tools/material-ports.py` resolves this into `native-materials-v2.json`
(diffuse/normal/emissive texture ids, `color`, `opacity`, `unlit`,
`two_sided`, optional `program`, a `status` label). It matches *parameter
names* with case-insensitive lists near the top of the script; **add your
project's master-material parameter names there** (for example `Albedo`,
`MainTex`) if its materials are not picked up.

## Outputs

| Member | Content |
| --- | --- |
| `<ns>/<name>_<id16>.md3` | static geometry |
| `<ns>/<name>_<id16>.djk_prop` | weighted geometry for skeletal assets |
| `<ns>/t_<id>.png`, `n_<id>.png` | base-colour / normal textures |
| `<ns>/neutral.tga`, `unsupported.tga` | authored-white / diagnostic checker |
| `shaders/<leaf of ns>_props.shader` | one shader per used material |
| `<ns>/index.json` | models, materials, failures and known limitations |

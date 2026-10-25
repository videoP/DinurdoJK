"""Export Anny's authored targets, without requiring PyTorch at game runtime.

Ports full_model.load_macrodetails/load_all_blendshapes, including target.json
ordering and positive/negative pairs. No fitted or hand-authored morph curves.
Run: python tools/jiggle-shape-preview/anny/export_runtime.py D:/Code/anny-main
"""
import argparse
import hashlib
import io
import json
from itertools import product
from pathlib import Path
import zipfile

import numpy as np
from anny_data import AnnyData, VARIATIONS, bone_mapping
from trim_runtime import ROOT, pack_bytes


def export(checkout, destination, full=False):
    source = AnnyData(checkout, gender=0.5)
    # full_model.load_mesh(eyes=True, tongue=True), with mesh_utils._split_quad's
    # shorter-diagonal rule. Joint helper cubes never enter the projection mesh.
    triangles = []
    group = ""
    for line in (source.root / "3dobjs/base.obj").read_text().splitlines():
        words = line.split()
        if not words:
            continue
        if words[0] == "g":
            group = words[1]
        if words[0] != "f" or group not in ("body", "helper-l-eye", "helper-r-eye", "helper-tongue"):
            continue
        face = [int(x.split("/")[0])-1 for x in words[1:]]
        if len(face) == 3:
            triangles.append(face)
        else:
            a, b, c, d = face
            if np.linalg.norm(source.template[a]-source.template[c]) < np.linalg.norm(source.template[b]-source.template[d]):
                triangles.extend(([a,b,c], [c,d,a]))
            else:
                triangles.extend(([a,b,d], [d,b,c]))
    source.faces = np.asarray(triangles, dtype=np.int32)
    root = source.root / "targets"
    arrays = bytearray()
    blocks = []

    def array(value, dtype):
        value = np.asarray(value, dtype=dtype)
        offset = len(arrays)
        # Lossless byte shuffle groups IEEE exponent/sign bytes for deflate.
        # It reduces the asset data without quantizing targets.
        arrays.extend(np.frombuffer(value.tobytes(), dtype=np.uint8).reshape(-1, 4).T.tobytes())
        blocks.append([offset, value.size])
        return [offset, value.size]

    targets = []
    for axes, prefix in [
        (("gender", "age", "muscle", "weight"), "macrodetails/universal-"),
        (("race", "gender", "age"), "macrodetails/"),
        (("gender", "age", "muscle", "weight", "height"), "macrodetails/height/"),
        (("gender", "age", "muscle", "weight", "proportions"), "macrodetails/proportions/"),
        (("gender", "age", "muscle", "weight", "cupsize", "firmness"), "breast/"),
    ]:
        for names in product(*(VARIATIONS[a] for a in axes)):
            if axes[-1] == "proportions" and names[1] in ("baby", "newborn"):
                continue
            if axes[-1] == "firmness" and names[0] != "female":
                continue
            relative = prefix + "-".join(names)
            if "newborn" in names:
                relative = relative.replace("newborn", "baby")
            if not (root / (relative + ".target.gz")).exists():
                if axes[-1] == "firmness":
                    continue
                raise FileNotFoundError(relative)
            delta = source.target(relative).copy()
            if "newborn" in names:
                scale = np.array([0.922, 0.922, 0.75])
                delta = delta * scale + source.template * ((scale - 1) / 3)
            ids = np.flatnonzero(np.any(delta != 0, axis=1))
            targets.append(dict(terms=[[a, VARIATIONS[a].index(n)] for a, n in zip(axes, names)],
                                indices=array(ids, "<u4"), delta=array(delta[ids], "<f4")))

    controls = []
    for group, metadata in json.loads((root / "target.json").read_text()).items():
        if group == "genitals":  # Same exclusion as full_model.py.
            continue
        for category in metadata["categories"]:
            for side in ("left", "right", "unsided"):
                opposites = category.get("opposites", {})
                positive, negative = opposites.get("positive-" + side), opposites.get("negative-" + side)
                if not positive or not negative:
                    continue
                control = dict(label=positive, group=group, sides=[])
                for name in (positive, negative):
                    delta = source.target(group + "/" + name)
                    ids = np.flatnonzero(np.any(delta != 0, axis=1))
                    control["sides"].append(dict(indices=array(ids, "<u4"), delta=array(delta[ids], "<f4")))
                controls.append(control)

    # Full-model facial actions use the upstream faceunit targets verbatim.
    face_root = Path(checkout) / "src/anny/data/faceunits01/targets/faceunits"
    facial = []
    for path in sorted(face_root.glob("*.target")):
        text = path.read_text().strip()
        rows = np.fromstring(text, sep=" ").reshape(-1, 4) if text else np.empty((0, 4))
        facial.append(dict(label=path.stem, indices=array(rows[:, 0], "<u4"),
                           delta=array(source.coordinates(rows[:, 1:]), "<f4")))

    weights = []
    for row in source.weights:
        weights.append([[int(i), float(row[i])] for i in np.flatnonzero(row)])
    mapping = bone_mapping()
    bones = [dict(name=name, target=mapping.get(name, ""), parent=source.rig[name]["parent"],
                  head=source.head_indices[i].tolist(), tail=source.tail_indices[i].tolist())
             for i, name in enumerate(source.bones)]
    # Oracle comes from the pre-existing independent AnnyData evaluator and raw
    # opposite target files, rather than the exported runtime term table.
    oracle = []
    for values, cup, local_value in [([.5,.5,.5,.25], .5, 0.),
                                      ([.16,.81,.32,.62], .7, -.65),
                                      ([.91,.24,.79,.06], .2, .8)]:
        expected = source.evaluate(values, cupsize=cup)
        suffix = "incr" if local_value >= 0 else "decr"
        expected += abs(local_value) * source.target("buttocks/buttocks-volume-" + suffix)
        oracle.append(dict(params=[*values[:3], .5, 1/3 + values[3]*2/3],
                           details=f"cupsize={cup},buttocks-volume-incr={local_value}",
                           expected=array(expected, "<f4")))
    metadata = dict(version=1, vertices=array(source.template, "<f4"), faces=array(source.faces, "<u4"),
                    weights=weights, bones=bones, targets=targets, controls=controls, facial=facial,
                    variations=VARIATIONS, blocks=blocks, oracle=oracle)
    destination.mkdir(parents=True, exist_ok=True)
    bundle = io.BytesIO()
    with zipfile.ZipFile(bundle, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        archive.writestr("model.json", json.dumps(metadata, separators=(",", ":")))
        archive.writestr("arrays.bin", arrays)
        archive.write(Path(checkout) / "LICENSE", "Anny-LICENSE.txt")
        archive.write(source.root / "LICENSE.md", "MakeHuman-LICENSE.md")
    catalog = ["// Generated by tools/jiggle-shape-preview/anny/export_runtime.py; upstream order.",
               f"pub const LOCAL_COUNT: usize = {len(controls)};",
               f"pub const FACIAL_COUNT: usize = {len(facial)};",
               "pub const LOCAL_CONTROLS: [(&str, &str); LOCAL_COUNT] = ["]
    catalog += [f'    ("{c["label"]}", "{c["group"]}"),' for c in controls]
    catalog += ["];", "pub const FACIAL_CONTROLS: [&str; FACIAL_COUNT] = ["]
    catalog += [f'    "{c["label"]}",' for c in facial]
    catalog += ["];", ""]
    catalog_path = destination / "catalog.rs"
    catalog_text = "\n".join(catalog)
    if not catalog_path.exists() or catalog_path.read_text() != catalog_text:
        catalog_path.write_text(catalog_text)
    if full:
        (destination / "anny_targets.zip").write_bytes(bundle.getvalue())
        print(json.dumps(dict(local=len(controls), facial=len(facial), macro=len(targets),
                              bytes=len(bundle.getvalue()), sha256=hashlib.sha256(bundle.getvalue()).hexdigest())))
    else:
        pack_bytes(bundle.getvalue(), ROOT / "assets/dinurdo-assets.pk3",
                   ROOT / "crates/jka-assets/tests/data/anny-neutral-oracle.bin")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("checkout", type=Path)
    parser.add_argument("--full", action="store_true", help="Write the full offline authoring archive instead of updating runtime assets")
    parser.add_argument("--destination", type=Path,
                        default=Path(__file__).resolve().parents[3] / "crates/jka-assets/src/body_shape/anny")
    args = parser.parse_args()
    export(args.checkout, args.destination, args.full)

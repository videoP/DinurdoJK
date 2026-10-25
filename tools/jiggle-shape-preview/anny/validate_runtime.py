"""Validate the shipped Anny data against original targets, entirely in memory."""
import argparse
import json
from pathlib import Path
import zipfile

import numpy as np
from anny_data import AnnyData, VARIATIONS


def validate(checkout, bundle):
    with zipfile.ZipFile(bundle) as archive:
        prefix = "customization/body/" if bundle.suffix == ".pk3" else ""
        metadata = json.loads(archive.read(prefix + "model.json"))
        shuffled = archive.read(prefix + "arrays.bin")
    data = bytearray(shuffled)
    for offset, count in metadata["blocks"]:
        data[offset:offset + count*4] = np.frombuffer(shuffled, np.uint8, count=count*4,
                                                    offset=offset).reshape(4, count).T.tobytes()

    def array(spec, dtype):
        offset, count = spec
        return np.frombuffer(data, dtype=dtype, count=count, offset=offset)

    def sparse(spec):
        indices = array(spec["indices"], "<u4")
        delta = array(spec["delta"], "<f4").reshape(-1, 3)
        result = np.zeros_like(source.template)
        result[indices] = delta
        return result

    source = AnnyData(checkout, gender=.5)
    worst = 0.
    prefixes = {
        ("gender", "age", "muscle", "weight"): "macrodetails/universal-",
        ("race", "gender", "age"): "macrodetails/",
        ("gender", "age", "muscle", "weight", "height"): "macrodetails/height/",
        ("gender", "age", "muscle", "weight", "proportions"): "macrodetails/proportions/",
        ("gender", "age", "muscle", "weight", "cupsize", "firmness"): "breast/",
    }
    for target in metadata["targets"]:
        axes = tuple(axis for axis, _ in target["terms"])
        names = [VARIATIONS[axis][anchor] for axis, anchor in target["terms"]]
        relative = prefixes[axes] + "-".join(names)
        expected = source.target(relative.replace("newborn", "baby")).copy()
        if "newborn" in names:
            scale = np.array([.922, .922, .75])
            expected = expected * scale + source.template * ((scale - 1) / 3)
        assert np.array_equal(sparse(target), expected.astype(np.float32)), relative
    for control in metadata["controls"]:
        target_metadata = json.loads((source.root / "targets/target.json").read_text())
        pairs = [(category["opposites"]["positive-" + side], category["opposites"]["negative-" + side])
                 for category in target_metadata[control["group"]]["categories"] if "opposites" in category
                 for side in ("left", "right", "unsided")]
        positive, negative = next(pair for pair in pairs if pair[0] == control["label"])
        for spec, name in zip(control["sides"], (positive, negative)):
            expected = source.target(control["group"] + "/" + name).astype(np.float32)
            actual = sparse(spec)
            assert np.array_equal(actual, expected), name
    face_root = Path(checkout) / "src/anny/data/faceunits01/targets/faceunits"
    for face in metadata["facial"]:
        rows = np.fromstring((face_root / (face["label"] + ".target")).read_text(), sep=" ").reshape(-1, 4)
        expected = np.zeros_like(source.template)
        expected[rows[:, 0].astype(int)] = source.coordinates(rows[:, 1:]).astype(np.float32)
        assert np.array_equal(sparse(face), expected), face["label"]
    if metadata["version"] == 2:
        assert metadata["fixed_params"] == [.5] * 5
        reference = np.fromfile(Path(__file__).resolve().parents[3] /
                                "crates/jka-assets/tests/data/anny-neutral-oracle.bin", dtype="<f4").reshape(-1, 3)
        expected = source.evaluate([.5, .5, .5, .25]).astype(np.float32)
        assert np.array_equal(reference, expected)
    for oracle in metadata.get("oracle", []):
        muscle, weight, proportions, height, age = oracle["params"]
        assert height == .5
        details = dict(term.split("=") for term in oracle["details"].split(","))
        expected = source.evaluate([muscle, weight, proportions, (age-1/3)*1.5],
                                   cupsize=float(details["cupsize"]),
                                   buttocks=float(details["buttocks-volume-incr"]))
        actual = array(oracle["expected"], "<f4").reshape(-1, 3)
        worst = max(worst, float(np.max(np.abs(actual-expected))))
        assert np.allclose(actual, expected, atol=1e-7, rtol=0)
    print(json.dumps(dict(macro_targets_checked=len(metadata["targets"]),
                          local_targets_checked=2*len(metadata["controls"]),
                          facial_shapes_checked=len(metadata["facial"]),
                          oracle_cases=len(metadata.get("oracle", [])), worst_error=worst)))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("checkout", type=Path)
    parser.add_argument("--bundle", type=Path,
                        default=Path(__file__).resolve().parents[3] / "assets/dinurdo-assets.pk3")
    args = parser.parse_args()
    validate(args.checkout, args.bundle)

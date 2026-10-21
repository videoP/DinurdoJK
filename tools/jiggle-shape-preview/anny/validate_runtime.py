"""Validate the shipped Anny data against original targets, entirely in memory."""
import argparse
import json
from pathlib import Path
import zipfile

import numpy as np
from anny_data import AnnyData


def validate(checkout, bundle):
    with zipfile.ZipFile(bundle) as archive:
        metadata = json.loads(archive.read("model.json"))
        shuffled = archive.read("arrays.bin")
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
    for oracle in metadata["oracle"]:
        muscle, weight, proportions, height, age = oracle["params"]
        assert height == .5
        details = dict(term.split("=") for term in oracle["details"].split(","))
        expected = source.evaluate([muscle, weight, proportions, (age-1/3)*1.5],
                                   cupsize=float(details["cupsize"]),
                                   buttocks=float(details["buttocks-volume-incr"]))
        actual = array(oracle["expected"], "<f4").reshape(-1, 3)
        worst = max(worst, float(np.max(np.abs(actual-expected))))
        assert np.allclose(actual, expected, atol=1e-7, rtol=0)
    print(json.dumps(dict(local_targets_checked=2*len(metadata["controls"]),
                          oracle_cases=len(metadata["oracle"]), worst_error=worst)))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("checkout", type=Path)
    parser.add_argument("--bundle", type=Path,
                        default=Path(__file__).resolve().parents[3] / "crates/jka-assets/src/body_shape/anny/anny_targets.zip")
    args = parser.parse_args()
    validate(args.checkout, args.bundle)

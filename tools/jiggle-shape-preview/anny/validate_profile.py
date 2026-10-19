"""Independent float64 recipe -> float32 export checks and Rust oracle fixtures."""
from pathlib import Path
from itertools import product
import argparse
import json
import struct
import hashlib
import numpy as np
from anny_data import AnnyData, NEUTRAL
from profile_preview import Assets, read_glm, read_gla, read_bind, fingerprint


def read_bundle(path):
    raw = path.read_bytes(); offset = 8
    assert raw[:8] == b"JKBODY2\0"
    fp, bind_hash, bones, nlods, nsamples, adult = struct.unpack_from("<QQIIII", raw, offset); offset += 32
    knots = []
    for _ in range(4):
        n = struct.unpack_from("<I", raw, offset)[0]; offset += 4
        knots.append(np.frombuffer(raw, "<f4", n, offset).copy()); offset += 4 * n
    surfaces = []
    for _ in range(nlods):
        count = struct.unpack_from("<I", raw, offset)[0]; offset += 4
        for _ in range(count):
            si, n = struct.unpack_from("<II", raw, offset); offset += 8
            surfaces.append(n)
    samples = np.frombuffer(raw, "<f4", offset=offset).reshape(nsamples, bones + sum(surfaces), 3)
    return fp, knots, samples


def interpolate_grid(knots, samples, controls):
    choices = []
    for axis, v in zip(knots, controls):
        hi = int(np.clip(np.searchsorted(axis, v), 1, len(axis) - 1))
        t = (v - axis[hi - 1]) / (axis[hi] - axis[hi - 1])
        choices.append([(hi - 1, 1 - t), (hi, t)])
    result = np.zeros_like(samples[0], dtype=np.float64)
    for terms in product(*choices):
        index = 0; weight = 1
        for (i, w), axis in zip(terms, knots):
            index = index * len(axis) + i; weight *= w
        result += samples[index] * weight
    return result


def validate(model, gender, root, checkout, out, bind=None):
    assets = Assets(root / "target/release/base")
    if bind:
        source_fp, lods, names, heads = read_bind(bind)
    else:
        raw = assets.read(f"models/players/{model}/model.glm")
        source_fp = fingerprint(raw)
        lods, anim = read_glm(raw)
        names, heads = read_gla(assets.read(anim if anim.endswith(".gla") else anim + ".gla"))
    assets.close()
    source = AnnyData(checkout, gender)
    from anatomical_transfer import AnatomicalTransfer
    transfer = AnatomicalTransfer(source, names, heads, lods)
    fp, knots, samples = read_bundle(out / f"{model}.bodyshape")
    assert fp == source_fp
    neutral = interpolate_grid(knots, samples, NEUTRAL)
    cases = [NEUTRAL, [0, 0, 0, 0], [1, 1, 1, 1], [0.23, 0.84, 0.45, 0.67], [0.8, 0.15, 0.72, 0.3]]
    oracle_count = len(cases)
    cases += list(product(*knots))
    cases += list(np.random.default_rng(9127).uniform(size=(8, 4)))
    oracle = bytearray(b"JKORACLE" + struct.pack("<II", oracle_count, samples.shape[1]))
    bundle_raw = (out / f"{model}.bodyshape").read_bytes()
    report = dict(model=model, cases=[], lods=len(lods), bundle_sha256=hashlib.sha256(bundle_raw).hexdigest())
    for case_index, controls in enumerate(cases):
        controls = np.asarray(controls, dtype=np.float32).astype(float)
        direct = transfer.bake(controls)
        from_grid = interpolate_grid(knots, samples, controls) - neutral
        error = float(abs(from_grid - direct).max())
        at_anchor = all(any(abs(v-k)<1e-7 for k in axis) for axis,v in zip(knots,controls))
        assert error < (2e-5 if at_anchor else .15), (model, controls, error)
        assert np.isfinite(from_grid).all()
        offset = len(names); seam_error = 0.; flipped = 0; collapsed = 0
        for lod in lods:
            positions = {}
            for s in lod:
                shifted = s.positions + from_grid[offset:offset + len(s.positions)]
                offset += len(s.positions)
                for old, new in zip(s.positions, shifted):
                    key = tuple(old)
                    if key in positions:
                        seam_error = max(seam_error, float(abs(new - positions[key]).max()))
                    positions[key] = new
                old_tri, new_tri = s.positions[s.triangles], shifted[s.triangles]
                a = np.cross(old_tri[:, 1] - old_tri[:, 0], old_tri[:, 2] - old_tri[:, 0])
                b = np.cross(new_tri[:, 1] - new_tri[:, 0], new_tri[:, 2] - new_tri[:, 0])
                valid = np.linalg.norm(a, axis=1) > 1e-7
                flipped += int(np.sum((np.sum(a * b, axis=1) < 0) & valid))
                # Child bodies legitimately reduce triangle area. Detect loss
                # relative to each authored face, rather than an absolute area.
                collapsed += int(np.sum((np.linalg.norm(b, axis=1) < np.linalg.norm(a, axis=1) * 1e-5) & valid))
        assert seam_error < 2e-5, (model, controls, seam_error)
        assert collapsed == 0, (model, controls, collapsed)
        assert flipped == 0, (model, controls, flipped)
        if case_index < oracle_count:
            oracle.extend(controls.astype("<f4").tobytes())
            # Runtime interpolation parity is separate from the direct authoring
            # error and the independent anatomical measurements below.
            oracle.extend(from_grid.astype("<f4").tobytes())
        report["cases"].append(dict(controls=list(controls), interpolation_error=error,
                                     seam_error=seam_error, reversed_faces=flipped, collapsed_faces=collapsed))
    from shape_metrics import check_semantics
    report["semantics"] = check_semantics(transfer, knots, samples, lods)
    report["max_local_relaxation"] = max(r.max_correction for r in transfer.relaxations)
    (out / f"{model}_oracle.bin").write_bytes(oracle)
    (out / f"{model}_validation.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(dict(model=model, checked_recipes=len(cases), lods=len(lods),
                         max_interpolation_error=max(c['interpolation_error'] for c in report['cases']),
                         max_seam_error=max(c['seam_error'] for c in report['cases']),
                         reversed_faces=sum(c['reversed_faces'] for c in report['cases']),
                         collapsed_faces=sum(c['collapsed_faces'] for c in report['cases']))))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", default="jedi_hf")
    parser.add_argument("--gender", type=float, default=1)
    parser.add_argument("--bind", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[3]
    validate(args.model, args.gender, root, root.parent / "anny-main", Path(__file__).parent / "output", args.bind)

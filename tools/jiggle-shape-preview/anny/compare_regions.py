"""Contour A/B on the accepted jedi_zf preview meshes; no detection replacement.

Anny's field is restricted to the existing size footprint and scaled to match
peak outward projection. This compares shape contours, not equal slider units,
motion solvers, or an alleged Anny breast/glute detector.
"""
from pathlib import Path
import json
import numpy as np
from matplotlib import pyplot as plt
from anny_data import AnnyData, NEUTRAL
from profile_preview import Assets, LegacyCageTransfer, read_glm, read_gla, draw, ROOT


def main():
    here = Path(__file__).resolve().parent
    baseline = here.parent
    assets = Assets(ROOT / "target/release/base")
    lods, anim = read_glm(assets.read("models/players/jedi_zf/model.glm"))
    names, heads = read_gla(assets.read(anim + ".gla"))
    assets.close()
    transfer = LegacyCageTransfer(AnnyData(ROOT.parent / "anny-main"), names, heads)
    breast = np.load(baseline / "bra_fix/jedi_zf_bra_reference.npz")
    glute = np.load(baseline / "glute/thigh_round_pass/approved_I.npz")
    cases = [
        ("Breast / side", breast["bind"], breast["connected_2_5"], "side", -1, "breast"),
        ("Glute / side", glute["stock"][glute["hips"]], glute["I_25"][glute["hips"]], "side", 1, "glute"),
        ("Glute / back", glute["stock"][glute["hips"]], glute["I_25"][glute["hips"]], "back", 1, "glute"),
    ]
    fig, axes = plt.subplots(3, 3, figsize=(12, 12), facecolor="#111b28")
    report = dict(model="jedi_zf", comparison="existing accepted size footprint; matched peak outward projection", cases=[])
    for row, (label, bind, current, view, sign, kind) in enumerate(cases):
        positions = bind.reshape(-1, 3)
        triangles = np.arange(len(positions)).reshape(-1, 3)
        shape = transfer.delta(NEUTRAL, cupsize=1.0 if kind == "breast" else 0.5,
                               buttocks=1.0 if kind == "glute" else 0)
        delta = transfer.sample(shape, transfer.correspondence(positions))
        current_delta = (current - bind).reshape(-1, 3)
        gate = np.clip((np.linalg.norm(current_delta, axis=1) - 0.02) / 0.13, 0, 1)
        gate = gate * gate * (3 - 2 * gate)
        delta *= gate[:, None]
        peak = max(float(np.max(sign * current_delta[:, 1])), 1e-12)
        candidate_peak = max(float(np.max(sign * delta[:, 1])), 1e-12)
        gain = peak / candidate_peak
        candidate = positions + delta * gain
        if kind == "breast":
            keep = (positions[triangles].mean(1)[:, 2] > 43.5) & (positions[triangles].mean(1)[:, 2] < 58)
            triangles = triangles[keep]
            limits = [(-4.5, 12.0), (43.5, 58)]
        else:
            limits = [(-12.0, 4.5), (21, 42)] if view == "side" else [(-10, 10), (21, 42)]
        for col, (title, mesh) in enumerate([("Authored", positions), ("Current +2.5", current.reshape(-1, 3)), ("Anny field / matched projection", candidate)]):
            draw(axes[row, col], mesh, triangles, limits, view)
            axes[row, col].set_title(f"{label}\n{title}", color="white", fontsize=10)
        report['cases'].append(dict(region=kind, view=view, matched_projection=peak, anny_gain=gain))
    fig.suptitle("jedi_zf / breast and glute contour A/B\nSame camera and existing footprint; no change to detection or physics", color="white", fontsize=13)
    fig.tight_layout(rect=(0, 0, 1, 0.94))
    fig.savefig(here / "output/jedi_zf_region_ab.png", dpi=125)
    plt.close(fig)
    (here / "output/jedi_zf_region_ab.json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()

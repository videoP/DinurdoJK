"""Generate v1 Anny shape previews and registration-time Ghoul2 morph bundles.

Run with --help. Only reads the supplied Anny checkout and local game archives.
All generated files stay in --out. No game executable or build is launched.
"""
from __future__ import annotations

import argparse
from dataclasses import dataclass
from itertools import product
from pathlib import Path
import hashlib
import json
import struct
import zipfile

import numpy as np
from scipy.spatial import cKDTree
from scipy.spatial.distance import cdist
from scipy.linalg import lu_factor, lu_solve
import matplotlib
matplotlib.use("Agg")
from matplotlib import pyplot as plt
from matplotlib.collections import PolyCollection

from anny_data import AXES, NEUTRAL, AnnyData, bone_mapping

ROOT = Path(__file__).resolve().parents[3]


def cstr(raw):
    return raw.split(b"\0", 1)[0].decode("latin1")


class Assets:
    def __init__(self, base):
        self.base = Path(base)
        self.archives = [zipfile.ZipFile(p) for p in sorted(self.base.glob("*.pk3"), key=lambda p: p.name.lower())]
        self.entries = {}
        for z in self.archives:
            self.entries.update({n.lower(): (z, n) for n in z.namelist()})

    def read(self, name, optional=False):
        loose = self.base / name
        if loose.is_file():
            return loose.read_bytes()
        if name.lower() in self.entries:
            z, entry = self.entries[name.lower()]
            return z.read(entry)
        if optional:
            return None
        raise FileNotFoundError(name)

    def close(self):
        for z in self.archives:
            z.close()


@dataclass
class Surface:
    index: int
    name: str
    flags: int
    positions: np.ndarray
    normals: np.ndarray
    triangles: np.ndarray
    bone_weights: np.ndarray | None = None


def read_glm(raw):
    if raw[:4] != b"2LGM":
        raise ValueError("Expected a native Ghoul2 GLM")
    count = struct.unpack_from("<i", raw, 152)[0]
    bone_count = struct.unpack_from("<i", raw, 140)[0]
    names = []
    for i in range(count):
        offset = 164 + struct.unpack_from("<i", raw, 164 + 4 * i)[0]
        names.append((cstr(raw[offset:offset + 64]), struct.unpack_from("<I", raw, offset + 64)[0]))
    lod_count, cursor = struct.unpack_from("<ii", raw, 144)
    lods = []
    for _ in range(lod_count):
        end = cursor + struct.unpack_from("<i", raw, cursor)[0]
        table = cursor + 4
        surfaces = []
        for i in range(count):
            start = table + struct.unpack_from("<i", raw, table + 4 * i)[0]
            _, si, _, nv, vo, nt, to, nb, bo, _ = struct.unpack_from("<10i", raw, start)
            vs = [struct.unpack_from("<6f", raw, start + vo + 32 * v) for v in range(nv)]
            values = np.asarray(vs, dtype=float).reshape(-1, 6)
            triangles = np.array([struct.unpack_from("<3i", raw, start + to + 12 * t) for t in range(nt)], dtype=np.int32)
            references = struct.unpack_from(f"<{nb}i", raw, start + bo)
            weights = np.zeros((nv, bone_count))
            for vi in range(nv):
                p = start + vo + 32 * vi
                packed = struct.unpack_from("<I", raw, p + 24)[0]
                total = 0.0
                for wi in range((packed >> 30) + 1):
                    weight = (1 - total if wi == (packed >> 30) else
                              (raw[p + 28 + wi] | ((packed >> (12 + wi * 2)) & 0x300)) / 1023.0)
                    total += weight
                    weights[vi, references[(packed >> (5 * wi)) & 31]] += weight
            surfaces.append(Surface(si, *names[si], values[:, 3:], values[:, :3], triangles, weights))
        lods.append(surfaces)
        cursor = end
    return lods, cstr(raw[72:136])


def read_gla(raw):
    names, heads = [], []
    for i in range(struct.unpack_from("<i", raw, 84)[0]):
        start = 100 + struct.unpack_from("<i", raw, 100 + 4 * i)[0]
        names.append(cstr(raw[start:start + 64]))
        matrix = np.array(struct.unpack_from("<12f", raw, start + 72)).reshape(3, 4)
        heads.append(matrix[:, 3])
    return names, np.asarray(heads)


def read_bind(path):
    raw = path.read_bytes(); offset = 8
    if raw[:8] != b"JKSHBND2":
        raise ValueError("Expected Source shape-bind capture")
    def take(fmt):
        nonlocal offset
        values = struct.unpack_from(fmt, raw, offset); offset += struct.calcsize(fmt)
        return values
    def name():
        nonlocal offset
        n, = take("<I"); value = raw[offset:offset+n].decode(); offset += n
        return value
    fp, bh, bones, nlods = take("<QQII")
    names, heads = [], []
    for _ in range(bones):
        names.append(name()); heads.append(take("<3f"))
    lods = []
    for _ in range(nlods):
        count, = take("<I"); surfaces = []
        for _ in range(count):
            si, flags = take("<II"); label = name(); nv, nt = take("<II")
            values = np.zeros((nv, 6)); weights = np.zeros((nv, bones))
            for vi in range(nv):
                values[vi] = take("<6f")
                nw, = take("<I")
                for _ in range(nw):
                    bone, weight = take("<If"); weights[vi, bone] += weight
            triangles = np.frombuffer(raw, "<u4", nt*3, offset).reshape(nt,3).astype(np.int32); offset += nt*12
            surfaces.append(Surface(si, label, flags, values[:,:3], values[:,3:], triangles, weights))
        lods.append(surfaces)
    if offset != len(raw) or binding_fingerprint(lods, bones) != bh:
        raise ValueError("Shape-bind capture size/hash mismatch")
    return fp, lods, names, np.asarray(heads)


def minimal_rotation(a, b):
    a = a / max(np.linalg.norm(a), 1e-12)
    b = b / max(np.linalg.norm(b), 1e-12)
    v, c = np.cross(a, b), np.clip(a @ b, -1, 1)
    if c < -0.999999:
        perpendicular = np.cross(a, np.eye(3)[np.argmin(abs(a))])
        perpendicular /= np.linalg.norm(perpendicular)
        return 2 * np.outer(perpendicular, perpendicular) - np.eye(3)
    k = np.array([[0, -v[2], v[1]], [v[2], 0, -v[0]], [-v[1], v[0], 0]])
    return np.eye(3) + k + k @ k / max(1 + c, 1e-12)


class LegacyCageTransfer:
    """Historical failed body cage; retained only for the separate region A/B.

    Never use this for body bundles. AnatomicalTransfer is the v2 authoring path.
    """
    def __init__(self, source, target_names, target_heads, regularization=5.0):
        self.source = source
        self.neutral = source.evaluate()
        heads, tails = source.joints(self.neutral), source.tails(self.neutral)
        targets = dict(zip(target_names, target_heads))
        mapping = bone_mapping()
        self.target_names, self.target_heads = target_names, target_heads
        self.mapped = []
        for bone in source.bones:
            name = mapping[bone]
            if name not in targets:
                name = name.replace("tarsal", "talus").replace("_d3_", "_d2_").replace("_d5_", "_d4_").replace("_j3", "_j2")
            if name not in targets:
                raise ValueError(f"No anatomical counterpart for {bone}: {name}")
            self.mapped.append(name)
        destinations = np.array([targets[n] for n in self.mapped])
        ratios = []
        for bone, child in [("thigh_l", "calf_l"), ("calf_l", "foot_l"),
                            ("upperarm_l", "lowerarm_l"), ("lowerarm_l", "hand_l"),
                            ("pelvis", "neck_01")]:
            i, j = source.bones.index(bone), source.bones.index(child)
            ratios.append(np.linalg.norm(destinations[j] - destinations[i]) / np.linalg.norm(heads[j] - heads[i]))
        self.scale = float(np.median(ratios))
        self.linears = {}
        def linear_for(i):
            if i in self.linears:
                return self.linears[i]
            bone = source.bones[i]
            children = [j for j, b in enumerate(source.bones) if source.rig[b]["parent"] == bone
                        and self.mapped[j] != self.mapped[i]]
            if bone.startswith("hand_"):
                children = [source.bones.index("middle_01_" + bone[-1])]
            if bone == "pelvis":
                children = [source.bones.index("spine_01")]
            child = children[0] if children else None
            direction = tails[i] - heads[i]
            if child is not None:
                direction = heads[child] - heads[i]
                desired = destinations[child] - destinations[i]
                along = np.linalg.norm(desired) / max(np.linalg.norm(direction), 1e-12)
                u = direction / max(np.linalg.norm(direction), 1e-12)
                stretch = self.scale * np.eye(3) + (along - self.scale) * np.outer(u, u)
                linear = minimal_rotation(direction, desired) @ stretch
            else:
                parent = source.rig[bone]["parent"]
                if parent and parent in source.bones:
                    linear = linear_for(source.bones.index(parent))
                else:
                    linear = self.scale * np.eye(3)
            self.linears[i] = linear
            return linear
        self.linears = np.asarray([linear_for(i) for i in range(len(source.bones))])
        self.offsets = destinations - np.einsum("bij,bj->bi", self.linears, heads)
        self.cage = self.place(self.neutral)
        self.indices = source.body_indices
        self.tree = cKDTree(self.cage[self.indices])
        # A smooth volume field avoids nearest-surface switches across thin
        # triangles, clothing layers, necks and fingers. Fit once at neutral.
        candidates = self.cage[self.indices] / self.scale
        distances = np.full(len(candidates), np.inf)
        selected, next_index = [], int(np.argmin(np.linalg.norm(candidates, axis=1)))
        for _ in range(min(384, len(candidates))):
            selected.append(next_index)
            distances = np.minimum(distances, np.sum((candidates - candidates[next_index]) ** 2, axis=1))
            next_index = int(np.argmax(distances))
        self.controls = self.indices[selected]
        self.control_points = candidates[selected]
        self.polynomial = np.c_[np.ones(len(selected)), self.control_points]
        self.kernel_base = self.kernel(cdist(self.control_points, self.control_points))
        # Broad v1 anatomy should follow a smooth volume warp. Stronger
        # regularization prevents a tightly fitted cage from folding clothing.
        self.set_regularization(regularization)

    def set_regularization(self, value):
        self.regularization = value
        kernel = self.kernel_base + np.eye(len(self.controls)) * value
        self.factor = lu_factor(np.block([[kernel, self.polynomial],
                                         [self.polynomial.T, np.zeros((4, 4))]]))

    def regularize_for_mesh(self, lods):
        """Choose the least smoothing that preserves face orientation at every
        exported parameter corner, across all authored surfaces and LODs."""
        surfaces = [s for lod in lods for s in lod]
        points = np.concatenate([s.positions for s in surfaces])
        faces, offset = [], 0
        for s in surfaces:
            faces.extend((s.triangles + offset).tolist()); offset += len(s.positions)
        faces = np.asarray(faces)
        triangles = points[faces]
        normals = np.cross(triangles[:, 1]-triangles[:, 0], triangles[:, 2]-triangles[:, 0])
        valid = np.linalg.norm(normals, axis=1) > 1e-7
        age_knots = [0, 1] if self.source.adult_only else [0, 0.5, 1]
        deltas = [self.delta(p) for p in product([0, 0.5, 1], [0, 0.5, 1], [0, 1], age_knots)]
        for value in (5.0, 20.0, 100.0, 500.0, 2000.0):
            self.set_regularization(value)
            correspondence = self.correspondence(points)
            good = True
            for delta in deltas:
                q = (points + self.sample(delta, correspondence))[faces]
                normal = np.cross(q[:, 1]-q[:, 0], q[:, 2]-q[:, 0])
                if np.any((np.sum(normals*normal, axis=1) <= 0) & valid):
                    good = False; break
            if good:
                return
        raise ValueError("This mesh needs a fitted authoring cage before v1 shape data can be exported")

    @staticmethod
    def kernel(distance):
        return distance ** 2 * np.log(np.maximum(distance, 1e-20))

    def place(self, vertices):
        result = np.zeros_like(vertices)
        for bone, (linear, offset) in enumerate(zip(self.linears, self.offsets)):
            result += (vertices @ linear.T + offset) * self.source.weights[:, bone, None]
        return result

    def correspondence(self, positions):
        normalized = positions / self.scale
        evaluation = np.c_[self.kernel(cdist(normalized, self.control_points)),
                           np.ones(len(positions)), normalized]
        weights = lu_solve(self.factor, evaluation.T).T[:, :len(self.controls)].copy()
        distance, _ = self.tree.query(positions)
        return self.controls, weights, distance

    def sample(self, delta, correspondence):
        ids, weights, _ = correspondence
        return weights @ delta[ids]

    def delta(self, values, cupsize=0.5, buttocks=0):
        return self.place(self.source.evaluate(values, cupsize, buttocks)) - self.cage

    def pivot_delta(self, vertices):
        changes = self.source.joints(vertices) - self.source.joints(self.neutral)
        changes = np.einsum("bij,bj->bi", self.linears, changes)
        mapped = {name: changes[i] for i, name in enumerate(self.mapped)}
        result = []
        for name in self.target_names:
            if name in ("model_root", "Motion"):
                value = np.zeros(3)
            elif name in mapped:
                value = mapped[name]
            elif name in ("lhumerusX", "rhumerusX", "lradiusX", "rradiusX"):
                value = mapped[name[:-1]]
            elif name in ("lfemurX", "rfemurX", "ltail", "rtail"):
                value = mapped[name[0] + "femurYZ"]
            elif "hang_tag" in name or name in ("mc5", "mc7"):
                value = mapped[("l" if name == "mc5" else "r") + "hand"] if name.startswith("mc") else mapped[name[0] + "hand"]
            else:
                value = mapped["cranium"]
            result.append(value)
        return np.asarray(result)


def fingerprint(raw):
    result = 0xcbf29ce484222325
    for value in raw:
        result = ((result ^ value) * 0x100000001b3) & 0xffffffffffffffff
    return result


def binding_fingerprint(lods, bones):
    raw = bytearray(struct.pack("<II", bones, len(lods)))
    for lod in lods:
        raw.extend(struct.pack("<I", len(lod)))
        for s in lod:
            raw.extend(struct.pack("<III", s.index, len(s.positions), len(s.triangles)))
            raw.extend(np.c_[s.positions, s.normals].astype("<f4").tobytes())
            raw.extend(s.triangles.astype("<u4").tobytes())
    return fingerprint(raw)


def export_bundle(raw, lods, transfer, destination, source_fingerprint=None):
    knots = ([0, 0.5, 1], [0, 0.5, 1], [0, 0.5, 1],
             [0, 0.25, 0.5, 0.75, 1])
    grid = list(product(*knots))
    payload = bytearray(b"JKBODY2\0")
    source_fingerprint = fingerprint(raw) if source_fingerprint is None else source_fingerprint
    payload.extend(struct.pack("<QQIII", source_fingerprint, binding_fingerprint(lods, len(transfer.target_names)), len(transfer.target_names), len(lods), len(grid)))
    payload.extend(struct.pack("<I", int(transfer.source.adult_only)))
    for values in knots:
        payload.extend(struct.pack("<I", len(values)))
        payload.extend(np.asarray(values, dtype="<f4").tobytes())
    for lod in lods:
        payload.extend(struct.pack("<I", len(lod)))
        for s in lod:
            payload.extend(struct.pack("<II", s.index, len(s.positions)))
    for controls in grid:
        payload.extend(transfer.bake(controls).astype("<f4").tobytes())
    destination.write_bytes(payload)
    return dict(samples=len(grid), bytes=len(payload), sha256=hashlib.sha256(payload).hexdigest(),
                model_fingerprint=f"{source_fingerprint:016x}", knots=knots)


def draw(ax, positions, faces, limits, view="front", color=np.array([0.37, 0.65, 0.82])):
    tri = positions[faces]
    if view == "front":
        xy, depth = tri[:, :, [0, 2]], tri[:, :, 1].mean(axis=1)
    elif view == "back":
        xy = np.stack([-tri[:, :, 0], tri[:, :, 2]], axis=-1)
        depth = -tri[:, :, 1].mean(axis=1)
    else:
        xy = np.stack([-tri[:, :, 1], tri[:, :, 2]], axis=-1)
        depth = -tri[:, :, 0].mean(axis=1)
    n = np.cross(tri[:, 1] - tri[:, 0], tri[:, 2] - tri[:, 0])
    n /= np.maximum(np.linalg.norm(n, axis=1, keepdims=True), 1e-12)
    shade = 0.4 + 0.6 * abs(n @ np.array([0.36, -0.8, 0.48]))
    order = np.argsort(-depth)
    ax.add_collection(PolyCollection(xy[order], facecolors=(shade[:, None] * color)[order], edgecolors="none"))
    ax.set_xlim(*limits[0]); ax.set_ylim(*limits[1]); ax.set_aspect("equal")
    ax.set_facecolor("#111b28"); ax.axis("off")


def visible_mesh(lod, skin):
    off = set()
    if skin:
        for line in skin.decode("latin1").splitlines():
            if "," in line:
                name, shader = line.split(",", 1)
                if shader.strip().lower() == "*off":
                    off.add(name.strip().lower())
    vertices, triangles, start = [], [], 0
    for surface in lod:
        if surface.name.startswith("*") or surface.flags & 3 or surface.name.lower() in off:
            continue
        vertices.append(surface.positions)
        triangles.append(surface.triangles + start)
        start += len(surface.positions)
    return np.concatenate(vertices), np.concatenate(triangles)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--anny", type=Path, default=ROOT.parent / "anny-main")
    parser.add_argument("--base", type=Path, default=ROOT / "target/release/base")
    parser.add_argument("--model", default="jedi_hf")
    parser.add_argument("--bind", type=Path, help="Exact Source neutral capture from export_body_shape_bind")
    parser.add_argument("--gender", type=float, choices=[0, 1], default=1)
    parser.add_argument("--adult-only", action="store_true", help="Diagnostic adult-only range; v1 defaults to child through elder")
    parser.add_argument("--export", action="store_true", help="Also export model.bodyshape and an installable PK3")
    parser.add_argument("--out", type=Path, default=Path(__file__).parent / "output")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    assets = Assets(args.base)
    source_fp = None
    if args.bind:
        raw = b""
        source_fp, lods, names, heads = read_bind(args.bind)
    else:
        raw = assets.read(f"models/players/{args.model}/model.glm")
        lods, animation = read_glm(raw)
        names, heads = read_gla(assets.read(animation if animation.endswith(".gla") else animation + ".gla"))
    skin = assets.read(f"models/players/{args.model}/model_default.skin", optional=True)
    assets.close()
    source = AnnyData(args.anny, args.gender, args.adult_only)
    from anatomical_transfer import AnatomicalTransfer
    transfer = AnatomicalTransfer(source, names, heads, lods)
    vertices, faces = visible_mesh(lods[0], skin)
    from copy import deepcopy
    def visible_deformed(controls):
        surfaces = deepcopy(lods[0]); positions = transfer.deformed_lod(controls); offset = 0
        for surface in surfaces:
            count = len(surface.positions); surface.positions = positions[offset:offset+count]; offset += count
        return visible_mesh(surfaces, skin)[0]
    limits = [(-38, 38), (vertices[:, 2].min() - 3, vertices[:, 2].max() + 4)]
    cases = [(axis, value, np.where(np.arange(4) == i, value, NEUTRAL))
             for i, axis in enumerate(AXES) for value in (0.0, 0.5, 1.0)]
    report = dict(model=args.model, source="Anny MakeHuman macrodetail recipe (NumPy reader)",
                  age_range="adult" if args.adult_only else "child-to-elder", source_bones=len(source.bones),
                  transfer="anatomical-segments-v2", destination_bones=len(names),
                  neutral=list(NEUTRAL), cases=[])
    if args.export:
        destination = args.out / f"{args.model}.bodyshape"
        report["bundle"] = export_bundle(raw, lods, transfer, destination, source_fp)
        with zipfile.ZipFile(args.out / f"anny_profile_{args.model}.pk3", "w", zipfile.ZIP_DEFLATED) as archive:
            archive.write(destination, f"models/players/{args.model}/model.bodyshape")
    fig, axes = plt.subplots(4, 6, figsize=(15, 13), facecolor="#111b28")
    for row, axis in enumerate(AXES):
        for col, value in enumerate((0.0, 0.5, 1.0)):
            controls = NEUTRAL.copy(); controls[row] = value
            moved = visible_deformed(controls)
            for vi, view in enumerate(("front", "side")):
                ax = axes[row, col * 2 + vi]
                draw(ax, moved, faces, limits, view)
                ax.set_title(f"{axis} {value:.1f} / {view}", color="white", fontsize=9)
            report["cases"].append(dict(axis=axis, value=value, max_displacement=float(np.linalg.norm(moved-vertices, axis=1).max()), finite=bool(np.isfinite(moved).all())))
    fig.suptitle(f"{args.model} / Anny v1 shape transfer / fixed camera\nNeutral preserves authored mesh; existing region detection unchanged", color="white", fontsize=12)
    fig.tight_layout(rect=(0, 0, 1, 0.95))
    fig.savefig(args.out / f"{args.model}_v1.png", dpi=110)
    plt.close(fig)
    (args.out / f"{args.model}_report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()

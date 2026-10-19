"""Small, offline NumPy reader for Anny's MakeHuman shape assets.

Implements the macrodetail recipe in anny/models/full_model.py and phenotype.py.
No PyTorch, network downloads, model fitting, or game installation is performed.
The source checkout supplies CC0 mesh/targets; see README.md for attribution.
This reader deliberately exposes only the four v1 controls and two A/B controls.
"""
from __future__ import annotations

from functools import lru_cache
from itertools import product
from pathlib import Path
import gzip
import json

import numpy as np


AXES = ("muscle", "weight", "proportions", "age")
NEUTRAL = np.array([0.5, 0.5, 0.5, 0.5])
VARIATIONS = {
    "gender": ["male", "female"],
    "age": ["newborn", "baby", "child", "young", "old"],
    "muscle": ["minmuscle", "averagemuscle", "maxmuscle"],
    "weight": ["minweight", "averageweight", "maxweight"],
    "height": ["minheight", "maxheight"],
    "proportions": ["idealproportions", "uncommonproportions"],
    "cupsize": ["mincup", "averagecup", "maxcup"],
    "firmness": ["minfirmness", "averagefirmness", "maxfirmness"],
    "race": ["african", "asian", "caucasian"],
}


def anchors(axis):
    return (np.linspace(-1 / 3, 1, 5) if axis == "age"
            else np.linspace(0, 1, len(VARIATIONS[axis])))


def interpolate(value, knots):
    value = float(np.clip(value, knots[0], knots[-1]))
    hi = int(np.clip(np.searchsorted(knots, value), 1, len(knots) - 1))
    t = (value - knots[hi - 1]) / (knots[hi] - knots[hi - 1])
    result = np.zeros(len(knots))
    result[hi - 1], result[hi] = 1 - t, t
    return result


def bone_mapping():
    result = dict(Root="model_root", pelvis="pelvis", spine_01="lower_lumbar",
                  spine_02="upper_lumbar", spine_03="thoracic",
                  neck_01="cervical", head="cranium")
    for side in "lr":
        for source, target in dict(clavicle="clavical", upperarm="humerus",
                                   lowerarm="radius", hand="hand", thigh="femurYZ",
                                   calf="tibia", foot="talus", ball="tarsal").items():
            result[f"{source}_{side}"] = side + target
        for digit, number in zip(("thumb", "index", "middle", "ring", "pinky"), range(1, 6)):
            for joint in range(1, 4):
                result[f"{digit}_{joint:02}_{side}"] = f"{side}_d{number}_j{joint}"
    return result


class AnnyData:
    def __init__(self, checkout, gender=1.0, adult_only=False):
        self.checkout = Path(checkout).resolve()
        self.root = self.checkout / "src/anny/data/mpfb2"
        self.gender, self.adult_only = gender, adult_only
        vertices, faces, groups = [], [], {}
        current = "noname"
        for line in (self.root / "3dobjs/base.obj").read_text().splitlines():
            words = line.split()
            if not words:
                continue
            if words[0] == "v":
                vertices.append([float(x) for x in words[1:4]])
            elif words[0] == "g":
                current = words[1]
            elif words[0] == "f":
                ids = [int(x.split("/")[0]) - 1 for x in words[1:]]
                groups.setdefault(current, set()).update(ids)
                if current == "body":
                    faces.extend([ids[0], ids[j], ids[j + 1]] for j in range(1, len(ids) - 1))
        self.template = self.coordinates(np.array(vertices))
        self.faces = np.asarray(faces, dtype=np.int32)
        self.groups = {k: np.array(sorted(v)) for k, v in groups.items()}
        self.rig = json.loads((self.root / "rigs/standard/rig.game_engine.json").read_text())
        self.bones = list(self.rig)
        self.head_indices = [self.regressor(self.rig[b]["head"]) for b in self.bones]
        self.tail_indices = [self.regressor(self.rig[b]["tail"]) for b in self.bones]
        raw = json.loads((self.root / "rigs/standard/weights.game_engine.json").read_text())["weights"]
        self.weights = np.zeros((len(vertices), len(self.bones)))
        for b, name in enumerate(self.bones):
            for v, w in raw[name]:
                self.weights[v, b] += w
        total = self.weights.sum(axis=1)
        self.weights /= np.maximum(total[:, None], 1e-20)
        self.body_indices = np.unique(self.faces)

    @staticmethod
    def coordinates(values):
        # Same +90 X rotation and decimeters -> meters as Anny full_model.py.
        return values[..., [0, 2, 1]] * np.array([0.1, -0.1, 0.1])

    def regressor(self, data):
        strategy = data["strategy"]
        if strategy == "CUBE":
            return self.groups[data["cube_name"]]
        if strategy == "MEAN":
            return np.asarray(data["vertex_indices"])
        if strategy == "VERTEX":
            return np.asarray([data["vertex_index"]])
        raise ValueError(f"Unknown joint strategy {strategy!r}")

    @lru_cache(maxsize=1024)
    def target(self, relative):
        with gzip.open(self.root / "targets" / (relative + ".target.gz"), "rt") as stream:
            text = stream.read().strip()
        result = np.zeros_like(self.template)
        if not text:
            return result
        data = np.fromstring(text, sep=" ").reshape(-1, 4)
        result[data[:, 0].astype(int)] = self.coordinates(data[:, 1:4])
        return result

    def evaluate(self, values=NEUTRAL, cupsize=0.5, buttocks=0.0):
        values = np.asarray(values, dtype=float)
        if values.shape != (4,) or not np.isfinite(values).all():
            raise ValueError("Expected four finite v1 control values")
        values = np.clip(values, 0, 1)
        # v1 UI age: child -> authored young adult -> elder. Infant/newborn
        # targets remain outside this UI range.
        age = 2 / 3 + values[3] / 3 if self.adult_only else 1 / 3 + values[3] * 2 / 3
        params = dict(zip(AXES, values))
        params.update(gender=self.gender, age=age, height=0.5, cupsize=cupsize, firmness=0.5)
        active = {}
        for axis in VARIATIONS:
            ws = (np.full(3, 1 / 3) if axis == "race"
                  else interpolate(params[axis], anchors(axis)))
            active[axis] = [(name, w) for name, w in zip(VARIATIONS[axis], ws) if w > 1e-12]
        result = self.template.copy()
        for axes, prefix in [
            (("gender", "age", "muscle", "weight"), "macrodetails/universal-"),
            (("race", "gender", "age"), "macrodetails/"),
            (("gender", "age", "muscle", "weight", "height"), "macrodetails/height/"),
            (("gender", "age", "muscle", "weight", "proportions"), "macrodetails/proportions/"),
            (("gender", "age", "muscle", "weight", "cupsize", "firmness"), "breast/"),
        ]:
            for terms in product(*(active[a] for a in axes)):
                names, ws = zip(*terms)
                if axes[-1] == "firmness" and names[0] != "female":
                    continue
                if axes[-1] == "proportions" and names[1] in ("baby", "newborn"):
                    continue
                relative = prefix + "-".join(names)
                newborn = "newborn" in names
                if newborn:
                    relative = relative.replace("newborn", "baby")
                if axes[-1] == "firmness" and not (self.root / "targets" / (relative + ".target.gz")).exists():
                    continue
                delta = self.target(relative)
                if newborn:
                    scale = np.array([0.922, 0.922, 0.75])
                    delta = delta * scale + self.template * ((scale - 1) / 3)
                result += np.prod(ws) * delta
        if buttocks:
            name = "incr" if buttocks > 0 else "decr"
            result += abs(buttocks) * self.target("buttocks/buttocks-volume-" + name)
        return result

    def joints(self, vertices):
        return np.array([vertices[ids].mean(axis=0) for ids in self.head_indices])

    def tails(self, vertices):
        return np.array([vertices[ids].mean(axis=0) for ids in self.tail_indices])

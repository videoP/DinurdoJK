"""Anny-derived segment lengths and tissue radii on the authored Ghoul2 rig.

Soft tissue changes transverse radii, never bone lengths. Age/proportions
change segment lengths. Geometry and pivots use the same segment transforms;
there is no spatial cage regression or affine extrapolation across the body.
"""
from functools import lru_cache
import re
import numpy as np
from anny_data import NEUTRAL, bone_mapping


def frame(direction):
    z = direction / max(np.linalg.norm(direction), 1e-12)
    reference = np.array([0., -1., 0.])
    if abs(reference @ z) > .95:
        reference = np.array([1., 0., 0.])
    x = np.cross(reference, z); x /= np.linalg.norm(x)
    return np.column_stack([x, np.cross(z, x), z])


class AnatomicalTransfer:
    def __init__(self, source, names, heads, lods):
        self.source, self.target_names = source, names
        self.target_heads = np.asarray(heads)
        self.lods = lods
        self.neutral = source.evaluate()
        self.source_heads = source.joints(self.neutral)
        self.source_tails = source.tails(self.neutral)
        self.source_ids = {name: i for i, name in enumerate(source.bones)}
        self.ids = {name: i for i, name in enumerate(names)}
        self.mapping = bone_mapping()
        self.reverse = {target: bone for bone, target in self.mapping.items() if target in self.ids}
        self.groups = list(self.reverse)
        self.group_ids = {name: i for i, name in enumerate(self.groups)}
        self.parents = dict(pelvis="model_root", lower_lumbar="pelvis", upper_lumbar="lower_lumbar",
                            thoracic="upper_lumbar", cervical="thoracic", cranium="cervical")
        for side in "lr":
            self.parents.update({side+"clavical":"thoracic", side+"humerus":side+"clavical",
                                 side+"radius":side+"humerus", side+"hand":side+"radius",
                                 side+"femurYZ":"pelvis", side+"tibia":side+"femurYZ",
                                 side+"talus":side+"tibia", side+"tarsal":side+"talus"})
            for digit in range(1, 6):
                for joint in range(1, 4):
                    self.parents[f"{side}_d{digit}_j{joint}"] = side+"hand" if joint == 1 else f"{side}_d{digit}_j{joint-1}"
        self.endpoints = dict(model_root="pelvis", pelvis="lower_lumbar", lower_lumbar="upper_lumbar",
                              upper_lumbar="thoracic", thoracic="cervical", cervical="cranium")
        for side in "lr":
            self.endpoints.update({side+"clavical":side+"humerus", side+"humerus":side+"radius",
                                   side+"radius":side+"hand", side+"hand":side+"_d2_j1",
                                   side+"femurYZ":side+"tibia", side+"tibia":side+"talus",
                                   side+"talus":side+"tarsal"})
            for digit in range(1, 6):
                for joint in range(1, 3):
                    self.endpoints[f"{side}_d{digit}_j{joint}"] = f"{side}_d{digit}_j{joint+1}"
        self.target_frames, self.source_frames, self.profile_weights = [], [], []
        body = np.zeros(len(source.template), dtype=bool); body[source.body_indices] = True
        for group in self.groups:
            si = self.source_ids[self.reverse[group]]
            end = self.endpoints.get(group)
            if end in self.ids:
                direction = heads[self.ids[end]] - heads[self.ids[group]]
            elif group in ("cranium", "model_root"):
                direction = np.array([0., 0., 1.])
            elif group.endswith(("talus", "tarsal")):
                direction = np.array([0., -1., 0.])
            else:
                parent = self.parents.get(group)
                direction = heads[self.ids[group]] - heads[self.ids[parent]] if parent in self.ids else np.array([0., 0., 1.])
            self.target_frames.append(frame(direction))
            # The bone's authored tail, rather than the first JSON child.
            self.source_frames.append(frame(self.source_tails[si] - self.source_heads[si]))
            w = source.weights[:, si] * body
            self.profile_weights.append(w / max(w.sum(), 1e-12))
        self.target_frames = np.asarray(self.target_frames)
        self.source_frames = np.asarray(self.source_frames)
        self.profile_weights = np.asarray(self.profile_weights)
        self.neutral_radii = self.radii(self.neutral)
        self.owner = [self.owner_for(name) for name in names]
        self.layouts = []
        self.relaxations = []
        for lod in lods:
            points = np.concatenate([s.positions for s in lod])
            weights = np.concatenate([s.bone_weights for s in lod])
            # Share the authored skin-weight field at coincident material seams.
            _, inv = np.unique(points, axis=0, return_inverse=True)
            summed = np.zeros((inv.max()+1, len(names))); count = np.bincount(inv)
            np.add.at(summed, inv, weights)
            weights = summed[inv] / count[inv, None]
            grouped = np.zeros((len(points), len(self.groups)))
            for i, owner in enumerate(self.owner):
                grouped[:, self.group_ids[owner]] += weights[:, i]
            grouped /= np.maximum(grouped.sum(axis=1, keepdims=True), 1e-12)
            self.layouts.append((points, grouped))
            faces, offset = [], 0
            for surface in lod:
                faces.extend(inv[surface.triangles + offset]); offset += len(surface.positions)
            self.relaxations.append(MorphRelaxation(points, inv, np.asarray(faces)))
        points, grouped = self.layouts[0]
        visible = np.concatenate([np.full(len(s.positions), not s.name.startswith('*') and not (s.flags & 3)) for s in lods[0]])
        floor = points[visible, 2].min()
        self.ground = (points[:, 2] < floor + .2) & visible

    def owner_for(self, name):
        if name in self.reverse:
            return name
        if name in ("lhumerusX", "rhumerusX", "lradiusX", "rradiusX"):
            return name[:-1]
        if name in ("lfemurX", "rfemurX", "ltail", "rtail"):
            return name[0] + "femurYZ"
        if name == "Motion":
            return "model_root"
        if "hang_tag" in name or name in ("mc5", "mc7"):
            return ("l" if name == "mc5" else "r")+"hand" if name.startswith('mc') else name[0]+"hand"
        return "cranium"

    def radii(self, vertices):
        result = []
        for basis, weights in zip(self.source_frames, self.profile_weights):
            points = vertices @ basis
            mean = weights @ points
            result.append(np.sqrt(np.maximum(weights @ ((points-mean)**2), 1e-12)))
        return np.asarray(result)

    @lru_cache(maxsize=512)
    def recipe_radii(self, values):
        return self.radii(self.source.evaluate(values))

    @lru_cache(maxsize=256)
    def transforms(self, values):
        values = np.asarray(values)
        skeleton_values = values.copy(); skeleton_values[:2] = .5
        skeletal = self.source.evaluate(skeleton_values)
        joints = self.source.joints(skeletal)
        tails = self.source.tails(skeletal)
        base_radii = self.recipe_radii(tuple(skeleton_values))
        skeletal_scales = base_radii / self.neutral_radii
        # Product controls have distinct meanings. Native macrodetails mix fat
        # loss and muscle gain, sometimes making higher weight narrower on a
        # muscular child. Derive independent fat and lean-mass contrasts from
        # the actual source anchors, instead of inheriting that reversal.
        # The low-muscle cohort exposes the source's adipose range without
        # conflating increasing weight with the muscular cohort's lean mass.
        fat_values = values.copy(); fat_values[0] = 0
        fat_reference = skeleton_values.copy(); fat_reference[0] = 0
        fat_ratio = self.recipe_radii(tuple(fat_values)) / self.recipe_radii(tuple(fat_reference))
        muscle_reference = skeleton_values.copy()
        muscle_reference[1] = 0 if values[0] < .5 else 1
        muscle_extreme = muscle_reference.copy()
        muscle_extreme[0] = 0 if values[0] < .5 else 1
        contrast = self.recipe_radii(tuple(muscle_extreme)) / self.recipe_radii(tuple(muscle_reference))
        muscle_ratio = 1 + abs(values[0]-.5)*2*(contrast-1)
        scales = skeletal_scales * fat_ratio * muscle_ratio
        new = self.target_heads.copy()
        state = set()
        def joint(name):
            if name in state:
                return
            state.add(name)
            if name == "model_root":
                return
            parent = self.parents.get(name, "model_root")
            joint(parent)
            si = self.source_ids[self.reverse[name]]
            pi = self.source_ids[self.reverse[parent]]
            before = self.source_heads[si] - self.source_heads[pi]
            after = joints[si] - joints[pi]
            ratio = np.linalg.norm(after) / max(np.linalg.norm(before), 1e-12)
            if name == "pelvis":
                floor0 = self.neutral[self.source.body_indices, 2].min()
                floor1 = skeletal[self.source.body_indices, 2].min()
                ratio = (joints[si, 2]-floor1) / (self.source_heads[si, 2]-floor0)
            i, p = self.ids[name], self.ids[parent]
            new[i] = new[p] + (self.target_heads[i]-self.target_heads[p])*ratio
        for name in self.groups:
            joint(name)
        matrices, skeletal_matrices = [], []
        for gi, group in enumerate(self.groups):
            si = self.source_ids[self.reverse[group]]
            end = self.endpoints.get(group)
            if end in self.reverse:
                sj = self.source_ids[self.reverse[end]]
                length = np.linalg.norm(joints[sj]-joints[si]) / np.linalg.norm(self.source_heads[sj]-self.source_heads[si])
            else:
                length = np.linalg.norm(tails[si]-joints[si]) / max(np.linalg.norm(self.source_tails[si]-self.source_heads[si]), 1e-12)
            scale = scales[gi].copy(); scale[2] = length
            if group in ("model_root",):
                scale[:] = 1
            elif group == "cranium":
                scale = skeletal_scales[gi].copy()
            elif group.endswith(("hand", "talus", "tarsal")) or "_d" in group:
                # Rigid extremities preserve shoe/finger/head detail; only
                # skeletal proportions change their scale.
                scale[:] = length
            basis = self.target_frames[gi]
            matrices.append(basis @ np.diag(scale) @ basis.T)
            pivot_scale = skeletal_scales[gi].copy(); pivot_scale[2] = length
            if group == "model_root": pivot_scale[:] = 1
            elif group == "cranium": pivot_scale = skeletal_scales[gi].copy()
            elif group.endswith(("hand", "talus", "tarsal")) or "_d" in group: pivot_scale[:] = length
            skeletal_matrices.append(basis @ np.diag(pivot_scale) @ basis.T)
        matrices = np.asarray(matrices)
        # Locate every helper/face pivot through the exact mesh transform that
        # owns it, rather than assigning the proximal joint's displacement.
        for i, name in enumerate(self.target_names):
            if name not in self.reverse:
                group = self.owner[i]; gi = self.group_ids[group]; parent = self.ids[group]
                new[i] = new[parent] + skeletal_matrices[gi] @ (self.target_heads[i]-self.target_heads[parent])
        ground_points, ground_weights = (x[self.ground] for x in self.layouts[0])
        ground_delta = self.apply_points(ground_points, ground_weights, new, matrices)
        shift = -ground_delta[:, 2].mean()
        new[:, 2] += shift
        return new, matrices

    def apply_points(self, points, weights, new, matrices):
        result = np.zeros_like(points)
        for gi, group in enumerate(self.groups):
            i = self.ids[group]
            result += ((points-self.target_heads[i]) @ matrices[gi].T + new[i] - points) * weights[:, gi, None]
        return result

    def bake(self, values):
        values = tuple(float(x) for x in values)
        if np.array_equal(values, NEUTRAL):
            return np.zeros((len(self.target_names)+sum(len(p) for p, _ in self.layouts), 3))
        new, matrices = self.transforms(values)
        result = [new-self.target_heads]
        for (p,w), relaxation in zip(self.layouts,self.relaxations):
            delta = self.apply_points(p,w,new,matrices)
            result.append(relaxation.apply(p+delta)-p)
        return np.concatenate(result)

    def deformed_lod(self, values, lod_index=0):
        delta = self.bake(values)
        offset = len(self.target_names) + sum(len(p) for p, _ in self.layouts[:lod_index])
        return self.layouts[lod_index][0] + delta[offset:offset+len(self.layouts[lod_index][0])]


class MorphRelaxation:
    """Local clothing validity constraint, with exact material-seam welding.

    Preserve at least 10% of each rest triangle's signed projected area.
    Only violating faces and their vertices move: unlike global smoothing,
    this does not erase waist or muscle volume across the whole character.
    Applied offline, then checked again after runtime grid interpolation.
    """
    def __init__(self, points, inverse, faces):
        self.inverse = inverse
        self.count = np.bincount(inverse)
        self.rest = np.zeros((len(self.count),3))
        np.add.at(self.rest,inverse,points); self.rest /= self.count[:,None]
        # Coincident variants contribute the same geometric constraint once.
        self.faces = np.unique(faces,axis=0)
        tri = self.rest[self.faces]
        normal = np.cross(tri[:,1]-tri[:,0],tri[:,2]-tri[:,0])
        area = np.linalg.norm(normal,axis=1)
        valid = area > 1e-7
        self.faces = self.faces[valid]
        self.normal = normal[valid] / area[valid,None]
        self.minimum = area[valid] * .1
        self.max_correction = 0.0

    def apply(self, points):
        current = np.zeros_like(self.rest)
        np.add.at(current,self.inverse,points); current /= self.count[:,None]
        for _ in range(300):
            tri = current[self.faces]
            a,b = tri[:,1]-tri[:,0],tri[:,2]-tri[:,0]
            area = np.einsum('ij,ij->i',np.cross(a,b),self.normal)
            violating = area < self.minimum * (1-1e-5)
            if not violating.any():
                result = current[self.inverse]
                self.max_correction = max(self.max_correction,float(np.linalg.norm(result-points,axis=1).max()))
                return result
            ids = self.faces[violating]; n = self.normal[violating]
            gb = np.cross(b[violating],n); gc = np.cross(n,a[violating]); ga = -gb-gc
            gradients = np.stack([ga,gb,gc],axis=1)
            step = (self.minimum[violating]-area[violating]) / np.maximum(np.sum(gradients**2,axis=(1,2)),1e-20)
            corrections = gradients * step[:,None,None]
            total = np.zeros_like(current); counts = np.zeros(len(current))
            np.add.at(total,ids.ravel(),corrections.reshape(-1,3))
            np.add.at(counts,ids.ravel(),1)
            current += total/np.maximum(counts[:,None],1)
        raise ValueError("Clothing area constraints did not converge; this fit needs authoring review")

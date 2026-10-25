//! Retopology adapter: fit the donor rig to the authored bind skeleton, project
//! onto its surface, and interpolate authored displacements barycentrically.
//! The target's neutral vertex positions, UVs, topology and weights are retained.
use super::*;

#[derive(Clone, Copy)]
struct Frame {
    axes: [[f32; 3]; 3],
    scale: f32,
    source: [f32; 3],
    target: [f32; 3],
}
fn basis(direction: [f32; 3]) -> [[f32; 3]; 3] {
    let z = normalize(direction);
    let reference = if z[1].abs() < 0.95 {
        [0., -1., 0.]
    } else {
        [1., 0., 0.]
    };
    let x = normalize(cross(reference, z));
    [x, cross(z, x), z]
}
impl Frame {
    fn vector(self, p: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|k| self.scale * (0..3).map(|j| self.axes[k][j] * p[j]).sum::<f32>())
    }
    fn point(self, p: [f32; 3]) -> [f32; 3] {
        add(self.target, self.vector(sub(p, self.source)))
    }
}
fn frames(
    gla: &GlaAnimation,
    native: Option<&[[f32; 3]]>,
) -> Option<(Vec<Frame>, Vec<Option<usize>>)> {
    let data = model().ok()?;
    if native.is_some_and(|v| v.len() != gla.skeleton.len()) {
        return None;
    }
    let target: Vec<_> = gla
        .skeleton
        .iter()
        .enumerate()
        .map(|(i, b)| native.map_or_else(|| std::array::from_fn(|k| b.base_pose[k][3]), |p| p[i]))
        .collect();
    let ids: Vec<_> = data
        .meta
        .bones
        .iter()
        .map(|b| {
            gla.skeleton
                .iter()
                .position(|t| !b.target.is_empty() && t.name.eq_ignore_ascii_case(&b.target))
        })
        .collect();
    let source_id = |name: &str| data.meta.bones.iter().position(|b| b.name == name);
    for name in [
        "pelvis",
        "head",
        "upperarm_l",
        "upperarm_r",
        "thigh_l",
        "thigh_r",
        "foot_l",
        "foot_r",
    ] {
        ids[source_id(name)?]?;
    }
    let pelvis = source_id("pelvis")?;
    let head = source_id("head")?;
    let scale = length(sub(target[ids[head]?], target[ids[pelvis]?]))
        / length(sub(data.heads[head], data.heads[pelvis])).max(1e-6);
    if !scale.is_finite() || scale <= 0. {
        return None;
    }
    let mut out = Vec::new();
    for (i, b) in data.meta.bones.iter().enumerate() {
        // Unmapped finger joints use their nearest mapped ancestor, preserving
        // compatibility with both native 53- and 72-bone Ghoul2 rigs.
        let mut ancestor = i;
        while ids[ancestor].is_none() {
            ancestor = source_id(&data.meta.bones[ancestor].parent)?;
        }
        let id = ids[ancestor]?;
        let child = data
            .meta
            .bones
            .iter()
            .enumerate()
            .filter(|(j, c)| c.parent == b.name && ids[*j].is_some())
            .min_by(|(a, _), (b, _)| {
                length(sub(data.heads[*a], data.tails[i]))
                    .total_cmp(&length(sub(data.heads[*b], data.tails[i])))
            });
        let source_direction = sub(data.tails[i], data.heads[i]);
        let (source_direction, target_direction, bone_scale) = if let Some((child, _)) = child {
            let direction = sub(target[ids[child]?], target[id]);
            let link = sub(data.heads[child], data.heads[i]);
            let source_len = length(link);
            if length(direction) > 1e-5 && source_len > 1e-5 {
                (link, direction, length(direction) / source_len)
            } else {
                (source_direction, source_direction, scale)
            }
        } else if let Some(parent) = source_id(&b.parent).filter(|&p| ids[p].is_some()) {
            let direction = sub(target[id], target[ids[parent]?]);
            let old = sub(data.heads[i], data.heads[parent]);
            // A terminal bone continues its authored direction. The parent link
            // supplies its scale, while the source/target bases supply rotation.
            let s = basis(old);
            let t = basis(direction);
            let mapped = std::array::from_fn(|k| {
                (0..3).map(|j| t[j][k] * dot(s[j], source_direction)).sum()
            });
            (
                source_direction,
                mapped,
                if length(old) > 1e-6 {
                    length(direction) / length(old)
                } else {
                    scale
                },
            )
        } else {
            (source_direction, source_direction, scale)
        };
        let s = basis(source_direction);
        let t = basis(target_direction);
        let axes = std::array::from_fn(|k| {
            std::array::from_fn(|j| (0..3).map(|a| t[a][k] * s[a][j]).sum())
        });
        out.push(Frame {
            axes,
            scale: bone_scale,
            source: data.heads[i],
            target: target[id],
        });
    }
    for i in 0..out.len() {
        if ids[i].is_some() {
            continue;
        }
        let mut ancestor = i;
        while ids[ancestor].is_none() {
            ancestor = source_id(&data.meta.bones[ancestor].parent)?;
        }
        out[i] = out[ancestor];
    }
    Some((out, ids))
}

#[derive(Clone, Copy)]
struct Projection {
    face: usize,
    weights: [f32; 3],
}
struct Mapping {
    frames: Vec<Frame>,
    bone_ids: Vec<Option<usize>>,
    tree: SurfaceTree,
    projections: Vec<Vec<Vec<Projection>>>,
}
fn fitted(
    gla: &GlaAnimation,
    native: Option<&[[f32; 3]]>,
) -> Option<(Vec<Frame>, Vec<Option<usize>>, SurfaceTree)> {
    let data = model().ok()?;
    let (frames, ids) = frames(gla, native)?;
    let vertices = data
        .neutral
        .iter()
        .enumerate()
        .map(|(i, &p)| {
            let weights = &data.meta.weights[i];
            if weights.is_empty() {
                frames[0].point(p)
            } else {
                weights
                    .iter()
                    .fold([0.; 3], |v, &(b, w)| add(v, mul(frames[b].point(p), w)))
            }
        })
        .collect();
    Some((frames, ids, SurfaceTree::new(vertices, data.faces.clone())))
}
pub fn supports(glm: &GlmModel, gla: &GlaAnimation, native: Option<&[[f32; 3]]>) -> bool {
    glm.num_bones >= 20
        && glm.num_bones <= gla.skeleton.len()
        && !glm.lods.is_empty()
        && frames(gla, native).is_some()
}
fn mapping(
    glm: &GlmModel,
    gla: &GlaAnimation,
    native: Option<&[[f32; 3]]>,
) -> Result<Arc<Mapping>, String> {
    static CACHE: OnceLock<Mutex<HashMap<u64, Arc<Mapping>>>> = OnceLock::new();
    let mut key = super::super::binding_fingerprint(glm);
    // Bind pivots can differ even when a model shares geometry or a GLA name.
    for (i, b) in gla.skeleton.iter().enumerate() {
        let p = native.map_or_else(|| std::array::from_fn(|k| b.base_pose[k][3]), |p| p[i]);
        key ^= super::super::model_fingerprint(b.name.as_bytes());
        for x in p {
            key = (key ^ u64::from(x.to_bits())).wrapping_mul(0x100000001b3);
        }
    }
    let cache = CACHE.get_or_init(Default::default);
    if let Some(m) = cache
        .lock()
        .map_err(|_| "Anny mapping cache poisoned")?
        .get(&key)
    {
        return Ok(Arc::clone(m));
    }
    let (frames, bone_ids, tree) =
        fitted(gla, native).ok_or("Anny requires a mapped humanoid skeleton")?;
    // Exact-position welding means material seams always sample the same field.
    let mut weld = HashMap::new();
    let projections = glm
        .lods
        .iter()
        .map(|lod| {
            lod.surfaces
                .iter()
                .map(|surface| {
                    surface
                        .vertices
                        .iter()
                        .map(|v| {
                            *weld
                                .entry(v.position.map(|x| if x == 0. { 0 } else { x.to_bits() }))
                                .or_insert_with(|| tree.project(v.position))
                        })
                        .collect()
                })
                .collect()
        })
        .collect();
    let out = Arc::new(Mapping {
        frames,
        bone_ids,
        tree,
        projections,
    });
    let mut cache = cache.lock().map_err(|_| "Anny mapping cache poisoned")?;
    if cache.len() >= 8 {
        cache.clear();
    }
    cache.insert(key, Arc::clone(&out));
    Ok(out)
}

/// Apply authored detail shapes at neutral whole-body parameters.
/// Use `apply_profile` to combine details with the procedural whole-body sliders.
pub fn apply(
    glm: &mut GlmModel,
    gla: &GlaAnimation,
    params: BodyShapeParams,
    details: Details,
    native: Option<&[[f32; 3]]>,
) -> Result<Option<Vec<[f32; 3]>>, String> {
    if params.is_neutral() && details.is_neutral() {
        return Ok(None);
    }
    if !params.is_neutral() {
        return Err(
            "Anny runtime data fixes whole-body axes; use apply_profile for whole-body sliders"
                .into(),
        );
    }
    let data = model()?;
    if !supports(glm, gla, native) {
        return Err("Anny requires a mapped humanoid skeleton".into());
    }
    let map = mapping(glm, gla, native)?;
    let shaped = data.evaluate(params, details);
    let heads: Vec<_> = data
        .meta
        .bones
        .iter()
        .map(|b| mean(&shaped, &b.head))
        .collect();
    let original: Vec<_> = gla
        .skeleton
        .iter()
        .enumerate()
        .map(|(i, b)| native.map_or_else(|| std::array::from_fn(|k| b.base_pose[k][3]), |p| p[i]))
        .collect();
    let parents: Vec<_> = data
        .meta
        .bones
        .iter()
        .map(|b| data.meta.bones.iter().position(|p| p.name == b.parent))
        .collect();
    let owners: Vec<_> = (0..heads.len())
        .map(|mut i| {
            while map.bone_ids[i].is_none() {
                i = parents[i].expect("validated mapped ancestor");
            }
            i
        })
        .collect();
    // Port the previous AnatomicalTransfer's parent-relative joint placement.
    // Transform changes to a segment, never the joint's full model-space motion:
    // otherwise two adjacent fingers rotate/scale the same body translation
    // differently and a height change can turn a one-unit link into 27 units.
    let mut changes = vec![None; heads.len()];
    fn joint_change(
        data: &Model,
        i: usize,
        parents: &[Option<usize>],
        owners: &[usize],
        map: &Mapping,
        heads: &[[f32; 3]],
        original: &[[f32; 3]],
        changes: &mut [Option<[f32; 3]>],
    ) -> [f32; 3] {
        if let Some(change) = changes[i] {
            return change;
        }
        let owner = owners[i];
        let change = if owner != i {
            joint_change(data, owner, parents, owners, map, heads, original, changes)
        } else if let Some(parent) = parents[i].map(|p| owners[p]) {
            let inherited =
                joint_change(data, parent, parents, owners, map, heads, original, changes);
            let before = sub(data.heads[i], data.heads[parent]);
            let after = sub(heads[i], heads[parent]);
            let target = sub(
                original[map.bone_ids[i].unwrap()],
                original[map.bone_ids[parent].unwrap()],
            );
            let delta = if dot(before, before) > 1e-12 {
                super::super::map_link(sub(after, before), before, target)
            } else {
                map.frames[parent].vector(sub(after, before))
            };
            add(inherited, delta)
        } else {
            map.frames[i].vector(sub(heads[i], data.heads[i]))
        };
        changes[i] = Some(change);
        change
    }
    for i in 0..heads.len() {
        joint_change(
            data,
            i,
            &parents,
            &owners,
            &map,
            &heads,
            &original,
            &mut changes,
        );
    }
    let mut target_changes = vec![None; gla.skeleton.len()];
    for (i, &target) in map.bone_ids.iter().enumerate() {
        if let Some(target) = target {
            target_changes[target] = Some(i);
        }
    }
    let pivots = original
        .iter()
        .enumerate()
        .map(|(i, &old)| {
            let mut ancestor = Some(i);
            for _ in 0..gla.skeleton.len() {
                let Some(a) = ancestor else {
                    break;
                };
                if let Some(source) = target_changes[a] {
                    let offset = sub(old, original[a]);
                    let before =
                        map.frames[source].vector(sub(data.tails[source], data.heads[source]));
                    let after = map.frames[source].vector(sub(
                        mean(&shaped, &data.meta.bones[source].tail),
                        heads[source],
                    ));
                    let helper = super::super::map_link(offset, before, after);
                    return add(old, add(changes[source].unwrap(), sub(helper, offset)));
                }
                ancestor = crate::skeleton::anatomical_parent(&gla.skeleton, a);
            }
            old
        })
        .collect();
    let delta: Vec<_> = shaped
        .iter()
        .zip(&data.neutral)
        .enumerate()
        .map(|(i, (a, b))| {
            let d = sub(*a, *b);
            // Carry the fitted joint with the skeleton; transfer only the
            // authored deformation relative to that joint through its frame.
            // The mesh and its pivots must share the same anatomical motion.
            let transfer = |bone: usize| {
                let owner = owners[bone];
                add(
                    changes[owner].unwrap(),
                    map.frames[bone].vector(sub(d, sub(heads[owner], data.heads[owner]))),
                )
            };
            let weights = &data.meta.weights[i];
            if weights.is_empty() {
                transfer(0)
            } else {
                weights
                    .iter()
                    .fold([0.; 3], |v, &(b, w)| add(v, mul(transfer(b), w)))
            }
        })
        .collect();
    for (li, lod) in glm.lods.iter_mut().enumerate() {
        let before: Vec<_> = lod
            .surfaces
            .iter()
            .map(|s| s.vertices.iter().map(|v| v.position).collect::<Vec<_>>())
            .collect();
        for (si, surface) in lod.surfaces.iter_mut().enumerate() {
            for (vi, v) in surface.vertices.iter_mut().enumerate() {
                let projection = map.projections[li][si][vi];
                for (i, w) in data.faces[projection.face].iter().zip(projection.weights) {
                    v.position = add(v.position, mul(delta[*i], w));
                }
                if !v.position.iter().all(|x| x.is_finite()) {
                    return Err("nonfinite Anny vertex".into());
                }
            }
        }
        update_normals(lod, &before);
    }
    Ok(Some(pivots))
}

/// Classify by the authored local targets influencing the projected donor
/// surface. No screen-space body rectangles or guessed body-height thresholds.
fn region_labels(data: &Model) -> &'static [&'static str] {
    static LABELS: OnceLock<Vec<&'static str>> = OnceLock::new();
    LABELS.get_or_init(|| {
        let mut groups: Vec<&str> = LOCAL_CONTROLS.iter().map(|c| c.1).collect();
        groups.sort();
        groups.dedup();
        let mut scores = vec![vec![0.; groups.len()]; data.neutral.len()];
        for (i, c) in data.meta.controls.iter().enumerate() {
            let group = groups
                .iter()
                .position(|&g| g == LOCAL_CONTROLS[i].1)
                .unwrap();
            // Each authored target votes by normalized displacement magnitude.
            for sparse in &c.sides {
                let mut delta = vec![[0.; 3]; data.neutral.len()];
                data.accumulate(&mut delta, sparse, 1.);
                let total = delta.iter().copied().map(length).sum::<f32>().max(1e-6);
                for (v, d) in scores.iter_mut().zip(delta) {
                    v[group] += length(d) / total;
                }
            }
        }
        for (g, group) in groups.iter().enumerate() {
            let count = LOCAL_CONTROLS.iter().filter(|c| c.1 == *group).count() as f32;
            for row in &mut scores {
                row[g] /= count;
            }
        }
        scores
            .iter()
            .map(|s| {
                groups[s
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.total_cmp(b.1))
                    .unwrap()
                    .0]
            })
            .collect()
    })
}

fn projection_region(data: &Model, projection: Projection) -> &'static str {
    let index = (0..3)
        .max_by(|&a, &b| projection.weights[a].total_cmp(&projection.weights[b]))
        .unwrap();
    region_labels(data)[data.faces[projection.face][index]]
}

pub fn region(
    glm: &GlmModel,
    gla: &GlaAnimation,
    native: Option<&[[f32; 3]]>,
    point: [f32; 3],
) -> Option<&'static str> {
    let data = model().ok()?;
    let map = mapping(glm, gla, native).ok()?;
    Some(projection_region(data, map.tree.project(point)))
}

/// Batched counterpart used to cache selection masks for rendered triangles.
/// Uses exactly the same donor classification as body picking.
pub fn regions(
    glm: &GlmModel,
    gla: &GlaAnimation,
    native: Option<&[[f32; 3]]>,
    points: impl IntoIterator<Item = [f32; 3]>,
) -> Result<Vec<&'static str>, String> {
    let data = model()?;
    let map = mapping(glm, gla, native)?;
    Ok(points
        .into_iter()
        .map(|p| projection_region(data, map.tree.project(p)))
        .collect())
}

fn update_normals(lod: &mut crate::ghoul2::GlmLod, before: &[Vec<[f32; 3]>]) {
    let mut normals = HashMap::<[u32; 3], ([f32; 3], [f32; 3])>::new();
    for (s, old) in lod.surfaces.iter().zip(before) {
        for face in &s.triangles {
            let [a, b, c] = face.map(|x| x as usize);
            let na = cross(sub(old[b], old[a]), sub(old[c], old[a]));
            let nb = cross(
                sub(s.vertices[b].position, s.vertices[a].position),
                sub(s.vertices[c].position, s.vertices[a].position),
            );
            for id in [a, b, c] {
                let key = old[id].map(|x| if x == 0. { 0 } else { x.to_bits() });
                let entry = normals.entry(key).or_default();
                entry.0 = add(entry.0, na);
                entry.1 = add(entry.1, nb);
            }
        }
    }
    for (s, old) in lod.surfaces.iter_mut().zip(before) {
        for (v, p) in s.vertices.iter_mut().zip(old) {
            if let Some(&(a, b)) = normals.get(&p.map(|x| if x == 0. { 0 } else { x.to_bits() })) {
                let a = normalize(a);
                let b = normalize(b);
                let sign = if dot(a, v.normal) < 0. { -1. } else { 1. };
                v.normal = normalize(add(v.normal, mul(sub(b, a), sign)));
            }
        }
    }
}

/// Port of Anny warp_mesh_utils point_triangle_distance's planar/edge projection,
/// returning the barycentrics consumed by model_transforms.apply_retopology.
pub fn closest_triangle(p: [f32; 3], v: [[f32; 3]; 3]) -> ([f32; 3], f32) {
    let ab = sub(v[1], v[0]);
    let ac = sub(v[2], v[0]);
    let ax = sub(p, v[0]);
    let bb = dot(ab, ab);
    let cc = dot(ac, ac);
    let bc = dot(ab, ac);
    let bx = dot(ab, ax);
    let cx = dot(ac, ax);
    let det = bb * cc - bc * bc;
    if det > 1e-16 {
        let beta = (cc * bx - bc * cx) / det;
        let gamma = (bb * cx - bc * bx) / det;
        let alpha = 1. - beta - gamma;
        if alpha >= 0. && beta >= 0. && gamma >= 0. {
            let d = sub(p, add(v[0], add(mul(ab, beta), mul(ac, gamma))));
            return ([alpha, beta, gamma], dot(d, d));
        }
    }
    let mut best = ([1., 0., 0.], f32::INFINITY);
    for (a, b) in [(0, 1), (0, 2), (1, 2)] {
        let d = sub(v[b], v[a]);
        let t = (dot(sub(p, v[a]), d) / dot(d, d).max(1e-20)).clamp(0., 1.);
        let d = sub(p, add(v[a], mul(d, t)));
        let distance = dot(d, d);
        if distance < best.1 {
            let mut weights = [0.; 3];
            weights[a] = 1. - t;
            weights[b] = t;
            best = (weights, distance);
        }
    }
    best
}
struct Node {
    min: [f32; 3],
    max: [f32; 3],
    range: std::ops::Range<usize>,
    children: Option<[usize; 2]>,
}
pub struct SurfaceTree {
    vertices: Vec<[f32; 3]>,
    faces: Vec<[usize; 3]>,
    order: Vec<usize>,
    nodes: Vec<Node>,
}
impl SurfaceTree {
    pub fn new(vertices: Vec<[f32; 3]>, faces: Vec<[usize; 3]>) -> Self {
        let order = (0..faces.len()).collect();
        let count = faces.len();
        let mut out = Self {
            vertices,
            faces,
            order,
            nodes: vec![],
        };
        out.build(0..count);
        out
    }
    fn build(&mut self, range: std::ops::Range<usize>) -> usize {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for &f in &self.order[range.clone()] {
            for &v in &self.faces[f] {
                for k in 0..3 {
                    min[k] = min[k].min(self.vertices[v][k]);
                    max[k] = max[k].max(self.vertices[v][k]);
                }
            }
        }
        let id = self.nodes.len();
        self.nodes.push(Node {
            min,
            max,
            range: range.clone(),
            children: None,
        });
        if range.len() > 8 {
            let axis = (0..3)
                .max_by(|&a, &b| (max[a] - min[a]).total_cmp(&(max[b] - min[b])))
                .unwrap();
            let center = |f: usize| {
                self.faces[f]
                    .iter()
                    .map(|&v| self.vertices[v][axis])
                    .sum::<f32>()
            };
            self.order[range.clone()].sort_unstable_by(|&a, &b| center(a).total_cmp(&center(b)));
            let mid = range.start + range.len() / 2;
            let a = self.build(range.start..mid);
            let b = self.build(mid..range.end);
            self.nodes[id].children = Some([a, b]);
        }
        id
    }
    fn project(&self, p: [f32; 3]) -> Projection {
        let mut best = (
            Projection {
                face: 0,
                weights: [1., 0., 0.],
            },
            f32::INFINITY,
        );
        self.query(0, p, &mut best);
        best.0
    }
    fn bound_distance(&self, id: usize, p: [f32; 3]) -> f32 {
        let n = &self.nodes[id];
        (0..3)
            .map(|k| (p[k] - p[k].clamp(n.min[k], n.max[k])).powi(2))
            .sum()
    }
    fn query(&self, id: usize, p: [f32; 3], best: &mut (Projection, f32)) {
        if self.bound_distance(id, p) > best.1 {
            return;
        }
        let n = &self.nodes[id];
        if let Some([a, b]) = n.children {
            let (a, b) = if self.bound_distance(a, p) <= self.bound_distance(b, p) {
                (a, b)
            } else {
                (b, a)
            };
            self.query(a, p, best);
            self.query(b, p, best);
        } else {
            for &face in &self.order[n.range.clone()] {
                let (weights, distance) =
                    closest_triangle(p, self.faces[face].map(|v| self.vertices[v]));
                if distance < best.1 {
                    *best = (Projection { face, weights }, distance);
                }
            }
        }
    }
}

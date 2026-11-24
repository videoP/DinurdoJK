//! DinurdoJK adapter, NOT a translation of Minecraft voxel traversal.
//! Static surfaces: borrowed shared acoustic mesh + median-split CPU BVH.
//! Solid volumes: existing BSP collision tree + convex brush plane clipping.
//! Coordinates/distances are JKA units. The algorithm boundary converts to
//! upstream metres and y-up explicitly; never change the gameplay CM.
use jka_assets::bsp::{
    AcousticCollision, AcousticMesh, ACOUSTIC_SKY, ACOUSTIC_THIN, CONTENTS_SOLID, CONTENTS_TERRAIN,
    MATERIAL_MASK, SURF_SKY,
};
use std::sync::Arc;

pub const JKA_UNITS_PER_METER: f64 = 32.0; // existing steam_audio.rs convention
pub type Vec3 = [f64; 3];
fn sub(a: Vec3, b: Vec3) -> Vec3 {
    std::array::from_fn(|i| a[i] - b[i])
}
fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn point(a: Vec3, d: Vec3, t: f64) -> Vec3 {
    std::array::from_fn(|i| a[i] + d[i] * t)
}
pub fn to_upstream(v: Vec3) -> Vec3 {
    [
        v[0] / JKA_UNITS_PER_METER,
        v[2] / JKA_UNITS_PER_METER,
        -v[1] / JKA_UNITS_PER_METER,
    ]
}
pub fn from_upstream(v: Vec3) -> Vec3 {
    [
        v[0] * JKA_UNITS_PER_METER,
        -v[2] * JKA_UNITS_PER_METER,
        v[1] * JKA_UNITS_PER_METER,
    ]
}

#[derive(Clone, Copy)]
struct Bounds {
    lo: [f32; 3],
    hi: [f32; 3],
}
impl Bounds {
    fn empty() -> Self {
        Self {
            lo: [f32::INFINITY; 3],
            hi: [f32::NEG_INFINITY; 3],
        }
    }
    fn include(&mut self, p: [f32; 3]) {
        for i in 0..3 {
            self.lo[i] = self.lo[i].min(p[i]);
            self.hi[i] = self.hi[i].max(p[i]);
        }
    }
    fn entry(&self, from: Vec3, dir: Vec3, mut lo: f64, mut hi: f64) -> Option<f64> {
        for i in 0..3 {
            let a = f64::from(self.lo[i]);
            let b = f64::from(self.hi[i]);
            if dir[i] == 0.0 {
                if from[i] < a || from[i] > b {
                    return None;
                }
                continue;
            }
            let p = (a - from[i]) / dir[i];
            let q = (b - from[i]) / dir[i];
            lo = lo.max(p.min(q));
            hi = hi.min(p.max(q));
            if lo > hi {
                return None;
            }
        }
        Some(lo)
    }
}
#[derive(Clone, Copy)]
enum Child {
    Leaf { start: usize, end: usize },
    Branch { left: usize, right: usize },
}
struct Node {
    bounds: Bounds,
    child: Child,
    has_thin: bool,
}

/// Shares the mesh allocation with map loading/other consumers. The BVH stores
/// an index permutation and bounds only; no triangle vertex duplication.
pub struct SurfaceBvh {
    mesh: Arc<AcousticMesh>,
    nodes: Vec<Node>,
    order: Vec<u32>,
}
#[derive(Clone, Copy, Debug)]
pub struct SurfaceHit {
    pub fraction: f64,
    pub position: Vec3,
    pub normal: Vec3,
    pub material: u8,
    pub triangle: u32,
    pub flags: u8,
}
impl SurfaceBvh {
    pub fn build(mesh: Arc<AcousticMesh>) -> Result<Self, &'static str> {
        Self::build_cancellable(mesh, &|| false)
    }
    pub fn build_cancellable(
        mesh: Arc<AcousticMesh>,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Self, &'static str> {
        if mesh.triangles.len() != mesh.materials.len()
            || mesh.triangles.len() != mesh.acoustic_flags.len()
            || mesh.triangles.len() > u32::MAX as usize
        {
            return Err("invalid acoustic mesh counts");
        }
        if mesh.vertices.iter().flatten().any(|x| !x.is_finite())
            || mesh
                .triangles
                .iter()
                .flatten()
                .any(|&i| i as usize >= mesh.vertices.len())
        {
            return Err("invalid acoustic mesh vertex");
        }
        let mut result = Self {
            order: (0..mesh.triangles.len() as u32).collect(),
            mesh,
            nodes: Vec::new(),
        };
        if !result.order.is_empty() {
            result.build_node(0, result.order.len(), cancelled)?;
        }
        Ok(result)
    }
    fn build_node(
        &mut self,
        start: usize,
        end: usize,
        cancelled: &impl Fn() -> bool,
    ) -> Result<usize, &'static str> {
        if cancelled() {
            return Err("EchoField BVH cancelled");
        }
        let mut bounds = Bounds::empty();
        for &i in &self.order[start..end] {
            for index in self.mesh.triangles[i as usize] {
                bounds.include(self.mesh.vertices[index as usize]);
            }
        }
        let node = self.nodes.len();
        let has_thin = self.order[start..end].iter().any(|&i| {
            self.mesh.acoustic_flags[i as usize] & (ACOUSTIC_THIN | ACOUSTIC_SKY) == ACOUSTIC_THIN
        });
        self.nodes.push(Node {
            bounds,
            child: Child::Leaf { start, end },
            has_thin,
        });
        if end - start > 8 {
            let axis = (0..3)
                .max_by(|&a, &b| {
                    (bounds.hi[a] - bounds.lo[a]).total_cmp(&(bounds.hi[b] - bounds.lo[b]))
                })
                .unwrap();
            let mesh = &self.mesh;
            let centroid = |i: u32| {
                mesh.triangles[i as usize]
                    .iter()
                    .map(|&v| f64::from(mesh.vertices[v as usize][axis]))
                    .sum::<f64>()
            };
            let mid = start + (end - start) / 2;
            self.order[start..end].select_nth_unstable_by(mid - start, |a, b| {
                centroid(*a).total_cmp(&centroid(*b)).then_with(|| a.cmp(b))
            });
            let left = self.build_node(start, mid, cancelled)?;
            let right = self.build_node(mid, end, cancelled)?;
            self.nodes[node].child = Child::Branch { left, right };
        }
        Ok(node)
    }
    /// Two-sided closest intersection on a finite segment. `min_fraction`
    /// allows the propagation caller to avoid self-hits without global bias.
    pub fn nearest(&self, from: Vec3, to: Vec3, min_fraction: f64) -> Option<SurfaceHit> {
        self.nearest_filtered(from, to, min_fraction, false)
    }
    /// Only explicit patch/mesh surfaces, excluding sky. Brush volumes are handled separately.
    pub fn nearest_thin(&self, from: Vec3, to: Vec3, min_fraction: f64) -> Option<SurfaceHit> {
        self.nearest_filtered(from, to, min_fraction, true)
    }
    fn nearest_filtered(
        &self,
        from: Vec3,
        to: Vec3,
        min_fraction: f64,
        thin: bool,
    ) -> Option<SurfaceHit> {
        if self.nodes.is_empty()
            || !from.iter().chain(to.iter()).all(|v| v.is_finite())
            || !(0.0..=1.0).contains(&min_fraction)
        {
            return None;
        }
        let direction = sub(to, from);
        let mut hit = None;
        self.visit(0, from, direction, min_fraction, thin, &mut hit);
        hit
    }
    fn visit(
        &self,
        index: usize,
        from: Vec3,
        dir: Vec3,
        min: f64,
        thin: bool,
        best: &mut Option<SurfaceHit>,
    ) {
        let max = best.as_ref().map_or(1.0, |h| h.fraction);
        let node = &self.nodes[index];
        if (thin && !node.has_thin) || node.bounds.entry(from, dir, min, max).is_none() {
            return;
        }
        match node.child {
            Child::Branch { left, right } => {
                let a = self.nodes[left].bounds.entry(from, dir, min, max);
                let b = self.nodes[right].bounds.entry(from, dir, min, max);
                match (a, b) {
                    (Some(a), Some(b)) => {
                        let (first, second) = if a <= b { (left, right) } else { (right, left) };
                        self.visit(first, from, dir, min, thin, best);
                        self.visit(second, from, dir, min, thin, best);
                    }
                    (Some(_), None) => self.visit(left, from, dir, min, thin, best),
                    (None, Some(_)) => self.visit(right, from, dir, min, thin, best),
                    _ => {}
                }
            }
            Child::Leaf { start, end } => {
                for &i in &self.order[start..end] {
                    let flags = self.mesh.acoustic_flags[i as usize];
                    if thin && (flags & ACOUSTIC_THIN == 0 || flags & ACOUSTIC_SKY != 0) {
                        continue;
                    }
                    let [a, b, c] = self.mesh.triangles[i as usize]
                        .map(|j| self.mesh.vertices[j as usize].map(f64::from));
                    let e1 = sub(b, a);
                    let e2 = sub(c, a);
                    let p = cross(dir, e2);
                    let det = dot(e1, p);
                    if det == 0.0 {
                        continue;
                    }
                    let s = sub(from, a);
                    let u = dot(s, p) / det;
                    if !(0.0..=1.0).contains(&u) {
                        continue;
                    }
                    let q = cross(s, e1);
                    let v = dot(dir, q) / det;
                    if v < 0.0 || u + v > 1.0 {
                        continue;
                    }
                    let t = dot(e2, q) / det;
                    if !(min..=1.0).contains(&t)
                        || best
                            .as_ref()
                            .is_some_and(|h| t > h.fraction || (t == h.fraction && i >= h.triangle))
                    {
                        continue;
                    }
                    let n = cross(e1, e2);
                    let len = dot(n, n).sqrt();
                    if len == 0.0 {
                        continue;
                    }
                    *best = Some(SurfaceHit {
                        fraction: t,
                        position: point(from, dir, t),
                        normal: n.map(|x| x / len),
                        material: self.mesh.materials[i as usize],
                        triangle: i,
                        flags,
                    });
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SolidInterval {
    pub brush: usize,
    pub enter: f64,
    pub exit: f64,
    pub material: u8,
}
struct BrushHit {
    interval: SolidInterval,
    entry_normal: Option<Vec3>,
}

fn clip_brush(
    bsp: &AcousticCollision,
    index: usize,
    from: Vec3,
    delta: Vec3,
    contents: u32,
) -> Option<BrushHit> {
    let brush = &bsp.brushes[index];
    let shader = &bsp.shaders[brush.shader];
    if shader.contents & contents == 0 || brush.sides.is_empty() {
        return None;
    }
    let (mut enter, mut exit) = (0.0, 1.0);
    let (mut entry_side, mut exit_side) = (None, None);
    for side in &bsp.brush_sides[brush.sides.clone()] {
        let plane = bsp.planes[side.plane];
        let normal = plane.normal.map(f64::from);
        let a = dot(normal, from) - f64::from(plane.distance);
        let d = dot(normal, delta);
        if d == 0.0 {
            if a > 0.0 {
                return None;
            }
            continue;
        }
        let t = -a / d;
        if d > 0.0 {
            if t < exit {
                exit = t;
                exit_side = Some(side);
            }
        } else if t > enter {
            enter = t;
            entry_side = Some(side);
        }
        if enter >= exit {
            return None;
        }
    }
    let flags = entry_side
        .or(exit_side)
        .map_or(shader.surface_flags, |side| {
            bsp.shaders[side.shader].surface_flags
        });
    if flags & SURF_SKY != 0 {
        return None;
    }
    Some(BrushHit {
        interval: SolidInterval {
            brush: index,
            enter,
            exit,
            material: (flags & MATERIAL_MASK) as u8,
        },
        entry_normal: entry_side.map(|side| bsp.planes[side.plane].normal.map(f64::from)),
    })
}

/// Interpolated inline BSP transform in the same basis as CGame rendering.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mover {
    pub entity: u16,
    pub model: usize,
    pub origin: [f32; 3],
    pub axis: [[f32; 3]; 3],
}

struct MoverBounds {
    mover: Mover,
    local: Bounds,
    world: Bounds,
}

#[derive(Default)]
pub(crate) struct Movers {
    active: Vec<MoverBounds>,
}

impl Movers {
    pub fn update(&mut self, bsp: &AcousticCollision, incoming: &[Mover]) {
        self.active.clear();
        for &mover in incoming {
            let Some(model) = bsp
                .models
                .get(mover.model)
                .filter(|model| !model.brushes.is_empty())
            else {
                continue;
            };
            if mover.model == 0
                || !mover
                    .origin
                    .iter()
                    .chain(mover.axis.iter().flatten())
                    .all(|v| v.is_finite())
            {
                continue;
            }
            let local = Bounds {
                lo: model.mins,
                hi: model.maxs,
            };
            let mut world = Bounds::empty();
            for x in [local.lo[0], local.hi[0]] {
                for y in [local.lo[1], local.hi[1]] {
                    for z in [local.lo[2], local.hi[2]] {
                        world.include(std::array::from_fn(|i| {
                            mover.origin[i]
                                + mover.axis[0][i] * x
                                + mover.axis[1][i] * y
                                + mover.axis[2][i] * z
                        }));
                    }
                }
            }
            for i in 0..3 {
                world.lo[i] -= 0.01;
                world.hi[i] += 0.01;
            }
            self.active.push(MoverBounds {
                mover,
                local,
                world,
            });
        }
    }

    pub fn hits<'a>(
        &'a self,
        bsp: &'a AcousticCollision,
        from: Vec3,
        to: Vec3,
        contents: u32,
    ) -> impl Iterator<Item = (SolidInterval, Option<Vec3>)> + 'a {
        let delta = sub(to, from);
        self.active
            .iter()
            .filter_map(move |active| {
                active.world.entry(from, delta, 0.0, 1.0)?;
                let basis = active.mover.axis.map(|axis| axis.map(f64::from));
                let relative = sub(from, active.mover.origin.map(f64::from));
                let local_from = basis.map(|axis| dot(relative, axis));
                let local_delta = basis.map(|axis| dot(delta, axis));
                active.local.entry(local_from, local_delta, 0.0, 1.0)?;
                Some((
                    basis,
                    local_from,
                    local_delta,
                    bsp.models[active.mover.model].brushes.clone(),
                ))
            })
            .flat_map(move |(basis, local_from, local_delta, brushes)| {
                brushes.filter_map(move |index| {
                    let hit = clip_brush(bsp, index, local_from, local_delta, contents)?;
                    let normal = hit.entry_normal.map(|normal| {
                        std::array::from_fn(|i| {
                            normal[0] * basis[0][i]
                                + normal[1] * basis[1][i]
                                + normal[2] * basis[2][i]
                        })
                    });
                    Some((hit.interval, normal))
                })
            })
    }
}

/// Per-worker reusable memory. Raw intervals are not attenuation coefficients:
/// overlapping brushes can intersect; material precedence/union must be settled
/// in the propagation adapter before accumulating resistance.
#[derive(Default)]
pub struct SolidTraceScratch {
    stack: Vec<(i32, f64, f64)>,
    seen: Vec<u32>,
    generation: u32,
    intervals: Vec<SolidInterval>,
}
impl SolidTraceScratch {
    pub fn trace<'a>(
        &'a mut self,
        bsp: &AcousticCollision,
        from: Vec3,
        to: Vec3,
    ) -> Result<&'a [SolidInterval], &'static str> {
        self.trace_contents(bsp, from, to, CONTENTS_SOLID | CONTENTS_TERRAIN)
    }

    pub fn trace_contents<'a>(
        &'a mut self,
        bsp: &AcousticCollision,
        from: Vec3,
        to: Vec3,
        contents: u32,
    ) -> Result<&'a [SolidInterval], &'static str> {
        let tree = &bsp.collision;
        self.stack.clear();
        self.intervals.clear();
        if !from.iter().chain(to.iter()).all(|v| v.is_finite()) {
            return Err("invalid ray");
        }
        if tree.nodes.is_empty() || from == to {
            return Ok(&self.intervals);
        }
        if self.seen.len() != bsp.brushes.len() {
            self.seen.resize(bsp.brushes.len(), 0);
        }
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.seen.fill(0);
            self.generation = 1;
        }
        let delta = sub(to, from);
        self.stack.push((0, 0.0, 1.0));
        while let Some((index, lo, hi)) = self.stack.pop() {
            if index < 0 {
                let leaf = &tree.leaves[(-(index + 1)) as usize];
                for &brush_index in &tree.leaf_brushes[leaf.brushes.clone()] {
                    if !bsp.world_brushes.contains(&brush_index)
                        || self.seen[brush_index] == self.generation
                    {
                        continue;
                    }
                    self.seen[brush_index] = self.generation;
                    if let Some(hit) = clip_brush(bsp, brush_index, from, delta, contents) {
                        self.intervals.push(hit.interval);
                    }
                }
                continue;
            }
            let node = &tree.nodes[index as usize];
            let plane = bsp.planes[node.plane];
            let normal = plane.normal.map(f64::from);
            let a = dot(normal, from) - f64::from(plane.distance);
            let d = dot(normal, delta);
            let p = a + d * lo;
            let q = a + d * hi;
            if p > 0.0 && q > 0.0 {
                self.stack.push((node.children[0], lo, hi));
            } else if p < 0.0 && q < 0.0 {
                self.stack.push((node.children[1], lo, hi));
            } else if d == 0.0 {
                self.stack.push((node.children[0], lo, hi));
                self.stack.push((node.children[1], lo, hi));
            } else {
                let t = (-a / d).clamp(lo, hi);
                let (first, second) = if d > 0.0 { (1, 0) } else { (0, 1) };
                self.stack.push((node.children[second], t, hi));
                self.stack.push((node.children[first], lo, t));
            }
        }
        self.intervals.sort_unstable_by(|a, b| {
            a.enter
                .total_cmp(&b.enter)
                .then_with(|| a.exit.total_cmp(&b.exit))
                .then_with(|| a.brush.cmp(&b.brush))
        });
        Ok(&self.intervals)
    }
}

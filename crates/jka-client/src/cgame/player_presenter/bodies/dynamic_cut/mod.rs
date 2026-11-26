//! cg_dismemberStyle 1: client-side dynamic saber cuts.
//!
//! Instead of hiding an authored Ghoul2 subtree and enabling its `*_cap_*`
//! surfaces, the swept area of a real saber blade cuts the corpse mesh:
//!
//! 1. every presented blade (base, tip) is sampled per frame; consecutive
//!    samples form swept quads, and near-coplanar quads join into a swing;
//! 2. the swing plane crosses some of the Rapier corpse's simulated segments
//!    inside the swept area. Removing those segments from the skeleton graph
//!    gives the pieces (one swing can take several limbs at once);
//! 3. vertices follow their bones to a piece; vertices weighted to a crossed
//!    segment use the plane side (LimbHacker's severable-vertex rule);
//! 4. `slice` splits straddling triangles and ear clips SkinGore-textured
//!    caps; the Rapier corpse is split in place into sub-ragdolls.
//!
//! A swing that separates nothing leaves a SkinGore gash instead. The server
//! still decides deaths; its G2_MODEL_PART limbs are hidden while this runs.
//! Presentation only: nothing here feeds prediction or gameplay.

pub(in crate::cgame::player_presenter) mod slice;
mod worker;

use crate::cgame::player_presenter::{
    assets::types::{Ghoul2GpuMeshSource, PlayerModelAsset, PlayerSurfaceAsset},
    transform_jka_model_point, Arc, DynamicModelSurface, HashMap, HashSet, Matrix3x4,
    PlayerFxRequest, PlayerPresenter,
};
use crate::renderer::DynamicModelAlphaMode;
use crate::cgame::player_presenter::submission::mesh::build_gpu_mesh_source;
use crate::cgame::jiggle::JiggleRemeshTuning;
use crate::cgame::player_gore::{impact_position, PendingGore};
use crate::cgame::ragdoll::{body_segment_spec, RagdollCut, RagdollCutGroup};
use jka_assets::ghoul2::{GlaAnimation, GlmLod, GlmModel, GlmSurface, GlmSurfaceHierarchy};
use slice::{add, cross, dot, length, lerp3, normalize, scale, sub, Label, Plane, SliceSource};
use worker::{CutDone, CutJob, CutMeta};
use std::sync::mpsc::{self, Receiver, Sender};

/// A blade sample older than this starts a new swing.
const BLADE_GAP_MS: i32 = 120;
/// Longest swing kept as one cut plane.
const SWING_MAX_MS: i32 = 320;
/// Consecutive swept quads stay in one swing within ~25 degrees.
const SWING_PLANE_DOT: f32 = 0.9;
/// A blade that moves less than this per frame is a thrust/hold, not a cut.
const MIN_SWEEP: f32 = 0.75;
/// Newly dead corpses are also tested against swings this recent, so the
/// killing blow can sever even though EF_DEAD arrives after the hit.
const SWING_HISTORY_MS: i32 = 350;
/// Tolerance around the swing plane for the swept-area test.
const PLANE_SLAB: f32 = 10.0;
const CAP_UV_SCALE: f32 = 12.0;
const MAX_PIECES: usize = 16;
const SOURCE_MAX_AGE_MS: i32 = 250;
/// Generous corpse / piece extents for the per-frame bounding-sphere reject.
const BODY_REACH: f32 = 72.0;
const PIECE_REACH: f32 = 48.0;
/// A job whose result never came back (panicked on the shared lane) stops
/// blocking its target after this long.
const IN_FLIGHT_TIMEOUT_MS: i32 = 2000;
const SPINE_AND_HEAD: [&str; 4] = ["cranium", "thoracic", "upper_lumbar", "lower_lumbar"];

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct BladeKey {
    owner: u16,
    saber: u8,
    blade: u8,
}

#[derive(Clone, Copy)]
struct BladeSample {
    base: [f32; 3],
    tip: [f32; 3],
    time: i32,
}

/// Near-coplanar swept blade quads (base0, tip0, tip1, base1), world space.
#[derive(Clone)]
struct Swing {
    id: u64,
    owner: u16,
    quads: Vec<[[f32; 3]; 4]>,
    normal_sum: [f32; 3],
    centroid_sum: [f32; 3],
    area_sum: f32,
    sweep_sum: [f32; 3],
    started: i32,
    last: i32,
}

impl Swing {
    fn plane(&self) -> ([f32; 3], [f32; 3]) {
        let normal = normalize(self.normal_sum);
        let point = scale(self.centroid_sum, 1.0 / self.area_sum.max(1.0e-6));
        (normal, point)
    }

    fn sweep_direction(&self) -> [f32; 3] {
        normalize(self.sweep_sum)
    }

    /// World point inside the swept area (projected onto each quad's plane).
    fn contains(&self, p: [f32; 3], margin: f32) -> bool {
        let (normal, point) = self.plane();
        if dot(normal, sub(p, point)).abs() > PLANE_SLAB + margin {
            return false;
        }
        self.quads.iter().any(|q| {
            in_triangle_margin(p, q[0], q[1], q[2], margin) || in_triangle_margin(p, q[0], q[2], q[3], margin)
        })
    }

    fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for q in &self.quads {
            for p in q {
                for i in 0..3 {
                    lo[i] = lo[i].min(p[i]);
                    hi[i] = hi[i].max(p[i]);
                }
            }
        }
        (lo, hi)
    }
}

/// Point-in-triangle after projection to the triangle plane, with an
/// edge margin (world units) so a limb's thickness counts as touched.
fn in_triangle_margin(p: [f32; 3], a: [f32; 3], b: [f32; 3], c: [f32; 3], margin: f32) -> bool {
    let n = cross(sub(b, a), sub(c, a));
    let n_len = length(n);
    if n_len < 1.0e-6 {
        return false;
    }
    let n = scale(n, 1.0 / n_len);
    for (e0, e1) in [(a, b), (b, c), (c, a)] {
        let edge = sub(e1, e0);
        let inward = normalize(cross(n, edge));
        if dot(inward, sub(p, e0)) < -margin {
            return false;
        }
    }
    true
}

/// Last presented corpse pose, captured where the body is submitted.
struct CutSource {
    model: Arc<PlayerModelAsset>,
    pose: Vec<Matrix3x4>,
    axis: [[f32; 3]; 3],
    origin: [f32; 3],
    client: i32,
    rgba: [f32; 4],
    rgb: [f32; 3],
    captured_at: i32,
    /// First capture of this corpse: test it against recent swings once.
    fresh: bool,
}

/// A corpse whose rendered mesh was replaced by a cut root piece.
struct CutBody {
    serial: u64,
    model: Arc<PlayerModelAsset>,
    hand_detached: bool,
    swings: HashSet<u64>,
}

/// A detached piece driven by its own Rapier sub-ragdoll.
struct CutPiece {
    serial: u64,
    model: Arc<PlayerModelAsset>,
    client: i32,
    rgba: [f32; 4],
    rgb: [f32; 3],
    anchor_bone: usize,
    label: String,
    swings: HashSet<u64>,
    /// Last drawn pose, render frame and anchor world position.
    last: Option<(Vec<Matrix3x4>, [[f32; 3]; 3], [f32; 3], [f32; 3])>,
}

#[derive(Default)]
pub(in crate::cgame::player_presenter) struct DynamicCuts {
    pub(in crate::cgame::player_presenter) reference_inline: bool,
    blades: HashMap<BladeKey, BladeSample>,
    open: HashMap<BladeKey, Swing>,
    touched: HashSet<BladeKey>,
    finished: Vec<Swing>,
    history: Vec<(Swing, i32)>,
    sources: HashMap<u16, CutSource>,
    bodies: HashMap<u16, CutBody>,
    pieces: HashMap<u16, CutPiece>,
    /// (swing, target key) -> crossed segment bones at the last evaluation.
    pending: HashMap<(u64, TargetKey), Vec<usize>>,
    next_swing: u64,
    next_cut: u64,
    /// The app's shared CPU lane; None in tools/previews (cut inline then).
    jobs: Option<crate::app::CpuJobHandle>,
    /// Results back from the lane. Survives `clear`; stale results are
    /// rejected on arrival.
    results: Option<(Sender<CutDone>, Receiver<CutDone>)>,
    /// Targets with a job queued (submit time); skipped until it lands.
    in_flight: HashMap<TargetKey, i32>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum TargetKey {
    Body(u16),
    Piece(u16),
}

impl DynamicCuts {
    pub(in crate::cgame::player_presenter) fn clear(&mut self) {
        // Keep the lane handle and result channel; results of jobs in flight
        // fail the staleness check once their target state is gone.
        let jobs = self.jobs.take();
        let reference_inline = self.reference_inline;
        let results = self.results.take();
        *self = Self::default();
        self.jobs = jobs;
        self.reference_inline = reference_inline;
        self.results = results;
    }

    /// CG_BodyQueueCopy moves the solved corpse (and its Rapier instance)
    /// to the ET_BODY slot; the cut mesh follows it.
    pub(in crate::cgame::player_presenter) fn body_queue_copy(&mut self, source: u16, body: u16) {
        self.bodies.remove(&body);
        if let Some(cut) = self.bodies.remove(&source) {
            self.bodies.insert(body, cut);
        }
        if let Some(src) = self.sources.remove(&source) {
            self.sources.insert(body, src);
        }
    }
}

/// One simulated segment of the target's Rapier corpse, posed model space.
struct Segment {
    bone: usize,
    start: [f32; 3],
    end: [f32; 3],
    radius: f32,
    parent: Option<usize>,
    name: &'static str,
}

struct Crossing {
    segment: usize,
    model_point: [f32; 3],
    /// Fraction along the segment from its joint (pose independent).
    fraction: f32,
}

/// Everything a cut needs about its target, copied out of the presenter.
struct CutTarget {
    key: TargetKey,
    ragdoll_key: u16,
    model: Arc<PlayerModelAsset>,
    pose: Vec<Matrix3x4>,
    axis: [[f32; 3]; 3],
    origin: [f32; 3],
    client: i32,
    rgba: [f32; 4],
    rgb: [f32; 3],
    /// Surface indices drawn now (all for cut models, default for stock).
    visible: HashSet<usize>,
}

struct SkeletonCut {
    segments: Vec<Segment>,
    crossings: Vec<Crossing>,
    /// Union-find group per (segment, near/far) node.
    node_group: Vec<usize>,
    root_group: usize,
    plane_model: Plane,
}

impl SkeletonCut {
    fn near(&self, segment: usize) -> usize {
        self.node_group[segment * 2]
    }
    fn far(&self, segment: usize) -> usize {
        self.node_group[segment * 2 + 1]
    }
    fn crossed(&self, segment: usize) -> bool {
        self.crossings.iter().any(|c| c.segment == segment)
    }
    fn group_count(&self) -> usize {
        self.node_group.iter().copied().collect::<HashSet<_>>().len()
    }
}

fn find(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

fn union(parent: &mut [usize], a: usize, b: usize) {
    let (ra, rb) = (find(parent, a), find(parent, b));
    if ra != rb {
        parent[ra.max(rb)] = ra.min(rb);
    }
}

fn posed_translation(gla: &GlaAnimation, pose: &[Matrix3x4], bone: usize) -> Option<[f32; 3]> {
    let skin = pose.get(bone)?;
    let base = &gla.skeleton.get(bone)?.base_pose;
    let m = jka_assets::ghoul2::multiply_3x4(skin, base);
    Some([m[0][3], m[1][3], m[2][3]])
}

fn bone_named(gla: &GlaAnimation, name: &str) -> Option<usize> {
    gla.skeleton.iter().position(|bone| bone.name.eq_ignore_ascii_case(name))
}

/// Simulated segments in the same shape Rapier uses for their capsules.
fn driven_segments(gla: &GlaAnimation, pose: &[Matrix3x4], driven: &[usize]) -> Vec<Segment> {
    let mut segments: Vec<Segment> = driven
        .iter()
        .filter_map(|&bone| {
            let (name, end_name, radius) = body_segment_spec(&gla.skeleton.get(bone)?.name)?;
            let start = posed_translation(gla, pose, bone)?;
            let end = end_name
                .and_then(|end| bone_named(gla, end))
                .and_then(|end| posed_translation(gla, pose, end));
            Some(Segment { bone, start, end: end.unwrap_or(start), radius, parent: None, name })
        })
        .collect();
    let index_of: HashMap<usize, usize> =
        segments.iter().enumerate().map(|(i, s)| (s.bone, i)).collect();
    for i in 0..segments.len() {
        let mut parent = jka_assets::skeleton::mesh_pose_parent(&gla.skeleton, segments[i].bone);
        while let Some(bone) = parent {
            if let Some(&p) = index_of.get(&bone) {
                segments[i].parent = Some(p);
                break;
            }
            parent = jka_assets::skeleton::mesh_pose_parent(&gla.skeleton, bone);
        }
    }
    // Ball segments (head, hands, feet) continue their parent's direction.
    for i in 0..segments.len() {
        if segments[i].end == segments[i].start {
            let from = segments[i].parent.map_or(sub(segments[i].start, [0.0, 0.0, 1.0]), |p| segments[p].start);
            let dir = normalize(sub(segments[i].start, from));
            segments[i].end = add(segments[i].start, scale(dir, segments[i].radius * 2.0));
        }
    }
    segments
}

fn world_plane_to_model(normal: [f32; 3], point: [f32; 3], axis: [[f32; 3]; 3], origin: [f32; 3]) -> Plane {
    // world = origin + sum(axis[i] * p[i]) => n_m[i] = dot(axis[i], n_w).
    let d_world = -dot(normal, point);
    let n = [dot(axis[0], normal), dot(axis[1], normal), dot(axis[2], normal)];
    let d = dot(normal, origin) + d_world;
    let len = length(n).max(1.0e-9);
    Plane { normal: scale(n, 1.0 / len), distance: d / len }
}

fn side(plane: &Plane, p: [f32; 3]) -> f32 {
    dot(plane.normal, p) + plane.distance
}

/// Skeleton-level decision: which segments the swing crosses inside its
/// swept area, and the resulting piece groups.
fn skeleton_cut(
    axis: [[f32; 3]; 3],
    origin: [f32; 3],
    segments: Vec<Segment>,
    swing: &Swing,
    model_scale: f32,
) -> SkeletonCut {
    let (normal, point) = swing.plane();
    let plane_model = world_plane_to_model(normal, point, axis, origin);
    let mut crossings = Vec::new();
    for (index, segment) in segments.iter().enumerate() {
        let (s0, s1) = (side(&plane_model, segment.start), side(&plane_model, segment.end));
        if (s0 >= 0.0) == (s1 >= 0.0) {
            continue;
        }
        let t = s0 / (s0 - s1);
        let model_point = lerp3(segment.start, segment.end, t);
        let world = transform_jka_model_point(model_point, axis, origin);
        if swing.contains(world, segment.radius * model_scale + 2.0) {
            crossings.push(Crossing { segment: index, model_point, fraction: t });
        }
    }
    let n = segments.len();
    let mut parent: Vec<usize> = (0..n * 2).collect();
    let crossed: HashSet<usize> = crossings.iter().map(|c| c.segment).collect();
    for i in 0..n {
        if !crossed.contains(&i) {
            union(&mut parent, i * 2, i * 2 + 1);
        }
    }
    for (child, segment) in segments.iter().enumerate() {
        let Some(p) = segment.parent else { continue };
        // A child joint attaches to whichever side of a crossed parent its
        // joint lies on; uncrossed parents are a single node.
        let attach = if crossed.contains(&p) {
            let same = (side(&plane_model, segment.start) >= 0.0)
                == (side(&plane_model, segments[p].start) >= 0.0);
            if same { p * 2 } else { p * 2 + 1 }
        } else {
            p * 2
        };
        union(&mut parent, child * 2, attach);
    }
    let node_group: Vec<usize> = (0..n * 2).map(|x| find(&mut parent, x)).collect();
    let root_segment = segments.iter().position(|s| s.name.eq_ignore_ascii_case("pelvis")).unwrap_or(0);
    let root_group = node_group.get(root_segment * 2).copied().unwrap_or(0);
    SkeletonCut { segments, crossings, node_group, root_group, plane_model }
}

impl PlayerPresenter {
    pub(in crate::cgame::player_presenter) fn dynamic_cuts_on(&self) -> bool {
        self.ragdolls.dynamic_cuts_active() && self.assets.jk2_version().is_none()
    }

    /// Hooked beside `queue_saber_gore_blade` for every presented blade.
    pub(in crate::cgame::player_presenter) fn note_cut_blade(
        &mut self,
        owner: u16,
        (saber, blade): (usize, usize),
        base: [f32; 3],
        direction: [f32; 3],
        length_units: f32,
        now: i32,
    ) {
        if !self.dynamic_cuts_on() || length_units <= 0.0 {
            return;
        }
        let key = BladeKey { owner, saber: saber as u8, blade: blade as u8 };
        let tip = add(base, scale(normalize(direction), length_units));
        let cuts = &mut self.dynamic_cuts;
        cuts.touched.insert(key);
        let sample = BladeSample { base, tip, time: now };
        let Some(previous) = cuts.blades.insert(key, sample) else { return };
        let dt = now.saturating_sub(previous.time);
        if dt <= 0 {
            cuts.blades.insert(key, previous);
            return;
        }
        let quad = [previous.base, previous.tip, tip, base];
        let mid0 = scale(add(previous.base, previous.tip), 0.5);
        let mid1 = scale(add(base, tip), 0.5);
        let sweep = sub(mid1, mid0);
        let blade_dir = add(sub(previous.tip, previous.base), sub(tip, base));
        let face = cross(blade_dir, sweep);
        let area = length(face) * 0.5;
        if dt > BLADE_GAP_MS || length(sweep) < MIN_SWEEP || area < 1.0e-3 {
            if let Some(swing) = cuts.open.remove(&key) {
                cuts.finished.push(swing);
            }
            return;
        }
        let normal = scale(face, 1.0 / (2.0 * area));
        let centroid = scale(add(add(quad[0], quad[1]), add(quad[2], quad[3])), 0.25);
        let continues = cuts.open.get(&key).is_some_and(|swing| {
            dot(normalize(swing.normal_sum), normal) >= SWING_PLANE_DOT
                && now.saturating_sub(swing.started) <= SWING_MAX_MS
        });
        if !continues {
            if let Some(swing) = cuts.open.remove(&key) {
                cuts.finished.push(swing);
            }
            cuts.next_swing += 1;
            cuts.open.insert(key, Swing {
                id: cuts.next_swing,
                owner,
                quads: Vec::new(),
                normal_sum: [0.0; 3],
                centroid_sum: [0.0; 3],
                area_sum: 0.0,
                sweep_sum: [0.0; 3],
                started: previous.time,
                last: now,
            });
        }
        if let Some(swing) = cuts.open.get_mut(&key) {
            swing.quads.push(quad);
            swing.normal_sum = add(swing.normal_sum, scale(normal, area));
            swing.centroid_sum = add(swing.centroid_sum, scale(centroid, area));
            swing.area_sum += area;
            swing.sweep_sum = add(swing.sweep_sum, sweep);
            swing.last = now;
        }
    }

    /// Hooked where the corpse body is submitted, after Rapier solved it.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::cgame::player_presenter) fn capture_cut_source(
        &mut self,
        entity: u16,
        client: i32,
        model: &Arc<PlayerModelAsset>,
        pose: &[Matrix3x4],
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
        rgba: [f32; 4],
        rgb: [f32; 3],
        now: i32,
    ) {
        if !self.dynamic_cuts_on() || !self.ragdolls.has_instance(entity) {
            return;
        }
        // Drop a cut mesh that belongs to a previous corpse in this slot.
        let serial = self.ragdolls.instance_serial(entity);
        if self.dynamic_cuts.bodies.get(&entity).is_some_and(|body| Some(body.serial) != serial) {
            self.dynamic_cuts.bodies.remove(&entity);
        }
        let model = self
            .dynamic_cuts
            .bodies
            .get(&entity)
            .map_or_else(|| Arc::clone(model), |body| Arc::clone(&body.model));
        // Reuse the per-corpse pose buffer; this runs every frame per corpse.
        if let Some(source) = self.dynamic_cuts.sources.get_mut(&entity) {
            source.model = model;
            source.pose.clear();
            source.pose.extend_from_slice(pose);
            (source.axis, source.origin, source.client) = (axis, origin, client);
            (source.rgba, source.rgb, source.captured_at) = (rgba, rgb, now);
        } else {
            self.dynamic_cuts.sources.insert(entity, CutSource {
                model,
                pose: pose.to_vec(),
                axis,
                origin,
                client,
                rgba,
                rgb,
                captured_at: now,
                fresh: true,
            });
        }
    }

    /// The cut root mesh replacing this corpse's stock model, if any.
    pub(in crate::cgame::player_presenter) fn dynamic_cut_body_model(
        &self,
        entity: u16,
    ) -> Option<Arc<PlayerModelAsset>> {
        if !self.dynamic_cuts_on() {
            return None;
        }
        let body = self.dynamic_cuts.bodies.get(&entity)?;
        (self.ragdolls.instance_serial(entity) == Some(body.serial)).then(|| Arc::clone(&body.model))
    }

    /// The weapon hand went with a piece: the corpse no longer holds model 1.
    pub(in crate::cgame::player_presenter) fn dynamic_cut_hand_detached(&self, entity: u16) -> bool {
        self.dynamic_cut_body_model(entity).is_some()
            && self.dynamic_cuts.bodies.get(&entity).is_some_and(|body| body.hand_detached)
    }

    /// Called once per frame after all entities (and blades) were presented.
    pub(crate) fn finish_dynamic_cuts(&mut self, now: i32) {
        if !self.dynamic_cuts_on() {
            if !self.dynamic_cuts.bodies.is_empty()
                || !self.dynamic_cuts.pieces.is_empty()
                || !self.dynamic_cuts.blades.is_empty()
            {
                self.dynamic_cuts.clear();
            }
            return;
        }
        // Results computed on the shared lane since last frame.
        while let Some(done) = self.dynamic_cuts.results.as_ref().and_then(|(_, rx)| rx.try_recv().ok()) {
            self.apply_cut_done(done, now);
        }
        self.dynamic_cuts
            .in_flight
            .retain(|_, submitted| now.saturating_sub(*submitted) <= IN_FLIGHT_TIMEOUT_MS);
        let cuts = &mut self.dynamic_cuts;
        let mut stale: Vec<BladeKey> = cuts
            .open
            .iter()
            .filter(|(key, swing)| !cuts.touched.contains(key) || now.saturating_sub(swing.last) > BLADE_GAP_MS)
            .map(|(key, _)| *key)
            .collect();
        if cuts.reference_inline { stale.sort_unstable_by_key(|k| (k.owner, k.saber, k.blade)); }
        for key in stale {
            if let Some(swing) = cuts.open.remove(&key) {
                cuts.finished.push(swing);
            }
        }
        cuts.touched.clear();
        cuts.blades.retain(|_, sample| now.saturating_sub(sample.time) <= BLADE_GAP_MS * 4);
        cuts.sources.retain(|_, source| now.saturating_sub(source.captured_at) <= SOURCE_MAX_AGE_MS);
        cuts.history.retain(|(_, finished)| now.saturating_sub(*finished) <= SWING_HISTORY_MS);
        let ragdolls = &self.ragdolls;
        cuts.bodies.retain(|&entity, body| ragdolls.instance_serial(entity) == Some(body.serial));
        let live: HashSet<u64> = cuts.open.values().map(|s| s.id).collect();
        cuts.pending.retain(|(swing, _), _| live.contains(swing));

        // Open swings may sever as soon as the crossed set stops growing;
        // finished swings resolve now (sever or gash).
        let mut open: Vec<Swing> = cuts.open.values().cloned().collect();
        let mut finished = std::mem::take(&mut cuts.finished);
        if cuts.reference_inline {
            open.sort_unstable_by_key(|s| s.id);
            finished.sort_unstable_by_key(|s| s.id);
        }
        let recent: Vec<Swing> = cuts.history.iter().map(|(s, _)| s.clone()).collect();
        for swing in &finished {
            self.dynamic_cuts.history.push((swing.clone(), now));
        }
        for swing in &open {
            self.evaluate_swing(swing, false, None, now);
        }
        for swing in &finished {
            self.evaluate_swing(swing, true, None, now);
        }
        // Killing blows: corpses that only just became visible as dead.
        let mut fresh = Vec::new();
        for (&entity, source) in &mut self.dynamic_cuts.sources {
            if std::mem::take(&mut source.fresh) {
                fresh.push(entity);
            }
        }
        if self.dynamic_cuts.reference_inline { fresh.sort_unstable(); }
        for entity in fresh {
            for swing in &recent {
                self.evaluate_swing(swing, true, Some(TargetKey::Body(entity)), now);
            }
        }
        if self.dynamic_cuts.reference_inline {
            while let Some(done) = self.dynamic_cuts.results.as_ref().and_then(|(_, rx)| rx.try_recv().ok()) {
                self.apply_cut_done(done, now);
            }
        }
    }

    fn cut_target_keys(&self, only: Option<TargetKey>) -> Vec<TargetKey> {
        if let Some(only) = only {
            return vec![only];
        }
        let mut keys: Vec<_> = self.dynamic_cuts
            .sources
            .keys()
            .map(|&entity| TargetKey::Body(entity))
            .chain(self.dynamic_cuts.pieces.keys().map(|&key| TargetKey::Piece(key)))
            .collect();
        if self.dynamic_cuts.reference_inline {
            keys.sort_unstable_by_key(|key| match key { TargetKey::Body(n) => (0, *n), TargetKey::Piece(n) => (1, *n) });
        }
        keys
    }

    /// Borrowed view used by the per-frame reject before anything is copied:
    /// (model, pose, axis, origin, client, centre, reach).
    #[allow(clippy::type_complexity)]
    fn target_view(
        &self,
        key: TargetKey,
    ) -> Option<(&Arc<PlayerModelAsset>, &[Matrix3x4], [[f32; 3]; 3], [f32; 3], i32, [f32; 3], f32)> {
        match key {
            TargetKey::Body(entity) => {
                let s = self.dynamic_cuts.sources.get(&entity)?;
                let reach = BODY_REACH * length(s.axis[0]).max(1.0e-3);
                Some((&s.model, s.pose.as_slice(), s.axis, s.origin, s.client, s.origin, reach))
            }
            TargetKey::Piece(piece) => {
                let p = self.dynamic_cuts.pieces.get(&piece)?;
                let (pose, axis, origin, centre) = p.last.as_ref()?;
                let reach = PIECE_REACH * length(axis[0]).max(1.0e-3);
                Some((&p.model, pose.as_slice(), *axis, *origin, p.client, *centre, reach))
            }
        }
    }

    fn build_cut_target(&self, key: TargetKey) -> Option<CutTarget> {
        let (model, pose, axis, origin, client, _, _) = self.target_view(key)?;
        let (ragdoll_key, rgba, rgb, all_surfaces) = match key {
            TargetKey::Body(entity) => {
                let s = self.dynamic_cuts.sources.get(&entity)?;
                (entity, s.rgba, s.rgb, self.dynamic_cuts.bodies.contains_key(&entity))
            }
            TargetKey::Piece(piece) => {
                let p = self.dynamic_cuts.pieces.get(&piece)?;
                (piece, p.rgba, p.rgb, true)
            }
        };
        let visible = model
            .surfaces
            .iter()
            .filter(|s| all_surfaces || s.default_visible)
            .map(|s| s.surface_index)
            .collect();
        Some(CutTarget {
            key,
            ragdoll_key,
            model: Arc::clone(model),
            pose: pose.to_vec(),
            axis,
            origin,
            client,
            rgba,
            rgb,
            visible,
        })
    }

    fn target_swings(&self, key: TargetKey) -> Option<&HashSet<u64>> {
        match key {
            TargetKey::Body(entity) => self.dynamic_cuts.bodies.get(&entity).map(|b| &b.swings),
            TargetKey::Piece(piece) => self.dynamic_cuts.pieces.get(&piece).map(|p| &p.swings),
        }
    }

    fn evaluate_swing(&mut self, swing: &Swing, final_pass: bool, only: Option<TargetKey>, now: i32) {
        if swing.quads.is_empty() {
            return;
        }
        let (lo, hi) = swing.bounds();
        for key in self.cut_target_keys(only) {
            if self.dynamic_cuts.in_flight.contains_key(&key)
                || self.target_swings(key).is_some_and(|s| s.contains(&swing.id))
            {
                continue;
            }
            // Cheapest reject first, on borrowed data: owner, then a bounding
            // sphere around the corpse against the swing bounds. Nothing is
            // allocated or copied for the common far-away case.
            let Some((_, _, _, _, client, centre, reach)) = self.target_view(key) else { continue };
            if client == i32::from(swing.owner) {
                continue;
            }
            let gap: f32 = (0..3)
                .map(|i| (lo[i] - centre[i]).max(centre[i] - hi[i]).max(0.0).powi(2))
                .sum::<f32>()
                .sqrt();
            if gap > reach {
                continue;
            }
            let ragdoll_key = match key {
                TargetKey::Body(entity) => entity,
                TargetKey::Piece(piece) => piece,
            };
            let Some(driven) = self.ragdolls.driven_bone_indices(ragdoll_key) else { continue };
            let Some((model, pose, axis, origin, ..)) = self.target_view(key) else { continue };
            let model_scale = length(axis[0]).max(1.0e-3);
            let segments = driven_segments(&model.gla, pose, &driven);
            let overlaps = segments.iter().any(|s| {
                [s.start, s.end].iter().any(|&p| {
                    let w = transform_jka_model_point(p, axis, origin);
                    let r = s.radius * model_scale + PLANE_SLAB;
                    (0..3).all(|i| w[i] >= lo[i] - r && w[i] <= hi[i] + r)
                })
            });
            if !overlaps {
                continue;
            }
            let cut = skeleton_cut(axis, origin, segments, swing, model_scale);
            let crossed: Vec<usize> = cut.crossings.iter().map(|c| cut.segments[c.segment].bone).collect();
            let severs = cut.group_count() > 1 && self.cut_allowed(&cut);
            if !final_pass {
                // Sever once the blade has passed: the crossed set stopped growing.
                let previous = self.dynamic_cuts.pending.insert((swing.id, key), crossed.clone());
                if !severs || previous.as_ref() != Some(&crossed) {
                    continue;
                }
            }
            // Only now copy the pose: the target is actually being cut.
            let Some(target) = self.build_cut_target(key) else { continue };
            if severs {
                if self.submit_cut(target, cut, swing, now) {
                    continue;
                }
                if !final_pass {
                    continue;
                }
                let Some(target) = self.build_cut_target(key) else { continue };
                self.saber_gash(&target, swing, now);
            } else if final_pass {
                self.saber_gash(&target, swing, now);
            } else {
                continue;
            }
            self.mark_swing(key, swing.id);
        }
    }

    fn mark_swing(&mut self, key: TargetKey, swing: u64) {
        match key {
            TargetKey::Body(entity) => {
                if let Some(body) = self.dynamic_cuts.bodies.get_mut(&entity) {
                    body.swings.insert(swing);
                }
            }
            TargetKey::Piece(piece) => {
                if let Some(piece) = self.dynamic_cuts.pieces.get_mut(&piece) {
                    piece.swings.insert(swing);
                }
            }
        }
    }

    /// cg_dismember 1 (Limbs): head and torso stay with the corpse.
    fn cut_allowed(&self, cut: &SkeletonCut) -> bool {
        self.ragdolls.dismemberment_level() >= 2
            || !cut.segments.iter().enumerate().any(|(i, s)| {
                SPINE_AND_HEAD.iter().any(|n| s.name.eq_ignore_ascii_case(n))
                    && (cut.near(i) != cut.root_group || cut.far(i) != cut.root_group)
            })
    }

    fn target_serial(&self, key: TargetKey) -> Option<u64> {
        match key {
            TargetKey::Body(entity) => self.ragdolls.instance_serial(entity),
            TargetKey::Piece(piece) => self.dynamic_cuts.pieces.get(&piece).map(|p| p.serial),
        }
    }

    /// Queue the mesh work on the shared CPU lane. False if it could not be
    /// queued (a full lane retries on the swing's next evaluation).
    fn submit_cut(&mut self, target: CutTarget, cut: SkeletonCut, swing: &Swing, now: i32) -> bool {
        let Some(serial) = self.target_serial(target.key) else { return false };
        self.dynamic_cuts.next_cut += 1;
        let cut_id = self.dynamic_cuts.next_cut;
        let cap_texture = crate::cgame::skin_gore::ensure_assets(&mut self.assets).map(|a| Arc::clone(&a.albedo));
        let key = target.key;
        let job = CutJob {
            meta: CutMeta {
                key,
                ragdoll_key: target.ragdoll_key,
                serial,
                source_model: target.model,
                client: target.client,
                rgba: target.rgba,
                rgb: target.rgb,
                swing: swing.clone(),
            },
            pose: target.pose,
            axis: target.axis,
            origin: target.origin,
            visible: target.visible,
            cut,
            cut_id,
            cap_texture,
        };
        let tx = self.dynamic_cuts.results.get_or_insert_with(mpsc::channel).0.clone();
        match self.dynamic_cuts.jobs.as_ref() {
            Some(jobs) => {
                if let Err(error) = jobs.submit(move || {
                    let _ = tx.send(worker::run(job));
                }) {
                    devprintln!(2, "DYNAMIC CUT: {error}");
                    return false;
                }
            }
            // Tools and previews have no app lane: cut inline.
            None => {
                let _ = tx.send(worker::run(job));
            }
        }
        self.dynamic_cuts.in_flight.insert(key, now);
        true
    }

    /// Install the app's shared `jka-background-cpu` lane for cut jobs.
    pub(crate) fn set_background_cpu(&mut self, jobs: crate::app::CpuJobHandle) {
        self.dynamic_cuts.jobs = Some(jobs);
    }

    pub(crate) fn has_background_cpu(&self) -> bool {
        self.dynamic_cuts.jobs.is_some()
    }

    /// The target model the presenter currently draws for `key`.
    fn current_target_model(&self, key: TargetKey) -> Option<&Arc<PlayerModelAsset>> {
        match key {
            TargetKey::Body(entity) => self.dynamic_cuts.sources.get(&entity).map(|s| &s.model),
            TargetKey::Piece(piece) => self.dynamic_cuts.pieces.get(&piece).map(|p| &p.model),
        }
    }

    /// Main-thread half of a cut: allocate piece keys, split Rapier, swap
    /// meshes and emit FX. Stale results (target re-cut, respawned, gone)
    /// are dropped.
    fn apply_cut_done(&mut self, done: CutDone, now: i32) {
        let CutDone { meta, outcome } = done;
        self.dynamic_cuts.in_flight.remove(&meta.key);
        let fresh = self.target_serial(meta.key) == Some(meta.serial)
            && self.current_target_model(meta.key).is_some_and(|m| Arc::ptr_eq(m, &meta.source_model));
        if !fresh || !self.dynamic_cuts_on() {
            return;
        }
        let computed = match outcome {
            Ok(computed) => computed,
            Err(reason) => {
                devprintln!(2, "DYNAMIC CUT: {:?} not severed: {reason}", meta.key);
                if let Some(target) = self.build_cut_target(meta.key) {
                    self.saber_gash(&target, &meta.swing, now);
                }
                self.mark_swing(meta.key, meta.swing.id);
                return;
            }
        };
        let mut groups = Vec::with_capacity(computed.pieces.len());
        let mut keyed = Vec::with_capacity(computed.pieces.len());
        for piece in computed.pieces {
            let Some(key) = self.ragdolls.alloc_cut_piece_key() else { break };
            groups.push(RagdollCutGroup {
                key,
                bones: piece.bones,
                synthesize: piece.synthesize,
                kick: piece.kick,
            });
            keyed.push((key, piece.model, piece.anchor_bone));
        }
        let gla = Arc::clone(&meta.source_model.gla);
        let created = self.ragdolls.split_for_cut(
            RagdollCut { source: meta.ragdoll_key, truncate: computed.truncate, groups },
            &gla,
            now,
        );
        if created.is_empty() {
            return;
        }
        let mut inherited = self.target_swings(meta.key).cloned().unwrap_or_default();
        inherited.insert(meta.swing.id);
        for (key, model, anchor_bone) in keyed {
            let Some(&(_, serial)) = created.iter().find(|(k, _)| *k == key) else { continue };
            self.dynamic_cuts.pieces.insert(key, CutPiece {
                serial,
                label: format!("{}#piece{key}", model.key),
                model,
                client: meta.client,
                rgba: meta.rgba,
                rgb: meta.rgb,
                anchor_bone,
                swings: inherited.clone(),
                last: None,
            });
        }
        match meta.key {
            TargetKey::Body(entity) => {
                let serial = self.ragdolls.instance_serial(entity).unwrap_or(0);
                let previous = self.dynamic_cuts.bodies.remove(&entity);
                let hand_detached = previous.as_ref().is_some_and(|b| b.hand_detached) || computed.hand_detached;
                if let Some(source) = self.dynamic_cuts.sources.get_mut(&entity) {
                    source.model = Arc::clone(&computed.root);
                }
                self.dynamic_cuts.bodies.insert(entity, CutBody {
                    serial,
                    model: computed.root,
                    hand_detached,
                    swings: inherited,
                });
            }
            TargetKey::Piece(key) => {
                if let Some(piece) = self.dynamic_cuts.pieces.get_mut(&key) {
                    piece.model = computed.root;
                    piece.swings.insert(meta.swing.id);
                }
            }
        }
        // Blood and the stock sever smoke from each new cut face.
        for (origin, dir, root_face) in computed.faces {
            self.fx_requests.push(PlayerFxRequest::EffectDir { name: "saber/blood_sparks_mp".to_owned(), origin, dir });
            if root_face {
                self.fx_requests.push(PlayerFxRequest::EffectDir { name: "blaster/smoke_bolton".to_owned(), origin, dir });
            }
        }
        devprintln!(
            1,
            "DYNAMIC CUT: target={:?} crossed={} pieces={}",
            meta.key,
            computed.crossings,
            created.len()
        );
    }

    /// No piece separated: stamp the swing's path on the skin instead.
    fn saber_gash(&mut self, target: &CutTarget, swing: &Swing, now: i32) {
        let TargetKey::Body(entity) = target.key else { return };
        let Some(snaps) = self.last_gpu.get(&entity).cloned().filter(|s| !s.is_empty()) else { return };
        let mut stamps = 0;
        for (index, quad) in swing.quads.iter().enumerate() {
            for t in [0.0, 0.5, 1.0] {
                if stamps >= 12 {
                    return;
                }
                let base = lerp3(quad[0], quad[3], t);
                let tip = lerp3(quad[1], quad[2], t);
                let Some(position) = impact_position(&snaps, base, tip) else { continue };
                stamps += 1;
                if self.skin_gore_enabled {
                    self.skin_gore.add_damage(entity, &snaps, position, 3.5, 1.0);
                } else {
                    let direction = normalize(sub(tip, base));
                    self.pending_gore.push(PendingGore {
                        entity,
                        start: position,
                        end: add(position, scale(direction, 4.0)),
                        trace: false,
                        size: 3.5,
                        skin_scale: 1.0,
                        life_ms: 8_000,
                        shader: "gfx/damage/saberglowmark".to_owned(),
                        time: now,
                        sequence: swing.id.wrapping_mul(97) ^ index as u64,
                    });
                }
            }
        }
    }

    /// Draw every live cut piece from its sub-ragdoll.
    pub(in crate::cgame::player_presenter) fn present_dynamic_cut_pieces(
        &mut self,
        now: i32,
    ) -> Vec<DynamicModelSurface> {
        let _ = now;
        let mut draws = Vec::new();
        if !self.dynamic_cuts_on() {
            return draws;
        }
        let keys: Vec<u16> = self.dynamic_cuts.pieces.keys().copied().collect();
        for key in keys {
            let alive = self
                .dynamic_cuts
                .pieces
                .get(&key)
                .is_some_and(|piece| self.ragdolls.instance_serial(key) == Some(piece.serial));
            if !alive {
                self.dynamic_cuts.pieces.remove(&key);
                continue;
            }
            let piece = &self.dynamic_cuts.pieces[&key];
            let model = Arc::clone(&piece.model);
            let (label, rgba, rgb, anchor) = (piece.label.clone(), piece.rgba, piece.rgb, piece.anchor_bone);
            let Some((pose, frame, model_scale)) = self.ragdolls.cut_piece_pose(key) else { continue };
            let mut axis = [
                [frame[0][0], frame[1][0], frame[2][0]],
                [frame[0][1], frame[1][1], frame[2][1]],
                [frame[0][2], frame[1][2], frame[2][2]],
            ];
            for basis in &mut axis {
                for component in basis.iter_mut() {
                    *component *= model_scale;
                }
            }
            let origin = [frame[0][3], frame[1][3], frame[2][3]];
            let light = posed_translation(&model.gla, &pose, anchor)
                .map_or(origin, |p| transform_jka_model_point(p, axis, origin));
            match self.render_glm_surfaces_tinted_selected(
                1023, &label, &model.glm, &model.gla, &model.surfaces, None, &pose, 0, axis, origin,
                rgba, rgb, None, true, None, true,
            ) {
                Ok(mut piece_draws) => {
                    for draw in &mut piece_draws {
                        draw.lighting_origin = Some(light);
                        if let Some(rt) = &mut draw.rt_skinned_key {
                            *rt = Arc::from(format!("{rt}#{label}"));
                        }
                    }
                    draws.append(&mut piece_draws);
                }
                Err(error) => { devprintln!(1, "DYNAMIC CUT: piece {key} draw failed: {error}"); }
            }
            if let Some(piece) = self.dynamic_cuts.pieces.get_mut(&key) {
                piece.last = Some((pose, axis, origin, light));
            }
        }
        draws
    }
}

/// One-LOD model asset sharing the source skeleton/materials, with cut
/// surfaces (and an optional SkinGore cap) uploaded under `#dyncut` keys.
fn build_cut_model(
    base: &PlayerModelAsset,
    surfaces: Vec<GlmSurface>,
    cap: Option<GlmSurface>,
    cap_texture: Option<Arc<crate::materials::TextureData>>,
    cut_id: u64,
    label: usize,
) -> Result<Arc<PlayerModelAsset>, String> {
    let tag = format!("{}#dyncut{cut_id}#{label}", base.key);
    let mut hierarchy = base.glm.hierarchy.clone();
    let mut lod_surfaces = Vec::with_capacity(surfaces.len() + 1);
    let mut assets = Vec::with_capacity(surfaces.len() + 1);
    for surface in surfaces {
        let Some(template) = base.surfaces.iter().find(|a| a.surface_index == surface.surface_index) else {
            continue;
        };
        let two_sided = template
            .gpu_meshes
            .iter()
            .flatten()
            .next()
            .is_some_and(|mesh| mesh.base_key.contains("#twosided"));
        let mesh = build_gpu_mesh_source(&tag, 0, &surface, two_sided, None, JiggleRemeshTuning::default(), None)?;
        assets.push(cut_surface_asset(template, mesh));
        lod_surfaces.push(surface);
    }
    if let Some(mut cap) = cap {
        // SkinGore's gore tile; without its textures use the cut skin itself.
        let cap_texture = cap_texture.or_else(|| assets.iter().find_map(|a| a.texture.clone()));
        let index = hierarchy.len();
        hierarchy.push(GlmSurfaceHierarchy {
            name: format!("dyncut_cap{cut_id}"),
            flags: 0,
            shader: String::new(),
            parent_index: -1,
            children: Vec::new(),
        });
        cap.surface_index = index;
        // Two-sided: a cap must never vanish to back-face culling.
        let mesh = build_gpu_mesh_source(&tag, 0, &cap, true, None, JiggleRemeshTuning::default(), None)?;
        assets.push(PlayerSurfaceAsset {
            surface_index: index,
            default_visible: true,
            surface_name: format!("dyncut_cap{cut_id}"),
            texture: cap_texture,
            animation: None,
            alpha_mode: DynamicModelAlphaMode::Opaque,
            authored_glow: false,
            shader_time_offset: 0.0,
            shader_clamp_time: None,
            fallback_gray: false,
            entity_tint: false,
            overlay: None,
            stages: Vec::new(),
            gpu_meshes: vec![Some(mesh)],
            source_material: None,
        });
        lod_surfaces.push(cap);
    }
    if lod_surfaces.is_empty() {
        return Err(format!("{tag} has no surfaces"));
    }
    let glm = GlmModel {
        name: base.glm.name.clone(),
        anim_name: base.glm.anim_name.clone(),
        num_bones: base.glm.num_bones,
        hierarchy,
        lods: vec![GlmLod { surfaces: lod_surfaces }],
    };
    Ok(Arc::new(PlayerModelAsset {
        body_shape_available: false,
        body_shape_binding: None,
        key: tag,
        glm: Arc::new(glm),
        gla: Arc::clone(&base.gla),
        animation_gla: Arc::clone(&base.animation_gla),
        retargeter: base.retargeter.clone(),
        source_native_pivots: base.source_native_pivots.clone(),
        source_imported: base.source_imported,
        source_jiggles: base.source_jiggles.clone(),
        source_jiggles_enabled: false,
        emote_contacts: std::sync::OnceLock::new(),
        humanoid: base.humanoid,
        animations: base.animations.clone(),
        anim_events: base.anim_events.clone(),
        surfaces: assets,
        jiggle: None,
        female: base.female,
        idle_effects: Vec::new(),
    }))
}

fn cut_surface_asset(template: &PlayerSurfaceAsset, mesh: Arc<Ghoul2GpuMeshSource>) -> PlayerSurfaceAsset {
    let mut asset = template.clone();
    asset.default_visible = true;
    asset.gpu_meshes = vec![Some(mesh)];
    asset
}

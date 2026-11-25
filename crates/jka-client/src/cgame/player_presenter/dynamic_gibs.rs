//! cg_blood 3: client-only, rigid fragments of the actual presented Ghoul2 body.
//! The sever topology/caps come from the existing OpenJK dismemberment port;
//! motion, retention and blood stay in the existing LE_FRAGMENT/Rapier path.
use super::{Arc, DynamicModelSurface, HashSet, Matrix3x4, PlayerModelAsset, PlayerPresenter,
    skin_surface_timed, transform_jka_model_point};
use super::bodies::dismemberment::{
    DismemberPart, collect_surface_subtree, default_surface_set, hierarchy_surface_index,
    resolve_dismember_root, source_dismember_surface_set, variant_cap_name,
    presentation_matrix, presentation_axis_origin,
};
use crate::cgame::weapon_fx::{ChunkModel, ChunkSpec, DynamicGibRequest, dynamic_gib_specs};

const PARTS: [DismemberPart; 7] = [
    DismemberPart::Head, DismemberPart::Waist, DismemberPart::LeftArm,
    DismemberPart::RightArm, DismemberPart::RightHand, DismemberPart::LeftLeg,
    DismemberPart::RightLeg,
];
const SOURCE_MAX_AGE_MS: i32 = 1000;

// A snapshot survives a hidden/gibbed victim and is shared by all its pieces.
// The skin and LOD resources are Arc-owned; only the bone pose is copied.
pub(super) struct GibSource {
    captured_at: i32,
    entity_origin: [f32; 3],
    model: Arc<PlayerModelAsset>,
    pose: Arc<Vec<Matrix3x4>>,
    axis: [[f32; 3]; 3],
    origin: [f32; 3],
    rgb: [f32; 3],
    already_cut: Option<HashSet<usize>>,
}

struct GibVisual {
    model: Arc<PlayerModelAsset>,
    pose: Arc<Vec<Matrix3x4>>,
    surfaces: HashSet<usize>,
    // The frozen model-to-piece transform. The piece's initial rigid frame
    // is identity rotation at the actual posed fragment's bounding centre.
    relative: Matrix3x4,
    rgb: [f32; 3],
    half_extents: [f32; 3],
    label: String,
}

/// Typed, shared payload: the MD3 VFS is never asked to load a fake qpath.
/// Both the legacy chunk and the promoted Rapier body own this same payload.
#[derive(Clone)]
pub(crate) struct DynamicGib(Arc<GibVisual>);

impl std::fmt::Debug for DynamicGib {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("DynamicGib").field(&self.0.label).finish()
    }
}
impl PartialEq for DynamicGib {
    fn eq(&self, other: &Self) -> bool { Arc::ptr_eq(&self.0, &other.0) }
}
impl DynamicGib {
    pub(crate) fn half_extents(&self) -> [f32; 3] { self.0.half_extents }
}

struct PieceSurfaces {
    part: Option<DismemberPart>,
    root: Option<usize>,
    subtree: HashSet<usize>,
    visible: HashSet<usize>,
}

/// Partition every currently visible body surface exactly once. A nested
/// sever (hand inside arm, head/arms inside torso) belongs to the deepest
/// severable root, never to both the ancestor and the descendant. Keep the
/// remaining hips/root geometry too. Only authored caps are enabled.
fn partition(model: &PlayerModelAsset, visible: &HashSet<usize>) -> Vec<PieceSurfaces> {
    let mut pieces = Vec::new();
    for part in PARTS {
        let spec = part.spec();
        let Some(root) = resolve_dismember_root(&model.glm, spec.limb_root) else { continue; };
        let mut subtree = HashSet::new();
        collect_surface_subtree(&model.glm, root, &mut subtree);
        pieces.push(PieceSurfaces { part: Some(part), root: Some(root), subtree, visible: HashSet::new() });
    }
    // Descendants have a strictly smaller subtree in the GLM hierarchy.
    pieces.sort_by_key(|piece| piece.subtree.len());
    let mut remaining = visible.clone();
    for piece in &mut pieces {
        piece.visible = remaining.intersection(&piece.subtree).copied().collect();
        remaining.retain(|index| !piece.visible.contains(index));
    }
    // If the model has no supported sever root, do not fling a whole player
    // or substitute stock MD3s. There is no authored dismemberment to reuse.
    if !pieces.iter().any(|piece| !piece.visible.is_empty()) { return Vec::new(); }
    pieces.push(PieceSurfaces { part: None, root: None, subtree: HashSet::new(), visible: remaining });
    let drawable: HashSet<usize> = model.surfaces.iter().map(|s| s.surface_index).collect();
    let mut caps = Vec::new();
    for (index, piece) in pieces.iter().enumerate() {
        let (Some(part), Some(root)) = (piece.part, piece.root) else { continue; };
        if piece.visible.is_empty() { continue; }
        let spec = part.spec();
        let cap = variant_cap_name(&model.glm, root, spec.limb_root, spec.limb_tag);
        if let Some(cap) = hierarchy_surface_index(&model.glm, &cap).filter(|id| drawable.contains(id)) {
            caps.push((index, cap));
        }
        let Some(stub) = resolve_dismember_root(&model.glm, spec.stub_root) else { continue; };
        let stub_owner = pieces.iter().position(|other| other.subtree.contains(&stub))
            .unwrap_or(pieces.len() - 1);
        if pieces[stub_owner].visible.is_empty() { continue; }
        let cap = variant_cap_name(&model.glm, stub, spec.stub_root, spec.stub_tag);
        if let Some(cap) = hierarchy_surface_index(&model.glm, &cap).filter(|id| drawable.contains(id)) {
            caps.push((stub_owner, cap));
        }
    }
    // A cap already visible from a prior server sever must still have only
    // one owner. Never include the same mesh surface twice in a burst.
    for (owner, cap) in caps {
        for piece in &mut pieces { piece.visible.remove(&cap); }
        pieces[owner].visible.insert(cap);
    }
    pieces.retain(|piece| !piece.visible.is_empty());
    pieces
}

impl PlayerPresenter {
    pub(crate) fn set_dynamic_gibs_enabled(&mut self, enabled: bool) {
        self.dynamic_gibs_enabled = enabled && self.assets.jk2_version().is_none();
        if !self.dynamic_gibs_enabled { self.dynamic_gib_sources.clear(); }
    }

    pub(super) fn retain_dynamic_gib_sources(&mut self, now: i32) {
        if self.dynamic_gibs_enabled {
            self.dynamic_gib_sources.retain(|_, source| {
                (0..=SOURCE_MAX_AGE_MS).contains(&now.saturating_sub(source.captured_at))
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn capture_dynamic_gib_source(&mut self, entity: u16, time: i32,
        entity_origin: [f32; 3], model: Arc<PlayerModelAsset>, pose: &[Matrix3x4],
        axis: [[f32; 3]; 3], origin: [f32; 3], rgb: [f32; 3],
    ) {
        let already_cut = source_dismember_surface_set(&model, self.dismembered.get(&entity));
        self.dynamic_gib_sources.insert(entity, GibSource {
            captured_at: time, entity_origin, model, pose: Arc::new(pose.to_vec()), axis,
            origin, rgb, already_cut,
        });
    }

    pub(crate) fn build_dynamic_gibs(&mut self, request: DynamicGibRequest, now: i32) -> Option<Vec<ChunkSpec>> {
        if !self.dynamic_gibs_enabled { return Some(Vec::new()); }
        let source = self.dynamic_gib_sources.get(&request.entity)?;
        if !(0..=SOURCE_MAX_AGE_MS).contains(&now.saturating_sub(source.captured_at)) { return None; }
        // The first frame after a gib can hide its victim before CG_Player
        // submits geometry. Use the last real pose, shifted to the event's
        // origin, rather than constructing a default/standing mannequin.
        let model = Arc::clone(&source.model);
        let pose = Arc::clone(&source.pose);
        let axis = source.axis;
        let origin = std::array::from_fn(|i| source.origin[i] + request.origin[i] - source.entity_origin[i]);
        let rgb = source.rgb;
        let visible = source.already_cut.clone().unwrap_or_else(|| default_surface_set(&model));
        let pieces = partition(&model, &visible);
        let Some(lod) = model.glm.lods.first() else { return Some(Vec::new()); };
        let mut result = Vec::with_capacity(pieces.len());
        for (index, piece) in pieces.into_iter().enumerate() {
            // Compute a cheap collision proxy directly from the real posed
            // vertices once, not from the stock MD3 gib radius. Rendering
            // still uses the original skinned mesh, skin and shader stages.
            let mut lo = [f32::INFINITY; 3];
            let mut hi = [f32::NEG_INFINITY; 3];
            for surface in lod.surfaces.iter().filter(|s| piece.visible.contains(&s.surface_index)) {
                let Ok(skinned) = skin_surface_timed(&mut self.perf, surface, pose.as_slice()) else { continue; };
                for vertex in skinned.vertices {
                    let p = transform_jka_model_point(vertex.position, axis, origin);
                    for i in 0..3 { lo[i] = lo[i].min(p[i]); hi[i] = hi[i].max(p[i]); }
                }
            }
            if !lo.iter().chain(hi.iter()).all(|v| v.is_finite()) { continue; }
            let centre = std::array::from_fn(|i| (lo[i] + hi[i]) * 0.5);
            let half_extents = std::array::from_fn(|i| ((hi[i] - lo[i]) * 0.5).max(0.25));
            let relative = presentation_matrix(axis, std::array::from_fn(|i| origin[i] - centre[i]));
            let label = format!("{}#gib-{}-{}-{}-{}", model.key, request.entity, request.time, request.seed, index);
            result.push((centre, DynamicGib(Arc::new(GibVisual {
                model: Arc::clone(&model), pose: Arc::clone(&pose), surfaces: piece.visible,
                relative, rgb, half_extents, label,
            }))));
        }
        if result.is_empty() {
            devprintln!(1, "DYNAMIC GIB: {} has no drawable authored sever pieces", model.key);
        } else {
            devprintln!(1, "DYNAMIC GIB: {} -> {} model fragments", model.key, result.len());
        }
        Some(dynamic_gib_specs(result, request.seed))
    }

    pub(crate) fn present_dynamic_gib(&mut self, piece: &DynamicGib, chunk: &ChunkModel)
        -> Result<Vec<DynamicModelSurface>, String>
    {
        let visual = &piece.0;
        let body = presentation_matrix(chunk.axis, chunk.origin);
        let world = super::multiply_3x4(&body, &visual.relative);
        let (axis, origin) = presentation_axis_origin(&world);
        let mut draws = self.render_glm_surfaces_tinted_selected(
            1023, &visual.label, &visual.model.glm, &visual.model.gla,
            &visual.model.surfaces, None, visual.pose.as_slice(), 0, axis, origin,
            [1.0, 1.0, 1.0, chunk.alpha], visual.rgb, None, true, Some(&visual.surfaces), true,
        )?;
        for draw in &mut draws {
            draw.lighting_origin = Some(chunk.origin);
            // RT skinned BLAS identity includes entity_num; local fragments
            // share the world entity, so distinguish independent copies of
            // a mesh (e.g. two victims using the same player skin).
            if let Some(key) = &mut draw.rt_skinned_key {
                *key = Arc::from(format!("{}#{}", key, visual.label));
            }
        }
        Ok(draws)
    }
}

//! Lightweight secondary-motion deformation for stock `_humanoid.gla` player meshes.
//!
//! Stock JKA's 53-bone humanoid has no dedicated breast/soft-tissue bones, so this
//! intentionally works after Ghoul2 skinning. `_humanoid` models are classified
//! automatically from Raven helper surfaces or Source-converted joint pivots,
//! plus mesh normals and real GLM skin weights (per-model `.jiggle` files are no
//! longer supported). Masks and spring
//! anchors are compiled once at model registration; runtime
//! work is then only a handful of damped points plus masked offsets.

use jka_assets::ghoul2::{
    transform_point, GlaAnimation, Ghoul2SkinnedSurface, GlmModel, GlmSurface, GlmVertex,
    Matrix3x4,
};
use std::collections::HashMap;

mod glute_shape;
mod tickrate;
pub(crate) use glute_shape::GluteShape;

const RESET_DT: f32 = 0.100;
const TELEPORT_DISTANCE: f32 = 96.0;
/// Reference tick rate for the locomotion impulse cvar (JKA units at 60 Hz).
const IMPULSE_REFERENCE_HZ: f32 = 60.0;
/// Swirl turn rate at r_jiggleSwirl 1, rad/s: about the ~3 Hz natural
/// frequency of the auto profile, where straight bounces open into loops.
const SWIRL_RATE: f32 = 18.0;
/// Vertical inset (JKA units, each end) of the breast size footprint inside
/// the motion mask.
const BREAST_SIZE_BOTTOM_INSET: f32 = 1.0;
const BREAST_SIZE_TOP_INSET: f32 = 0.3;
/// Size inflation is zero within GAP |x| of the body midline (JKA units) and
/// full at FULL. Breasts nearly meet at the base; glutes keep a cleft.
const BREAST_MIDLINE_GAP: f32 = 0.0;
const BREAST_MIDLINE_FULL: f32 = 0.9;
const GLUTE_MIDLINE_GAP: f32 = 0.35;
const GLUTE_MIDLINE_FULL: f32 = 1.9;

/// Breast size (positive) is a shrink-wrap onto a per-side "egg": a piecewise
/// ellipsoid (tighter lower half, longer upper half sheared back into the
/// chest) that grows, projects and drops with the slider. Chest vertices
/// inside it move outward along rays from a point inside the chest until they
/// reach its surface. Tuned against reference profiles with the offline
/// ortho preview (jedi_hf, size +3, "candidate J").
struct BreastEgg;
impl BreastEgg {
    /// Ray origin offsets from the region center: behind (+Y) / above (+Z).
    const ORIGIN_BACK: f32 = 2.0;
    const ORIGIN_UP: f32 = 0.5;
    /// Egg center: x = region x * OUT, y = region y - (FWD0 + FWD*s),
    /// z = region z - (DROP0 + DROP*s).
    const OUT: f32 = 1.10;
    const FWD0: f32 = -0.5;
    const FWD: f32 = 0.55;
    const DROP0: f32 = 0.1;
    const DROP: f32 = 0.15;
    /// Radii = base + per-size-unit growth: lateral, depth (projection),
    /// lower, upper. ("candidate L": J plus +0.5 projection, -0.2 width at +3.)
    const RX: (f32, f32) = (2.6, 0.38);
    const RY: (f32, f32) = (2.2, 0.717);
    const RZ_LOWER: (f32, f32) = (2.6, 0.50);
    const RZ_UPPER: (f32, f32) = (3.6, 0.70);
    /// Upper half leans back into the chest by this much depth per unit up.
    const SHEAR_UP: f32 = 0.6;
    /// The wrap fades out from this normalized height of the upper half to
    /// the egg top, so the upper slope blends into the chest (no lip where
    /// the egg tip meets the upper chest).
    const TOP_FADE: f32 = 0.65;
    /// Vertices never cross to within this |x| of the midline.
    const MIDLINE: f32 = 0.05;
    /// Size below which the wrap fades in, so tiny sizes don't jump.
    const FADE_IN: f32 = 0.75;
    /// Subtle nipple: Gaussian bump at the egg's forward tip, which points
    /// slightly outward and down. Added after smoothing.
    const NIPPLE_HEIGHT: f32 = 0.2;
    const NIPPLE_RADIUS: f32 = 0.55;
    const TIP_OUT: f32 = 0.25;
    const TIP_DOWN: f32 = 0.15;
}

/// One side's egg for a given breast size.
struct EggShape {
    side: RegionSide,
    center: [f32; 3],
    origin: [f32; 3],
    rx: f32,
    ry: f32,
    rz_lower: f32,
    rz_upper: f32,
}

impl EggShape {
    fn new(region: &RegionSpec, size: f32) -> Self {
        let c = region.center;
        let grow = |(base, per_unit): (f32, f32)| base + per_unit * size;
        Self {
            side: region.side,
            center: [
                c[0] * BreastEgg::OUT,
                c[1] - (BreastEgg::FWD0 + BreastEgg::FWD * size),
                c[2] - (BreastEgg::DROP0 + BreastEgg::DROP * size),
            ],
            origin: [c[0], c[1] + BreastEgg::ORIGIN_BACK, c[2] + BreastEgg::ORIGIN_UP],
            rx: grow(BreastEgg::RX),
            ry: grow(BreastEgg::RY),
            rz_lower: grow(BreastEgg::RZ_LOWER),
            rz_upper: grow(BreastEgg::RZ_UPPER),
        }
    }

    fn contains(&self, point: [f32; 3]) -> bool {
        let d = sub(point, self.center);
        let (rz, shear) = if d[2] > 0.0 {
            (self.rz_upper, BreastEgg::SHEAR_UP)
        } else {
            (self.rz_lower, 0.0)
        };
        let dy = d[1] - shear * d[2].abs();
        (d[0] / self.rx).powi(2) + (dy / self.ry).powi(2) + (d[2] / rz).powi(2) < 1.0
    }
}

/// Exit point of the ray from `origin` through `position` out of a convex
/// shape that contains `position`: bracket, then bisect.
fn ray_exit(position: [f32; 3], origin: [f32; 3], contains: impl Fn([f32; 3]) -> bool) -> Option<[f32; 3]> {
    let ray = sub(position, origin);
    let ray_length = length(ray);
    if ray_length <= 0.0001 {
        return None;
    }
    let ray = mul(ray, 1.0 / ray_length);
    let (mut lo, mut hi) = (0.0f32, 0.25f32);
    while contains(add(position, mul(ray, hi))) && hi < 64.0 {
        lo = hi;
        hi *= 2.0;
    }
    for _ in 0..24 {
        let mid = (lo + hi) * 0.5;
        if contains(add(position, mul(ray, mid))) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some(add(position, mul(ray, hi)))
}
/// Dinurdo extension (not KawaiiPhysics): per positive size unit, the region
/// stiffness input is divided by (1 + STIFFNESS * size) and the travel limit
/// grows by (1 + TRAVEL * size). At +3: stiffness / 1.3, travel x1.45.
const SIZE_STIFFNESS_PER_UNIT: f32 = 0.10;
const SIZE_TRAVEL_PER_UNIT: f32 = 0.15;
/// Glute region center height above the rear-protrusion apex (JKA units).
const GLUTE_APEX_CENTER_OFFSET: f32 = 0.8;
/// Samples within this depth of the most-protruding one define the apex.
const PROTRUSION_APEX_BAND: f32 = 0.75;
pub(crate) const MAX_GPU_JIGGLE_REGIONS: usize = 4;
const NO_GPU_JIGGLE_REGION: u32 = u32::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JiggleSolver {
    KawaiiPhysics,
    JigglePhysics,
}

impl JiggleSolver {
    pub(crate) fn from_u8(value: u8) -> Self {
        if value == 1 { Self::JigglePhysics } else { Self::KawaiiPhysics }
    }
}

/// Live user tuning layered on top of the per-model/auto profile. KawaiiPhysics
/// keeps its damping/stiffness multipliers; JigglePhysics exposes the upstream
/// one-child Motionless Root controls separately so the two solvers stay A/B-able.
#[derive(Debug, Clone, Copy)]
pub(crate) struct JiggleTuning {
    pub solver: JiggleSolver,
    pub overall_strength: f32,
    pub breast_strength: f32,
    pub glute_strength: f32,
    pub stiffness_scale: f32,
    pub damping_scale: f32,
    /// Signed model-space Z offsets (JKA units) applied only to the glute motion
    /// envelope. Bottom + raises the lower edge; top - lowers the upper edge.
    pub glute_jiggle_bottom_z: f32,
    pub glute_jiggle_top_z: f32,
    /// naelstrof/JigglePhysics parameters. These follow the source's authorable
    /// 0..1 controls for a one-child rig with Motionless Root, which is the
    /// source's recommended setup for breasts.
    pub jp_stiffness: f32,
    pub jp_drag: f32,
    pub jp_air_drag: f32,
    pub jp_stretch: f32,
    pub jp_soften: f32,
    pub jp_gravity: f32,
    /// How strongly the entity's own origin acceleration (start/stop, jump,
    /// landing, direction change) kicks the tissue. Constant-velocity travel
    /// never contributes at any setting.
    pub locomotion: f32,
    /// Dinurdo input extension: maximum locomotion kick magnitude per reference 60 Hz
    /// step. Scales with dt to preserve the maximum velocity impulse.
    /// Zero removes locomotion impulses while animated secondary motion stays.
    pub impulse_cap: f32,
    /// Gyroscopic coupling that bends the anchor-relative velocity around the
    /// model depth axis, mirrored per side, turning straight bounces into
    /// elliptical/circular orbits without adding energy.
    pub swirl: f32,
    /// KawaiiPhysics per-region multipliers, stacked on stiffness_scale /
    /// damping_scale.
    pub breast_stiffness: f32,
    pub breast_damping: f32,
    pub glute_stiffness: f32,
    pub glute_damping: f32,
    /// Size sliders (same values as JiggleRemeshTuning); see
    /// SIZE_STIFFNESS_PER_UNIT / SIZE_TRAVEL_PER_UNIT.
    pub breast_size: f32,
    pub glute_size: f32,
}


#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct JiggleRemeshTuning {
    /// Anny v1 shape recipe, applied once before anatomical fitting/promotion.
    pub body_shape: jka_assets::body_shape::BodyShapeParams,
    /// Signed model-space Z offsets in JKA units from the auto-fitted remesh bounds.
    pub breast_bottom_z: f32,
    pub breast_top_z: f32,
    pub glute_bottom_z: f32,
    pub glute_top_z: f32,
    /// Peak bind-pose mass added (+) or removed (-) at the region center, in
    /// JKA units. Baked into the cached promoted mesh.
    pub breast_size: f32,
    pub glute_size: f32,
}

impl Default for JiggleRemeshTuning {
    fn default() -> Self {
        Self {
            body_shape: Default::default(),
            breast_bottom_z: 0.0,
            breast_top_z: 0.0,
            glute_bottom_z: 0.0,
            glute_top_z: 0.0,
            breast_size: 0.0,
            glute_size: 0.0,
        }
    }
}

impl JiggleRemeshTuning {
    pub(crate) fn clamped(self) -> Self {
        Self {
            body_shape: self.body_shape.clamped(),
            breast_bottom_z: self.breast_bottom_z.clamp(-8.0, 8.0),
            breast_top_z: self.breast_top_z.clamp(-8.0, 8.0),
            glute_bottom_z: self.glute_bottom_z.clamp(-8.0, 8.0),
            glute_top_z: self.glute_top_z.clamp(-8.0, 8.0),
            breast_size: self.breast_size.clamp(-2.0, 3.0),
            glute_size: self.glute_size.clamp(-2.0, 4.0),
        }
    }
}
impl Default for JiggleTuning {
    fn default() -> Self {
        Self {
            solver: JiggleSolver::KawaiiPhysics,
            overall_strength: 1.0,
            breast_strength: 1.0,
            glute_strength: 1.0,
            stiffness_scale: 1.0,
            damping_scale: 1.0,
            glute_jiggle_bottom_z: 0.0,
            glute_jiggle_top_z: 0.0,
            jp_stiffness: 0.4,
            jp_drag: 0.4,
            jp_air_drag: 0.1,
            jp_stretch: 0.8,
            jp_soften: 0.0,
            jp_gravity: 0.0,
            locomotion: 1.0,
            impulse_cap: 0.5,
            swirl: 0.35,
            breast_stiffness: 1.0,
            breast_damping: 1.0,
            glute_stiffness: 1.0,
            glute_damping: 1.0,
            breast_size: 0.0,
            glute_size: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegionSide {
    NegativeX,
    PositiveX,
}

#[derive(Debug, Clone, Copy)]
struct AnchorTerm {
    bone_index: usize,
    weight: f32,
    point: [f32; 3],
}

#[derive(Debug, Clone)]
struct SemanticMask {
    /// Global `_humanoid.gla` bone indices that are allowed to contribute to the mask.
    allowed_bones: Vec<usize>,
    /// -1 = model-forward (-Y), +1 = model-rear (+Y).
    facing_y: f32,
}

#[derive(Debug, Clone)]
struct RegionSpec {
    name: String,
    surfaces: Vec<String>,
    anchor_terms: Vec<AnchorTerm>,
    /// Parent/root for the naelstrof/JigglePhysics one-child virtual rig
    /// (thoracic/pelvis); falls back to the dominant anchor bone after mask
    /// compilation if the skeleton lacks it.
    solver_root_bone: Option<usize>,
    center: [f32; 3],
    radius: [f32; 3],
    response: [f32; 3],
    stiffness: f32,
    damping: f32,
    max_offset: f32,
    strength: f32,
    side: RegionSide,
    semantic: SemanticMask,
}

#[derive(Debug, Clone)]
struct SurfaceMask {
    /// One smooth weight array per region. Arrays are the exact GLM vertex count.
    region_weights: Vec<Vec<f32>>,
    /// GPU skinning stores the strongest region per vertex. Auto regions are
    /// deliberately non-overlapping; any overlap keeps exact multi-region
    /// behavior on the CPU skinning path.
    dominant_region: Vec<u32>,
    dominant_weight: Vec<f32>,
    /// Bind-pose vertical coordinate normalized by the dominant region's
    /// center/radius. GPU shaders use this for live glute-height attenuation.
    dominant_coord: Vec<f32>,
}

fn surface_boundary_vertices(surface: &GlmSurface) -> Vec<bool> {
    let mut edge_users = HashMap::<(u32, u32), u32>::new();
    for triangle in &surface.triangles {
        for (a, b) in [
            (triangle[0], triangle[1]),
            (triangle[1], triangle[2]),
            (triangle[2], triangle[0]),
        ] {
            if a != b {
                *edge_users.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
    }
    let mut boundary = vec![false; surface.vertices.len()];
    for ((a, b), users) in edge_users {
        if users == 1 {
            if let Some(vertex) = boundary.get_mut(a as usize) {
                *vertex = true;
            }
            if let Some(vertex) = boundary.get_mut(b as usize) {
                *vertex = true;
            }
        }
    }
    boundary
}

/// Where a `_humanoid` model gets its anatomical search landmarks.
/// The soft-tissue fit and its physics are identical for both variants.
#[derive(Clone, Copy)]
enum AnatomyReference<'a> {
    Raven,
    Source { native_pivots: Option<&'a [[f32; 3]]> },
}

#[derive(Debug, Clone)]
pub(crate) struct JiggleProfile {
    regions: Vec<RegionSpec>,
    /// LOD -> surface index -> compiled region masks.
    lod_masks: Vec<HashMap<usize, SurfaceMask>>,
    /// The compact GPU vertex payload stores one region per vertex. Profiles
    /// with overlapping regions retain exact behavior through the CPU fallback.
    gpu_compatible: bool,
    /// Source rebinding can merge arms, hands, and hips into one GLM surface.
    /// Keep stock JKA's authored surface-specific size shaping unchanged.
    source_anatomy: bool,
}

impl JiggleProfile {
    #[cfg(test)]
    pub(crate) fn source_size_test_profile() -> Self {
        let mut profile = Self::worker_test_profile();
        profile.source_anatomy = true;
        let region = &mut profile.regions[0];
        region.name = "glute_left".into();
        region.semantic.allowed_bones = vec![0]; // synthetic pelvis
        region.center = [0.0, 3.22789141, 37.4822428 + GLUTE_APEX_CENTER_OFFSET];
        region.radius = [4.74686644, 1.0, 6.4];
        profile
    }

    #[cfg(test)]
    pub(crate) fn worker_test_profile() -> Self {
        Self {
            regions: vec![RegionSpec {
                name: "breast".into(), surfaces: vec![],
                anchor_terms: vec![AnchorTerm { bone_index: 0, weight: 1.0, point: [1.0, 0.0, 0.0] }],
                solver_root_bone: Some(0), center: [1.0, 0.0, 0.0], radius: [1.0; 3],
                response: [1.0; 3], stiffness: 0.2, damping: 0.1, max_offset: 4.0,
                strength: 1.0, side: RegionSide::PositiveX,
                semantic: SemanticMask { allowed_bones: vec![0], facing_y: -1.0 },
            }],
            lod_masks: vec![], gpu_compatible: true, source_anatomy: false,
        }
    }

    /// Build the stock `_humanoid` profile from Raven's chest/hip helpers.
    pub(crate) fn auto_humanoid(
        glm: &GlmModel,
        gla: &GlaAnimation,
    ) -> Result<Option<Self>, String> {
        Self::auto_anatomical(glm, gla, AnatomyReference::Raven)
    }

    /// Source meshes have the same rebound `_humanoid` skin weights but no
    /// Raven anatomical helper surfaces. Use their converted joint pivots and
    /// all non-bolt meshes for *the same* fitting and mask compilation path.
    /// No Source jiggle bones are required or synthesized.
    pub(crate) fn auto_source_humanoid(
        glm: &GlmModel,
        gla: &GlaAnimation,
        native_pivots: Option<&[[f32; 3]]>,
    ) -> Result<Option<Self>, String> {
        Self::auto_anatomical(glm, gla, AnatomyReference::Source { native_pivots })
    }

    /// Common anatomical fitting for stock JKA and Source-rebound GLMs.
    /// Only mesh selection and search landmarks differ; semantic weights,
    /// region physics, symmetry, masks and spring anchors stay shared.
    fn auto_anatomical(
        glm: &GlmModel,
        gla: &GlaAnimation,
        reference: AnatomyReference<'_>,
    ) -> Result<Option<Self>, String> {
        if !glm.anim_name.to_ascii_lowercase().contains("_humanoid") {
            return Ok(None);
        }
        let (chest_surfaces, hip_surfaces) = match reference {
            AnatomyReference::Raven => (surface_family(glm, "torso"), surface_family(glm, "hips")),
            AnatomyReference::Source { .. } => {
                // Generated Source bodypart names are arbitrary. Skin weights,
                // facing normals and search height reject other anatomy.
                // Never deform borrowed JKA attachment/bolt surfaces.
                let surfaces = glm.hierarchy.iter()
                    .filter(|surface| !surface.name.starts_with('*'))
                    .map(|surface| surface.name.to_ascii_lowercase())
                    .collect::<Vec<_>>();
                (surfaces.clone(), surfaces)
            }
        };
        if chest_surfaces.is_empty() && hip_surfaces.is_empty() {
            return Ok(None);
        }
        if matches!(reference, AnatomyReference::Source { .. }) && glm.lods.is_empty() {
            return Ok(None);
        }

        let lower_lumbar = find_bone(gla, "lower_lumbar");
        let upper_lumbar = find_bone(gla, "upper_lumbar");
        let thoracic = find_bone(gla, "thoracic");
        let pelvis = find_bone(gla, "pelvis");
        let lclavical = find_bone(gla, "lclavical");
        let rclavical = find_bone(gla, "rclavical");
        let lfemur_yz = find_bone(gla, "lfemurYZ");
        let lfemur_x = find_bone(gla, "lfemurX");
        let rfemur_yz = find_bone(gla, "rfemurYZ");
        let rfemur_x = find_bone(gla, "rfemurX");

        // Source detection needs a complete central spine to place search
        // windows. Raven profiles retain partial-region fallback semantics.
        let source_heights = match reference {
            AnatomyReference::Raven => None,
            AnatomyReference::Source { native_pivots } => {
                let (Some(_), Some(upper), Some(chest), Some(hips)) =
                    (lower_lumbar, upper_lumbar, thoracic, pelvis) else {
                        return Ok(None);
                    };
                let heights = (
                    source_joint_z(gla, native_pivots, upper),
                    source_joint_z(gla, native_pivots, chest),
                    source_joint_z(gla, native_pivots, hips),
                );
                if !heights.0.is_finite() || !heights.1.is_finite() || !heights.2.is_finite() {
                    return Ok(None);
                }
                Some(heights)
            }
        };

        let mut regions = Vec::with_capacity(MAX_GPU_JIGGLE_REGIONS);
        if let (Some(lower_lumbar), Some(upper_lumbar), Some(thoracic)) =
            (lower_lumbar, upper_lumbar, thoracic)
        {
            for (name, side, lower_marker, upper_marker, clavicle) in [
                ("breast_left", RegionSide::PositiveX, "*lchest_l", "*uchest_l", lclavical),
                ("breast_right", RegionSide::NegativeX, "*lchest_r", "*uchest_r", rclavical),
            ] {
                let Some(clavicle) = clavicle else { continue };
                // Only the reference points and (stock-tuned) footprint caps
                // differ. The actual flesh fitting is exactly the same.
                let (z_min, z_max, lateral, max_radius) = match reference {
                    AnatomyReference::Raven => {
                        let (Some(lower), Some(upper)) =
                            (marker_centroid(glm, lower_marker), marker_centroid(glm, upper_marker))
                        else { continue };
                        let x = (lower[0] + upper[0]) * 0.5;
                        (lower[2].min(upper[2]) - 1.5,
                         lower[2].max(upper[2]) + 2.5,
                         Some((x, (x.abs() * 1.18).clamp(3.2, 4.0))),
                         [4.8, 5.5])
                    }
                    AnatomyReference::Source { native_pivots } => {
                        let Some((lumbar_z, chest_z, pelvis_z)) = source_heights else {
                            continue;
                        };
                        let collar_z = source_joint_z(gla, native_pivots, clavicle);
                        let waist_z = (lumbar_z + pelvis_z) * 0.5;
                        let z_min = (lumbar_z.min(chest_z) - 2.0).max(waist_z + 0.5);
                        let z_max = collar_z.max(chest_z) + 2.0;
                        if z_max <= z_min + 1.0 { continue }
                        (z_min, z_max, None, [5.0, 6.5])
                    }
                };
                let semantic = SemanticMask {
                    allowed_bones: vec![lower_lumbar, upper_lumbar, thoracic, clavicle],
                    facing_y: -1.0,
                };
                if let Some((mut center, mut radius, _)) = fit_semantic_region(
                    glm, &chest_surfaces, side, &semantic, z_min, z_max,
                    [2.8, 3.8], max_radius,
                ) {
                    if let Some((x, rx)) = lateral {
                        center[0] = x;
                        radius[0] = rx;
                    }
                    regions.push(RegionSpec {
                        name: name.into(),
                        surfaces: chest_surfaces.clone(),
                        anchor_terms: Vec::new(),
                        solver_root_bone: Some(thoracic),
                        center, radius,
                        response: [0.45, 0.80, 1.00],
                        stiffness: 0.10, damping: 0.18, max_offset: 1.15,
                        strength: 1.0, side, semantic,
                    });
                }
            }
        }

        if let Some(pelvis) = pelvis {
            for (name, side, back_marker, femur_yz, femur_x) in [
                ("glute_left", RegionSide::PositiveX, "*hip_bl", lfemur_yz, lfemur_x),
                ("glute_right", RegionSide::NegativeX, "*hip_br", rfemur_yz, rfemur_x),
            ] {
                let (Some(femur_yz), Some(femur_x)) = (femur_yz, femur_x) else { continue };
                let (z_min, z_max, lateral) = match reference {
                    AnatomyReference::Raven => {
                        let Some(back) = marker_centroid(glm, back_marker) else { continue };
                        (back[2] - 11.5, back[2] + 1.2,
                         Some((back[0] * 0.85, (back[0].abs() * 1.35).clamp(4.0, 5.5))))
                    }
                    AnatomyReference::Source { native_pivots } => {
                        let Some((lumbar_z, _, pelvis_z)) = source_heights else {
                            continue;
                        };
                        let waist_z = (lumbar_z + pelvis_z) * 0.5;
                        let z_min = (pelvis_z - 11.5)
                            .min(source_joint_z(gla, native_pivots, femur_yz) - 2.0);
                        let z_max = (pelvis_z + 2.5).min(waist_z - 0.5);
                        if z_max <= z_min + 1.0 { continue }
                        (z_min, z_max, None)
                    }
                };
                let mut allowed_bones = vec![pelvis, femur_yz, femur_x];
                if let Some(lower_lumbar) = lower_lumbar {
                    allowed_bones.push(lower_lumbar);
                }
                let semantic = SemanticMask { allowed_bones, facing_y: 1.0 };
                if let Some((mut center, mut radius, apex_z)) = fit_semantic_region(
                    glm, &hip_surfaces, side, &semantic, z_min, z_max,
                    [3.4, 5.0], [6.0, 7.5],
                ) {
                    if let Some((x, rx)) = lateral {
                        center[0] = x;
                        radius[0] = rx;
                    }
                    // Use the rear-protrusion apex, not the mean of belt or
                    // garment vertices. Both model formats share this rule.
                    center[2] = apex_z.map_or(center[2] + 0.35,
                        |z| z + GLUTE_APEX_CENTER_OFFSET);
                    radius[2] = (radius[2] * 0.88).clamp(4.6, 6.4);
                    regions.push(RegionSpec {
                        name: name.into(),
                        surfaces: hip_surfaces.clone(),
                        anchor_terms: Vec::new(),
                        solver_root_bone: Some(pelvis),
                        center, radius,
                        response: [0.30, 0.80, 1.00],
                        stiffness: 0.12, damping: 0.20, max_offset: 0.95,
                        strength: 1.0, side, semantic,
                    });
                }
            }
        }

        if regions.is_empty() { return Ok(None) }
        // Raven helpers can be asymmetrical; both formats use one correction.
        symmetrize_pair(&mut regions, "breast_left", "breast_right");
        symmetrize_pair(&mut regions, "glute_left", "glute_right");

        if matches!(reference, AnatomyReference::Source { .. }) {
            // A Source bodypart may be extremely low-poly: discard regions
            // without movable interior vertices before compiling the profile.
            regions.retain(|region| {
                glm.lods.first().is_some_and(|lod| lod.surfaces.iter().any(|surface| {
                    glm.hierarchy.get(surface.surface_index).is_some_and(|entry| {
                        region.surfaces.iter().any(|name| entry.name.eq_ignore_ascii_case(name))
                    }) && {
                        let boundary = region.name.contains("glute")
                            .then(|| surface_boundary_vertices(surface));
                        surface.vertices.iter().enumerate().any(|(index, vertex)| {
                            !boundary.as_ref().is_some_and(|boundary| boundary[index])
                                && region_vertex_weight(region, surface, vertex) > 0.0001
                        })
                    }
                }))
            });
            if regions.is_empty() { return Ok(None) }
        }
        Self::compile(regions, glm, gla, matches!(reference, AnatomyReference::Source { .. })).map(Some)
    }

    fn compile(
        mut regions: Vec<RegionSpec>,
        glm: &GlmModel,
        gla: &GlaAnimation,
        source_anatomy: bool,
    ) -> Result<Self, String> {
        let mut lod_masks = Vec::with_capacity(glm.lods.len());
        let mut gpu_compatible = regions.len() <= MAX_GPU_JIGGLE_REGIONS;
        let mut region_selected = vec![false; regions.len()];
        for lod in &glm.lods {
            let mut masks = HashMap::<usize, SurfaceMask>::new();
            for surface in &lod.surfaces {
                let Some(hierarchy) = glm.hierarchy.get(surface.surface_index) else {
                    continue;
                };
                let surface_name = hierarchy.name.to_ascii_lowercase();
                let boundary = surface_boundary_vertices(surface);
                let mut region_weights = Vec::with_capacity(regions.len());
                let mut any_surface_weight = false;

                for (region_index, region) in regions.iter().enumerate() {
                    let enabled = region.surfaces.iter().any(|name| name == &surface_name);
                    let mut weights = vec![0.0f32; surface.vertices.len()];
                    if enabled {
                        for (index, vertex) in surface.vertices.iter().enumerate() {
                            // A neighbouring surface may have no glute motion. Keep
                            // their shared authored edge on the skinned position.
                            let weight = if region.name.contains("glute") && boundary[index] {
                                0.0
                            } else {
                                region_vertex_weight(region, surface, vertex)
                            };
                            weights[index] = weight;
                            if weight > 0.0001 {
                                any_surface_weight = true;
                                region_selected[region_index] = true;
                            }
                        }
                    }
                    region_weights.push(weights);
                }

                if any_surface_weight {
                    let mut dominant_region = vec![NO_GPU_JIGGLE_REGION; surface.vertices.len()];
                    let mut dominant_weight = vec![0.0f32; surface.vertices.len()];
                    let mut dominant_coord = vec![0.0f32; surface.vertices.len()];
                    for vertex_index in 0..surface.vertices.len() {
                        let mut contributors = 0usize;
                        for (region_index, weights) in region_weights.iter().enumerate() {
                            let weight = weights[vertex_index];
                            if weight > 0.0001 {
                                contributors += 1;
                            }
                            if weight > dominant_weight[vertex_index] {
                                dominant_weight[vertex_index] = weight;
                                dominant_region[vertex_index] = region_index as u32;
                                let region = &regions[region_index];
                                dominant_coord[vertex_index] = if region
                                    .name
                                    .to_ascii_lowercase()
                                    .contains("glute")
                                {
                                    (surface.vertices[vertex_index].position[2] - region.center[2])
                                        / region.radius[2].max(0.0001)
                                } else {
                                    // Chest/non-glute sentinel.  Values in [-2, 2]
                                    // are reserved for the normalized glute Z coord.
                                    4.0
                                };
                            }
                        }
                        if contributors > 1 {
                            gpu_compatible = false;
                        }
                    }
                    masks.insert(
                        surface.surface_index,
                        SurfaceMask {
                            region_weights,
                            dominant_region,
                            dominant_weight,
                            dominant_coord,
                        },
                    );
                }
            }
            lod_masks.push(masks);
        }

        if lod_masks.iter().all(|masks| masks.is_empty()) {
            return Err("jiggle profile did not select any GLM vertices".into());
        }
        if let Some((index, _)) = region_selected
            .iter()
            .enumerate()
            .find(|(_, selected)| !**selected)
        {
            return Err(format!(
                "region `{}` did not select any GLM vertices",
                regions[index].name
            ));
        }

        for region in &mut regions {
            region.anchor_terms = compile_region_anchor(glm, gla, region)?;
            if region.solver_root_bone.is_none() {
                region.solver_root_bone = region
                    .anchor_terms
                    .iter()
                    .max_by(|a, b| a.weight.total_cmp(&b.weight))
                    .map(|term| term.bone_index);
            }
        }

        Ok(Self { regions, lod_masks, gpu_compatible, source_anatomy })
    }

    pub(crate) fn uses_source_anatomy(&self) -> bool {
        self.source_anatomy
    }

    /// Source conversion may combine the entire body in one mesh. Positive size
    /// shaping is intentionally broader than the motion mask (to include thigh
    /// and cloth fades), but must NEVER reshape a nearby hand/arm just because
    /// it happens to overlap the butt/chest in bind-pose coordinates.
    ///
    /// Use the *same semantic bone sets* as the existing detector, applied to
    /// the model's global GLA skin indices. Weight-based fade avoids seams
    /// across hips/thighs while rejecting unrelated appendages entirely.
    pub(crate) fn size_skin_gate(
        &self,
        glute: bool,
        indices: &[u32; 4],
        weights: &[f32; 4],
        count: u32,
    ) -> f32 {
        if !self.source_anatomy {
            return 1.0;
        }
        let category = if glute { "glute" } else { "breast" };
        let weight = (0..count.min(4) as usize)
            .filter(|&i| self.regions.iter().any(|region| {
                region.name.contains(category)
                    && region.semantic.allowed_bones.contains(&(indices[i] as usize))
            }))
            .map(|i| weights[i].max(0.0))
            .sum::<f32>()
            .clamp(0.0, 1.0);
        smoothstep(0.08, 0.40, weight)
    }

    pub(crate) fn affects_surface(&self, lod_index: usize, surface_index: usize) -> bool {
        self.lod_masks
            .get(lod_index)
            .is_some_and(|lod| lod.contains_key(&surface_index))
    }

    /// GPU skinning keeps one dominant region per vertex so the existing
    /// 80-byte Ghoul2 vertex can reuse its padding without growing every player
    /// vertex. Four regions cover the stock breasts/glutes profile.
    pub(crate) fn gpu_vertex_binding(
        &self,
        lod_index: usize,
        surface_index: usize,
        vertex_index: usize,
    ) -> (u32, f32, f32) {
        if !self.gpu_compatible {
            return (NO_GPU_JIGGLE_REGION, 0.0, 0.0);
        }
        let Some(mask) = self
            .lod_masks
            .get(lod_index)
            .and_then(|lod| lod.get(&surface_index))
        else {
            return (NO_GPU_JIGGLE_REGION, 0.0, 0.0);
        };
        let region = mask
            .dominant_region
            .get(vertex_index)
            .copied()
            .unwrap_or(NO_GPU_JIGGLE_REGION);
        let weight = mask.dominant_weight.get(vertex_index).copied().unwrap_or(0.0);
        let coord = mask.dominant_coord.get(vertex_index).copied().unwrap_or(0.0);
        (region, weight, coord)
    }

    pub(crate) fn gpu_supported(&self) -> bool {
        self.gpu_compatible
    }

    pub(crate) fn gpu_offsets(
        &self,
        offsets: &[[f32; 3]],
        tuning: JiggleTuning,
    ) -> [[f32; 4]; MAX_GPU_JIGGLE_REGIONS] {
        let mut packed = [[0.0f32; 4]; MAX_GPU_JIGGLE_REGIONS];
        for (index, offset) in offsets.iter().take(MAX_GPU_JIGGLE_REGIONS).enumerate() {
            // Bake global/regional strength into the vector itself. This frees the
            // .w lanes for the two explicit glute-height controls without growing
            // SkinDraw or the per-draw storage buffer.
            let strength = self.region_strength(index, tuning);
            packed[index] = [
                offset[0] * strength,
                offset[1] * strength,
                offset[2] * strength,
                0.0,
            ];
        }
        let glute_count = self
            .regions
            .iter()
            .filter(|region| region.name.to_ascii_lowercase().contains("glute"))
            .count();
        let radius = if glute_count == 0 {
            1.0
        } else {
            self.regions
                .iter()
                .filter(|region| region.name.to_ascii_lowercase().contains("glute"))
                .map(|region| region.radius[2].max(0.0001))
                .sum::<f32>()
                / glute_count as f32
        };
        // GPU vertices store normalized region Z. Convert the user-facing JKA-unit
        // offsets back to normalized deltas once per draw.
        packed[0][3] = (tuning.glute_jiggle_bottom_z / radius).clamp(-1.5, 1.5);
        packed[1][3] = (tuning.glute_jiggle_top_z / radius).clamp(-1.5, 1.5);
        packed
    }

    fn region_strength(&self, region_index: usize, tuning: JiggleTuning) -> f32 {
        let regional = self.regions.get(region_index).map_or(1.0, |region| {
            if region.name.to_ascii_lowercase().contains("glute") {
                tuning.glute_strength
            } else {
                tuning.breast_strength
            }
        });
        (regional * tuning.overall_strength).clamp(0.0, 4.0)
    }

    fn glute_vertical_factor(&self, region_index: usize, coord: f32, tuning: JiggleTuning) -> f32 {
        let Some(region) = self.regions.get(region_index) else { return 1.0 };
        if !region.name.to_ascii_lowercase().contains("glute") {
            return 1.0;
        }
        let radius = region.radius[2].max(0.0001);
        let bottom = (tuning.glute_jiggle_bottom_z / radius).clamp(-1.5, 1.5);
        let top = (tuning.glute_jiggle_top_z / radius).clamp(-1.5, 1.5);

        // Preserve the old lower glute fade at zero offset, but express its
        // translation in model units. Positive bottom Z raises the entire lower
        // fade. A negative top Z adds an upper fade/cut before the fitted mask's
        // natural +radius edge; positive top Z leaves the original fitted cap.
        let lower = smoothstep(-0.70 + bottom, -0.15 + bottom, coord);
        let upper = if top < 0.0 {
            1.0 - smoothstep(0.65 + top, 1.0 + top, coord)
        } else {
            1.0
        };
        lower * upper
    }

    /// Topology-only vertical gate for promoted jiggle geometry. The cvars are
    /// signed model-space offsets in JKA units; zero reproduces the current
    /// breast ellipsoid and glute lower-trim boundary.
    pub(crate) fn remesh_vertical_factor(
        &self,
        region_index: u32,
        model_z: f32,
        tuning: JiggleRemeshTuning,
    ) -> f32 {
        let Some(region) = self.regions.get(region_index as usize) else { return 0.0 };
        let radius = region.radius[2].max(0.0001);
        let local_z = model_z - region.center[2];
        let glute = region.name.to_ascii_lowercase().contains("glute");
        let (bottom_offset, top_offset) = if glute {
            (tuning.glute_bottom_z, tuning.glute_top_z)
        } else {
            (tuning.breast_bottom_z, tuning.breast_top_z)
        };
        let lower = if glute { -0.70 * radius } else { -radius } + bottom_offset;
        let upper = radius + top_offset;
        if upper <= lower || local_z < lower || local_z > upper {
            return 0.0;
        }
        if glute {
            let full = (-0.15 * radius) + bottom_offset;
            smoothstep(lower, full.max(lower + 0.001), local_z)
        } else {
            1.0
        }
    }

    /// Bind-pose displacement for the breast/glute size sliders, peak = slider
    /// value in JKA units. The profile is a flat-topped dome over the region's
    /// X/Z ellipse (not the pointed (1-r^2)^2 mask weight, which grew a cone),
    /// pushed mostly along the surface normal with an outward lateral bias so
    /// added mass widens and rounds instead of projecting forward.
    pub(crate) fn size_displacement(
        &self,
        region_index: u32,
        weight: f32,
        position: [f32; 3],
        normal: [f32; 3],
        tuning: JiggleRemeshTuning,
    ) -> [f32; 3] {
        let Some(region) = self.regions.get(region_index as usize) else { return [0.0; 3] };
        if weight <= 0.0 {
            return [0.0; 3];
        }
        let glute = region.name.to_ascii_lowercase().contains("glute");
        let size = if glute { tuning.glute_size } else { tuning.breast_size };
        if size == 0.0 {
            return [0.0; 3];
        }
        let nx = (position[0] - region.center[0]) / region.radius[0].max(0.0001);
        // The breast motion mask reaches lower than the visible mass; inset the
        // size footprint's lower edge, keep the upper edge high so the
        // expansion starts up near the upper chest.
        let local_z = position[2] - region.center[2];
        let size_radius_z = if glute {
            region.radius[2]
        } else if local_z < 0.0 {
            region.radius[2] - BREAST_SIZE_BOTTOM_INSET
        } else {
            region.radius[2] - BREAST_SIZE_TOP_INSET
        };
        let nz = local_z / size_radius_z.max(0.0001);
        let r = (nx * nx + nz * nz).sqrt();
        if r >= 1.0 {
            return [0.0; 3];
        }
        // Broad plateau, then a zero-slope falloff: full, wide volume rather
        // than the mask's pointed (1-r^2)^2.
        let dome = 1.0 - smoothstep(0.25, 1.0, r);
        let shape = if glute {
            // Same lower trim as the motion envelope so the upper thigh does not
            // thicken, plus an upper fade before the hips/torso seam.
            smoothstep(-0.70, -0.15, nz) * (1.0 - smoothstep(0.45, 0.90, nz))
        } else {
            // Full from the center down; the upper slope eases toward the
            // clavicle without starting the bulge low.
            1.0 - 0.40 * smoothstep(0.05, 0.95, nz)
        };
        let facing_y = region.semantic.facing_y;
        // Fade out on surfaces turning away from the region facing, instead of
        // stepping to zero where the mask's facing cutoff drops them.
        let facing = smoothstep(0.03, 0.40, normal[1] * facing_y);
        // Breasts may nearly meet at the base; glutes keep a defined cleft.
        let (gap, full) = if glute {
            (GLUTE_MIDLINE_GAP, GLUTE_MIDLINE_FULL)
        } else {
            (BREAST_MIDLINE_GAP, BREAST_MIDLINE_FULL)
        };
        let medial = smoothstep(gap, full, position[0].abs());
        let amount = size * dome * shape * facing * medial;
        if amount == 0.0 {
            return [0.0; 3];
        }
        let side_sign = match region.side {
            RegionSide::PositiveX => 1.0,
            RegionSide::NegativeX => -1.0,
        };
        // Widen about the region's own center: the outer half pushes outward,
        // the inner half toward the midline (fills the cleavage gap). The
        // inward push eases off near x = 0 so the two sides can meet without
        // crossing.
        let inner = nx * side_sign < 0.0;
        let medial_ease = if inner {
            smoothstep(gap, full + 0.6, position[0].abs())
        } else {
            1.0
        };
        let lateral = nx.clamp(-1.0, 1.0) * 0.32 * medial_ease;
        // Breast mass hangs: the lower half pushes increasingly downward, and
        // the whole mass tilts further down as size grows (gravity).
        let sag = if glute {
            0.0
        } else {
            let lower = 1.0 - smoothstep(-0.5, 0.3, nz);
            -(0.10 + 0.25 * lower) - 0.05 * size.max(0.0)
        };
        // Mostly along the surface normal (inflation rounds and widens), less
        // straight-forward projection than before (that read as tubular).
        let direction = [
            normal[0] * 0.70 + lateral,
            normal[1] * 0.70 + facing_y * 0.28,
            normal[2] * 0.70 + sag,
        ];
        let direction_length = length(direction);
        if direction_length <= 0.0001 {
            return [0.0; 3];
        }
        mul(direction, amount / direction_length)
    }

    pub(crate) fn is_breast_region(&self, region_index: u32) -> bool {
        self.regions
            .get(region_index as usize)
            .is_some_and(|region| region.name.to_ascii_lowercase().contains("breast"))
    }

    /// Positive breast size: shrink-wrap `position` outward onto its side's
    /// egg (see `BreastEgg`). Applies to every chest vertex, not only the
    /// motion mask, so undersides and sides can fill; vertices outside the
    /// egg, behind the ray origin or facing backward never move.
    pub(crate) fn breast_egg_displacement(
        &self,
        position: [f32; 3],
        normal: [f32; 3],
        size: f32,
    ) -> [f32; 3] {
        let Some(egg) = self.breast_egg(position, size) else {
            return [0.0; 3];
        };
        if position[1] > egg.origin[1] || normal[1] > 0.5 || !egg.contains(position) {
            return [0.0; 3];
        }
        let Some(mut target) = ray_exit(position, egg.origin, |point| egg.contains(point)) else {
            return [0.0; 3];
        };
        // Each side may meet the midline (cleavage) but never cross it.
        target[0] = match egg.side {
            RegionSide::PositiveX => target[0].max(position[0].min(BreastEgg::MIDLINE)),
            RegionSide::NegativeX => target[0].min(position[0].max(-BreastEgg::MIDLINE)),
        };
        let height = (target[2] - egg.center[2]) / egg.rz_upper.max(0.0001);
        let top_fade = 1.0 - smoothstep(BreastEgg::TOP_FADE, 1.0, height);
        mul(
            sub(target, position),
            top_fade * smoothstep(0.0, BreastEgg::FADE_IN, size),
        )
    }

    /// Subtle nipple bump for an already wrapped position (see `BreastEgg`).
    pub(crate) fn breast_nipple_displacement(&self, position: [f32; 3], size: f32) -> [f32; 3] {
        let Some(egg) = self.breast_egg(position, size) else {
            return [0.0; 3];
        };
        let side = match egg.side {
            RegionSide::PositiveX => 1.0,
            RegionSide::NegativeX => -1.0,
        };
        let tip = [BreastEgg::TIP_OUT * side, -1.0, -BreastEgg::TIP_DOWN];
        let tip = mul(tip, 1.0 / length(tip));
        // Tip lies in the lower half (no shear), so this is on the egg surface.
        let apex = add(egg.center, [tip[0] * egg.rx, tip[1] * egg.ry, tip[2] * egg.rz_lower]);
        let outward = [tip[0] / egg.rx, tip[1] / egg.ry, tip[2] / egg.rz_lower];
        let outward = mul(outward, 1.0 / length(outward));
        let falloff = (-(distance(position, apex) / BreastEgg::NIPPLE_RADIUS).powi(2)).exp();
        mul(
            outward,
            BreastEgg::NIPPLE_HEIGHT * falloff * smoothstep(0.0, BreastEgg::FADE_IN, size),
        )
    }

    pub(crate) fn is_glute_region(&self, region_index: u32) -> bool {
        self.regions
            .get(region_index as usize)
            .is_some_and(|region| region.name.to_ascii_lowercase().contains("glute"))
    }

    /// Candidate I's positive glute size field, prepared once per cached hips
    /// surface. Position-only evaluation preserves cloth/skin openings and
    /// leaves jiggle weights unchanged. Negative sizes use `size_displacement`.
    pub(crate) fn glute_size_shape(&self, size: f32) -> Option<GluteShape> {
        if size <= 0.0 {
            return None;
        }
        self.regions
            .iter()
            .find(|region| region.name.to_ascii_lowercase().contains("glute"))
            .map(|region| GluteShape::new(region.center, region.radius, size))
    }

    /// The egg for the breast region on `position`'s side, if any.
    fn breast_egg(&self, position: [f32; 3], size: f32) -> Option<EggShape> {
        if size <= 0.0 {
            return None;
        }
        let side = if position[0] >= 0.0 { RegionSide::PositiveX } else { RegionSide::NegativeX };
        self.regions
            .iter()
            .find(|region| region.side == side && region.name.to_ascii_lowercase().contains("breast"))
            .map(|region| EggShape::new(region, size))
    }

    pub(crate) fn deform_surface(
        &self,
        lod_index: usize,
        surface_index: usize,
        region_offsets: &[[f32; 3]],
        tuning: JiggleTuning,
        skinned: &mut Ghoul2SkinnedSurface,
    ) {
        let Some(mask) = self
            .lod_masks
            .get(lod_index)
            .and_then(|lod| lod.get(&surface_index))
        else {
            return;
        };
        if region_offsets.len() != self.regions.len()
            || mask.region_weights.len() != self.regions.len()
        {
            return;
        }

        for (vertex_index, vertex) in skinned.vertices.iter_mut().enumerate() {
            let mut offset = [0.0f32; 3];
            for (region_index, weights) in mask.region_weights.iter().enumerate() {
                let Some(&weight) = weights.get(vertex_index) else {
                    continue;
                };
                if weight <= 0.0 {
                    continue;
                }
                let region_offset = region_offsets[region_index];
                let coord = mask
                    .dominant_region
                    .get(vertex_index)
                    .zip(mask.dominant_coord.get(vertex_index))
                    .and_then(|(&dominant, &coord)| (dominant as usize == region_index).then_some(coord))
                    .unwrap_or(0.0);
                let effective_weight = weight
                    * self.region_strength(region_index, tuning)
                    * self.glute_vertical_factor(region_index, coord, tuning);
                offset[0] += region_offset[0] * effective_weight;
                offset[1] += region_offset[1] * effective_weight;
                offset[2] += region_offset[2] * effective_weight;
            }
            vertex.position[0] += offset[0];
            vertex.position[1] += offset[1];
            vertex.position[2] += offset[2];
        }
        // Deliberately retain Ghoul2's authored/fast-path normal here.  The initial
        // deformation amplitudes are small, and recomputing triangle normals would
        // destroy hard/smoothed normal choices authored into the GLM.
    }

    /// Animated chest/glute callout, derived from the *fitted* soft-tissue regions,
    /// not from guessed proportions or fixed screen coordinates. Average both
    /// sides to give each slider one stable anchor as the character turns.
    pub(crate) fn studio_body_anchor(
        &self,
        glute: bool,
        pose: &[Matrix3x4],
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
    ) -> Option<[f32; 3]> {
        let name = if glute { "glute" } else { "breast" };
        let mut total = [0.0f32; 3];
        let mut count = 0u32;
        for region in self.regions.iter().filter(|region| region.name.contains(name)) {
            if let Some((point, _root)) = region_anchor_root(region, pose, axis, origin) {
                for i in 0..3 {
                    total[i] += point[i] + origin[i];
                }
                count += 1;
            }
        }
        (count != 0).then(|| total.map(|v| v / count as f32))
    }

    pub(crate) fn region_count(&self) -> usize {
        self.regions.len()
    }
}

/// The solve runs in a frame that translates with the entity origin but keeps
/// world axes. Damping in world space made constant-velocity running a steady
/// drag (several units of lag at 320 u/s) that pinned every region against
/// `max_offset`, so walking/running showed no oscillation at all. In this frame
/// steady travel cancels; origin *acceleration* re-enters as an inertial kick.
#[derive(Debug, Clone, Copy, Default)]
struct RegionState {
    valid: bool,
    /// Particle and its previous fixed step, origin-relative.
    position: [f32; 3],
    prev_position: [f32; 3],
    /// Origin-relative anchor/root at the last fixed step.
    last_anchor: [f32; 3],
    last_root: [f32; 3],
    /// Actual root at the last step, before reference-rate input timing.
    last_input_root: [f32; 3],
    /// Anchor motion over the last fixed step; swirl bends only the velocity
    /// relative to it.
    last_anchor_step: [f32; 3],
    /// Origin-relative anchor/root at the last frame that ran fixed steps.
    latch_anchor: [f32; 3],
    latch_root: [f32; 3],
    /// Particle-minus-anchor at the latest fixed step. The async presenter
    /// interpolates completed snapshots rather than delaying the solve here.
    offset: [f32; 3],
}

impl RegionState {
    fn at_rest(anchor: [f32; 3], root: [f32; 3]) -> Self {
        Self {
            valid: true,
            position: anchor,
            prev_position: anchor,
            last_anchor: anchor,
            last_root: root,
            last_input_root: root,
            latch_anchor: anchor,
            latch_root: root,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Default)]
struct EntityState {
    initialized: bool,
    last_time_ms: i32,
    solved_time_ms: f64,
    substep_accumulator: f32,
    /// Render time and origin at the last frame that ran fixed steps. Step
    /// targets are placed by real elapsed time, so render-rate jitter in when
    /// steps fire does not read as anchor/origin acceleration.
    latch_time_ms: i32,
    latch_origin: [f32; 3],
    /// Origin at the last fixed step and its per-step motion. `None` until one
    /// step has measured it, so first sight of a running player is calm.
    step_origin: [f32; 3],
    step_origin_delta: Option<[f32; 3]>,
    regions: Vec<RegionState>,
}

pub(crate) struct JiggleSystem {
    enabled: bool,
    hz: u32,
    max_substeps: usize,
    tuning: JiggleTuning,
    entities: HashMap<(u16, String), EntityState>,
}

/// Raw fixed-step result. Presentation supplies the only interpolation delay.
pub(crate) struct JiggleSnapshot {
    pub time_ms: f64,
    pub offsets: Vec<[f32; 3]>,
}

impl Default for JiggleSystem {
    fn default() -> Self {
        Self {
            enabled: false,
            hz: 60,
            max_substeps: 4,
            tuning: JiggleTuning::default(),
            entities: HashMap::new(),
        }
    }
}

impl JiggleSystem {
    pub(crate) fn set_config(
        &mut self,
        enabled: bool,
        hz: u32,
        max_substeps: u32,
        tuning: JiggleTuning,
    ) {
        let hz = hz.clamp(30, 240);
        if (self.enabled && !enabled) || self.tuning.solver != tuning.solver || self.hz != hz {
            self.entities.clear();
        }
        self.enabled = enabled;
        self.hz = hz;
        self.max_substeps = tickrate::catch_up_steps(max_substeps, hz);
        self.tuning = JiggleTuning {
            solver: tuning.solver,
            overall_strength: tuning.overall_strength.clamp(0.0, 2.0),
            breast_strength: tuning.breast_strength.clamp(0.0, 2.0),
            glute_strength: tuning.glute_strength.clamp(0.0, 2.0),
            stiffness_scale: tuning.stiffness_scale.clamp(0.0, 3.0),
            damping_scale: tuning.damping_scale.clamp(0.0, 3.0),
            glute_jiggle_bottom_z: tuning.glute_jiggle_bottom_z.clamp(-8.0, 8.0),
            glute_jiggle_top_z: tuning.glute_jiggle_top_z.clamp(-8.0, 8.0),
            jp_stiffness: tuning.jp_stiffness.clamp(0.0, 1.0),
            jp_drag: tuning.jp_drag.clamp(0.0, 1.0),
            jp_air_drag: tuning.jp_air_drag.clamp(0.0, 1.0),
            jp_stretch: tuning.jp_stretch.clamp(0.0, 1.0),
            jp_soften: tuning.jp_soften.clamp(0.0, 1.0),
            jp_gravity: tuning.jp_gravity.clamp(0.0, 2.0),
            locomotion: tuning.locomotion.clamp(0.0, 2.0),
            impulse_cap: tuning.impulse_cap.clamp(0.0, 3.0),
            swirl: tuning.swirl.clamp(0.0, 1.0),
            breast_stiffness: tuning.breast_stiffness.clamp(0.0, 3.0),
            breast_damping: tuning.breast_damping.clamp(0.0, 3.0),
            glute_stiffness: tuning.glute_stiffness.clamp(0.0, 3.0),
            glute_damping: tuning.glute_damping.clamp(0.0, 3.0),
            breast_size: tuning.breast_size.clamp(-2.0, 3.0),
            glute_size: tuning.glute_size.clamp(-2.0, 4.0),
        };
    }

    pub(crate) fn tuning(&self) -> JiggleTuning {
        self.tuning
    }

    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn clear(&mut self) {
        self.entities.clear();
    }

    pub(crate) fn retain_entities(&mut self, live: &std::collections::HashSet<u16>) {
        self.entities
            .retain(|(entity_num, _), _| live.contains(entity_num));
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn simulate_snapshot(
        &mut self,
        entity_num: u16,
        model_key: &str,
        profile: &JiggleProfile,
        pose: &[Matrix3x4],
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
        time_ms: i32,
    ) -> JiggleSnapshot {
        let tuning = self.tuning;
        let state = self
            .entities
            .entry((entity_num, model_key.to_owned()))
            .or_default();
        let region_count = profile.region_count();
        let fixed_step = 1.0 / self.hz as f32;
        let anchors = profile
            .regions
            .iter()
            .map(|region| region_anchor_root(region, pose, axis, origin))
            .collect::<Vec<_>>();

        let raw_dt = (time_ms - state.last_time_ms) as f32 * 0.001;
        let reset = !state.initialized
            || state.regions.len() != region_count
            || raw_dt < 0.0
            || raw_dt > RESET_DT
            || distance(state.latch_origin, origin) > TELEPORT_DISTANCE
            || state
                .regions
                .iter()
                .zip(&anchors)
                .any(|(region_state, anchor)| {
                    anchor.is_some_and(|(anchor, _)| {
                        region_state.valid
                            && distance(region_state.latch_anchor, anchor) > TELEPORT_DISTANCE
                    })
                });
        if reset {
            state.initialized = true;
            state.last_time_ms = time_ms;
            state.solved_time_ms = time_ms as f64;
            state.substep_accumulator = 0.0;
            state.latch_time_ms = time_ms;
            state.latch_origin = origin;
            state.step_origin = origin;
            state.step_origin_delta = None;
            state.regions = anchors
                .iter()
                .map(|anchor| {
                    anchor.map_or_else(RegionState::default, |(anchor, root)| {
                        RegionState::at_rest(anchor, root)
                    })
                })
                .collect();
            return JiggleSnapshot { time_ms: time_ms as f64, offsets: vec![[0.0; 3]; region_count] };
        }

        // KawaiiPhysics fixed-substep semantics: accumulate real frame time,
        // clamp catch-up to MaxSubsteps, and discard spiral-of-death excess.
        state.substep_accumulator = (state.substep_accumulator + raw_dt.max(0.0))
            .min(self.max_substeps as f32 * fixed_step);
        let steps = (state.substep_accumulator / fixed_step).floor() as usize;
        state.substep_accumulator -= steps as f32 * fixed_step;
        state.last_time_ms = time_ms;

        if steps > 0 {
            state.solved_time_ms = time_ms as f64 - state.substep_accumulator as f64 * 1000.0;
            // The solve trails render time by the accumulator remainder. Place
            // each step's end on the latch->now segment by real elapsed time:
            // 0 = last latch, 1 = this frame.
            let remainder = state.substep_accumulator;
            let span = ((time_ms - state.latch_time_ms) as f32 * 0.001).max(0.0001);
            let fractions = (0..steps)
                .map(|step| 1.0 - (remainder + (steps - 1 - step) as f32 * fixed_step) / span)
                .collect::<Vec<_>>();

            // Only a change in per-step origin motion (acceleration) disturbs
            // the tissue: start/stop, jumps, landings, direction changes.
            let mut kicks = Vec::with_capacity(steps);
            let mut step_origin = state.step_origin;
            let mut step_origin_delta = state.step_origin_delta;
            for &fraction in &fractions {
                let origin_at = lerp(state.latch_origin, origin, fraction);
                let delta = sub(origin_at, step_origin);
                let kick = step_origin_delta.map_or(([0.0; 3], false), |previous| {
                    locomotion_input(
                        delta,
                        previous,
                        tuning.locomotion,
                        tuning.impulse_cap,
                        fixed_step,
                    )
                });
                kicks.push(kick);
                step_origin = origin_at;
                step_origin_delta = Some(delta);
            }
            state.step_origin = step_origin;
            state.step_origin_delta = step_origin_delta;
            state.latch_time_ms = time_ms;
            state.latch_origin = origin;

            let depth_axis_length = length(axis[1]);
            let swirl_axis = if depth_axis_length > 0.0001 {
                mul(axis[1], 1.0 / depth_axis_length)
            } else {
                [0.0; 3]
            };

            for (region, (region_state, anchor)) in profile
                .regions
                .iter()
                .zip(state.regions.iter_mut().zip(&anchors))
            {
                let Some((anchor, root)) = *anchor else {
                    continue;
                };
                if !region_state.valid {
                    *region_state = RegionState::at_rest(anchor, root);
                    continue;
                }
                // Mirrored per side so left/right orbit symmetrically.
                let side_sign = match region.side {
                    RegionSide::PositiveX => 1.0,
                    RegionSide::NegativeX => -1.0,
                };
                let swirl_angle = tuning.swirl * SWIRL_RATE * fixed_step * side_sign;
                let (region_stiffness, region_damping, region_size) =
                    if region.name.to_ascii_lowercase().contains("glute") {
                        (
                            tuning.glute_stiffness,
                            tuning.glute_damping,
                            tuning.glute_size,
                        )
                    } else {
                        (
                            tuning.breast_stiffness,
                            tuning.breast_damping,
                            tuning.breast_size,
                        )
                    };
                // Dinurdo extension, not KawaiiPhysics: added size scales the
                // inputs (like moving the region stiffness slider and the
                // travel limit), leaving the solver step itself untouched.
                let added = region_size.max(0.0);
                let size_stiffness = 1.0 / (1.0 + SIZE_STIFFNESS_PER_UNIT * added);
                let region_stiffness = region_stiffness * size_stiffness;
                let max_offset = region.max_offset * (1.0 + SIZE_TRAVEL_PER_UNIT * added);

                // Dinurdo rate conversion: slider inputs describe the existing
                // 60 Hz response. Convert after region/size multipliers, leaving
                // the two solver equations and world-unit travel limits alone.
                let kawaii = (tuning.solver == JiggleSolver::KawaiiPhysics).then(|| {
                    tickrate::reference_response(
                        (region.stiffness * tuning.stiffness_scale * region_stiffness)
                            .clamp(0.0, 0.999),
                        (region.damping * tuning.damping_scale * region_damping).clamp(0.0, 0.999),
                        fixed_step,
                    )
                });
                let jiggle_reference = tickrate::JiggleReference {
                    stiffness: (tuning.jp_stiffness * size_stiffness).clamp(0.0, 1.0),
                    drag: tuning.jp_drag.clamp(0.0, 1.0),
                    air_drag: tuning.jp_air_drag.clamp(0.0, 1.0),
                    stretch: tuning.jp_stretch.clamp(0.0, 1.0),
                    soften: tuning.jp_soften.clamp(0.0, 1.0),
                    gravity: tuning.jp_gravity.clamp(0.0, 2.0),
                };
                let jiggle = (tuning.solver == JiggleSolver::JigglePhysics).then(|| {
                    let mut inputs = tickrate::jiggle_inputs(
                        jiggle_reference.stiffness,
                        jiggle_reference.drag,
                        jiggle_reference.air_drag,
                        jiggle_reference.stretch,
                        fixed_step,
                    );
                    inputs.soften = jiggle_reference.soften;
                    inputs
                });
                let raw_gravity_step = [
                    0.0,
                    0.0,
                    -800.0 * jiggle_reference.gravity * fixed_step * fixed_step,
                ];

                for (&fraction, &(kick, impulse)) in fractions.iter().zip(&kicks) {
                    let input_target = lerp(region_state.latch_anchor, anchor, fraction);
                    let input_root = lerp(region_state.latch_root, root, fraction);
                    let target = tickrate::reference_anchor(
                        input_target,
                        region_state.last_anchor,
                        fixed_step,
                    );
                    let root_at = tickrate::reference_anchor(
                        input_root,
                        region_state.last_input_root,
                        fixed_step,
                    );
                    let jiggle = jiggle.map(|inputs| {
                        if inputs.soften > 0.0 {
                            tickrate::softened_jiggle_inputs(
                                jiggle_reference,
                                region_state.position,
                                region_state.prev_position,
                                root_at,
                                region_state.last_root,
                                target,
                                kick,
                                impulse,
                                fixed_step,
                            )
                        } else {
                            inputs
                        }
                    });
                    let (force_scale, impulse_scale) = match tuning.solver {
                        JiggleSolver::KawaiiPhysics => {
                            let inputs = kawaii.expect("selected Kawaii rate inputs");
                            (inputs.force_scale, inputs.impulse_scale)
                        }
                        JiggleSolver::JigglePhysics => {
                            let inputs = jiggle.expect("selected Jiggle rate inputs");
                            (inputs.force_scale, inputs.impulse_scale)
                        }
                    };
                    let kick = mul(kick, if impulse { impulse_scale } else { force_scale });
                    let gravity_step =
                        jiggle.map_or([0.0; 3], |inputs| mul(raw_gravity_step, inputs.force_scale));
                    let previous = region_state.position;
                    let next = match tuning.solver {
                        JiggleSolver::KawaiiPhysics => {
                            let kawaii = kawaii.expect("selected Kawaii rate inputs");
                            let damping = kawaii.damping;
                            let stiffness = kawaii.stiffness;
                            // KawaiiPhysics Verlet core on one virtual soft point.
                            // Velocity is frame-relative (Kawaii simulates in
                            // component space), damping is raw per-step, and
                            // stiffness keeps the source exponent form.
                            let stiffness_pull =
                                1.0 - (1.0 - stiffness).powf(self.hz as f32 * fixed_step);
                            let velocity = swirl_velocity(
                                sub(region_state.position, region_state.prev_position),
                                region_state.last_anchor_step,
                                swirl_axis,
                                swirl_angle,
                            );
                            let mut next = add(region_state.position, mul(velocity, 1.0 - damping));
                            next = add(next, kick);
                            add(next, mul(sub(target, next), stiffness_pull))
                        }
                        JiggleSolver::JigglePhysics => {
                            let jiggle = jiggle.expect("selected Jiggle rate inputs");
                            // Ported from naelstrof/JigglePhysics (MIT); see
                            // LICENSE-JigglePhysics-MIT. Motionless Root one-child
                            // solve, run in the origin-relative frame so Air Drag
                            // acts on root animation/turning, not steady travel.
                            let drag = jiggle.drag;
                            let air_drag = jiggle.air_drag;
                            let stiffness = jiggle.stiffness;
                            let soften = jiggle.soften;
                            let stretch = jiggle.stretch;
                            let angle_elasticity = stiffness * stiffness;
                            let length_elasticity = (1.0 + (stiffness - 1.0) * stretch).powi(2);
                            let particle_delta =
                                sub(region_state.position, region_state.prev_position);
                            let parent_delta = sub(root_at, region_state.last_root);
                            // Exact JigglePhysics Verlet decomposition: parent
                            // motion uses Air Drag; child-local motion uses Drag.
                            let local_space_velocity = sub(particle_delta, parent_delta);
                            let local_space_velocity = if swirl_angle != 0.0 {
                                rotate_about(local_space_velocity, swirl_axis, swirl_angle)
                            } else {
                                local_space_velocity
                            };
                            let mut next = add(
                                region_state.position,
                                add(
                                    mul(parent_delta, 1.0 - air_drag),
                                    mul(local_space_velocity, 1.0 - drag),
                                ),
                            );
                            next = add(add(next, gravity_step), kick);

                            // Direct-child Motionless Root angle/rest constraint.
                            let rest_length = distance(root_at, target).max(0.001);
                            let normalized_error =
                                (distance(next, target) / rest_length).clamp(0.0, 1.0);
                            let soften_factor = normalized_error.powf(soften * 2.0);
                            next = lerp(
                                next,
                                target,
                                (angle_elasticity * soften_factor).clamp(0.0, 1.0),
                            );

                            // Preserve the authored root->child length with the
                            // source-shaped stretch/elasticity control.
                            let from_root = sub(next, root_at);
                            let from_root_len = length(from_root);
                            if from_root_len > 0.0001 {
                                let length_target =
                                    add(root_at, mul(from_root, rest_length / from_root_len));
                                next = lerp(next, length_target, length_elasticity.clamp(0.0, 1.0));
                            }
                            next
                        }
                    };
                    region_state.prev_position = previous;
                    region_state.position = next;

                    let mut offset = sub(next, input_target);
                    let offset_length = length(offset);
                    if offset_length > max_offset {
                        offset = mul(offset, max_offset / offset_length);
                        region_state.position = add(input_target, offset);
                        // Verlet stores velocity implicitly in the previous
                        // position. Collapse only the outward component at the
                        // clamp so the next step does not re-launch past it.
                        let implicit_velocity =
                            sub(region_state.position, region_state.prev_position);
                        let radial =
                            dot(implicit_velocity, offset) / (max_offset * max_offset).max(0.0001);
                        if radial > 0.0 {
                            let corrected = sub(implicit_velocity, mul(offset, radial));
                            region_state.prev_position = sub(region_state.position, corrected);
                        }
                    }

                    region_state.offset = offset;
                    region_state.last_anchor_step = sub(input_target, region_state.last_anchor);
                    region_state.last_anchor = input_target;
                    region_state.last_input_root = input_root;
                    region_state.last_root = root_at;
                }
                region_state.latch_anchor = anchor;
                region_state.latch_root = root;
            }
        }

        // Publish the latest actual solve, not an already delayed render pose.
        // Its timestamp excludes time still waiting in the fixed-step accumulator.
        let mut offsets = vec![[0.0f32; 3]; region_count];
        for (index, (region, region_state)) in
            profile.regions.iter().zip(&state.regions).enumerate()
        {
            if !region_state.valid {
                continue;
            }
            let mut model_offset = world_vector_to_model(region_state.offset, axis);
            for component in 0..3 {
                model_offset[component] *= region.response[component];
            }
            offsets[index] = model_offset;
        }
        JiggleSnapshot {
            time_ms: state.solved_time_ms,
            offsets,
        }
    }
}

/// Average a left/right region pair (|x|, y, z, radii) and mirror it in X.
fn symmetrize_pair(regions: &mut [RegionSpec], left: &str, right: &str) {
    let (Some(l), Some(r)) = (
        regions.iter().position(|region| region.name == left),
        regions.iter().position(|region| region.name == right),
    ) else {
        return;
    };
    let center_x = (regions[l].center[0].abs() + regions[r].center[0].abs()) * 0.5;
    let center_y = (regions[l].center[1] + regions[r].center[1]) * 0.5;
    let center_z = (regions[l].center[2] + regions[r].center[2]) * 0.5;
    let radius = [
        (regions[l].radius[0] + regions[r].radius[0]) * 0.5,
        (regions[l].radius[1] + regions[r].radius[1]) * 0.5,
        (regions[l].radius[2] + regions[r].radius[2]) * 0.5,
    ];
    for index in [l, r] {
        let sign = match regions[index].side {
            RegionSide::PositiveX => 1.0,
            RegionSide::NegativeX => -1.0,
        };
        regions[index].center = [center_x * sign, center_y, center_z];
        regions[index].radius = radius;
    }
}

/// Dinurdo input extension, outside both solver equations. A discontinuous
/// step-up can create two large kicks (rise and stop); limit each full vector
/// before the region solve so direction is preserved and steps cannot inject
/// the old maximum response into both breasts and glutes.
fn locomotion_input(
    delta: [f32; 3],
    previous: [f32; 3],
    strength: f32,
    impulse_cap: f32,
    fixed_step: f32,
) -> ([f32; 3], bool) {
    let kick = mul(sub(delta, previous), -strength);
    // This ceiling limits a velocity impulse, not a continuous acceleration:
    // kick/dt has the same upper bound at every r_physicsHz. Smooth origin
    // acceleration already scales with dt^2 before reaching this ceiling.
    let limit = impulse_cap * IMPULSE_REFERENCE_HZ * fixed_step;
    let kick_length = length(kick);
    if kick_length > limit {
        (mul(kick, limit / kick_length), true)
    } else {
        (kick, false)
    }
}

#[cfg(test)]
fn locomotion_kick(
    delta: [f32; 3],
    previous: [f32; 3],
    strength: f32,
    impulse_cap: f32,
    fixed_step: f32,
) -> [f32; 3] {
    locomotion_input(delta, previous, strength, impulse_cap, fixed_step).0
}

/// Origin-relative (world axes) anchor and solver root for one region.
fn region_anchor_root(
    region: &RegionSpec,
    pose: &[Matrix3x4],
    axis: [[f32; 3]; 3],
    origin: [f32; 3],
) -> Option<([f32; 3], [f32; 3])> {
    let mut anchor_model = [0.0f32; 3];
    for term in &region.anchor_terms {
        let point = transform_point(pose.get(term.bone_index)?, term.point);
        anchor_model[0] += point[0] * term.weight;
        anchor_model[1] += point[1] * term.weight;
        anchor_model[2] += point[2] * term.weight;
    }
    let anchor = sub(model_to_jka_world(anchor_model, axis, origin), origin);
    let root = region
        .solver_root_bone
        .and_then(|bone_index| pose.get(bone_index))
        .map(|matrix| sub(model_to_jka_world(transform_point(matrix, [0.0; 3]), axis, origin), origin))
        .unwrap_or(anchor);
    Some((anchor, root))
}

/// Bend only the anchor-relative part of a Verlet velocity around the model
/// depth axis. A pure rotation of velocity is gyroscopic: it curves the
/// bounce into an orbit without adding energy, so damping still settles it.
fn swirl_velocity(
    velocity: [f32; 3],
    anchor_velocity: [f32; 3],
    swirl_axis: [f32; 3],
    angle: f32,
) -> [f32; 3] {
    if angle == 0.0 {
        return velocity;
    }
    add(
        anchor_velocity,
        rotate_about(sub(velocity, anchor_velocity), swirl_axis, angle),
    )
}

/// Rodrigues rotation of `vector` about unit `axis`.
fn rotate_about(vector: [f32; 3], axis: [f32; 3], angle: f32) -> [f32; 3] {
    let (sin, cos) = angle.sin_cos();
    add(
        add(mul(vector, cos), mul(cross(axis, vector), sin)),
        mul(axis, dot(axis, vector) * (1.0 - cos)),
    )
}

fn region_vertex_weight(region: &RegionSpec, surface: &GlmSurface, vertex: &GlmVertex) -> f32 {
    let position = vertex.position;
    if !region_side_matches(region.side, position[0]) {
        return 0.0;
    }

    let semantic = &region.semantic;
    let nx = (position[0] - region.center[0]) / region.radius[0];
    let nz = (position[2] - region.center[2]) / region.radius[2];
    let q = nx * nx + nz * nz;
    if q >= 1.0 {
        return 0.0;
    }
    let facing = (vertex.normal[1] * semantic.facing_y).clamp(0.0, 1.0);
    if facing <= 0.03 {
        return 0.0;
    }
    let bone_weight = semantic_bone_weight(surface, vertex, &semantic.allowed_bones);
    if bone_weight <= 0.10 {
        return 0.0;
    }
    // Keep rounded side vertices participating while strongly rejecting the
    // opposite side of the torso/hips. The geometry falloff still reaches
    // zero with zero derivative at the region boundary.
    let facing_weight = ((facing - 0.03) / 0.55).clamp(0.0, 1.0);
    let base = 1.0 - q;
    (base * base * (0.30 + 0.70 * facing_weight) * bone_weight.clamp(0.0, 1.0) * region.strength)
        .clamp(0.0, 1.0)
}

#[derive(Debug, Clone, Copy, Default)]
struct AnchorAccum {
    coefficient: f32,
    weighted_point: [f32; 3],
}

/// Build a pose target from the actual LOD0 GLM skin weights in the selected
/// region.  Because Ghoul2 skinning is linear, one weighted source-point term
/// per contributing bone reproduces the weighted centroid of the selected
/// vertices without re-skinning the region just to drive the spring.
fn compile_region_anchor(
    glm: &GlmModel,
    gla: &GlaAnimation,
    region: &RegionSpec,
) -> Result<Vec<AnchorTerm>, String> {
    let lod = glm
        .lods
        .first()
        .ok_or_else(|| format!("region `{}` cannot build an anchor: GLM has no LOD0", region.name))?;
    let mut accum = HashMap::<usize, AnchorAccum>::new();

    for surface in &lod.surfaces {
        let Some(hierarchy) = glm.hierarchy.get(surface.surface_index) else {
            continue;
        };
        if !region
            .surfaces
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&hierarchy.name))
        {
            continue;
        }

        for vertex in &surface.vertices {
            let mask_weight = region_vertex_weight(region, surface, vertex);
            if mask_weight <= 0.0001 {
                continue;
            }
            for skin_weight in &vertex.weights {
                let Some(&bone_index) = surface.bone_references.get(skin_weight.local_bone_index)
                else {
                    return Err(format!(
                        "region `{}` references an invalid local GLM bone index {}",
                        region.name, skin_weight.local_bone_index
                    ));
                };
                if bone_index >= gla.skeleton.len() {
                    // Source-only native procedural jigglebones are appended
                    // after the GLA skin palette. They are not anatomical
                    // anchor candidates for breast/glute auto-detection.
                    if glm.name.to_ascii_lowercase().ends_with(".mdl") {
                        continue;
                    }
                    return Err(format!(
                        "region `{}` references GLM bone {} but GLA has only {} bones",
                        region.name,
                        bone_index,
                        gla.skeleton.len()
                    ));
                }
                let coefficient = mask_weight * skin_weight.weight;
                if coefficient <= 0.0 {
                    continue;
                }
                let entry = accum.entry(bone_index).or_default();
                entry.coefficient += coefficient;
                for axis in 0..3 {
                    entry.weighted_point[axis] += vertex.position[axis] * coefficient;
                }
            }
        }
    }

    let total = accum.values().map(|entry| entry.coefficient).sum::<f32>();
    if !total.is_finite() || total <= 0.0001 {
        return Err(format!(
            "region `{}` could not derive an automatic skin-weight anchor",
            region.name
        ));
    }

    let mut terms = accum
        .into_iter()
        .filter_map(|(bone_index, entry)| {
            if entry.coefficient <= 0.0001 {
                return None;
            }
            let inv = 1.0 / entry.coefficient;
            Some(AnchorTerm {
                bone_index,
                weight: entry.coefficient / total,
                point: [
                    entry.weighted_point[0] * inv,
                    entry.weighted_point[1] * inv,
                    entry.weighted_point[2] * inv,
                ],
            })
        })
        .collect::<Vec<_>>();
    terms.sort_by_key(|term| term.bone_index);
    Ok(terms)
}

/// Height of a converted Source joint in the bind-pose coordinate system of
/// the generated GLM. The normal Source FK path has model-specific pivots;
/// legacy rebind modes use the generic humanoid GLA rest joints instead.
fn source_joint_z(gla: &GlaAnimation, native_pivots: Option<&[[f32; 3]]>, index: usize) -> f32 {
    native_pivots
        .and_then(|pivots| pivots.get(index))
        .map(|pivot| pivot[2])
        .filter(|height| height.is_finite())
        .unwrap_or(gla.skeleton[index].base_pose[2][3])
}

fn find_bone(gla: &GlaAnimation, name: &str) -> Option<usize> {
    gla.skeleton
        .iter()
        .position(|bone| bone.name.eq_ignore_ascii_case(name))
}

fn surface_family(glm: &GlmModel, prefix: &str) -> Vec<String> {
    let prefix = prefix.to_ascii_lowercase();
    glm.hierarchy
        .iter()
        .filter_map(|surface| {
            let name = surface.name.to_ascii_lowercase();
            (!name.starts_with('*') && name.starts_with(&prefix)).then_some(name)
        })
        .collect()
}

fn marker_centroid(glm: &GlmModel, name: &str) -> Option<[f32; 3]> {
    let surface_index = glm
        .hierarchy
        .iter()
        .position(|surface| surface.name.eq_ignore_ascii_case(name))?;
    let surface = glm
        .lods
        .first()?
        .surfaces
        .iter()
        .find(|surface| surface.surface_index == surface_index)?;
    if surface.vertices.is_empty() {
        return None;
    }
    let mut center = [0.0f32; 3];
    for vertex in &surface.vertices {
        for axis in 0..3 {
            center[axis] += vertex.position[axis];
        }
    }
    let inv = 1.0 / surface.vertices.len() as f32;
    for value in &mut center {
        *value *= inv;
    }
    Some(center)
}

fn semantic_bone_weight(surface: &GlmSurface, vertex: &GlmVertex, allowed: &[usize]) -> f32 {
    vertex
        .weights
        .iter()
        .filter_map(|weight| {
            let bone = *surface.bone_references.get(weight.local_bone_index)?;
            allowed.contains(&bone).then_some(weight.weight.max(0.0))
        })
        .sum::<f32>()
        .clamp(0.0, 1.0)
}

/// Fit a stable X/Z soft-tissue footprint from the actual LOD0 body vertices.
/// The helper surfaces only establish anatomical meaning and a vertical window;
/// facing normals and skin weights decide which rendered vertices participate.
/// Also returns the height of the protrusion apex along `facing_y`: the
/// score/depth-weighted Z of samples within `PROTRUSION_APEX_BAND` of the
/// most-protruding one.
fn fit_semantic_region(
    glm: &GlmModel,
    surfaces: &[String],
    side: RegionSide,
    semantic: &SemanticMask,
    z_min: f32,
    z_max: f32,
    min_radius: [f32; 2],
    max_radius: [f32; 2],
) -> Option<([f32; 3], [f32; 3], Option<f32>)> {
    let lod = glm.lods.first()?;
    let mut total = 0.0f32;
    let mut mean = [0.0f32; 3];
    let mut samples = Vec::<([f32; 3], f32)>::new();

    for surface in &lod.surfaces {
        let name = glm.hierarchy.get(surface.surface_index)?.name.to_ascii_lowercase();
        if !surfaces.iter().any(|candidate| candidate == &name) {
            continue;
        }
        for vertex in &surface.vertices {
            if !region_side_matches(side, vertex.position[0])
                || vertex.position[2] < z_min
                || vertex.position[2] > z_max
            {
                continue;
            }
            let facing = (vertex.normal[1] * semantic.facing_y).clamp(0.0, 1.0);
            if facing <= 0.03 {
                continue;
            }
            let bone_weight = semantic_bone_weight(surface, vertex, &semantic.allowed_bones);
            if bone_weight <= 0.10 {
                continue;
            }
            let score = bone_weight * (0.25 + 0.75 * facing);
            total += score;
            for axis in 0..3 {
                mean[axis] += vertex.position[axis] * score;
            }
            samples.push((vertex.position, score));
        }
    }

    if samples.len() < 6 || total <= 0.0001 {
        return None;
    }
    for value in &mut mean {
        *value /= total;
    }

    let max_protrusion = samples
        .iter()
        .map(|(position, _)| position[1] * semantic.facing_y)
        .fold(f32::NEG_INFINITY, f32::max);
    let apex_floor = max_protrusion - PROTRUSION_APEX_BAND;
    let mut apex_total = 0.0f32;
    let mut apex_z = 0.0f32;
    let mut variance_x = 0.0f32;
    let mut variance_z = 0.0f32;
    for (position, score) in samples {
        variance_x += (position[0] - mean[0]).powi(2) * score;
        variance_z += (position[2] - mean[2]).powi(2) * score;
        let depth = position[1] * semantic.facing_y - apex_floor;
        if depth > 0.0 {
            apex_total += depth * score;
            apex_z += position[2] * depth * score;
        }
    }
    variance_x /= total;
    variance_z /= total;
    let radius_x = (variance_x.sqrt() * 2.75).clamp(min_radius[0], max_radius[0]);
    let radius_z = (variance_z.sqrt() * 2.75).clamp(min_radius[1], max_radius[1]);
    let apex_z = (apex_total > 0.0001).then(|| apex_z / apex_total);
    Some((mean, [radius_x, 1.0, radius_z], apex_z))
}

fn region_side_matches(side: RegionSide, x: f32) -> bool {
    match side {
        RegionSide::NegativeX => x < 0.0,
        RegionSide::PositiveX => x >= 0.0,
    }
}

fn model_to_jka_world(point: [f32; 3], axis: [[f32; 3]; 3], origin: [f32; 3]) -> [f32; 3] {
    [
        origin[0] + axis[0][0] * point[0] + axis[1][0] * point[1] + axis[2][0] * point[2],
        origin[1] + axis[0][1] * point[0] + axis[1][1] * point[1] + axis[2][1] * point[2],
        origin[2] + axis[0][2] * point[0] + axis[1][2] * point[1] + axis[2][2] * point[2],
    ]
}

fn world_vector_to_model(vector: [f32; 3], axis: [[f32; 3]; 3]) -> [f32; 3] {
    [dot(vector, axis[0]), dot(vector, axis[1]), dot(vector, axis[2])]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn mul(value: [f32; 3], scalar: f32) -> [f32; 3] {
    [value[0] * scalar, value[1] * scalar, value[2] * scalar]
}

fn length(value: [f32; 3]) -> f32 {
    dot(value, value).sqrt()
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    length(sub(a, b))
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let width = (edge1 - edge0).max(0.0001);
    let t = ((value - edge0) / width).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    // Build a minimal GLA with anatomical bones. This uses the real parser so
    // Source jiggle tests need no external PK3s, models or assets.
    fn synthetic_source_gla() -> GlaAnimation {
        let names_and_z: [(&str, f32); 11] = [
            ("pelvis", 39.0), ("lower_lumbar", 43.0),
            ("upper_lumbar", 49.0), ("thoracic", 53.0),
            ("lclavical", 57.0), ("rclavical", 57.0),
            ("lfemurYZ", 35.0), ("lfemurX", 35.0),
            ("rfemurYZ", 35.0), ("rfemurX", 35.0),
            ("head", 67.0),
        ];
        let count = names_and_z.len();
        let skeleton_start = 100 + count * 4;
        let frame_start = skeleton_start + count * 172;
        let pool_start = frame_start + count * 3;
        let total = pool_start + 14;
        let mut bytes = vec![0u8; total];
        // Fill the MDXA header, then the independent skeleton records.
        bytes[..4].copy_from_slice(b"2LGA");
        let mut put_i32 = |offset: usize, value: i32| {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        };
        put_i32(4, 6);
        put_i32(76, 1);
        put_i32(80, frame_start as i32);
        put_i32(84, count as i32);
        put_i32(88, pool_start as i32);
        put_i32(92, skeleton_start as i32);
        put_i32(96, total as i32);
        drop(put_i32);
        bytes[8..18].copy_from_slice(b"testsource");
        bytes[72..76].copy_from_slice(&1.0f32.to_le_bytes());
        for (index, (name, z)) in names_and_z.iter().enumerate() {
            let record = skeleton_start + index * 172;
            let relative = (record - 100) as i32;
            bytes[100 + index * 4..104 + index * 4]
                .copy_from_slice(&relative.to_le_bytes());
            bytes[record..record + name.len()].copy_from_slice(name.as_bytes());
            bytes[record + 68..record + 72].copy_from_slice(&(-1i32).to_le_bytes());
            for offset in [72, 120] {
                for row in 0..3 {
                    let start = record + offset + row * 16;
                    bytes[start + row * 4..start + row * 4 + 4]
                        .copy_from_slice(&1.0f32.to_le_bytes());
                    let translation = if row == 2 {
                        if offset == 72 { *z } else { -*z }
                    } else { 0.0 };
                    bytes[start + 12..start + 16]
                        .copy_from_slice(&translation.to_le_bytes());
                }
            }
        }
        jka_assets::ghoul2::parse_gla(&bytes).expect("synthetic test GLA parses")
    }

    // One arbitrary Source bodypart, containing disconnected anatomically
    // weighted pieces. No Raven torso/hips surface names or helper markers.
    fn synthetic_source_glm() -> GlmModel {
        use jka_assets::ghoul2::{GlmLod, GlmSurfaceHierarchy, GlmWeight};
        let mut body = GlmSurface {
            surface_index: 0,
            vertices: Vec::new(),
            texcoords: Vec::new(),
            triangles: Vec::new(),
            bone_references: vec![3, 0], // thoracic and pelvis
        };
        for (x_center, z_center, y, bone, facing) in [
            (3.0, 53.0, -4.0, 0, -1.0),
            (-3.0, 53.0, -4.0, 0, -1.0),
            (3.0, 37.5, 4.0, 1, 1.0),
            (-3.0, 37.5, 4.0, 1, 1.0),
        ] {
            let base = body.vertices.len() as u32;
            for row in 0..4 {
                for col in 0..4 {
                    body.vertices.push(GlmVertex {
                        position: [x_center + (col as f32 - 1.5) * 0.65,
                                   y, z_center + (row as f32 - 1.5) * 0.75],
                        normal: [0.0, facing, 0.0],
                        weights: vec![GlmWeight { local_bone_index: bone, weight: 1.0 }],
                    });
                    body.texcoords.push([col as f32 / 3.0, row as f32 / 3.0]);
                }
            }
            for row in 0..3 {
                for col in 0..3 {
                    let i = base + row * 4 + col;
                    body.triangles.push([i, i + 1, i + 4]);
                    body.triangles.push([i + 1, i + 5, i + 4]);
                }
            }
        }
        let head = GlmSurface {
            surface_index: 1,
            vertices: vec![GlmVertex {
                position: [2.5, -2.0, 67.0], normal: [0.0, -1.0, 0.0],
                // Intentionally torso-weighted: height must reject this vertex.
                weights: vec![GlmWeight { local_bone_index: 0, weight: 1.0 }],
            }],
            texcoords: vec![[0.0, 0.0]],
            triangles: Vec::new(), bone_references: vec![3],
        };
        GlmModel {
            name: "models/players/source/test.mdl".into(),
            anim_name: "models/players/_humanoid/_humanoid".into(),
            num_bones: 11,
            hierarchy: ["body_citizen_mesh_0", "face_head_mesh_1"]
                .into_iter()
                .map(|name| GlmSurfaceHierarchy {
                    name: name.into(), flags: 0, shader: String::new(),
                    parent_index: -1, children: Vec::new(),
                })
                .collect(),
            lods: vec![GlmLod { surfaces: vec![body, head] }],
        }
    }

    // Raven-style fixture with named torso/hips and four anatomical marker
    // surfaces. The common fitter must preserve the original stock marker
    // offsets/caps rather than adopting Source's joint-only placement.
    fn synthetic_raven_glm() -> GlmModel {
        use jka_assets::ghoul2::{GlmLod, GlmSurfaceHierarchy, GlmWeight};
        let source = synthetic_source_glm();
        let mut torso = source.lods[0].surfaces[0].clone();
        torso.surface_index = 0;
        let mut hips = torso.clone();
        hips.surface_index = 1;
        let marker_names_and_centers = [
            ("*lchest_l", [3.0, -2.0, 51.0]),
            ("*uchest_l", [3.0, -2.0, 55.0]),
            ("*lchest_r", [-3.0, -2.0, 51.0]),
            ("*uchest_r", [-3.0, -2.0, 55.0]),
            ("*hip_bl", [3.5, 3.0, 40.0]),
            ("*hip_br", [-3.5, 3.0, 40.0]),
        ];
        let mut hierarchy = ["torso_test", "hips_test"].into_iter()
            .map(|name| GlmSurfaceHierarchy {
                name: name.into(), flags: 0, shader: String::new(),
                parent_index: -1, children: Vec::new(),
            })
            .collect::<Vec<_>>();
        let mut surfaces = vec![torso, hips];
        for (name, center) in marker_names_and_centers {
            let surface_index = hierarchy.len();
            hierarchy.push(GlmSurfaceHierarchy {
                name: name.into(), flags: 0, shader: String::new(),
                parent_index: -1, children: Vec::new(),
            });
            surfaces.push(GlmSurface {
                surface_index,
                vertices: vec![GlmVertex {
                    position: center, normal: [0.0, 1.0, 0.0],
                    weights: vec![GlmWeight { local_bone_index: 0, weight: 1.0 }],
                }],
                texcoords: vec![[0.0, 0.0]],
                triangles: Vec::new(),
                bone_references: vec![0],
            });
        }
        GlmModel {
            name: "models/players/raven/test.glm".into(),
            anim_name: source.anim_name,
            num_bones: source.num_bones,
            hierarchy,
            lods: vec![GlmLod { surfaces }],
        }
    }

    #[test]
    fn stock_jka_still_fits_from_raven_markers() {
        let gla = synthetic_source_gla();
        let glm = synthetic_raven_glm();
        let profile = JiggleProfile::auto_humanoid(&glm, &gla)
            .unwrap().expect("Raven fixture should have four regions");
        assert_eq!(profile.region_count(), 4);
        assert!(profile.affects_surface(0, 0));
        assert!(profile.affects_surface(0, 1));
        assert!(!profile.affects_surface(0, 2)); // attachment marker
        let breast = profile.regions.iter().find(|r| r.name == "breast_left").unwrap();
        let glute = profile.regions.iter().find(|r| r.name == "glute_left").unwrap();
        assert!((breast.center[0] - 3.0).abs() < 0.001);
        assert!((breast.radius[0] - 3.54).abs() < 0.001);
        assert!((glute.center[0] - 2.975).abs() < 0.001);
        assert!((glute.radius[0] - 4.725).abs() < 0.001);
    }

    #[test]
    fn source_and_raven_share_anatomical_motion_parameters() {
        let gla = synthetic_source_gla();
        let raven = JiggleProfile::auto_humanoid(&synthetic_raven_glm(), &gla)
            .unwrap().unwrap();
        let source = JiggleProfile::auto_source_humanoid(&synthetic_source_glm(), &gla, None)
            .unwrap().unwrap();
        for source_region in &source.regions {
            let stock = raven.regions.iter()
                .find(|r| r.name == source_region.name).unwrap();
            assert_eq!(source_region.response, stock.response);
            assert_eq!(source_region.stiffness, stock.stiffness);
            assert_eq!(source_region.damping, stock.damping);
            assert_eq!(source_region.max_offset, stock.max_offset);
            assert_eq!(source_region.strength, stock.strength);
            assert_eq!(source_region.solver_root_bone, stock.solver_root_bone);
        }
    }

    #[test]
    fn source_jiggle_uses_geometry_not_raven_surface_names() {
        let gla = synthetic_source_gla();
        let glm = synthetic_source_glm();
        assert!(JiggleProfile::auto_humanoid(&glm, &gla).unwrap().is_none());
        let profile = JiggleProfile::auto_source_humanoid(&glm, &gla, None)
            .expect("Source profile compilation")
            .expect("Source profile should detect weighted torso and hips");
        assert_eq!(profile.region_count(), 4);
        assert!(profile.gpu_supported());
        assert!(profile.affects_surface(0, 0));
        assert!(!profile.affects_surface(0, 1));
        for start in [0, 16, 32, 48] {
            let (region, weight, _) = profile.gpu_vertex_binding(0, 0, start + 5);
            assert!(region < 4 && weight > 0.01,
                "Source body quadrant {start} should receive jiggle, got {region} {weight}");
        }
        assert_eq!(profile.gpu_vertex_binding(0, 1, 0).1, 0.0);
    }

    #[test]
    fn source_size_morph_rejects_unrelated_bones_in_a_shared_body_mesh() {
        let gla = synthetic_source_gla();
        let source = JiggleProfile::auto_source_humanoid(&synthetic_source_glm(), &gla, None)
            .unwrap().unwrap();
        let stock = JiggleProfile::auto_humanoid(&synthetic_raven_glm(), &gla)
            .unwrap().unwrap();
        // Synthetic model's pelvis (0) and thoracic (3) share one Source
        // body mesh with unrelated vertices (10 stands in for a hand bone).
        let bones = |bone| [bone, 0, 0, 0];
        let weights = [1.0, 0.0, 0.0, 0.0];
        assert_eq!(source.size_skin_gate(true, &bones(0), &weights, 1), 1.0);
        assert_eq!(source.size_skin_gate(true, &bones(3), &weights, 1), 0.0);
        assert_eq!(source.size_skin_gate(true, &bones(10), &weights, 1), 0.0);
        assert_eq!(source.size_skin_gate(false, &bones(3), &weights, 1), 1.0);
        assert_eq!(source.size_skin_gate(false, &bones(10), &weights, 1), 0.0);
        // A partial hip influence gets a smooth fade rather than creating a
        // hard split where neighboring vertices have mixed skin weights.
        let midway = source.size_skin_gate(true, &[0, 10, 0, 0], &[0.24, 0.76, 0.0, 0.0], 2);
        assert!(midway > 0.0 && midway < 1.0);
        // JKA retains its old surface-local shape semantics unchanged.
        assert_eq!(stock.size_skin_gate(true, &bones(10), &weights, 1), 1.0);
    }

    #[test]
    fn source_jiggle_rebased_pivots_are_used_for_anatomical_search() {
        let gla = synthetic_source_gla();
        let mut glm = synthetic_source_glm();
        // A taller Source rest skeleton, expressed in its own converted GLM
        // coordinates. Without pivots the model's tissue is outside the GLA
        // chest/hip windows; with pivots it has valid search windows.
        for surface in &mut glm.lods[0].surfaces {
            for vertex in &mut surface.vertices {
                vertex.position[2] += 20.0;
            }
        }
        assert!(JiggleProfile::auto_source_humanoid(&glm, &gla, None).unwrap().is_none());
        let pivots = gla.skeleton.iter().map(|bone| {
            [bone.base_pose[0][3], bone.base_pose[1][3], bone.base_pose[2][3] + 20.0]
        }).collect::<Vec<_>>();
        let profile = JiggleProfile::auto_source_humanoid(&glm, &gla, Some(&pivots))
            .unwrap().expect("native pivots must track shifted anatomy");
        assert_eq!(profile.region_count(), 4);
    }

    #[test]
    fn worker_snapshot_contains_raw_offset_at_actual_solve_time() {
        let profile = JiggleProfile::worker_test_profile();
        let pose = [[[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]];
        let axis = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let mut solver = JiggleSystem::default();
        solver.set_config(true, 60, 4, JiggleTuning::default());
        solver.simulate_snapshot(7, "jan", &profile, &pose, axis, [0.0; 3], 0);
        // No fixed step will run at 8ms. A presentation interpolation inside
        // the solver would incorrectly blend this known offset toward rest.
        solver.entities.get_mut(&(7, "jan".into())).unwrap().regions[0].offset = [2.0, 0.0, 0.0];
        let snapshot = solver.simulate_snapshot(7, "jan", &profile, &pose, axis, [0.0; 3], 8);
        assert_eq!(snapshot.offsets, vec![[2.0, 0.0, 0.0]]);
        assert!(snapshot.time_ms.abs() < 0.00001);
    }

    #[test]
    fn locomotion_impulse_cap_bounds_step_rise_and_stop_at_each_rate() {
        for hz in [30.0, 60.0, 120.0, 240.0] {
            let dt = 1.0 / hz;
            let rise = locomotion_kick([0.0, 0.0, 18.0], [0.0; 3], 1.0, 0.75, dt);
            let stop = locomotion_kick([0.0; 3], [0.0, 0.0, 18.0], 1.0, 0.75, dt);
            assert!((length(rise) / dt - 0.75 * 60.0).abs() < 0.001);
            assert_eq!(stop, mul(rise, -1.0));
            assert_eq!(
                locomotion_kick([0.0, 0.0, 18.0], [0.0; 3], 1.0, 0.0, dt),
                [0.0; 3]
            );
        }
    }

    #[test]
    fn locomotion_impulse_cap_preserves_small_inputs_direction_and_steady_travel() {
        let dt = 1.0 / 60.0;
        assert_eq!(
            locomotion_kick([0.2, 0.0, 0.0], [0.0; 3], 1.0, 0.75, dt),
            [-0.2, 0.0, 0.0]
        );
        let steady = [320.0 * dt, 0.0, 0.0];
        assert_eq!(locomotion_kick(steady, steady, 1.0, 0.75, dt), [0.0; 3]);
        let diagonal = locomotion_kick([30.0, 40.0, 0.0], [0.0; 3], 1.0, 0.75, dt);
        assert!((diagonal[0] + 0.45).abs() < 0.00001);
        assert!((diagonal[1] + 0.6).abs() < 0.00001);
    }

    #[test]
    fn ellipsoid_side_split_is_stable() {
        assert!(region_side_matches(RegionSide::NegativeX, -0.1));
        assert!(!region_side_matches(RegionSide::NegativeX, 0.1));
        assert!(region_side_matches(RegionSide::PositiveX, 0.1));
        assert!(!region_side_matches(RegionSide::PositiveX, -0.1));
    }

    #[test]
    fn world_model_round_trip_for_identity_axis() {
        let axis = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let point = [1.0, 2.0, 3.0];
        assert_eq!(model_to_jka_world(point, axis, [10.0, 20.0, 30.0]), [11.0, 22.0, 33.0]);
        assert_eq!(world_vector_to_model(point, axis), point);
    }
}

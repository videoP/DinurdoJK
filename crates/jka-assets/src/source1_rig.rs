//! Rebind a Source 1 model onto either supplied Ghoul2 humanoid skeleton.
//! The target GLA defines mesh indices/bind frames, independently of runtime
//! gameplay animation. JK2 targets preserve five digits, third joints and toes;
//! JKA targets retain the original reduced compatibility mapping.
//!
//! `Direction` is a rest-pose **animation of the SOURCE skeleton** into JKA's
//! anatomical limb directions, NOT independent per-JKA-joint translations.
//! We retain the Source hierarchy, its original segment lengths, shoulder/hip
//! location, vertex skin weights, and bone-local offsets; only upper/lower limbs
//! are rotated at their own native pivots toward the corresponding GLA segment
//! directions. The Source geometry is baked in this posed shape and influences
//! are THEN remapped onto GLA indices for ordinary Ghoul2 runtime animation.
//!
//! This avoids the mismatched-body-proportion shearing introduced by seating
//! every Source joint at the stock GLA position: Juliet's neck, pelvis and belt
//! are especially sensitive to those unrelated joint translations.
//!
//! Because JKA animation still uses `_humanoid` joints, extreme differences in
//! character proportions can leave the runtime pivot offset from the mesh.
//! Neutral silhouette parity is not proof of animation parity; animate actual
//! stock GLA frames before releasing a finished model.
use crate::ghoul2::{
    GlaAnimation, GlmLod, GlmModel, GlmSurface, GlmSurfaceHierarchy, GlmVertex, GlmWeight,
    Matrix3x4,
};
use crate::source1::{SourceJiggleParams, SourceMaterialTable, SourceSkeleton};
use crate::source1_geometry::{
    assemble_source_lod, parse_source_bodyparts, source_lod_vertices, SourceVtx, SourceVvd,
};
use std::collections::BTreeMap;

/// Ghoul2 packs at most four weights per vertex.
const MAX_WEIGHTS: usize = 4;

/// How a Source bone's geometry is re-placed onto its mapped GLA bone. All three
/// put the mesh in base-pose space; they differ in how much of the rest pose they
/// correct, and therefore in how much they reshape the model.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SourceRebindMode {
    /// Pose the Source skeleton with hierarchical forward kinematics into JKA
    /// arm/leg aim directions, preserving original segment lengths and shape.
    /// The rest mesh is baked before Source weights are mapped to GLA joints.
    #[default]
    Direction,
    /// LEGACY diagnostic: move each GLA joint independently. Preserves Source shape
    /// (median 1.3 units on HEV MK2) but leaves limbs pointing along the Source
    /// rest pose, so where the two rest poses genuinely disagree - `_humanoid`
    /// splays the legs, most Source bind poses do not - animation is applied to a
    /// limb that starts in the wrong direction.
    Translation,
    /// Adopt the GLA bone's full rest frame. Applies the two skeletons'
    /// axis-convention difference as a rotation and badly warps the mesh (median
    /// 6.7 units, max 14.7 on HEV MK2). Retained only so the regression can be
    /// reproduced and measured; never use it for a real character.
    BoneFrame,
}

impl SourceRebindMode {
    /// `source_model.json` `"rebind"` value.
    pub fn from_descriptor(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "direction" => Self::Direction,
            "translation" => Self::Translation,
            "bone_frame" | "boneframe" => Self::BoneFrame,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct SourceRigOptions {
    /// `$bodygroup` choice per bodypart; missing entries select choice 0.
    pub bodygroups: Vec<usize>,
    /// `$texturegroup` row used to resolve each mesh's skin reference.
    pub skin_family: usize,
    /// Upper bound on how many Source LODs become GLM LODs.
    pub max_lods: usize,
    /// Uniform Source-to-JKA scale. `None` fits it from the two skeletons.
    pub scale: Option<f32>,
    pub rebind: SourceRebindMode,
}

impl Default for SourceRigOptions {
    fn default() -> Self {
        Self {
            bodygroups: Vec::new(),
            skin_family: 0,
            max_lods: crate::source1_geometry::SOURCE_MAX_LODS,
            scale: None,
            rebind: SourceRebindMode::default(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SourceRigSurface {
    pub name: String,
    /// Candidate `materials/....vmt` qpaths, in `$cdmaterials` search order.
    pub vmt_candidates: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct SourceLodStats {
    pub lod: usize,
    pub surfaces: usize,
    pub vertices: usize,
    pub triangles: usize,
}

#[derive(Clone, Debug, Default)]
pub struct SourceRigDiagnostics {
    pub model_name: String,
    pub source_bones: usize,
    /// Source bones matched to a GLA bone by name or by the spine resolver.
    pub mapped_bones: usize,
    /// Source bones with no rule of their own, following a mapped ancestor.
    pub inherited_bones: Vec<(String, String)>,
    /// Uniform scale actually used, and whether it was fitted or configured.
    pub scale: f32,
    pub scale_was_fitted: bool,
    /// Residual of the skeleton alignment fit: how differently the two
    /// skeletons are proportioned, in JKA units.
    pub fit_pairs: usize,
    pub fit_yaw_degrees: f32,
    pub fit_rms: f32,
    pub fit_worst: Option<(String, String, f32)>,
    /// Largest disagreement between StudioMDL's authored `poseToBone` and the
    /// inverse derived from the bind chain. Large values mean the two disagree
    /// about the bind pose itself.
    pub max_inverse_bind_error: f32,
    /// Which rebind construction was used.
    pub rebind: SourceRebindMode,
    /// `_humanoid` joints whose rest direction was corrected onto the Source
    /// bone that owns them. The rest are leaves, or joints whose owner's children
    /// all share its GLA bone. Counted per joint, not per bone, because a joint
    /// gets one matrix however many Source bones land on it.
    pub direction_corrected: usize,
    /// Vertices whose merged GLA influence count exceeded Ghoul2's four. Source
    /// stores at most three influences per vertex and merging can only reduce
    /// that, so this must be 0; a nonzero value means the merge is wrong.
    pub weight_truncations: usize,
    /// Procedural Source bones retained as independent GPU skin influences.
    pub source_jiggle_bones: usize,
    pub source_jiggle_weighted_vertices: usize,
    /// Baked rest-pose bounds in JKA model units.
    pub rest_bounds: ([f32; 3], [f32; 3]),
    /// Largest distance any vertex moved relative to merely scaling and aligning
    /// the Source mesh, i.e. how much adopting the GLA joint centres reshaped the
    /// model. A few units is the expected cost of the rebind; tens of units means
    /// the bone correspondence is wrong.
    pub max_rebind_displacement: f32,
    pub lods: Vec<SourceLodStats>,
}

/// One extra skinning bone, following the ordinary GLA palette. The extra
/// index is `gla.skeleton.len() + index_in_source_jiggles`. Never mutate the
/// stock JK2/JKA GLA with these optional Source-specific bones.
#[derive(Clone, Debug)]
pub struct SourceJiggleBinding {
    pub name: String,
    pub parent_gla: usize,
    /// The closest procedural Source ancestor, if any; always earlier in the list.
    pub parent_jiggle: Option<usize>,
    /// Authored Source bone's neutral frame in the baked JKA model space.
    pub rest_frame: Matrix3x4,
    pub parameters: SourceJiggleParams,
    /// Authored +Z jiggle tip length converted from Source units to JKA units.
    pub length: f32,
}

/// One vertex of a rig input, in its source format's model space, with every
/// skinning influence as (source bone index, weight).
#[derive(Clone, Debug)]
pub struct RigVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub influences: Vec<(usize, f32)>,
}

/// One drawable surface's triangles in one LOD, indexing that LOD's vertex pool.
/// `key` identifies the surface across LODs.
#[derive(Clone, Debug)]
pub struct RigDraw {
    pub key: (usize, usize, usize),
    pub indices: Vec<u32>,
}

/// A surface as named by LOD0, in GLM surface order.
#[derive(Clone, Debug)]
pub struct RigSurfaceSpec {
    pub key: (usize, usize, usize),
    pub name: String,
    /// GLM hierarchy shader (a JKA shader or image path, or a Source texture name).
    pub shader: String,
    pub vmt_candidates: Vec<String>,
}

/// Format-neutral character geometry: everything [`build_rig`] needs besides
/// the skeleton. Source models and Unreal skeletal meshes both convert to it.
#[derive(Clone, Debug, Default)]
pub struct RigGeometry {
    pub surfaces: Vec<RigSurfaceSpec>,
    /// Per LOD: the vertex pool and the draws indexing it.
    pub lods: Vec<(Vec<RigVertex>, Vec<RigDraw>)>,
}

pub struct SourceRig {
    pub source_jiggles: Vec<SourceJiggleBinding>,
    pub glm: GlmModel,
    /// Per-model, anatomical GLA joint anchors in the newly posed Source mesh.
    /// Pass to `rebase_source_ghoul2_pose` AFTER normal Ghoul2 evaluation.
    /// `None` for diagnostic legacy rebind modes.
    pub native_pivots: Option<Vec<[f32; 3]>>,
    /// Indexed by GLM surface index.
    pub surfaces: Vec<SourceRigSurface>,
    pub diagnostics: SourceRigDiagnostics,
}

fn mul(a: &Matrix3x4, b: &Matrix3x4) -> Matrix3x4 {
    let mut out = [[0.0f32; 4]; 3];
    for r in 0..3 {
        for c in 0..3 {
            out[r][c] = (0..3).map(|k| a[r][k] * b[k][c]).sum();
        }
        out[r][3] = a[r][3] + (0..3).map(|k| a[r][k] * b[k][3]).sum::<f32>();
    }
    out
}

/// Inverse of a rigid (rotation + translation) 3x4 matrix.
fn rigid_inverse(m: &Matrix3x4) -> Matrix3x4 {
    let mut out = [[0.0f32; 4]; 3];
    for r in 0..3 {
        for c in 0..3 {
            out[r][c] = m[c][r];
        }
        out[r][3] = -(0..3).map(|k| out[r][k] * m[k][3]).sum::<f32>();
    }
    out
}

fn transform(m: &Matrix3x4, p: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2] + m[0][3],
        m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2] + m[1][3],
        m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2] + m[2][3],
    ]
}

fn rotate(m: &Matrix3x4, v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

fn origin(m: &Matrix3x4) -> [f32; 3] {
    [m[0][3], m[1][3], m[2][3]]
}

fn identity() -> Matrix3x4 {
    [
        [1., 0., 0., 0.],
        [0., 1., 0., 0.],
        [0., 0., 1., 0.],
    ]
}

fn translation(t: [f32; 3]) -> Matrix3x4 {
    [
        [1., 0., 0., t[0]],
        [0., 1., 0., t[1]],
        [0., 0., 1., t[2]],
    ]
}

fn normalized(v: [f32; 3]) -> Option<[f32; 3]> {
    let length = v.iter().map(|c| c * c).sum::<f32>().sqrt();
    (length.is_finite() && length > 1e-6).then(|| v.map(|c| c / length))
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// The minimal rotation taking unit vector `from` onto unit vector `to`, as a
/// linear 3x4 with no translation (Rodrigues' formula). Convention-free: it
/// introduces no roll about the axis, so it corrects a bone's direction without
/// imposing either skeleton's idea of which way is "up" along the bone.
fn minimal_rotation(from: [f32; 3], to: [f32; 3]) -> Matrix3x4 {
    let (Some(a), Some(b)) = (normalized(from), normalized(to)) else {
        return identity();
    };
    let dot = (0..3).map(|k| a[k] * b[k]).sum::<f32>().clamp(-1.0, 1.0);
    if dot > 1.0 - 1e-7 {
        return identity();
    }
    let axis = if dot < -1.0 + 1e-7 {
        // Antiparallel: any axis perpendicular to `a` gives a 180 degree turn.
        let helper = if a[0].abs() < 0.9 {
            [1., 0., 0.]
        } else {
            [0., 1., 0.]
        };
        match normalized(cross(a, helper)) {
            Some(axis) => axis,
            None => return identity(),
        }
    } else {
        match normalized(cross(a, b)) {
            Some(axis) => axis,
            None => return identity(),
        }
    };
    let (sin, cos) = dot.acos().sin_cos();
    let k = [
        [0., -axis[2], axis[1]],
        [axis[2], 0., -axis[0]],
        [-axis[1], axis[0], 0.],
    ];
    let mut out = identity();
    for r in 0..3 {
        for c in 0..3 {
            let k2 = (0..3).map(|m| k[r][m] * k[m][c]).sum::<f32>();
            out[r][c] = (if r == c { 1.0 } else { 0.0 }) + sin * k[r][c] + (1.0 - cos) * k2;
        }
    }
    out
}

/// A palm's forward and across axes can be recovered from joint landmarks;
/// comparing these directions, rather than Source/Ghoul2 bone frame matrices,
/// removes the characteristic backwards-rolled Source wrist. Both skeletons
/// use their OWN anatomical joint positions, so no segment length is changed.
fn palm_frame(wrist: [f32; 3], index: [f32; 3], outer: [f32; 3]) -> Option<[[f32; 3]; 3]> {
    let forward = normalized(std::array::from_fn(|k| (index[k] + outer[k]) * 0.5 - wrist[k]))?;
    let side: [f32; 3] = std::array::from_fn(|k| outer[k] - index[k]);
    let projection = (0..3).map(|k| side[k] * forward[k]).sum::<f32>();
    let side = normalized(std::array::from_fn(|k| side[k] - projection * forward[k]))?;
    let normal = normalized(cross(forward, side))?;
    Some([forward, side, normal])
}

fn rotation_between_frames(from: [[f32; 3]; 3], to: [[f32; 3]; 3]) -> Matrix3x4 {
    let mut rotation = identity();
    for row in 0..3 {
        for col in 0..3 {
            rotation[row][col] = (0..3).map(|axis| to[axis][row] * from[axis][col]).sum();
        }
    }
    rotation
}

/// Extract the Source digit and its level. Five Source digits map to JKA's
/// three animated digits; each Source digit keeps its own native mesh offsets.
fn source_digit(name: &str) -> Option<(char, u8, u8)> {
    let side = name.chars().next()?;
    if side != 'l' && side != 'r' { return None; }
    let tail = name.get(1..)?.strip_prefix("finger")?;
    let mut bytes = tail.bytes();
    let digit = bytes.next()?.wrapping_sub(b'0');
    if digit > 4 { return None; }
    let level = match bytes.next() {
        None => 0,
        Some(b'1') if bytes.next().is_none() => 1,
        _ => return None,
    };
    Some((side, digit, level))
}

fn quaternion_matrix(q: [f32; 4], position: [f32; 3]) -> Result<Matrix3x4, String> {
    let [x, y, z, w] = q;
    let norm = x * x + y * y + z * z + w * w;
    if !norm.is_finite() || norm < 1e-12 || !position.iter().all(|v| v.is_finite()) {
        return Err("Source bind quaternion or position is invalid".into());
    }
    let s = 2.0 / norm;
    Ok([
        [
            1. - s * (y * y + z * z),
            s * (x * y - z * w),
            s * (x * z + y * w),
            position[0],
        ],
        [
            s * (x * y + z * w),
            1. - s * (x * x + z * z),
            s * (y * z - x * w),
            position[1],
        ],
        [
            s * (x * z - y * w),
            s * (y * z + x * w),
            1. - s * (x * x + y * y),
            position[2],
        ],
    ])
}

/// The rotation (as a unit quaternion, `[x, y, z, w]`) a rigid 3x4 matrix
/// applies, by the numerically stable largest-diagonal-term method (Shepperd).
fn matrix_to_quaternion(m: &Matrix3x4) -> [f32; 4] {
    let (m00, m01, m02) = (m[0][0], m[0][1], m[0][2]);
    let (m10, m11, m12) = (m[1][0], m[1][1], m[1][2]);
    let (m20, m21, m22) = (m[2][0], m[2][1], m[2][2]);
    let trace = m00 + m11 + m22;
    if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        [(m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, 0.25 * s]
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).max(0.0).sqrt() * 2.0;
        [0.25 * s, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s]
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).max(0.0).sqrt() * 2.0;
        [(m01 + m10) / s, 0.25 * s, (m12 + m21) / s, (m02 - m20) / s]
    } else {
        let s = (1.0 + m22 - m00 - m11).max(0.0).sqrt() * 2.0;
        [(m02 + m20) / s, (m12 + m21) / s, 0.25 * s, (m10 - m01) / s]
    }
}

fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let (ax, ay, az, aw) = (a[0], a[1], a[2], a[3]);
    let (bx, by, bz, bw) = (b[0], b[1], b[2], b[3]);
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

fn quat_conjugate(q: [f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], -q[2], q[3]]
}

/// Decompose a uniform-scale affine transform into a unit-length rigid dual
/// quaternion and a *separate* positive uniform scale. A dual quaternion does
/// not encode scale. Feeding the scaled 3x3 directly into Shepperd's rotation
/// decoder silently produces the wrong rotation; converting it back to a rigid
/// matrix then also drops the fitted Source->JKA scale entirely.
///
/// Every rebind_matrix mode currently creates a rotation times a uniform scale
/// (BoneFrame additionally carries the authored GLA base-pose scale). Recover
/// that scale from the matrix itself rather than assuming the fitted scale is
/// also the complete per-bone scale.
fn dual_quat_from_scaled_matrix(m: &Matrix3x4) -> ([f32; 4], [f32; 4], f32) {
    let scale = (0..3).map(|r| m[r][0] * m[r][0]).sum::<f32>().sqrt();
    // build_source_rig checks its fitted scale, and rebind matrices only contain
    // proper positive uniform scales; retain explicit protection in debug builds.
    debug_assert!(scale.is_finite() && scale > 1e-6);
    let mut rigid = *m;
    for row in &mut rigid {
        for component in &mut row[..3] {
            *component /= scale;
        }
    }
    let real = matrix_to_quaternion(&rigid);
    let t = [m[0][3], m[1][3], m[2][3], 0.0];
    let dual = quat_mul(t, real).map(|c| c * 0.5);
    (real, dual, scale)
}

/// Weighted blend of dual quaternions representing several bones' rebind
/// matrices, for one vertex. Each part is `(weight, real, dual)`.
///
/// Quaternions `q` and `-q` represent the same rotation, but average to zero
/// if blended as-is; every part is first flipped into the same hemisphere as
/// the first (by construction, its highest-weighted bone is pushed first by
/// the caller) so the blend takes the shortest path between them, same as the
/// `minimal_rotation` correction elsewhere in this module.
fn blend_dual_quats(parts: &[(f32, [f32; 4], [f32; 4])]) -> ([f32; 4], [f32; 4]) {
    let reference = parts[0].1;
    let mut real = [0.0f32; 4];
    let mut dual = [0.0f32; 4];
    for &(weight, part_real, part_dual) in parts {
        let dot: f32 = (0..4).map(|k| part_real[k] * reference[k]).sum();
        let sign = if dot < 0.0 { -weight } else { weight };
        for k in 0..4 {
            real[k] += sign * part_real[k];
            dual[k] += sign * part_dual[k];
        }
    }
    let len = (0..4).map(|k| real[k] * real[k]).sum::<f32>().sqrt().max(1e-9);
    (real.map(|c| c / len), dual.map(|c| c / len))
}

/// The equivalent single rigid matrix for a blended dual quaternion.
fn dual_quat_to_matrix(real: [f32; 4], dual: [f32; 4]) -> Matrix3x4 {
    let t = quat_mul(dual, quat_conjugate(real)).map(|c| c * 2.0);
    quaternion_matrix(real, [t[0], t[1], t[2]]).unwrap_or_else(|_| identity())
}

/// Source bone rest transforms in Source model space, composed down the chain.
fn source_rest_world(skeleton: &SourceSkeleton) -> Result<Vec<Matrix3x4>, String> {
    let mut world = Vec::with_capacity(skeleton.bones.len());
    for (i, bone) in skeleton.bones.iter().enumerate() {
        let local = quaternion_matrix(bone.bind_quaternion, bone.bind_position)?;
        let absolute = match bone.parent {
            Some(parent) if parent < i => mul(&world[parent], &local),
            Some(parent) => {
                return Err(format!(
                    "Source bone {i} has non-ancestral parent {parent}"
                ))
            }
            None => local,
        };
        world.push(absolute);
    }
    Ok(world)
}

/// Lower-case alphanumeric tail of a ValveBiped bone name:
/// `ValveBiped.Bip01_L_UpperArm` becomes `lupperarm`.
fn normalize_source_bone(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    let tail = lower
        .strip_prefix("valvebiped.")
        .unwrap_or(&lower)
        .to_owned();
    let tail = tail.strip_prefix("bip01_").unwrap_or(&tail);
    tail.chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

/// Reusable ValveBiped to `_humanoid` correspondence. Model-independent by
/// construction: it is keyed on the standard ValveBiped names, never on any one
/// character's bone table.
///
/// `_humanoid` splits two joints that ValveBiped keeps whole. `lfemurX` and
/// `lhumerusX` are twist helpers hanging off `lfemurYZ`/`lhumerus` as *siblings*
/// of the next real joint, so the load-bearing chain is
/// `pelvis -> lfemurYZ -> ltibia -> ltalus` and
/// `thoracic -> lhumerus -> lradius -> lhand`. Those are the bones a limb must
/// follow; the `X` helpers only add skin twist and are deliberately not targets.
fn humanoid_bone_for(normalized: &str) -> Option<&'static str> {
    Some(match normalized {
        "pelvis" => "pelvis",
        // Spine* is resolved positionally; see `resolve_spine_chain`.
        "neck" | "neck1" | "neck2" => "cervical",
        "head" | "head1" | "forward" => "cranium",
        "lclavicle" => "lclavical",
        "rclavicle" => "rclavical",
        "lupperarm" => "lhumerus",
        "rupperarm" => "rhumerus",
        "lforearm" => "lradius",
        "rforearm" => "rradius",
        "lhand" | "animattachmentlh" => "lhand",
        "rhand" | "animattachmentrh" => "rhand",
        "lthigh" => "lfemurYZ",
        "rthigh" => "rfemurYZ",
        "lcalf" => "ltibia",
        "rcalf" => "rtibia",
        // `_humanoid` has no toe bone; the ankle carries the whole foot.
        "lfoot" | "ltoe0" | "ltoe" => "ltalus",
        "rfoot" | "rtoe0" | "rtoe" => "rtalus",
        // ValveBiped numbers digits thumb..pinky. `_humanoid` only carries a
        // thumb (d1), an index (d2) and an outer finger (d4), so middle folds
        // onto the index and ring onto the outer digit.
        "lfinger0" => "l_d1_j1",
        "lfinger01" | "lfinger02" => "l_d1_j2",
        "lfinger1" | "lfinger2" => "l_d2_j1",
        "lfinger11" | "lfinger12" | "lfinger21" | "lfinger22" => "l_d2_j2",
        "lfinger3" | "lfinger4" => "l_d4_j1",
        "lfinger31" | "lfinger32" | "lfinger41" | "lfinger42" => "l_d4_j2",
        "rfinger0" => "r_d1_j1",
        "rfinger01" | "rfinger02" => "r_d1_j2",
        "rfinger1" | "rfinger2" => "r_d2_j1",
        "rfinger11" | "rfinger12" | "rfinger21" | "rfinger22" => "r_d2_j2",
        "rfinger3" | "rfinger4" => "r_d4_j1",
        "rfinger31" | "rfinger32" | "rfinger41" | "rfinger42" => "r_d4_j2",
        _ => return None,
    })
}

/// Prefer all five independent digits, three phalanges and toes on JK2.
/// The original reduced mapping remains the 53-bone compatibility profile.
fn humanoid_target_for(gla: &GlaAnimation, normalized: &str) -> Option<String> {
    let digit = source_digit(normalized).or_else(|| {
        let stem = normalized.strip_suffix('2')?;
        source_digit(stem).filter(|(_, _, level)| *level == 0)
            .map(|(side, digit, _)| (side, digit, 2))
    });
    if let Some((side, digit, level)) = digit.filter(|_| gla.num_bones() == 72) {
        let name = format!("{side}_d{}_j{}", digit + 1, level + 1);
        if gla_bone_index(gla, &name).is_some() { return Some(name); }
    }
    let toe = match normalized {
        "ltoe" | "ltoe0" => Some("ltarsal"),
        "rtoe" | "rtoe0" => Some("rtarsal"),
        _ => None,
    };
    if let Some(name) = toe.filter(|name| gla_bone_index(gla, name).is_some()) {
        return Some(name.to_owned());
    }
    humanoid_bone_for(normalized).map(str::to_owned)
}

fn pivot_parent(gla: &GlaAnimation, bone: usize) -> Option<usize> {
    crate::skeleton::mesh_pose_parent(&gla.skeleton, bone)
}

/// `_humanoid` spine bones between the pelvis and the neck, root first.
const HUMANOID_SPINE: [&str; 3] = ["lower_lumbar", "upper_lumbar", "thoracic"];

/// The anchors used to fit the Source-to-JKA alignment. Deliberately excludes
/// the spine (whose subdivision differs per model and is resolved from the fit)
/// and every leaf helper, attachment and digit, which would bias it.
const FIT_ANCHORS: [&str; 17] = [
    "pelvis",
    "neck1",
    "head1",
    "lclavicle",
    "rclavicle",
    "lupperarm",
    "rupperarm",
    "lforearm",
    "rforearm",
    "lhand",
    "rhand",
    "lthigh",
    "rthigh",
    "lcalf",
    "rcalf",
    "lfoot",
    "rfoot",
];

fn gla_bone_index(gla: &GlaAnimation, name: &str) -> Option<usize> {
    gla.skeleton
        .iter()
        .position(|bone| bone.name.eq_ignore_ascii_case(name))
}

/// Best-fit similarity (yaw about Z, uniform scale, translation) taking Source
/// anchor positions onto their `_humanoid` counterparts. Only the scale is used
/// by the rebind - the rebind places every bone through its GLA base pose, so it
/// is orientation-agnostic by construction - but the fitted yaw and residual are
/// the honest measure of whether the bone correspondence is sane at all.
#[derive(Debug)]
struct SkeletonFit {
    scale: f32,
    yaw_degrees: f32,
    rms: f32,
    pairs: usize,
    worst: Option<(String, String, f32)>,
    /// The fitted similarity itself, so callers do not re-derive it.
    transform: Matrix3x4,
    /// Anchor centroids the similarity was built around, so it can be rebuilt at a
    /// different scale without sliding the model off the skeleton.
    source_centroid: [f32; 3],
    target_centroid: [f32; 3],
}

impl SkeletonFit {
    /// Source model space to JKA model space under the fitted similarity.
    fn align(&self, p: [f32; 3]) -> [f32; 3] {
        transform(&self.transform, p)
    }
}

fn fit_skeletons(
    anchors: &[(String, String, [f32; 3], [f32; 3])],
) -> Result<SkeletonFit, String> {
    let n = anchors.len();
    if n < 4 {
        return Err(format!(
            "only {n} ValveBiped anchor bones matched `_humanoid`; this does not look like a humanoid Source model"
        ));
    }
    let mean = |pick: fn(&(String, String, [f32; 3], [f32; 3])) -> [f32; 3]| {
        let mut c = [0.0f32; 3];
        for a in anchors {
            let p = pick(a);
            for k in 0..3 {
                c[k] += p[k];
            }
        }
        c.map(|v| v / n as f32)
    };
    let cs = mean(|a| a.2);
    let cj = mean(|a| a.3);
    let mut cross = 0.0f32;
    let mut dot_xy = 0.0f32;
    let mut src_norm = 0.0f32;
    for (_, _, s, j) in anchors {
        let (sx, sy) = (s[0] - cs[0], s[1] - cs[1]);
        let (jx, jy) = (j[0] - cj[0], j[1] - cj[1]);
        cross += sx * jy - sy * jx;
        dot_xy += sx * jx + sy * jy;
        src_norm += (0..3).map(|k| (s[k] - cs[k]).powi(2)).sum::<f32>();
    }
    if src_norm <= f32::EPSILON {
        return Err("Source anchor bones are coincident".into());
    }
    let yaw = cross.atan2(dot_xy);
    let (sin, cos) = yaw.sin_cos();
    let mut projection = 0.0f32;
    for (_, _, s, j) in anchors {
        let (sx, sy, sz) = (s[0] - cs[0], s[1] - cs[1], s[2] - cs[2]);
        let (jx, jy, jz) = (j[0] - cj[0], j[1] - cj[1], j[2] - cj[2]);
        let rotated = [cos * sx - sin * sy, sin * sx + cos * sy, sz];
        projection += rotated[0] * jx + rotated[1] * jy + rotated[2] * jz;
    }
    let scale = projection / src_norm;
    if !scale.is_finite() || scale <= 1e-4 {
        return Err(format!("Source-to-JKA skeleton fit produced a degenerate scale {scale}"));
    }
    let linear = [
        [cos * scale, -sin * scale, 0.0],
        [sin * scale, cos * scale, 0.0],
        [0.0, 0.0, scale],
    ];
    let mut fitted = [[0.0f32; 4]; 3];
    for r in 0..3 {
        fitted[r][..3].copy_from_slice(&linear[r]);
        fitted[r][3] = cj[r] - (0..3).map(|k| linear[r][k] * cs[k]).sum::<f32>();
    }
    let mut total = 0.0f32;
    let mut worst: Option<(String, String, f32)> = None;
    for (src_name, gla_name, s, j) in anchors {
        let mapped = transform(&fitted, *s);
        let error = (0..3)
            .map(|k| (mapped[k] - j[k]).powi(2))
            .sum::<f32>()
            .sqrt();
        total += error * error;
        if worst.as_ref().is_none_or(|(_, _, e)| error > *e) {
            worst = Some((src_name.clone(), gla_name.clone(), error));
        }
    }
    Ok(SkeletonFit {
        scale,
        yaw_degrees: yaw.to_degrees(),
        rms: (total / n as f32).sqrt(),
        pairs: n,
        worst,
        transform: fitted,
        source_centroid: cs,
        target_centroid: cj,
    })
}

/// Assign each Source `Spine*` bone to the nearest `_humanoid` spine bone once
/// the skeletons are aligned. ValveBiped models carry anywhere from one to five
/// spine bones against `_humanoid`'s three, so a fixed table would squash or
/// stretch the torso of every model whose subdivision differs.
fn resolve_spine_chain(
    gla: &GlaAnimation,
    spine_candidates: &[(usize, [f32; 3])],
    aligned: impl Fn([f32; 3]) -> [f32; 3],
) -> Vec<(usize, usize)> {
    let targets: Vec<(usize, [f32; 3])> = HUMANOID_SPINE
        .iter()
        .filter_map(|name| {
            let index = gla_bone_index(gla, name)?;
            Some((index, origin(&gla.skeleton[index].base_pose)))
        })
        .collect();
    if targets.is_empty() {
        return Vec::new();
    }
    spine_candidates
        .iter()
        .map(|(bone, position)| {
            let p = aligned(*position);
            let best = targets
                .iter()
                .min_by(|a, b| {
                    let d = |t: &(usize, [f32; 3])| {
                        (0..3).map(|k| (t.1[k] - p[k]).powi(2)).sum::<f32>()
                    };
                    d(a).total_cmp(&d(b))
                })
                .expect("targets is non-empty");
            (*bone, best.0)
        })
        .collect()
}

/// Per-Source-bone rebind state.
struct BoneRebind {
    /// GLA bone this Source bone drives.
    gla_bone: usize,
}

/// `_humanoid` splits several chain bones from a sibling twist helper
/// (`lhumerusX`/`rhumerusX`/`lradiusX`/`rradiusX`/`lfemurX`/`rfemurX`) that only
/// adds skin twist. No ValveBiped Source model carries a bone for that helper -
/// every Source file checked weights that skin to the chain bone instead - so
/// without this, every vertex along (say) the forearm is rigidly driven by the
/// chain bone's own rotation instead of easing into the twist the way a real
/// arm skins. Compared vertex-for-vertex in the shared `_humanoid` base-pose
/// space against a hand-ported reference model
/// (`zzzJulietStarling-S.pk3`/`jedi_jstarling`), the hand rig puts roughly 80%
/// of the sleeve's weight mass on the twist helper rather than the chain bone;
/// `twist_blend_fraction` approximates that by blending each chain-bone vertex
/// toward its twist helper by how far along the limb segment - the owning
/// joint to the next joint down the chain - it sits.
///
/// Keyed by chain-bone GLA index: `(twist helper GLA index, chain joint
/// origin, next joint origin)`, both origins in base-pose space (the space the
/// baked mesh already lives in), so this needs building only once per rig, not
/// per vertex.
fn twist_blend_table(gla: &GlaAnimation) -> BTreeMap<usize, (usize, [f32; 3], [f32; 3])> {
    const SIBLINGS: [(&str, &str, &str); 6] = [
        ("lhumerus", "lhumerusX", "lradius"),
        ("rhumerus", "rhumerusX", "rradius"),
        ("lradius", "lradiusX", "lhand"),
        ("rradius", "rradiusX", "rhand"),
        ("lfemurYZ", "lfemurX", "ltibia"),
        ("rfemurYZ", "rfemurX", "rtibia"),
    ];
    let mut table = BTreeMap::new();
    for (chain, twist, next) in SIBLINGS {
        let (Some(chain_index), Some(twist_index), Some(next_index)) = (
            gla_bone_index(gla, chain),
            gla_bone_index(gla, twist),
            gla_bone_index(gla, next),
        ) else {
            continue;
        };
        table.insert(
            chain_index,
            (
                twist_index,
                origin(&gla.skeleton[chain_index].base_pose),
                origin(&gla.skeleton[next_index].base_pose),
            ),
        );
    }
    table
}

/// How far `position` sits from `joint` towards `next_joint`, as a 0..1
/// fraction of that segment's length (clamped, so a vertex behind the joint or
/// past the next one saturates rather than extrapolating).
fn limb_fraction(position: [f32; 3], joint: [f32; 3], next_joint: [f32; 3]) -> f32 {
    let dir: [f32; 3] = std::array::from_fn(|k| next_joint[k] - joint[k]);
    let len2 = dir.iter().map(|c| c * c).sum::<f32>();
    if len2 <= 1e-6 {
        return 0.0;
    }
    let rel: [f32; 3] = std::array::from_fn(|k| position[k] - joint[k]);
    let dot: f32 = (0..3).map(|k| rel[k] * dir[k]).sum();
    (dot / len2).clamp(0.0, 1.0)
}

/// Smoothstep ramp from the owning joint (`t=0`, no twist yet) to the next
/// joint down the limb (`t=1`, twist helper carries most of the skin): see
/// [`twist_blend_table`] for where the 80% ceiling comes from.
fn twist_blend_fraction(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let s = t * t * (3.0 - 2.0 * t);
    0.85 * s
}

/// The Source bone each `_humanoid` joint takes its rebind matrix from.
///
/// Several Source bones routinely map to one joint - ValveBiped's toe onto the
/// ankle, its spine onto three lumbar bones, two fingers onto one digit, and
/// every helper without a rule of its own onto its nearest mapped ancestor. They
/// all animate as that one GLA bone, so they must also share one rebind matrix;
/// building a separate one per bone would seat each bone's own rest joint on the
/// shared GLA joint and collapse the parts onto each other.
///
/// The owner is the lowest-indexed bone on the joint. That is the shallowest one,
/// because [`source_rest_world`] rejects any bone table that does not store
/// parents before children.
fn joint_owners(gla_for_bone: &[Option<usize>]) -> BTreeMap<usize, usize> {
    let mut owners = BTreeMap::<usize, usize>::new();
    for (index, slot) in gla_for_bone.iter().enumerate() {
        if let Some(gla_bone) = slot {
            owners.entry(*gla_bone).or_insert(index);
        }
    }
    owners
}

/// Locate an actual finger knuckle below a Source/UE hand. Many UE rigs
/// insert `index_metacarpal_l` / `pinky_metacarpal_l` (or twist helpers)
/// between `hand_l` and `index_01_l` / `pinky_01_l`. Requiring the finger to
/// be an *immediate* child silently disables wrist/palm roll correction on
/// those rigs. Work from rest-world transforms relative to the hand so an
/// arbitrary number of intermediary joints does not change the landmark.
fn source_hand_knuckle(
    skeleton: &SourceSkeleton,
    names: &[String],
    rest_world: &[Matrix3x4],
    hand_index: usize,
    posed_hand: &Matrix3x4,
    digit: u8,
) -> Option<[f32; 3]> {
    let side = names.get(hand_index)?.chars().next()?;
    if !matches!(side, 'l' | 'r') { return None; }
    let finger_name = format!("{side}finger{digit}");
    let finger_index = skeleton.bones.iter().enumerate().find_map(|(candidate, bone)| {
        if names.get(candidate)?.as_str() != finger_name { return None; }
        let mut parent = bone.parent;
        while let Some(ancestor) = parent {
            if ancestor == hand_index { return Some(candidate); }
            parent = skeleton.bones.get(ancestor)?.parent;
        }
        None
    })?;
    let hand_rest = rest_world.get(hand_index)?;
    let finger_rest = rest_world.get(finger_index)?;
    let finger_local = mul(&rigid_inverse(hand_rest), finger_rest);
    Some(transform(posed_hand, origin(&finger_local)))
}

/// Pose the complete Source bind hierarchy into JKA's anatomical limb
/// directions WITHOUT independently translating bones to GLA joint positions.
///
/// Source rest transforms are world-space and topologically sorted. Parent->
/// child FK retains each authored local transform, with a correction rotation
/// attached to the SOURCE upperarm/forearm/thigh/calf pivot only. Children,
/// outfits, hair, toes, fingers and their weights ride the correctly propagated
/// hierarchy (unlike separately fitted GLA-joint matrices).
///
/// Return one Source->JKA bake transform PER SOURCE BONE, not per mapped GLA
/// bone. Multiple Source bones can share one GLA animation target, but their
/// different Source rest positions must be preserved until pose baking ends.
fn source_fk_rebinds(
    skeleton: &SourceSkeleton,
    rest_world: &[Matrix3x4],
    gla: &GlaAnimation,
    gla_for_bone: &[Option<usize>],
    placement: &Matrix3x4,
) -> (Vec<Matrix3x4>, usize) {
    let names: Vec<String> = skeleton.bones.iter()
        .map(|bone| normalize_source_bone(&bone.name)).collect();
    let mut posed = Vec::<Matrix3x4>::with_capacity(rest_world.len());
    let mut corrected = 0usize;

    for (index, bone) in skeleton.bones.iter().enumerate() {
        // Parent-local composition first. This is the essential difference
        // from independent pelvis/neck/shoulder GLA joint relocation.
        let mut current = if let Some(parent) = bone.parent {
            let authored_local = mul(&rigid_inverse(&rest_world[parent]), &rest_world[index]);
            mul(&posed[parent], &authored_local)
        } else {
            mul(placement, &rest_world[index])
        };
        // A wrist has no unique bone direction; its palm orientation comes
        // from the index/outer knuckles, not the forearm axis. This matches
        // both flexion AND roll of the stock JKA palm without imposing an
        // arbitrary local bone-frame convention on Source.
        if names[index] == "lhand" || names[index] == "rhand" {
            let side = &names[index][..1];
            let source_knuckle = |digit: u8| {
                source_hand_knuckle(skeleton, &names, rest_world, index, &current, digit)
            };
            let target_knuckle = |digit: u8| {
                let name = format!("{side}_d{digit}_j1");
                gla_bone_index(gla, &name).map(|bone| origin(&gla.skeleton[bone].base_pose))
            };
            // Several Source models carry only two or three digit chains.
            // The most outboard surviving chain still gives a palm roll axis.
            let outer = [4, 3, 2].into_iter().find_map(|digit| {
                source_knuckle(digit).map(|joint| (joint, digit))
            });
            let source_outer = outer.map(|(joint, _)| joint);
            let target_outer = if gla.num_bones() == 72 {
                outer.and_then(|(_, digit)| target_knuckle(digit + 1))
            } else {
                target_knuckle(4)
            };
            if let (Some(si), Some(so), Some(ti), Some(to)) = (
                source_knuckle(1), source_outer,
                target_knuckle(2), target_outer,
            ) {
                if let (Some(from), Some(to)) = (
                    palm_frame(origin(&current), si, so),
                    gla_bone_index(gla, &format!("{side}hand")).and_then(|j| {
                        palm_frame(origin(&gla.skeleton[j].base_pose), ti, to)
                    }),
                ) {
                    let turn = rotation_between_frames(from, to);
                    let previous = current;
                    for row in 0..3 {
                        for col in 0..3 {
                            current[row][col] = (0..3)
                                .map(|k| turn[row][k] * previous[k][col]).sum();
                        }
                    }
                    corrected += 1;
                }
            }
        }
        // Only anatomically comparable axes are used. Non-limb helpers,
        // clavicles, faces and clothing retain their authored Source frame.
        let continuation = match names[index].as_str() {
            "lupperarm" => Some(("lforearm", "lradius")),
            "rupperarm" => Some(("rforearm", "rradius")),
            "lforearm" => Some(("lhand", "lhand")),
            "rforearm" => Some(("rhand", "rhand")),
            "lthigh" => Some(("lcalf", "ltibia")),
            "rthigh" => Some(("rcalf", "rtibia")),
            "lcalf" => Some(("lfoot", "ltalus")),
            "rcalf" => Some(("rfoot", "rtalus")),
            "lfoot" if gla_bone_index(gla, "ltarsal").is_some() => Some(("ltoe0", "ltarsal")),
            "rfoot" if gla_bone_index(gla, "rtarsal").is_some() => Some(("rtoe0", "rtarsal")),
            _ => None,
        };
        // Aim each Source finger's two real phalanges independently toward
        // JKA's joint-to-joint curl, while leaving thumb/index/middle/ring/pinky
        // spacing intact. The last Source phalanx maps to the same GLA j2 and
        // retains a little additional inward curl in the rest shape.
        let digit_aim = source_digit(&names[index]).and_then(|(side, digit, level)| {
            let next = format!("{side}finger{digit}{}", level + 1);
            let native = gla.num_bones() == 72;
            let target_digit = if native { digit + 1 } else { match digit { 0 => 1, 1 | 2 => 2, _ => 4 } };
            let here = if native { level + 1 } else { 1 };
            let child = if native { level + 2 } else { 2 };
            let joint1 = gla_bone_index(gla, &format!("{side}_d{target_digit}_j{here}"))?;
            let joint2 = gla_bone_index(gla, &format!("{side}_d{target_digit}_j{child}"))?;
            Some((next, joint1, joint2, if native { 0 } else { level }))
        });
        let child_info = if let Some((name, target_name)) = continuation {
            gla_bone_index(gla, target_name).and_then(|child| {
                gla_for_bone[index].map(|here| (name.to_owned(), here, child, false))
            })
        } else {
            digit_aim.map(|(next, j1, j2, level)| (next, j1, j2, level == 1))
        };
        if let Some((child_name, target_here, target_child, distal_finger)) = child_info {
            let source_child = skeleton.bones.iter().enumerate().find(|(child_id, child)| {
                child.parent == Some(index) && names[*child_id] == child_name
            }).map(|(child_id, _)| child_id);
            if let Some(source_child) = source_child {
                // Position the real Source child by the currently posed
                // parent bone. Reorient about THIS bone's own joint, never
                // about the target skeleton's differently proportioned pivot.
                let source_child_local = mul(
                    &rigid_inverse(&rest_world[index]), &rest_world[source_child]
                );
                let child_here = transform(&current, origin(&source_child_local));
                let here = origin(&current);
                let from = std::array::from_fn(|k| child_here[k] - here[k]);
                let a = origin(&gla.skeleton[target_here].base_pose);
                let b = origin(&gla.skeleton[target_child].base_pose);
                let mut to: [f32; 3] = std::array::from_fn(|k| b[k] - a[k]);
                if distal_finger {
                    // JKA has only j1/j2; Source has a third phalanx. Nudge
                    // that tip toward the inward-facing (-Y) palm rather than
                    // letting it extend backwards out of the grip.
                    if let Some(direction) = normalized(to) {
                        to = [direction[0], direction[1] - 0.28, direction[2]];
                    }
                }
                if normalized(from).is_some() && normalized(to).is_some() {
                    let turn = minimal_rotation(from, to);
                    // Apply rotational correction, retaining the existing
                    // joint translation and the global model fit scale.
                    let previous = current;
                    for row in 0..3 {
                        for col in 0..3 {
                            current[row][col] = (0..3)
                                .map(|k| turn[row][k] * previous[k][col]).sum();
                        }
                    }
                    corrected += 1;
                }
            }
        }
        posed.push(current);
    }
    let bake = rest_world.iter().enumerate().map(|(index, rest)| {
        mul(&posed[index], &rigid_inverse(rest))
    }).collect();
    (bake, corrected)
}

/// Map the posed Source joint centres to the corresponding GLA bone anchors.
/// The JKA skeleton is untouched; these pivots are model-specific skinning
/// metadata. Unrepresented JKA helpers (e.g. twist and face) inherit their
/// parent model-space offset, including GLA's non-topological face indices.
fn source_native_pivots(
    gla: &GlaAnimation,
    owner_of_gla: &BTreeMap<usize, usize>,
    rest_world: &[Matrix3x4],
    per_source_bake: &[Matrix3x4],
) -> Vec<[f32; 3]> {
    let count = gla.skeleton.len();
    let mut pivot = vec![[0.0; 3]; count];
    let mut known = vec![false; count];
    for (&target, &source) in owner_of_gla {
        if target < count && gla.skeleton[target].parent >= 0 {
            pivot[target] = transform(&per_source_bake[source], origin(&rest_world[source]));
            known[target] = true;
        }
    }
    for (j, bone) in gla.skeleton.iter().enumerate() {
        if bone.parent < 0 {
            pivot[j] = origin(&bone.base_pose);
            known[j] = true;
        }
    }
    // Some authored GLA bones precede their parents in the file: face children
    // 16..23 are parented to index 52. Never assume index order = hierarchy.
    for _ in 0..count {
        if known.iter().all(|&v| v) { break; }
        for (j, bone) in gla.skeleton.iter().enumerate() {
            if known[j] { continue; }
            let Some(parent) = pivot_parent(gla, j) else { continue; };
            if parent >= count || !known[parent] { continue; }
            let parent_gl = origin(&gla.skeleton[parent].base_pose);
            let here_gl = origin(&bone.base_pose);
            for axis in 0..3 {
                pivot[j][axis] = pivot[parent][axis] + here_gl[axis] - parent_gl[axis];
            }
            known[j] = true;
        }
    }
    pivot
}

/// Move normal Ghoul2 skin transforms from stock GLA rest joints to this
/// model's native Source-FK joints. The authored animation rotations, frame
/// translations and GLA hierarchy are preserved; only the lever arm (rest
/// bone lengths) changes. Keep the same rebased pose for GPU skinning, tags,
/// ragdolls and effects to avoid bolt/mesh disagreement.
///
/// Unlike rebuilding each bone as `T(native) * R * T(-native)`, this also
/// carries each child's parent motion and preserves animation translation
/// residuals, so elbows/knees do not drift away from their parent limbs.
/// An identity stock pose MUST remain identity for every valid pivot set.
pub fn rebase_source_ghoul2_pose(
    gla: &GlaAnimation,
    stock: &[Matrix3x4],
    pivots: &[[f32; 3]],
) -> Result<Vec<Matrix3x4>, String> {
    let n = gla.skeleton.len();
    if stock.len() != n || pivots.len() != n {
        return Err(format!("Source pose / GLA pivot count mismatch: stock={} pivot={} gla={n}", stock.len(), pivots.len()));
    }
    fn visit(
        j: usize, gla: &GlaAnimation, stock: &[Matrix3x4],
        pivots: &[[f32; 3]], output: &mut [Matrix3x4],
        joints: &mut [[f32; 3]], state: &mut [u8],
    ) -> Result<(), String> {
        if state[j] == 2 { return Ok(()); }
        if state[j] == 1 { return Err(format!("Cyclic GLA ancestry at bone {j}")); }
        state[j] = 1;
        let here = origin(&gla.skeleton[j].base_pose);
        let animated_here = transform(&stock[j], here);
        let parent = pivot_parent(gla, j);
        if parent.is_none() {
            joints[j] = transform(&stock[j], pivots[j]);
        } else {
            let p = parent.expect("parent checked above");
            if p >= stock.len() { return Err(format!("Invalid GLA parent {p} for bone {j}")); }
            visit(p, gla, stock, pivots, output, joints, state)?;
            let parent_rest = origin(&gla.skeleton[p].base_pose);
            let animated_parent = transform(&stock[p], parent_rest);
            let expected = rotate(&stock[p], std::array::from_fn(|k| here[k] - parent_rest[k]));
            let model_link = rotate(&stock[p], std::array::from_fn(|k| pivots[j][k] - pivots[p][k]));
            for k in 0..3 {
                let residual = animated_here[k] - animated_parent[k] - expected[k];
                joints[j][k] = joints[p][k] + model_link[k] + residual;
            }
        }
        output[j] = stock[j];
        let rotated_pivot = rotate(&stock[j], pivots[j]);
        for k in 0..3 { output[j][k][3] = joints[j][k] - rotated_pivot[k]; }
        state[j] = 2;
        Ok(())
    }
    let mut result = stock.to_vec();
    let mut joints = vec![[0.0; 3]; n];
    let mut state = vec![0u8; n];
    for j in 0..n {
        visit(j, gla, stock, pivots, &mut result, &mut joints, &mut state)?;
    }
    Ok(result)
}

/// One Source bone's model-space-to-base-pose-space transform.
///
/// `placement` is the global similarity (yaw, uniform scale, translation) that
/// carries Source model space into JKA model space. `align` is the rotation
/// correction to apply (see [`minimal_rotation`]), or `None` to keep the
/// authored orientation. See the module documentation for why
/// [`SourceRebindMode::BoneFrame`] is wrong, and [`resolve_align`] for why a
/// joint with no direction of its own still usually gets a non-`None` align.
fn rebind_matrix(
    mode: SourceRebindMode,
    placement: &Matrix3x4,
    scale: f32,
    source_rest: &Matrix3x4,
    gla_base: &Matrix3x4,
    align: Option<Matrix3x4>,
) -> Matrix3x4 {
    if mode == SourceRebindMode::BoneFrame {
        let uniform = [
            [scale, 0., 0., 0.],
            [0., scale, 0., 0.],
            [0., 0., scale, 0.],
        ];
        return mul(gla_base, &mul(&uniform, &rigid_inverse(source_rest)));
    }
    let align = align.unwrap_or_else(identity);
    // Carry the mesh into JKA space, bring this bone's joint to the origin, apply
    // the direction correction, then seat the joint on the GLA joint so animation
    // pivots about the right point.
    let joint = transform(placement, origin(source_rest));
    let centred = mul(&translation(joint.map(|v| -v)), placement);
    mul(&translation(origin(gla_base)), &mul(&align, &centred))
}

#[allow(clippy::too_many_arguments)]
pub fn build_source_rig(
    model_qpath: &str,
    mdl: &[u8],
    vvd: &SourceVvd,
    vtx: &SourceVtx,
    skeleton: &SourceSkeleton,
    materials: &SourceMaterialTable,
    gla: &GlaAnimation,
    options: &SourceRigOptions,
) -> Result<SourceRig, String> {
    let geometry = source_rig_geometry(mdl, vvd, vtx, materials, options)?;
    build_rig(model_qpath, skeleton, &geometry, gla, options)
}

/// Read a Source model's bodypart/LOD geometry into the neutral rig input.
/// Surface identity must be stable across LODs, so it is keyed on the MDL
/// (bodypart, model, mesh) triple rather than on per-LOD ordering.
fn source_rig_geometry(
    mdl: &[u8],
    vvd: &SourceVvd,
    vtx: &SourceVtx,
    materials: &SourceMaterialTable,
    options: &SourceRigOptions,
) -> Result<RigGeometry, String> {
    let bodyparts = parse_source_bodyparts(mdl)?;
    let lod_count = vvd
        .lod_count
        .min(vtx.lod_count)
        .min(options.max_lods.max(1));
    let base_draws = assemble_source_lod(mdl, vvd, vtx, 0, &options.bodygroups)?;
    let mut surfaces = Vec::with_capacity(base_draws.len());
    for draw in &base_draws {
        let body = &bodyparts[draw.bodypart];
        let model_stem = body.models[draw.model]
            .name
            .rsplit('/')
            .next()
            .unwrap_or("")
            .trim_end_matches(".smd")
            .trim_end_matches(".SMD");
        let sanitize = |text: &str| -> String {
            let cleaned: String = text
                .to_ascii_lowercase()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect();
            cleaned.trim_matches('_').to_owned()
        };
        let texture_index = materials.texture_index(options.skin_family, draw.skin_ref);
        surfaces.push(RigSurfaceSpec {
            key: (draw.bodypart, draw.model, draw.mesh),
            name: format!(
                "{}_{}_{}",
                sanitize(&body.name),
                sanitize(model_stem),
                draw.mesh
            ),
            shader: texture_index
                .and_then(|slot| materials.textures.get(slot).cloned())
                .unwrap_or_default(),
            vmt_candidates: texture_index
                .map(|slot| materials.vmt_candidates(slot))
                .unwrap_or_default(),
        });
    }
    let mut lods = Vec::with_capacity(lod_count);
    for lod in 0..lod_count {
        let pool = source_lod_vertices(vvd, lod)?
            .iter()
            .map(|v| RigVertex {
                position: v.position,
                normal: v.normal,
                uv: v.uv,
                influences: (0..(v.bone_count as usize).min(3))
                    .map(|k| (v.bones[k] as usize, v.weights[k]))
                    .collect(),
            })
            .collect();
        let draws = if lod == 0 {
            base_draws.clone()
        } else {
            assemble_source_lod(mdl, vvd, vtx, lod, &options.bodygroups)?
        };
        lods.push((
            pool,
            draws
                .into_iter()
                .map(|d| RigDraw { key: (d.bodypart, d.model, d.mesh), indices: d.indices })
                .collect(),
        ));
    }
    Ok(RigGeometry { surfaces, lods })
}

/// Fit, rebind and reweight any skeleton plus geometry onto `gla`. The
/// skeleton's bone names select the mapping (ValveBiped names, or the GLA's own).
pub fn build_rig(
    model_qpath: &str,
    skeleton: &SourceSkeleton,
    geometry: &RigGeometry,
    gla: &GlaAnimation,
    options: &SourceRigOptions,
) -> Result<SourceRig, String> {
    let rest_world = source_rest_world(skeleton)?;

    // StudioMDL stores an independent inverse bind. Comparing it against the one
    // derived from the bind chain catches a misread bone table before it can
    // quietly deform the mesh.
    let mut max_inverse_bind_error = 0.0f32;
    for (bone, rest) in skeleton.bones.iter().zip(&rest_world) {
        let derived = rigid_inverse(rest);
        for r in 0..3 {
            for c in 0..4 {
                max_inverse_bind_error =
                    max_inverse_bind_error.max((bone.pose_to_bone[r][c] - derived[r][c]).abs());
            }
        }
    }

    // Name-based mapping first, so the fit anchors are available.
    let mut gla_for_bone: Vec<Option<usize>> = vec![None; skeleton.bones.len()];
    let mut spine_candidates = Vec::new();
    let mut anchors = Vec::new();
    let mut missing_humanoid = Vec::new();
    for (index, bone) in skeleton.bones.iter().enumerate() {
        let key = normalize_source_bone(&bone.name);
        // A model already rigged with JKA bone names needs no translation.
        let direct = gla_bone_index(gla, &bone.name);
        let mapped = match direct {
            Some(found) => Some(found),
            None => match humanoid_target_for(gla, &key) {
                Some(name) => {
                    let found = gla_bone_index(gla, &name);
                    if found.is_none() {
                        missing_humanoid.push(name);
                    }
                    found
                }
                None => {
                    if key == "spine" || key.strip_prefix("spine").is_some_and(|n| n.parse::<u32>().is_ok()) {
                        spine_candidates.push((index, origin(&rest_world[index])));
                    }
                    None
                }
            },
        };
        gla_for_bone[index] = mapped;
        if let (Some(gla_index), true) = (mapped, FIT_ANCHORS.contains(&key.as_str())) {
            anchors.push((
                bone.name.clone(),
                gla.skeleton[gla_index].name.clone(),
                origin(&rest_world[index]),
                origin(&gla.skeleton[gla_index].base_pose),
            ));
        }

    }
    if !missing_humanoid.is_empty() {
        missing_humanoid.sort_unstable();
        missing_humanoid.dedup();
        return Err(format!(
            "{model_qpath}: animation skeleton is missing bones the ValveBiped profile needs: {}",
            missing_humanoid.join(", ")
        ));
    }

    let fit = fit_skeletons(&anchors)?;
    let scale = options.scale.unwrap_or(fit.scale);
    if !scale.is_finite() || scale <= 1e-4 {
        return Err(format!("{model_qpath}: invalid Source model scale {scale}"));
    }

    // Resolve the spine with the fitted alignment, then let every remaining bone
    // follow its nearest mapped ancestor so its vertices still move with the body
    // instead of being pinned to the model origin.
    for (bone, gla_index) in resolve_spine_chain(gla, &spine_candidates, |p| fit.align(p)) {
        gla_for_bone[bone] = Some(gla_index);
    }

    let mapped_bones = gla_for_bone.iter().filter(|slot| slot.is_some()).count();
    // Where an orphan Source root lands. `_humanoid`'s own root is `model_root`;
    // finding it by structure rather than by name keeps this working for any
    // animation skeleton a descriptor points at.
    let gla_root = gla
        .skeleton
        .iter()
        .position(|bone| bone.parent < 0)
        .ok_or_else(|| format!("{model_qpath}: animation skeleton has no root bone"))?;
    let mut inherited_bones = Vec::new();
    for index in 0..skeleton.bones.len() {
        if gla_for_bone[index].is_some() {
            continue;
        }
        let mut cursor = skeleton.bones[index].parent;
        while let Some(parent) = cursor {
            if let Some(found) = gla_for_bone[parent] {
                gla_for_bone[index] = Some(found);
                inherited_bones.push((
                    skeleton.bones[index].name.clone(),
                    gla.skeleton[found].name.clone(),
                ));
                break;
            }
            cursor = skeleton.bones[parent].parent;
        }
        if gla_for_bone[index].is_none() {
            // A parentless bone has no ancestor to inherit from, and Source
            // models routinely carry a second one: an exporter's stray `root`
            // at the origin alongside `ValveBiped.Bip01_Pelvis`, usually with
            // no vertices on it at all. Rejecting the model over that threw away
            // a character that was otherwise entirely loadable, so an orphan
            // root falls back to the GLA root - the structural counterpart of a
            // parentless Source bone. Anything weighted to it then rides the
            // model origin, which is the honest answer for a bone that is not
            // attached to the character's skeleton. A genuinely wrong skeleton
            // is still caught, earlier and with a better message, by
            // `fit_skeletons` refusing to fit too few anchors.
            gla_for_bone[index] = Some(gla_root);
            inherited_bones.push((
                skeleton.bones[index].name.clone(),
                gla.skeleton[gla_root].name.clone(),
            ));
        }
    }

    // An authored scale replaces the fitted one. Rebuild the similarity around the
    // anchor centroids instead of scaling the matrix wholesale, which would scale
    // the translation column too and slide the model off the skeleton.
    let placement = {
        let ratio = scale / fit.scale;
        let mut out = identity();
        for r in 0..3 {
            for c in 0..3 {
                out[r][c] = fit.transform[r][c] * ratio;
            }
            out[r][3] = fit.target_centroid[r]
                - (0..3)
                    .map(|k| out[r][k] * fit.source_centroid[k])
                    .sum::<f32>();
        }
        out
    };
    // Several Source bones routinely land on one `_humanoid` joint: ValveBiped's
    // toe on the ankle, its four- or five-bone spine on three lumbar bones, its
    // index and middle finger on one digit, and every helper without a rule of
    // its own on the nearest mapped ancestor. Each of those bones has its own
    // rest joint, so seating each one *independently* on the shared GLA joint
    // translates them by different amounts and drags the parts onto each other:
    // the toe collapses into the ankle, the ears into the skull, a tunic skirt
    // into the pelvis. The joint therefore gets exactly one matrix, built from
    // the bone that owns it, and every other bone on that joint rides along, so
    // their rest offsets stay rigid. Nothing is lost by sharing: the weight
    // merge below already maps all of them to that one GLA bone, so they could
    // never animate apart.
    //
    // The owner is the lowest-indexed bone on the joint, which is the shallowest
    // one because `source_rest_world` has already rejected any table that does
    // not store parents before children.
    let owner_of_gla = joint_owners(&gla_for_bone);
    let twist_blend = twist_blend_table(gla);

    // The standard mode poses the ORIGINAL Source hierarchy, retaining body
    // proportions and joint-local clothing/hair offsets. All other modes remain
    // comparison controls; they are not used for normal model conversion.
    let (source_bake, direction_corrected) = if options.rebind == SourceRebindMode::Direction {
        let (matrices, count) = source_fk_rebinds(
            skeleton, &rest_world, gla, &gla_for_bone, &placement,
        );
        (Some(matrices), count)
    } else {
        (None, 0)
    };
    let native_pivots = source_bake.as_ref().map(|matrices| {
        source_native_pivots(gla, &owner_of_gla, &rest_world, matrices)
    });
    let mut matrix_of_gla = BTreeMap::<usize, Matrix3x4>::new();
    if source_bake.is_none() {
        for (&gla_bone, &owner) in &owner_of_gla {
            matrix_of_gla.insert(gla_bone, rebind_matrix(
                options.rebind, &placement, scale, &rest_world[owner],
                &gla.skeleton[gla_bone].base_pose, None,
            ));
        }
    }
    // Reserve appended skin-palette slots for *authored* Source jigglebones.
    // They retain their own VVD weight rather than being merged into Head.
    // A static pose for each extra slot is its owning GLA matrix, so models
    // appear unchanged until the secondary-motion animator runs.
    let mut jiggle_slot_of_source = vec![None; skeleton.bones.len()];
    let mut source_jiggles = Vec::new();
    for (source_index, bone) in skeleton.bones.iter().enumerate() {
        let Some(parameters) = bone.jiggle else { continue };
        if parameters.flags & (crate::source1::JIGGLE_IS_FLEXIBLE
            | crate::source1::JIGGLE_IS_RIGID) == 0 {
            // Boing-only procedural rules do not have a bending spring.
            continue;
        }
        let parent_gla = gla_for_bone[source_index].expect("mapped Source bone");
        let mut ancestor = bone.parent;
        let mut parent_jiggle = None;
        while let Some(parent) = ancestor {
            if let Some(slot) = jiggle_slot_of_source[parent] {
                parent_jiggle = Some(slot);
                break;
            }
            ancestor = skeleton.bones[parent].parent;
        }
        let rebind = source_bake.as_ref().map_or_else(
            || matrix_of_gla[&parent_gla],
            |matrices| matrices[source_index],
        );
        // The fitted rebind includes uniform Source->JKA scale. A rigid bone
        // frame is needed for correct pivots and inverse bind, not that scale.
        let baked_world = mul(&rebind, &rest_world[source_index]);
        let mut rest_frame = baked_world;
        for row in &mut rest_frame {
            for value in &mut row[..3] { *value /= scale; }
        }
        // `matrix_of_gla` can carry the same fitted scale; the rest frame
        // translation is already in final baked JKA coordinates.
        jiggle_slot_of_source[source_index] = Some(source_jiggles.len());
        source_jiggles.push(SourceJiggleBinding {
            name: bone.name.clone(), parent_gla, parent_jiggle, rest_frame,
            parameters, length: (parameters.length.abs() * scale).max(0.001),
        });
    }

    let rebinds: Vec<BoneRebind> = gla_for_bone.iter().map(|slot| {
        let gla_bone = slot.expect("every bone is mapped above");
        BoneRebind { gla_bone }
    }).collect();
    let dual_quat_of_source: Option<Vec<([f32; 4], [f32; 4], f32)>> = source_bake
        .as_ref().map(|mats| mats.iter().map(dual_quat_from_scaled_matrix).collect());
    let dual_quat_of_gla: BTreeMap<usize, ([f32; 4], [f32; 4], f32)> = matrix_of_gla
        .iter().map(|(&bone, matrix)| (bone, dual_quat_from_scaled_matrix(matrix)))
        .collect();

    let mut surface_index_of = BTreeMap::new();
    let mut surfaces = Vec::with_capacity(geometry.surfaces.len());
    let mut hierarchy = Vec::with_capacity(geometry.surfaces.len());
    for spec in &geometry.surfaces {
        surface_index_of.insert(spec.key, surfaces.len());
        hierarchy.push(GlmSurfaceHierarchy {
            name: spec.name.clone(),
            flags: 0,
            shader: spec.shader.clone(),
            parent_index: -1,
            children: Vec::new(),
        });
        surfaces.push(SourceRigSurface {
            name: spec.name.clone(),
            vmt_candidates: spec.vmt_candidates.clone(),
        });
    }
    let lod_count = geometry.lods.len();

    let mut weight_truncations = 0usize;
    let mut source_jiggle_weighted_vertices = 0usize;
    let mut max_rebind_displacement = 0.0f32;
    let mut rest_min = [f32::INFINITY; 3];
    let mut rest_max = [f32::NEG_INFINITY; 3];
    let mut lods = Vec::with_capacity(lod_count);
    let mut lod_stats = Vec::with_capacity(lod_count);
    for (lod, (pool, draws)) in geometry.lods.iter().enumerate() {
        let mut lod_surfaces = Vec::with_capacity(draws.len());
        let mut lod_vertices = 0usize;
        let mut lod_triangles = 0usize;
        for draw in draws {
            let Some(&surface_index) = surface_index_of.get(&draw.key) else {
                // A coarser LOD may only drop meshes, never introduce new ones.
                return Err(format!(
                    "{model_qpath}: LOD{lod} introduces surface {:?} that LOD0 does not have",
                    draw.key
                ));
            };
            if draw.indices.is_empty() {
                continue;
            }
            if draw.indices.len() % 3 != 0 {
                return Err(format!(
                    "{model_qpath}: LOD{lod} surface {surface_index} has an incomplete triangle"
                ));
            }
            // Compact the shared LOD pool into a per-surface vertex buffer.
            let mut remap = BTreeMap::<u32, u32>::new();
            let mut vertices: Vec<GlmVertex> = Vec::new();
            let mut texcoords: Vec<[f32; 2]> = Vec::new();
            let mut bone_references: Vec<usize> = Vec::new();
            let mut local_of_gla = BTreeMap::<usize, usize>::new();
            let mut triangles = Vec::with_capacity(draw.indices.len() / 3);
            let mut corner = [0u32; 3];
            for (slot, &pool_index) in draw.indices.iter().enumerate() {
                let local = match remap.get(&pool_index).copied() {
                    Some(found) => found,
                    None => {
                        let source = pool.get(pool_index as usize).ok_or_else(|| {
                            format!("{model_qpath}: LOD{lod} vertex {pool_index} outside the gathered pool")
                        })?;
                        let influences = source.influences.len();
                        let mut total = 0.0f32;
                        let mut merged = BTreeMap::<usize, f32>::new();
                        // Collected instead of blended immediately: the
                        // dominant (highest-weight) influence must go first
                        // so `blend_dual_quats` flips every other quaternion
                        // into its hemisphere, not an arbitrary bone-table one.
                        let mut parts: Vec<(f32, [f32; 4], [f32; 4])> = Vec::with_capacity(influences);
                        let mut scale_sum = 0.0f32;
                        for &(bone, weight) in &source.influences {
                            let rebind = rebinds.get(bone).ok_or_else(|| {
                                format!(
                                    "{model_qpath}: vertex {pool_index} references Source bone {bone}, but the skeleton has {}",
                                    rebinds.len()
                                )
                            })?;
                            if !weight.is_finite() || weight < 0.0 {
                                return Err(format!(
                                    "{model_qpath}: vertex {pool_index} has invalid weight {weight}"
                                ));
                            }
                            if weight <= 0.0 {
                                continue;
                            }
                            total += weight;
                            // Several Source bones can share one GLA bone (a
                            // five-bone ValveBiped spine against three lumbar
                            // bones), and they all placed the vertex through
                            // that joint's single matrix, so merging their
                            // influence here only compacts the weight list.
                            let palette_index = jiggle_slot_of_source[bone]
                                .map(|slot| gla.skeleton.len() + slot)
                                .unwrap_or(rebind.gla_bone);
                            if palette_index >= gla.skeleton.len() {
                                source_jiggle_weighted_vertices += 1;
                            }
                            *merged.entry(palette_index).or_default() += weight;
                            let &(real, dual, part_scale) = if let Some(ref per_source) = dual_quat_of_source {
                                &per_source[bone]
                            } else {
                                &dual_quat_of_gla[&rebind.gla_bone]
                            };
                            parts.push((weight, real, dual));
                            scale_sum += weight * part_scale;
                        }
                        if total <= 1e-6 || merged.is_empty() {
                            return Err(format!(
                                "{model_qpath}: vertex {pool_index} has no usable skinning weights"
                            ));
                        }
                        // Blend the bone *transforms* (dual quaternion
                        // skinning), not the positions each independently
                        // produces: the rebind corrects a Source T-pose onto
                        // `_humanoid`'s A-pose per bone, often by tens of
                        // degrees, and blending positions from two bones that
                        // disagree that much stretches the mesh between them
                        // (the classic linear-blend-skinning "candy wrapper"
                        // artifact) - confirmed on Juliet Starling, whose
                        // unrebound source mesh has an ordinary neck and limb
                        // proportions that only became stretched after this
                        // step.
                        parts.sort_by(|a, b| b.0.total_cmp(&a.0));
                        let (blended_real, blended_dual) = blend_dual_quats(&parts);
                        let blended = dual_quat_to_matrix(blended_real, blended_dual);
                        // DQ is a *rigid* transform. Apply the positive
                        // weighted uniform scale to Source positions before
                        // the rigid transform; normals must not be scaled.
                        let vertex_scale = scale_sum / total;
                        let position = transform(&blended, source.position.map(|v| v * vertex_scale));
                        let normal = rotate(&blended, source.normal);
                        // See `twist_blend_table`: ease each chain-bone's
                        // weight toward its twist-helper sibling by how far
                        // along the limb this vertex sits. Applied after the
                        // position is final so the limb fraction reflects
                        // where the vertex actually baked to, and before
                        // `influence_list` so it is truncated/normalized like
                        // any other weight.
                        let mut twist_additions: Vec<(usize, f32)> = Vec::new();
                        for (&bone, weight) in merged.iter_mut() {
                            if let Some(&(twist_bone, joint, next_joint)) = twist_blend.get(&bone) {
                                let t = limb_fraction(position, joint, next_joint);
                                let fraction = twist_blend_fraction(t);
                                if fraction > 0.0 {
                                    let moved = *weight * fraction;
                                    *weight -= moved;
                                    twist_additions.push((twist_bone, moved));
                                }
                            }
                        }
                        for (bone, moved) in twist_additions {
                            *merged.entry(bone).or_default() += moved;
                        }
                        let length = normal.iter().map(|v| v * v).sum::<f32>().sqrt();
                        let normal = if length > 1e-6 {
                            normal.map(|v| v / length)
                        } else {
                            // Fully cancelling normals mean the bone frames
                            // disagree; a +Z fallback keeps the vertex usable.
                            [0.0, 0.0, 1.0]
                        };
                        for axis in 0..3 {
                            rest_min[axis] = rest_min[axis].min(position[axis]);
                            rest_max[axis] = rest_max[axis].max(position[axis]);
                        }
                        // How far the rebind moved this vertex compared with
                        // merely scaling/aligning the Source mesh in place.
                        let merely_aligned = transform(&placement, source.position);
                        let displacement = (0..3)
                            .map(|k| (position[k] - merely_aligned[k]).powi(2))
                            .sum::<f32>()
                            .sqrt();
                        max_rebind_displacement = max_rebind_displacement.max(displacement);

                        let mut influence_list: Vec<(usize, f32)> = merged.into_iter().collect();
                        influence_list.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
                        if influence_list.len() > MAX_WEIGHTS {
                            influence_list.truncate(MAX_WEIGHTS);
                            weight_truncations += 1;
                        }
                        let sum: f32 = influence_list.iter().map(|(_, w)| *w).sum();
                        if sum <= 1e-6 {
                            return Err(format!(
                                "{model_qpath}: vertex {pool_index} lost all weight while merging"
                            ));
                        }
                        let weights: Vec<GlmWeight> = influence_list
                            .iter()
                            .map(|(gla_bone, weight)| {
                                let local_bone_index =
                                    *local_of_gla.entry(*gla_bone).or_insert_with(|| {
                                        bone_references.push(*gla_bone);
                                        bone_references.len() - 1
                                    });
                                GlmWeight {
                                    local_bone_index,
                                    weight: weight / sum,
                                }
                            })
                            .collect();
                        let local = u32::try_from(vertices.len())
                            .map_err(|_| "Source surface exceeds u32 vertices".to_owned())?;
                        vertices.push(GlmVertex {
                            normal,
                            position,
                            weights,
                        });
                        texcoords.push(source.uv);
                        remap.insert(pool_index, local);
                        local
                    }
                };
                corner[slot % 3] = local;
                if slot % 3 == 2 {
                    triangles.push(corner);
                }
            }
            lod_vertices += vertices.len();
            lod_triangles += triangles.len();
            lod_surfaces.push(GlmSurface {
                surface_index,
                vertices,
                texcoords,
                triangles,
                bone_references,
            });
        }
        if lod_surfaces.is_empty() {
            break;
        }
        lod_stats.push(SourceLodStats {
            lod,
            surfaces: lod_surfaces.len(),
            vertices: lod_vertices,
            triangles: lod_triangles,
        });
        lods.push(GlmLod {
            surfaces: lod_surfaces,
        });
    }
    if lods.is_empty() {
        return Err(format!("{model_qpath}: produced no drawable LODs"));
    }

    let diagnostics = SourceRigDiagnostics {
        model_name: skeleton.model_name.clone(),
        source_bones: skeleton.bones.len(),
        mapped_bones,
        inherited_bones,
        scale,
        scale_was_fitted: options.scale.is_none(),
        fit_pairs: fit.pairs,
        fit_yaw_degrees: fit.yaw_degrees,
        fit_rms: fit.rms,
        fit_worst: fit.worst,
        max_inverse_bind_error,
        rebind: options.rebind,
        direction_corrected,
        weight_truncations,
        source_jiggle_bones: source_jiggles.len(),
        source_jiggle_weighted_vertices,
        rest_bounds: (rest_min, rest_max),
        max_rebind_displacement,
        lods: lod_stats,
    };
    Ok(SourceRig {
        native_pivots,
        glm: GlmModel {
            name: model_qpath.to_owned(),
            anim_name: "models/players/_humanoid/_humanoid".to_owned(),
            // `num_bones` names the *primary* skeleton for bodyshape, bolts
            // and retargeting compatibility. Source procedural weights can
            // address further in-memory skin palette entries supplied by the
            // presenter; they are not a different Ghoul2 animation skeleton.
            num_bones: gla.skeleton.len(),
            hierarchy,
            lods,
        },
        surfaces,
        source_jiggles,
        diagnostics,
    })
}

/// Ghoul2 `G2SURFACEFLAG_ISBOLT`.
const G2SURFACEFLAG_ISBOLT: u32 = 0x0000_0001;

/// Copy the `*`-prefixed Ghoul2 tag surfaces from a stock `_humanoid` GLM onto a
/// rebound Source model, and report how many were attached.
///
/// A Source `.mdl` has no equivalent of a JKA tag surface, so without this a
/// rebound character has no bolts at all: `model_bolt_matrix` finds neither a
/// `*r_hand` surface nor a GLA bone of that name, returns `None`, and the saber,
/// weapon flash, jetpack and bolt-driven effects are all silently skipped.
///
/// Lifting them is sound rather than a hack. A tag surface is three vertices
/// weighted to `_humanoid` bones and authored in base-pose space - the very space
/// this module bakes into - so the donor's frames animate correctly on any
/// `_humanoid` pose, and because the rebind seats each Source joint on its GLA
/// counterpart the hand bolt lands in the rebound model's hand. Using JKA's own
/// authored frames also avoids inventing a hilt-in-palm orientation, which a raw
/// bone matrix would require.
pub fn attach_humanoid_bolts(
    rig: &mut SourceRig,
    donor: &GlmModel,
    gla: &GlaAnimation,
) -> Result<usize, String> {
    let Some(donor_lod) = donor.lods.first() else {
        return Err("bolt donor GLM has no LODs".into());
    };
    // Native JKA player meshes use approximately the same height. A least-
    // squares fit across shoulders/hands/legs does NOT constrain total height:
    // the included Juliet and Shrek Source examples are conspicuously short.
    // Normalize to the actual donor GLM mesh, not a guessed magic number.
    // Respect an explicitly configured `source_model.json` scale.
    if rig.diagnostics.scale_was_fitted {
        let bounds = |model: &GlmModel| -> Option<(f32, f32)> {
            let lod = model.lods.first()?;
            let mut lo = f32::INFINITY;
            let mut hi = f32::NEG_INFINITY;
            for surface in &lod.surfaces {
                if model.hierarchy.get(surface.surface_index)
                    .is_some_and(|h| h.name.starts_with('*')) { continue; }
                for vertex in &surface.vertices {
                    let z = vertex.position[2];
                    if !z.is_finite() { return None; }
                    lo = lo.min(z);
                    hi = hi.max(z);
                }
            }
            (hi > lo + 1.0).then_some((lo, hi))
        };
        if let (Some((source_lo, source_hi)), Some((donor_lo, donor_hi))) =
            (bounds(&rig.glm), bounds(donor))
        {
            let ratio = (donor_hi - donor_lo) / (source_hi - source_lo);
            if ratio.is_finite() && (0.5..=2.0).contains(&ratio) {
                let floor_offset = donor_lo - ratio * source_lo;
                for lod in &mut rig.glm.lods {
                    for surface in &mut lod.surfaces {
                        for vertex in &mut surface.vertices {
                            for k in 0..3 { vertex.position[k] *= ratio; }
                            vertex.position[2] += floor_offset;
                        }
                    }
                }
                for bone in &mut rig.source_jiggles {
                    for axis in 0..3 { bone.rest_frame[axis][3] *= ratio; }
                    bone.rest_frame[2][3] += floor_offset;
                    bone.length *= ratio;
                }
                if let Some(pivots) = &mut rig.native_pivots {
                    for pivot in pivots {
                        for k in 0..3 { pivot[k] *= ratio; }
                        pivot[2] += floor_offset;
                    }
                }
                for k in 0..3 {
                    rig.diagnostics.rest_bounds.0[k] *= ratio;
                    rig.diagnostics.rest_bounds.1[k] *= ratio;
                }
                rig.diagnostics.rest_bounds.0[2] += floor_offset;
                rig.diagnostics.rest_bounds.1[2] += floor_offset;
                rig.diagnostics.scale *= ratio;
            }
        }
    }
    let mut attached = 0usize;
    for (donor_index, hierarchy) in donor.hierarchy.iter().enumerate() {
        if !hierarchy.name.starts_with('*') {
            continue;
        }
        let Some(surface) = donor_lod
            .surfaces
            .iter()
            .find(|surface| surface.surface_index == donor_index)
        else {
            continue;
        };
        // `G2_ProcessSurfaceBolt2` reads the first three vertices, so anything
        // shorter cannot define a frame.
        if surface.vertices.len() < 3 {
            continue;
        }
        for &bone in &surface.bone_references {
            if bone >= gla.skeleton.len() {
                return Err(format!(
                    "bolt donor surface {:?} references GLA bone {bone}, but the skeleton has {}",
                    hierarchy.name,
                    gla.skeleton.len()
                ));
            }
        }
        let surface_index = rig.glm.hierarchy.len();
        rig.glm.hierarchy.push(GlmSurfaceHierarchy {
            name: hierarchy.name.clone(),
            flags: hierarchy.flags | G2SURFACEFLAG_ISBOLT,
            shader: String::new(),
            // The donor's parent/child indices address the donor's hierarchy, not
            // this one. Anything that walks the tree (cg_fpls, surface on/off
            // descendants) must not be able to follow them into a foreign index.
            parent_index: -1,
            children: Vec::new(),
        });
        // Hand tags must follow the model-specific palm pivot. Copying the
        // stock donor coordinates unchanged positions a saber in the STOCK
        // hand, even when the converted Source hand is elsewhere. Preserve
        // the authored Ghoul2 bolt triangle's orientation and adjust only its
        // neutral placement (and reach proportional to palm length).
        let mut bolt_vertices = surface.vertices.clone();
        let hand = match hierarchy.name.to_ascii_lowercase().as_str() {
            "*r_hand" => Some(("rhand", "r_d2_j1", "r_d4_j1")),
            "*l_hand" => Some(("lhand", "l_d2_j1", "l_d4_j1")),
            _ => None,
        };
        if let (Some((hand_name, index_name, outer_name)), Some(pivots)) =
            (hand, rig.native_pivots.as_ref())
        {
            if let (Some(hand_idx), Some(index_idx), Some(outer_idx)) = (
                gla_bone_index(gla, hand_name),
                gla_bone_index(gla, index_name),
                gla_bone_index(gla, outer_name),
            ) {
                if let (Some(&wrist), Some(&knuckle), Some(&outer)) = (
                    pivots.get(hand_idx), pivots.get(index_idx), pivots.get(outer_idx),
                ) {
                    let stock_wrist = origin(&gla.skeleton[hand_idx].base_pose);
                    let stock_index = origin(&gla.skeleton[index_idx].base_pose);
                    let stock_outer = origin(&gla.skeleton[outer_idx].base_pose);
                    let avg = |a: [f32; 3], b: [f32; 3]| -> [f32; 3] {
                        std::array::from_fn(|k| (a[k] + b[k]) * 0.5)
                    };
                    let length = |a: [f32; 3], b: [f32; 3]| -> f32 {
                        (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>().sqrt()
                    };
                    let donor_len = length(stock_wrist, avg(stock_index, stock_outer));
                    let palm_len = length(wrist, avg(knuckle, outer));
                    let reach = if donor_len > 1e-4 {
                        (palm_len / donor_len).clamp(0.75, 1.4)
                    } else { 1.0 };
                    for vertex in &mut bolt_vertices {
                        for k in 0..3 {
                            vertex.position[k] = wrist[k] + reach * (vertex.position[k] - stock_wrist[k]);
                        }
                    }
                }
            }
        }
        // Every LOD: a bolt is looked up in LOD 0, but dismemberment and effects
        // resolve surfaces against whichever LOD is current.
        for lod in rig.glm.lods.iter_mut() {
            lod.surfaces.push(GlmSurface {
                surface_index,
                vertices: bolt_vertices.clone(),
                texcoords: surface.texcoords.clone(),
                triangles: surface.triangles.clone(),
                bone_references: surface.bone_references.clone(),
            });
        }
        attached += 1;
    }
    Ok(attached)
}

/// Transfer tags authored for a different rig into the destination bind space.
/// Packed local weight indices are retained; only global references and bind
/// coordinates change. Ordinary 53-bone donors still use the original helper.
pub fn attach_humanoid_bolts_between_rigs(
    rig: &mut SourceRig,
    donor: &GlmModel,
    donor_gla: &GlaAnimation,
    target_gla: &GlaAnimation,
) -> Result<usize, String> {
    let mut remapped = donor.clone();
    for lod in &mut remapped.lods {
        for surface in &mut lod.surfaces {
            if !donor.hierarchy[surface.surface_index].name.starts_with('*') { continue; }
            let mut corrections = Vec::new();
            for bone in &mut surface.bone_references {
                let source = donor_gla.skeleton.get(*bone).ok_or("invalid donor tag bone")?;
                let target = crate::skeleton::source_for_target(&target_gla.skeleton, &donor_gla.skeleton, *bone)
                    .ok_or_else(|| format!("no target tag bone {}", source.name))?;
                corrections.push(mul(&target_gla.skeleton[target].base_pose, &source.base_pose_inv));
                *bone = target;
            }
            for vertex in &mut surface.vertices {
                let mut position = [0.0; 3];
                let mut normal = [0.0; 3];
                for weight in &vertex.weights {
                    let correction = corrections.get(weight.local_bone_index).ok_or("invalid donor tag weight")?;
                    let p = transform(correction, vertex.position);
                    let n = rotate(correction, vertex.normal);
                    for k in 0..3 { position[k] += weight.weight * p[k]; normal[k] += weight.weight * n[k]; }
                }
                vertex.position = position;
                vertex.normal = normalized(normal).unwrap_or(vertex.normal);
            }
        }
    }
    attach_humanoid_bolts(rig, &remapped, target_gla)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_jk2_profile_preserves_every_source_digit_and_toe() {
        use crate::skeleton::{humanoid_reference, HumanoidRig};
        let native = humanoid_reference(HumanoidRig::Jk2_72);
        let legacy = humanoid_reference(HumanoidRig::Jka53);
        for side in ['l', 'r'] {
            for digit in 0..5 {
                for (suffix, joint) in [("", 1), ("1", 2), ("2", 3)] {
                    let name = format!("{side}finger{digit}{suffix}");
                    assert_eq!(humanoid_target_for(&native, &name), Some(format!("{side}_d{}_j{joint}", digit+1)));
                    assert_eq!(humanoid_target_for(&legacy, &name).as_deref(), humanoid_bone_for(&name));
                }
            }
            assert_eq!(humanoid_target_for(&native, &format!("{side}toe0")), Some(format!("{side}tarsal")));
            assert_eq!(humanoid_target_for(&legacy, &format!("{side}toe0")), Some(format!("{side}talus")));
        }
    }

    #[test]
    fn normalizes_valvebiped_names() {
        assert_eq!(normalize_source_bone("ValveBiped.Bip01_Pelvis"), "pelvis");
        assert_eq!(
            normalize_source_bone("ValveBiped.Bip01_L_UpperArm"),
            "lupperarm"
        );
        assert_eq!(normalize_source_bone("ValveBiped.forward"), "forward");
        assert_eq!(
            normalize_source_bone("ValveBiped.Anim_Attachment_RH"),
            "animattachmentrh"
        );
        assert_eq!(normalize_source_bone("ValveBiped.Bip01_Spine4"), "spine4");
    }

    /// `_humanoid` splits the femur and humerus into a chain bone and a twist
    /// helper. A limb must follow the chain bone, because the next joint down
    /// parents to it, not to the helper.
    #[test]
    fn limbs_target_chain_bones_not_twist_helpers() {
        assert_eq!(humanoid_bone_for("lthigh"), Some("lfemurYZ"));
        assert_eq!(humanoid_bone_for("rupperarm"), Some("rhumerus"));
        assert_eq!(humanoid_bone_for("lforearm"), Some("lradius"));
        // No ValveBiped joint may target an `X` twist helper: the next joint
        // down parents to the chain bone, so a helper would not carry the limb.
        let helpers = ["lfemurx", "rfemurx", "lhumerusx", "rhumerusx", "lradiusx", "rradiusx"];
        for source in [
            "pelvis", "neck1", "head1", "lclavicle", "rclavicle", "lupperarm", "rupperarm",
            "lforearm", "rforearm", "lhand", "rhand", "lthigh", "rthigh", "lcalf", "rcalf",
            "lfoot", "rfoot", "ltoe0", "rtoe0",
        ] {
            let target = humanoid_bone_for(source).expect("mapped");
            assert!(
                !helpers.contains(&target.to_ascii_lowercase().as_str()),
                "{source} must not target twist helper {target}"
            );
        }
    }

    #[test]
    fn spine_is_not_name_mapped() {
        // Spine bones are resolved positionally, so the name table must not
        // claim them; otherwise models with a different subdivision squash.
        for name in ["spine", "spine1", "spine2", "spine3", "spine4"] {
            assert_eq!(humanoid_bone_for(name), None);
        }
    }

    #[test]
    fn twist_blend_ramps_from_the_joint_to_the_next_one_down() {
        let joint = [0.0_f32, 0.0, 40.0];
        let next_joint = [0.0_f32, 0.0, 20.0];
        assert_eq!(twist_blend_fraction(limb_fraction(joint, joint, next_joint)), 0.0);
        let far_past = [0.0_f32, 0.0, 10.0];
        assert!((twist_blend_fraction(limb_fraction(far_past, joint, next_joint)) - 0.85).abs() < 1e-6);
        let behind = [0.0_f32, 0.0, 50.0];
        assert_eq!(twist_blend_fraction(limb_fraction(behind, joint, next_joint)), 0.0);
        let midpoint = [0.0_f32, 0.0, 30.0];
        let mid_fraction = twist_blend_fraction(limb_fraction(midpoint, joint, next_joint));
        assert!(
            mid_fraction > 0.0 && mid_fraction < 0.85,
            "midpoint fraction {mid_fraction} should sit strictly between the ends"
        );
    }

    #[test]
    fn toes_fold_onto_the_ankle() {
        assert_eq!(humanoid_bone_for("ltoe0"), Some("ltalus"));
        assert_eq!(humanoid_bone_for("rfoot"), Some("rtalus"));
    }

    fn rigid(yaw: f32, t: [f32; 3]) -> Matrix3x4 {
        let (s, c) = yaw.sin_cos();
        [[c, -s, 0., t[0]], [s, c, 0., t[1]], [0., 0., 1., t[2]]]
    }

    fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
        (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>().sqrt()
    }

    #[test]
    fn source_digit_excludes_tip_bones_and_unrelated_names() {
        assert_eq!(source_digit("lfinger0"), Some(('l', 0, 0)));
        assert_eq!(source_digit("rfinger11"), Some(('r', 1, 1)));
        assert_eq!(source_digit("lfinger12"), None);
        assert_eq!(source_digit("lforearm"), None);
    }

    #[test]
    fn ue_metacarpals_do_not_disable_palm_orientation_retargeting() {
        // UE hands commonly have hand -> metacarpal -> index/pinky, while
        // ValveBiped commonly has hand -> index/pinky directly. Both must
        // produce the SAME hand-frame correction for otherwise identical
        // joint coordinates. A direct-parent-only lookup fails this fixture.
        use crate::source1::{SourceBone, SourceSkeleton};
        use crate::skeleton::{humanoid_reference, HumanoidRig};
        let gla = humanoid_reference(HumanoidRig::Jk2_72);
        let hand_idx = gla_bone_index(&gla, "lhand").unwrap();
        let index_idx = gla_bone_index(&gla, "l_d2_j1").unwrap();
        let pinky_idx = gla_bone_index(&gla, "l_d5_j1").unwrap();
        let source_positions = [
            [10., 2., 40.], // hand
            [10.5, 2.2, 40.], // index metacarpal
            [13., 1., 40.], // index knuckle
            [10.5, 3., 40.], // pinky metacarpal
            [13., 4., 40.], // pinky knuckle
        ];
        let names = ["lhand", "index_metacarpal_l", "lfinger1", "pinky_metacarpal_l", "lfinger4"];
        let parents = [None, Some(0), Some(1), Some(0), Some(3)];
        let skeleton = SourceSkeleton {
            model_name: "synthetic_ue_metacarpals".into(),
            bones: names.iter().zip(parents).map(|(name, parent)| SourceBone {
                name: (*name).into(), parent,
                bind_position: [0.; 3], bind_quaternion: [0., 0., 0., 1.],
                pose_to_bone: identity(), jiggle: None,
            }).collect(),
        };
        let rest: Vec<Matrix3x4> = source_positions.iter()
            .map(|&pos| rigid(0., pos)).collect();
        let normalized_names: Vec<String> = names.iter().map(|name| (*name).into()).collect();
        let index = source_hand_knuckle(&skeleton, &normalized_names, &rest, 0, &rest[0], 1)
            .expect("index knuckle behind an intermediary metacarpal");
        let pinky = source_hand_knuckle(&skeleton, &normalized_names, &rest, 0, &rest[0], 4)
            .expect("pinky knuckle behind an intermediary metacarpal");
        assert!(distance(index, source_positions[2]) < 1e-4);
        assert!(distance(pinky, source_positions[4]) < 1e-4);
        let targets = [Some(hand_idx), Some(hand_idx), Some(index_idx), Some(hand_idx), Some(pinky_idx)];
        let (bake, corrected) = source_fk_rebinds(
            &skeleton, &rest, &gla, &targets, &identity(),
        );
        assert!(corrected > 0, "UE hand with metacarpals did not receive palm rotation");
        let wrist = transform(&bake[0], source_positions[0]);
        let index = transform(&bake[2], source_positions[2]);
        let pinky = transform(&bake[4], source_positions[4]);
        let actual = palm_frame(wrist, index, pinky).unwrap();
        let target = palm_frame(
            origin(&gla.skeleton[hand_idx].base_pose),
            origin(&gla.skeleton[index_idx].base_pose),
            origin(&gla.skeleton[pinky_idx].base_pose),
        ).unwrap();
        for k in 0..3 {
            let dot = (0..3).map(|j| actual[k][j] * target[k][j]).sum::<f32>();
            assert!(dot > 0.999, "palm frame axis {k} misaligned: dot={dot}");
        }
    }

    #[test]
    fn palm_alignment_preserves_joint_distances_and_handedness() {
        let wrist = [10.0, 0.0, 40.0];
        let index = [12.5, -0.8, 40.0];
        let outer = [12.5, 0.8, 40.0];
        let target_wrist = [20.0, 0.0, 40.0];
        let target_index = [22.0, -0.8, 37.5];
        let target_outer = [22.0, 0.8, 37.5];
        let from = palm_frame(wrist, index, outer).unwrap();
        let to = palm_frame(target_wrist, target_index, target_outer).unwrap();
        let rotation = rotation_between_frames(from, to);
        let source_forward = std::array::from_fn(|k| (index[k] + outer[k]) * 0.5 - wrist[k]);
        let rotated_forward = normalized(rotate(&rotation, source_forward)).unwrap();
        let target_forward = normalized(std::array::from_fn(|k| {
            (target_index[k] + target_outer[k]) * 0.5 - target_wrist[k]
        })).unwrap();
        assert!(distance(rotated_forward, target_forward) < 1e-4);
        let p = [12.2, 1.1, 40.4];
        assert!((distance(rotate(&rotation, std::array::from_fn(|k| p[k] - wrist[k])), [0., 0., 0.])
            - distance(p, wrist)).abs() < 1e-4);
    }

    #[test]
    fn minimal_rotation_turns_one_direction_onto_another() {
        for (from, to) in [
            ([1., 0., 0.], [0., 1., 0.]),
            ([0., 0., 1.], [0., 0., -1.]),
            ([1., 2., 3.], [-3., 1., 0.5]),
            ([1., 0., 0.], [1., 0., 0.]),
        ] {
            let turned = rotate(&minimal_rotation(from, to), from);
            let want = normalized(to).unwrap();
            let have = normalized(turned).unwrap();
            for axis in 0..3 {
                assert!(
                    (have[axis] - want[axis]).abs() < 1e-4,
                    "{from:?} -> {to:?} gave {have:?}"
                );
            }
            // A rotation must not change the vector's length.
            let length = |v: [f32; 3]| v.iter().map(|c| c * c).sum::<f32>().sqrt();
            assert!((length(turned) - length(from)).abs() < 1e-4);
        }
    }

    /// The regression this module exists to prevent. ValveBiped and `_humanoid`
    /// bone frames differ by 100-145 degrees of pure convention while their joints
    /// sit within a unit or two of each other. `BoneFrame` turns that convention
    /// difference into a real rotation of the mesh about the joint;
    /// `Translation`/`Direction` must not, because a Ghoul2 pose matrix
    /// (`world_animated * inverse(basePose)`) cancels the convention at runtime.
    #[test]
    fn rebind_does_not_apply_the_axis_convention_difference() {
        // Same joint position, rest frames 100 degrees apart: pure convention.
        let joint = [3.0_f32, -2.0, 40.0];
        let source_rest = rigid(0.0, joint);
        let gla_base = rigid(100.0_f32.to_radians(), joint);
        let placement = identity();
        // A vertex 10 units out from the joint, i.e. ordinary limb flesh.
        let vertex = [joint[0] + 10.0, joint[1], joint[2]];

        let kept = transform(
            &rebind_matrix(
                SourceRebindMode::Translation,
                &placement,
                1.0,
                &source_rest,
                &gla_base,
                None,
            ),
            vertex,
        );
        assert!(
            distance(kept, vertex) < 1e-3,
            "translation rebind must leave a coincident joint's geometry alone, moved {:.3}",
            distance(kept, vertex)
        );

        let warped = transform(
            &rebind_matrix(
                SourceRebindMode::BoneFrame,
                &placement,
                1.0,
                &source_rest,
                &gla_base,
                None,
            ),
            vertex,
        );
        // 10 units swung through 100 degrees is over 15 units of travel.
        assert!(
            distance(warped, vertex) > 10.0,
            "BoneFrame is expected to warp; it moved only {:.3}",
            distance(warped, vertex)
        );
    }

    #[test]
    fn bones_sharing_a_gla_joint_share_one_matrix_and_keep_their_offsets() {
        // ValveBiped's toe maps onto the ankle, because `_humanoid` has no toe
        // bone. Rebinding the toe through its own rest joint used to seat the toe
        // joint on the ankle joint, dragging the front of the foot back into the
        // leg; the same collapse hit ears, tunic skirts and twist helpers.
        let ankle = rigid(0.0, [3.0, 0.0, 6.0]);
        let toe = rigid(0.0, [3.0, -8.0, 2.0]);
        let gla_ankle = rigid(0.4, [2.0, 1.0, 7.0]);

        // foot at index 0, toe at index 1, both on GLA bone 9.
        let owners = joint_owners(&[Some(9), Some(9)]);
        assert_eq!(owners.get(&9), Some(&0), "the foot owns the ankle, not the toe");

        let shared = rebind_matrix(
            SourceRebindMode::Direction,
            &identity(),
            1.0,
            &rest_world_of(&owners, &[ankle, toe], 9),
            &gla_ankle,
            None,
        );
        // A toe-tip vertex and the ankle joint must stay exactly as far apart as
        // they were, which is what "the foot is still a foot" means here.
        let tip = [3.0, -11.0, 1.0];
        let before = distance(tip, origin(&ankle));
        let after = distance(transform(&shared, tip), transform(&shared, origin(&ankle)));
        assert!(
            (before - after).abs() < 1e-3,
            "the toe moved relative to the ankle: {before:.3} -> {after:.3}"
        );
        assert!(
            distance(transform(&shared, origin(&ankle)), origin(&gla_ankle)) < 1e-3,
            "the owning joint must still land on its GLA counterpart"
        );
    }

    /// The rest frame the joint's matrix is built from: its owner's.
    fn rest_world_of(
        owners: &BTreeMap<usize, usize>,
        rest_world: &[Matrix3x4],
        gla_bone: usize,
    ) -> Matrix3x4 {
        rest_world[owners[&gla_bone]]
    }

    #[test]
    fn a_joint_owner_is_the_shallowest_bone_on_it() {
        // Unmapped helpers inherit a mapped ancestor, so they always sort after it.
        let owners = joint_owners(&[Some(1), Some(4), None, Some(4), Some(1)]);
        assert_eq!(owners[&1], 0);
        assert_eq!(owners[&4], 1);
        assert_eq!(owners.len(), 2);
    }

    #[test]
    fn rebind_seats_the_joint_on_its_gla_counterpart() {
        // Animation pivots about the GLA joint, so the bone's own joint has to land
        // there or every rotation is applied about the wrong point.
        let source_rest = rigid(0.3, [10., 2., 30.]);
        let gla_base = rigid(-1.1, [1., -2., 40.]);
        for mode in [SourceRebindMode::Translation, SourceRebindMode::Direction] {
            let m = rebind_matrix(mode, &identity(), 1.0, &source_rest, &gla_base, None);
            let placed = transform(&m, origin(&source_rest));
            assert!(
                distance(placed, origin(&gla_base)) < 1e-3,
                "{mode:?} put the joint at {placed:?}, wanted {:?}",
                origin(&gla_base)
            );
        }
    }

    #[test]
    fn direction_rebind_points_the_bone_the_gla_way_without_scaling_it() {
        // Source bone runs along +X from its joint; its GLA counterpart runs along
        // +Z. The rebind must turn the limb without stretching it.
        let joint = [0.0_f32, 0.0, 20.0];
        let source_rest = rigid(0.0, joint);
        let gla_base = rigid(0.0, joint);
        let m = rebind_matrix(
            SourceRebindMode::Direction,
            &identity(),
            1.0,
            &source_rest,
            &gla_base,
            Some(minimal_rotation([1., 0., 0.], [0., 0., 1.])),
        );
        let tip = transform(&m, [joint[0] + 7.0, joint[1], joint[2]]);
        let offset: [f32; 3] = std::array::from_fn(|k| tip[k] - joint[k]);
        assert!(
            (offset[2] - 7.0).abs() < 1e-3 && offset[0].abs() < 1e-3,
            "limb should now run +Z for 7 units, got {offset:?}"
        );
    }

    fn tag_surface(index: usize, bone: usize) -> GlmSurface {
        GlmSurface {
            surface_index: index,
            vertices: (0..3)
                .map(|k| GlmVertex {
                    normal: [0., 0., 1.],
                    position: [k as f32, 0., 0.],
                    weights: vec![GlmWeight {
                        local_bone_index: 0,
                        weight: 1.0,
                    }],
                })
                .collect(),
            texcoords: vec![[0., 0.]; 3],
            triangles: vec![[0, 1, 2]],
            bone_references: vec![bone],
        }
    }

    fn donor_glm() -> GlmModel {
        GlmModel {
            name: "donor".into(),
            anim_name: "models/players/_humanoid/_humanoid".into(),
            num_bones: 4,
            hierarchy: vec![
                GlmSurfaceHierarchy {
                    name: "torso".into(),
                    flags: 0,
                    shader: "some/shader".into(),
                    parent_index: -1,
                    children: vec![1],
                },
                GlmSurfaceHierarchy {
                    name: "*r_hand".into(),
                    flags: 0,
                    shader: String::new(),
                    // Deliberately non-trivial: must not survive the copy.
                    parent_index: 0,
                    children: vec![0],
                },
            ],
            lods: vec![GlmLod {
                surfaces: vec![tag_surface(0, 1), tag_surface(1, 2)],
            }],
        }
    }

    fn empty_rig() -> SourceRig {
        SourceRig {
            source_jiggles: Vec::new(),
            native_pivots: None,
            glm: GlmModel {
                name: "source".into(),
                anim_name: "models/players/_humanoid/_humanoid".into(),
                num_bones: 4,
                hierarchy: vec![GlmSurfaceHierarchy {
                    name: "body_chest_0".into(),
                    flags: 0,
                    shader: "mk2chest".into(),
                    parent_index: -1,
                    children: Vec::new(),
                }],
                lods: vec![
                    GlmLod {
                        surfaces: vec![tag_surface(0, 0)],
                    },
                    GlmLod {
                        surfaces: vec![tag_surface(0, 0)],
                    },
                ],
            },
            surfaces: vec![SourceRigSurface {
                name: "body_chest_0".into(),
                vmt_candidates: Vec::new(),
            }],
            diagnostics: SourceRigDiagnostics::default(),
        }
    }

    fn stub_gla(bones: usize) -> GlaAnimation {
        crate::ghoul2::openjk_default_gla()
            .map(|mut gla| {
                while gla.skeleton.len() < bones {
                    let mut extra = gla.skeleton[0].clone();
                    extra.name = format!("bone{}", gla.skeleton.len());
                    gla.skeleton.push(extra);
                }
                gla
            })
            .expect("built-in default GLA parses")
    }

    /// Without this a rebound Source character has no saber: `model_bolt_matrix`
    /// finds no `*r_hand` surface and no GLA bone of that name.
    #[test]
    fn bolts_are_lifted_into_every_lod_and_detached_from_the_donor_tree() {
        let mut rig = empty_rig();
        let attached = attach_humanoid_bolts(&mut rig, &donor_glm(), &stub_gla(4)).unwrap();
        assert_eq!(attached, 1, "only the *-prefixed surface is a bolt");

        let bolt = rig
            .glm
            .hierarchy
            .iter()
            .position(|h| h.name == "*r_hand")
            .expect("bolt hierarchy entry");
        assert_eq!(rig.glm.hierarchy[bolt].flags & G2SURFACEFLAG_ISBOLT, G2SURFACEFLAG_ISBOLT);
        // Donor indices must not leak: they address the donor's hierarchy.
        assert_eq!(rig.glm.hierarchy[bolt].parent_index, -1);
        assert!(rig.glm.hierarchy[bolt].children.is_empty());
        // The drawable surface must keep index 0, or PlayerSurfaceAsset lookups break.
        assert_eq!(rig.glm.hierarchy[0].name, "body_chest_0");
        // Bolts resolve in LOD 0, but effects resolve against the current LOD.
        for (index, lod) in rig.glm.lods.iter().enumerate() {
            assert!(
                lod.surfaces.iter().any(|s| s.surface_index == bolt),
                "LOD {index} is missing the bolt surface"
            );
        }
        // A bolt is not a drawable surface, so it gets no PlayerSurfaceAsset.
        assert_eq!(rig.surfaces.len(), 1);
    }

    #[test]
    fn bolt_donor_bone_references_are_validated_against_the_skeleton() {
        let mut rig = empty_rig();
        // The donor's bolt references bone 2; a 2-bone skeleton cannot hold it.
        let error = attach_humanoid_bolts(&mut rig, &donor_glm(), &stub_gla(2)).unwrap_err();
        assert!(error.contains("*r_hand"), "{error}");
    }

    #[test]
    fn rebind_mode_parses_from_the_descriptor() {
        assert_eq!(
            SourceRebindMode::from_descriptor("direction"),
            Some(SourceRebindMode::Direction)
        );
        assert_eq!(
            SourceRebindMode::from_descriptor("BONE_FRAME"),
            Some(SourceRebindMode::BoneFrame)
        );
        assert_eq!(SourceRebindMode::from_descriptor("nonsense"), None);
        assert_eq!(SourceRebindMode::default(), SourceRebindMode::Direction);
    }

    #[test]
    fn rigid_inverse_round_trips() {
        let m = rigid(0.7, [3., -4., 5.]);
        let back = mul(&rigid_inverse(&m), &m);
        for r in 0..3 {
            for c in 0..4 {
                assert!(
                    (back[r][c] - identity()[r][c]).abs() < 1e-5,
                    "row {r} col {c} = {}",
                    back[r][c]
                );
            }
        }
    }

    /// The configured/fitted uniform scale reaches the baked mesh through
    /// `placement`, so a Source limb keeps its proportions at JKA size.
    #[test]
    fn rebind_applies_the_uniform_scale_to_limb_length() {
        let scale = 0.5_f32;
        let placement = [
            [scale, 0., 0., 0.],
            [0., scale, 0., 0.],
            [0., 0., scale, 0.],
        ];
        let joint = [0.0_f32, 0.0, 40.0];
        let source_rest = rigid(0.0, joint);
        // The GLA joint sits wherever it likes; the limb length must not depend on it.
        let gla_base = rigid(0.7, [5., -3., 33.]);
        let m = rebind_matrix(
            SourceRebindMode::Translation,
            &placement,
            scale,
            &source_rest,
            &gla_base,
            None,
        );
        let tip = transform(&m, [joint[0] + 10.0, joint[1], joint[2]]);
        let length = distance(tip, origin(&gla_base));
        assert!(
            (length - 10.0 * scale).abs() < 1e-3,
            "limb measured {length}, wanted {}",
            10.0 * scale
        );
    }

    #[test]
    fn source_pose_rebase_is_exact_identity_with_anatomical_pivots() {
        let gla = stub_gla(53);
        let n = gla.skeleton.len();
        let stock = vec![identity(); n];
        let native: Vec<[f32; 3]> = gla.skeleton.iter().enumerate().map(|(i, bone)| {
            let mut p = origin(&bone.base_pose);
            // Make the character intentionally different from the stock GLA.
            p[2] += 2.0 + (i % 7) as f32 * 0.1;
            p
        }).collect();
        let adjusted = rebase_source_ghoul2_pose(&gla, &stock, &native).unwrap();
        for (j, m) in adjusted.iter().enumerate() {
            for row in 0..3 {
                for col in 0..4 {
                    assert!((m[row][col] - stock[j][row][col]).abs() < 0.001,
                        "rest pose moved at bone={j} row={row} col={col}");
                }
            }
        }
    }

    #[test]
    fn source_pose_rebase_reports_inconsistent_bone_counts() {
        let gla = stub_gla(53);
        let n = gla.skeleton.len();
        assert!(rebase_source_ghoul2_pose(&gla, &vec![identity(); n], &[]).is_err());
    }

    #[test]
    fn fit_recovers_a_known_similarity() {
        // Build anchors from a synthetic skeleton transformed by a known yaw and
        // scale; the fit must recover both.
        let yaw = 0.9f32;
        let scale = 1.4f32;
        let (s, c) = yaw.sin_cos();
        let source = [
            [0.0f32, 0., 10.],
            [3., 1., 20.],
            [-3., -1., 20.],
            [6., 0., 5.],
            [-6., 2., 7.],
        ];
        let anchors: Vec<_> = source
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mapped = [
                    scale * (c * p[0] - s * p[1]) + 2.0,
                    scale * (s * p[0] + c * p[1]) - 3.0,
                    scale * p[2] + 4.0,
                ];
                (format!("src{i}"), format!("gla{i}"), *p, mapped)
            })
            .collect();
        let fit = fit_skeletons(&anchors).unwrap();
        assert!((fit.scale - scale).abs() < 1e-3, "scale {}", fit.scale);
        assert!(
            (fit.yaw_degrees - yaw.to_degrees()).abs() < 0.1,
            "yaw {}",
            fit.yaw_degrees
        );
        assert!(fit.rms < 1e-2, "rms {}", fit.rms);
    }

    #[test]
    fn fit_refuses_too_few_anchors() {
        let anchors = vec![(
            "a".to_owned(),
            "b".to_owned(),
            [0., 0., 0.],
            [0., 0., 0.],
        )];
        assert!(fit_skeletons(&anchors).unwrap_err().contains("humanoid"));
    }

    #[test]
    fn dual_quat_decomposes_scale_before_extracting_rotation() {
        // A model-space point must remain scaled around a rotated bone's pivot.
        // The historical path fed scale*rotation to matrix_to_quaternion and
        // then dropped scale when materializing the blended rigid transform.
        for scale in [0.5f32, 0.793_993_8, 1.0, 1.7] {
            let angle = std::f32::consts::FRAC_PI_3;
            let (sn, cs) = angle.sin_cos();
            let transform_matrix = [
                [scale * cs, -scale * sn, 0.0, 3.0],
                [scale * sn, scale * cs, 0.0, -5.0],
                [0.0, 0.0, scale, 7.0],
            ];
            let (real, dual, recovered_scale) =
                dual_quat_from_scaled_matrix(&transform_matrix);
            let (real, dual) = blend_dual_quats(&[(1.0, real, dual)]);
            let rigid = dual_quat_to_matrix(real, dual);
            let p = [12.0, -4.0, 10.0];
            let expected = transform(&transform_matrix, p);
            let got = transform(&rigid, p.map(|x| x * recovered_scale));
            assert!((recovered_scale - scale).abs() < 1e-6);
            for axis in 0..3 {
                assert!((got[axis] - expected[axis]).abs() < 1e-4,
                    "axis {axis}, scale {scale}: {} vs {}", got[axis], expected[axis]);
            }
        }
    }

    #[test]
    fn dual_quat_uses_weighted_scale_without_changing_normals() {
        let mut first = identity();
        let mut second = identity();
        for axis in 0..3 {
            first[axis][axis] = 0.5;
            second[axis][axis] = 1.5;
        }
        first[0][3] = 3.0;
        second[0][3] = 3.0;
        let (r0, d0, s0) = dual_quat_from_scaled_matrix(&first);
        let (r1, d1, s1) = dual_quat_from_scaled_matrix(&second);
        let (r, d) = blend_dual_quats(&[(0.25, r0, d0), (0.75, r1, d1)]);
        let rigid = dual_quat_to_matrix(r, d);
        let p = transform(&rigid, [2.0 * (0.25 * s0 + 0.75 * s1), 0.0, 0.0]);
        assert!((p[0] - 5.5).abs() < 1e-5);
        let normal = rotate(&rigid, [0.0, 0.0, 1.0]);
        assert!((normal[2] - 1.0).abs() < 1e-5);
    }

}
